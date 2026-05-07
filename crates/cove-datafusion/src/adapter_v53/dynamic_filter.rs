//! Native-mode dynamic-filter snapshots for DataFusion 53.x.

use std::sync::Arc;

use datafusion::{
    common::{Result, ScalarValue},
    logical_expr::Operator,
    physical_expr::{
        expressions::{BinaryExpr, Column, InListExpr, IsNotNullExpr, IsNullExpr, Literal},
        PhysicalExpr,
    },
};

use crate::{
    dataset_state::DatasetState,
    expr_lowering::{lower_filter, LowerExpr, LowerLiteral, LowerOperator},
    planner::{CoveFilterUse, FilterPlan},
};

#[derive(Debug, Default)]
pub(crate) struct DynamicFilterSnapshot {
    pub(crate) filters: Vec<FilterPlan>,
    pub(crate) snapshots: usize,
    pub(crate) fallbacks: usize,
}

pub(crate) fn snapshot_dynamic_filters(
    state: &DatasetState,
    filters: &[Arc<dyn PhysicalExpr>],
) -> Result<DynamicFilterSnapshot> {
    let mut out = DynamicFilterSnapshot::default();
    for filter in filters {
        if filter.snapshot_generation() == 0 {
            continue;
        }
        out.snapshots += 1;
        let snapshot = filter.snapshot()?.unwrap_or_else(|| Arc::clone(filter));
        match lower_snapshot(state, snapshot.as_ref()) {
            Some(filters) => out.filters.extend(filters),
            None => out.fallbacks += 1,
        }
    }
    Ok(out)
}

fn lower_snapshot(state: &DatasetState, expr: &dyn PhysicalExpr) -> Option<Vec<FilterPlan>> {
    if literal_true(expr) {
        return Some(Vec::new());
    }
    if let Some(binary) = expr.as_any().downcast_ref::<BinaryExpr>() {
        if binary.op() == &Operator::And {
            let mut left = lower_snapshot(state, binary.left().as_ref())?;
            let right = lower_snapshot(state, binary.right().as_ref())?;
            left.extend(right);
            return Some(left);
        }
    }
    let lower = lower_expr(expr)?;
    let filter = lower_filter(state, &lower, expr.to_string());
    if filter.use_kind == CoveFilterUse::Unsupported {
        None
    } else {
        Some(vec![filter])
    }
}

fn lower_expr(expr: &dyn PhysicalExpr) -> Option<LowerExpr> {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        return Some(LowerExpr::Column(column.name().to_owned()));
    }
    if let Some(literal) = expr.as_any().downcast_ref::<Literal>() {
        return lower_scalar(literal.value()).map(LowerExpr::Literal);
    }
    if let Some(is_null) = expr.as_any().downcast_ref::<IsNullExpr>() {
        return Some(LowerExpr::IsNull(Box::new(lower_expr(
            is_null.arg().as_ref(),
        )?)));
    }
    if let Some(is_not_null) = expr.as_any().downcast_ref::<IsNotNullExpr>() {
        return Some(LowerExpr::IsNotNull(Box::new(lower_expr(
            is_not_null.arg().as_ref(),
        )?)));
    }
    if let Some(binary) = expr.as_any().downcast_ref::<BinaryExpr>() {
        let op = lower_operator(binary.op())?;
        return Some(LowerExpr::Binary {
            left: Box::new(lower_expr(binary.left().as_ref())?),
            op,
            right: Box::new(lower_expr(binary.right().as_ref())?),
        });
    }
    if let Some(in_list) = expr.as_any().downcast_ref::<InListExpr>() {
        return Some(LowerExpr::InList {
            expr: Box::new(lower_expr(in_list.expr().as_ref())?),
            list: in_list
                .list()
                .iter()
                .map(|item| lower_expr(item.as_ref()))
                .collect::<Option<Vec<_>>>()?,
            negated: in_list.negated(),
        });
    }
    None
}

fn lower_operator(op: &Operator) -> Option<LowerOperator> {
    match op {
        Operator::Eq => Some(LowerOperator::Eq),
        Operator::Lt => Some(LowerOperator::Lt),
        Operator::LtEq => Some(LowerOperator::LtEq),
        Operator::Gt => Some(LowerOperator::Gt),
        Operator::GtEq => Some(LowerOperator::GtEq),
        _ => None,
    }
}

fn lower_scalar(value: &ScalarValue) -> Option<LowerLiteral> {
    match value {
        ScalarValue::Null => Some(LowerLiteral::Null),
        ScalarValue::Boolean(Some(value)) => Some(LowerLiteral::Boolean(*value)),
        ScalarValue::Int8(Some(value)) => Some(LowerLiteral::Int64(i64::from(*value))),
        ScalarValue::Int16(Some(value)) => Some(LowerLiteral::Int64(i64::from(*value))),
        ScalarValue::Int32(Some(value)) => Some(LowerLiteral::Int64(i64::from(*value))),
        ScalarValue::Int64(Some(value)) => Some(LowerLiteral::Int64(*value)),
        ScalarValue::UInt8(Some(value)) => Some(LowerLiteral::UInt64(u64::from(*value))),
        ScalarValue::UInt16(Some(value)) => Some(LowerLiteral::UInt64(u64::from(*value))),
        ScalarValue::UInt32(Some(value)) => Some(LowerLiteral::UInt64(u64::from(*value))),
        ScalarValue::UInt64(Some(value)) => Some(LowerLiteral::UInt64(*value)),
        ScalarValue::Float32(Some(value)) => Some(LowerLiteral::Float64(f64::from(*value))),
        ScalarValue::Float64(Some(value)) => Some(LowerLiteral::Float64(*value)),
        ScalarValue::Utf8(Some(value))
        | ScalarValue::Utf8View(Some(value))
        | ScalarValue::LargeUtf8(Some(value)) => Some(LowerLiteral::Utf8(value.clone())),
        ScalarValue::Dictionary(_, value) => lower_scalar(value),
        _ if value.is_null() => Some(LowerLiteral::Null),
        _ => None,
    }
}

fn literal_true(expr: &dyn PhysicalExpr) -> bool {
    expr.as_any()
        .downcast_ref::<Literal>()
        .map(|literal| matches!(literal.value(), ScalarValue::Boolean(Some(true))))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cove_core::constants::CovePhysicalKind;
    use datafusion::physical_expr::expressions::{lit, BinaryExpr, DynamicFilterPhysicalExpr};

    const SCAN_TABLE: &[u8] =
        include_bytes!("../../../../conformance/accept/cove_t_bool_numcode_declared.cove");

    #[test]
    fn dynamic_snapshot_lowers_supported_numeric_filter() {
        let state = crate::bootstrap::bootstrap_bytes("dynamic.cove", SCAN_TABLE.to_vec())
            .expect("fixture should bootstrap");
        let column_index = state
            .table()
            .columns
            .iter()
            .position(|column| column.physical == CovePhysicalKind::NumCode)
            .expect("fixture should have a NumCode column");
        let name = state.table().columns[column_index].name.as_str();
        let column = Arc::new(Column::new(name, column_index)) as Arc<dyn PhysicalExpr>;
        let dynamic = Arc::new(DynamicFilterPhysicalExpr::new(
            vec![Arc::clone(&column)],
            lit(true),
        ));
        dynamic
            .update(Arc::new(BinaryExpr::new(column, Operator::GtEq, lit(0i64))))
            .expect("dynamic update");

        let snapshot = snapshot_dynamic_filters(&state, &[dynamic]);

        let snapshot = snapshot.expect("snapshot should not fail");
        assert_eq!(snapshot.snapshots, 1);
        assert_eq!(snapshot.fallbacks, 0);
        assert_eq!(snapshot.filters.len(), 1);
        assert_eq!(snapshot.filters[0].use_kind, CoveFilterUse::PruningOnly);
    }
}
