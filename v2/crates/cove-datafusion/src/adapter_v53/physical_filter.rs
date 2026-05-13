//! DataFusion 53.x physical-expression lowering for COVE scan planning.

use std::sync::Arc;

use datafusion::{
    common::ScalarValue,
    logical_expr::Operator,
    physical_expr::expressions::{
        BinaryExpr, Column, InListExpr, IsNotNullExpr, IsNullExpr, Literal,
    },
    physical_expr_common::physical_expr::PhysicalExpr,
};

use crate::{
    dataset_state::DatasetState,
    expr_lowering::{lower_filter, LowerExpr, LowerLiteral, LowerOperator},
    planner::{CoveFilterUse, FilterPlan},
};

#[derive(Debug, Default)]
pub(crate) struct PhysicalFilterLowering {
    pub(crate) filters: Vec<FilterPlan>,
    pub(crate) fallbacks: usize,
}

impl PhysicalFilterLowering {
    pub(crate) fn all_supported(&self) -> bool {
        self.fallbacks == 0
    }
}

pub(crate) fn lower_physical_filters(
    state: &DatasetState,
    filters: &[Arc<dyn PhysicalExpr>],
) -> PhysicalFilterLowering {
    let mut out = PhysicalFilterLowering::default();
    for filter in filters {
        let lowered = lower_physical_filter(state, filter.as_ref());
        out.filters.extend(lowered.filters);
        out.fallbacks += lowered.fallbacks;
    }
    out
}

pub(crate) fn lower_physical_filter(
    state: &DatasetState,
    expr: &dyn PhysicalExpr,
) -> PhysicalFilterLowering {
    let mut out = PhysicalFilterLowering::default();
    lower_into(state, expr, &mut out);
    out
}

fn lower_into(state: &DatasetState, expr: &dyn PhysicalExpr, out: &mut PhysicalFilterLowering) {
    if literal_true(expr) {
        return;
    }
    if let Some(binary) = expr.as_any().downcast_ref::<BinaryExpr>() {
        if binary.op() == &Operator::And {
            lower_into(state, binary.left().as_ref(), out);
            lower_into(state, binary.right().as_ref(), out);
            return;
        }
    }
    let Some(lower) = lower_expr(expr) else {
        out.fallbacks += 1;
        return;
    };
    let filter = lower_filter(state, &lower, expr.to_string());
    if filter.use_kind == CoveFilterUse::Unsupported {
        out.fallbacks += 1;
    } else {
        out.filters.push(filter);
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
        ScalarValue::Date32(Some(value)) => Some(LowerLiteral::Int64(i64::from(*value))),
        ScalarValue::TimestampMicrosecond(Some(value), None) => Some(LowerLiteral::Int64(*value)),
        ScalarValue::TimestampNanosecond(Some(value), None) => Some(LowerLiteral::Int64(*value)),
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
