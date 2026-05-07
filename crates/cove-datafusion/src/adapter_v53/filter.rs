//! DataFusion 53.x expression classification for COVE-native filter planning.

use datafusion::{
    common::ScalarValue,
    logical_expr::{Expr, Operator},
};

use crate::{
    dataset_state::DatasetState,
    expr_lowering::{lower_filter, LowerExpr, LowerLiteral, LowerOperator},
    planner::{CoveFilterUse, FilterPlan},
};

pub(crate) fn classify_filter(state: &DatasetState, expr: &Expr) -> FilterPlan {
    lower_filter(state, &lower_expr(expr), expr.to_string())
}

pub(crate) fn filter_use_is_pushed(use_kind: CoveFilterUse) -> bool {
    matches!(
        use_kind,
        CoveFilterUse::PruningOnly | CoveFilterUse::FullRowPredicateExact
    )
}

fn lower_expr(expr: &Expr) -> LowerExpr {
    match expr {
        Expr::Column(column) => LowerExpr::Column(column.name.clone()),
        Expr::Literal(value, _) => lower_scalar(value)
            .map(LowerExpr::Literal)
            .unwrap_or_else(|| LowerExpr::Unsupported(expr.to_string())),
        Expr::IsNull(input) => LowerExpr::IsNull(Box::new(lower_expr(input))),
        Expr::IsNotNull(input) => LowerExpr::IsNotNull(Box::new(lower_expr(input))),
        Expr::BinaryExpr(binary) => match lower_operator(binary.op) {
            Some(op) => LowerExpr::Binary {
                left: Box::new(lower_expr(&binary.left)),
                op,
                right: Box::new(lower_expr(&binary.right)),
            },
            None => LowerExpr::Unsupported(expr.to_string()),
        },
        Expr::InList(in_list) => LowerExpr::InList {
            expr: Box::new(lower_expr(&in_list.expr)),
            list: in_list.list.iter().map(lower_expr).collect(),
            negated: in_list.negated,
        },
        Expr::Between(between) if !between.negated => LowerExpr::Binary {
            left: Box::new(lower_expr(&between.expr)),
            op: LowerOperator::GtEq,
            right: Box::new(lower_expr(&between.low)),
        },
        _ => LowerExpr::Unsupported(expr.to_string()),
    }
}

fn lower_operator(op: Operator) -> Option<LowerOperator> {
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
