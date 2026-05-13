//! DataFusion 53.x expression classification for COVE-native filter planning.

use datafusion::{
    common::ScalarValue,
    logical_expr::{Expr, Operator},
};

use crate::{
    dataset_state::DatasetState,
    expr_lowering::{
        lower_filter, lower_filters, FilterLowering, LowerExpr, LowerLiteral, LowerOperator,
    },
    planner::{CoveFilterUse, FilterPlan},
};

pub(crate) fn classify_filter(state: &DatasetState, expr: &Expr) -> FilterPlan {
    lower_filter(state, &lower_expr(expr), expr.to_string())
}

pub(crate) fn classify_filters(state: &DatasetState, expr: &Expr) -> FilterLowering {
    lower_filters(state, &lower_expr(expr), expr.to_string())
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
            Some(LowerBinaryOperator::Predicate(op)) => LowerExpr::Binary {
                left: Box::new(lower_expr(&binary.left)),
                op,
                right: Box::new(lower_expr(&binary.right)),
            },
            Some(LowerBinaryOperator::And) => {
                let mut children = Vec::new();
                collect_and_terms(expr, &mut children);
                LowerExpr::And(children)
            }
            Some(LowerBinaryOperator::Or) => {
                let mut children = Vec::new();
                collect_or_terms(expr, &mut children);
                LowerExpr::Or(children)
            }
            None => LowerExpr::Unsupported(expr.to_string()),
        },
        Expr::InList(in_list) => LowerExpr::InList {
            expr: Box::new(lower_expr(&in_list.expr)),
            list: in_list.list.iter().map(lower_expr).collect(),
            negated: in_list.negated,
        },
        Expr::Between(between) if !between.negated => LowerExpr::Between {
            expr: Box::new(lower_expr(&between.expr)),
            low: Box::new(lower_expr(&between.low)),
            high: Box::new(lower_expr(&between.high)),
        },
        _ => LowerExpr::Unsupported(expr.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LowerBinaryOperator {
    Predicate(LowerOperator),
    And,
    Or,
}

fn lower_operator(op: Operator) -> Option<LowerBinaryOperator> {
    match op {
        Operator::Eq => Some(LowerBinaryOperator::Predicate(LowerOperator::Eq)),
        Operator::Lt => Some(LowerBinaryOperator::Predicate(LowerOperator::Lt)),
        Operator::LtEq => Some(LowerBinaryOperator::Predicate(LowerOperator::LtEq)),
        Operator::Gt => Some(LowerBinaryOperator::Predicate(LowerOperator::Gt)),
        Operator::GtEq => Some(LowerBinaryOperator::Predicate(LowerOperator::GtEq)),
        Operator::And => Some(LowerBinaryOperator::And),
        Operator::Or => Some(LowerBinaryOperator::Or),
        _ => None,
    }
}

fn collect_and_terms(expr: &Expr, out: &mut Vec<LowerExpr>) {
    match expr {
        Expr::BinaryExpr(binary) if binary.op == Operator::And => {
            collect_and_terms(&binary.left, out);
            collect_and_terms(&binary.right, out);
        }
        _ => out.push(lower_expr(expr)),
    }
}

fn collect_or_terms(expr: &Expr, out: &mut Vec<LowerExpr>) {
    match expr {
        Expr::BinaryExpr(binary) if binary.op == Operator::Or => {
            collect_or_terms(&binary.left, out);
            collect_or_terms(&binary.right, out);
        }
        _ => out.push(lower_expr(expr)),
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
