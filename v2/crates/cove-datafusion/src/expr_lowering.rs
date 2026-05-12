//! DataFusion-agnostic expression lowering into COVE-native predicate programs.

use cove_core::{
    canonical::CanonicalValue,
    constants::{CoveLogicalType, CovePhysicalKind},
    CoveError,
};

use crate::{
    dataset_state::DatasetState,
    planner::{FilterPlan, NullPredicateKind, NumericPredicateOp, PredicateLiteral},
    scan_program::promote_filter_exactness,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LowerOperator {
    Eq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LowerLiteral {
    Null,
    Boolean(bool),
    Int64(i64),
    UInt64(u64),
    Float64(f64),
    Utf8(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LowerExpr {
    Column(String),
    Literal(LowerLiteral),
    IsNull(Box<LowerExpr>),
    IsNotNull(Box<LowerExpr>),
    Binary {
        left: Box<LowerExpr>,
        op: LowerOperator,
        right: Box<LowerExpr>,
    },
    InList {
        expr: Box<LowerExpr>,
        list: Vec<LowerExpr>,
        negated: bool,
    },
    Unsupported(String),
}

pub fn lower_filter(
    state: &DatasetState,
    expr: &LowerExpr,
    display: impl Into<String>,
) -> FilterPlan {
    let display = display.into();
    let mut filter = match expr {
        LowerExpr::IsNull(input) => {
            classify_null_filter(state, input, NullPredicateKind::IsNull, display)
        }
        LowerExpr::IsNotNull(input) => {
            classify_null_filter(state, input, NullPredicateKind::IsNotNull, display)
        }
        LowerExpr::Binary { left, op, right } => {
            classify_binary_filter(state, left, *op, right, display)
        }
        LowerExpr::InList {
            expr,
            list,
            negated,
        } if !negated => classify_in_list_filter(state, expr, list, display),
        _ => FilterPlan::unsupported(display),
    };
    promote_filter_exactness(state, &mut filter);
    filter
}

fn classify_null_filter(
    state: &DatasetState,
    input: &LowerExpr,
    kind: NullPredicateKind,
    display: String,
) -> FilterPlan {
    let Some(column_index) = top_level_column_index(state, input) else {
        return FilterPlan::unsupported(display);
    };
    let column = &state.table().columns[column_index];
    if !is_top_level_scalar(column.logical, column.physical) {
        return FilterPlan::unsupported(display);
    }
    if matches!(kind, NullPredicateKind::IsNotNull) && !column.nullable {
        return FilterPlan::unsupported(display);
    }
    FilterPlan::pruning_null(column_index, kind, display)
}

fn classify_binary_filter(
    state: &DatasetState,
    left: &LowerExpr,
    op: LowerOperator,
    right: &LowerExpr,
    display: String,
) -> FilterPlan {
    if let Some((column_index, literal, op)) = column_literal_binary(state, left, op, right) {
        return classify_column_literal(state, column_index, op, &literal, display);
    }
    if let Some((column_index, literal, op)) =
        column_literal_binary(state, right, flip_op(op), left)
    {
        return classify_column_literal(state, column_index, op, &literal, display);
    }
    FilterPlan::unsupported(display)
}

fn classify_in_list_filter(
    state: &DatasetState,
    expr: &LowerExpr,
    list: &[LowerExpr],
    display: String,
) -> FilterPlan {
    let Some(column_index) = top_level_column_index(state, expr) else {
        return FilterPlan::unsupported(display);
    };
    let column = &state.table().columns[column_index];
    if column.physical != CovePhysicalKind::FileCode {
        return FilterPlan::unsupported(display);
    }
    let mut canonical_values = Vec::with_capacity(list.len());
    for item in list {
        let LowerExpr::Literal(literal) = item else {
            return FilterPlan::unsupported(display);
        };
        match file_code_canonical_literal(column.logical, literal) {
            Ok(Some(canonical)) => canonical_values.push(canonical),
            Ok(None) => {}
            Err(_) => return FilterPlan::unsupported(display),
        }
    }
    FilterPlan::pruning_file_code_in_with_canonical(
        column_index,
        Vec::new(),
        canonical_values,
        display,
    )
}

fn classify_column_literal(
    state: &DatasetState,
    column_index: usize,
    op: LowerOperator,
    literal: &LowerLiteral,
    display: String,
) -> FilterPlan {
    let column = &state.table().columns[column_index];
    match column.physical {
        CovePhysicalKind::NumCode => {
            let Some(literal) = numeric_literal(literal) else {
                return FilterPlan::unsupported(display);
            };
            FilterPlan::pruning_numeric(column_index, numeric_op(op), literal, display)
        }
        CovePhysicalKind::FileCode if op == LowerOperator::Eq => {
            match file_code_canonical_literal(column.logical, literal) {
                Ok(Some(canonical)) => FilterPlan::pruning_file_code_in_with_canonical(
                    column_index,
                    Vec::new(),
                    vec![canonical],
                    display,
                ),
                Ok(None) => FilterPlan::pruning_file_code_in_with_canonical(
                    column_index,
                    Vec::new(),
                    Vec::new(),
                    display,
                ),
                Err(_) => FilterPlan::unsupported(display),
            }
        }
        CovePhysicalKind::VarBytes
            if op == LowerOperator::Eq && column.logical == CoveLogicalType::Utf8 =>
        {
            let LowerLiteral::Utf8(value) = literal else {
                return FilterPlan::unsupported(display);
            };
            FilterPlan::pruning_varbytes_eq(column_index, value.as_bytes().to_vec(), display)
        }
        _ => FilterPlan::unsupported(display),
    }
}

fn column_literal_binary(
    state: &DatasetState,
    column: &LowerExpr,
    op: LowerOperator,
    literal: &LowerExpr,
) -> Option<(usize, LowerLiteral, LowerOperator)> {
    let column_index = top_level_column_index(state, column)?;
    let LowerExpr::Literal(literal) = literal else {
        return None;
    };
    Some((column_index, literal.clone(), op))
}

fn top_level_column_index(state: &DatasetState, expr: &LowerExpr) -> Option<usize> {
    let LowerExpr::Column(name) = expr else {
        return None;
    };
    state
        .table()
        .columns
        .iter()
        .position(|candidate| candidate.name == *name)
}

fn file_code_canonical_literal(
    logical: CoveLogicalType,
    literal: &LowerLiteral,
) -> Result<Option<Vec<u8>>, CoveError> {
    canonical_literal(logical, literal)
}

fn canonical_literal(
    logical: CoveLogicalType,
    literal: &LowerLiteral,
) -> Result<Option<Vec<u8>>, CoveError> {
    match (logical, literal) {
        (_, LowerLiteral::Null) => Ok(None),
        (CoveLogicalType::Utf8, LowerLiteral::Utf8(value)) => {
            CanonicalValue::Utf8(value).encode().map(Some)
        }
        (CoveLogicalType::Bool, LowerLiteral::Boolean(value)) => {
            CanonicalValue::Bool(*value).encode().map(Some)
        }
        (CoveLogicalType::Int64, LowerLiteral::Int64(value)) => CanonicalValue::Int {
            width: 64,
            value: i128::from(*value),
        }
        .encode()
        .map(Some),
        (CoveLogicalType::UInt64, LowerLiteral::UInt64(value)) => CanonicalValue::Uint {
            width: 64,
            value: u128::from(*value),
        }
        .encode()
        .map(Some),
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "unsupported FileCode literal for {logical:?}"
        ))),
    }
}

fn numeric_literal(literal: &LowerLiteral) -> Option<PredicateLiteral> {
    match literal {
        LowerLiteral::Int64(value) => Some(PredicateLiteral::Int64(*value)),
        LowerLiteral::UInt64(value) => Some(PredicateLiteral::UInt64(*value)),
        LowerLiteral::Float64(value) if !value.is_nan() => {
            Some(PredicateLiteral::Float64(*value).normalized())
        }
        _ => None,
    }
}

fn numeric_op(op: LowerOperator) -> NumericPredicateOp {
    match op {
        LowerOperator::Eq => NumericPredicateOp::Eq,
        LowerOperator::Lt => NumericPredicateOp::Lt,
        LowerOperator::LtEq => NumericPredicateOp::LtEq,
        LowerOperator::Gt => NumericPredicateOp::Gt,
        LowerOperator::GtEq => NumericPredicateOp::GtEq,
    }
}

fn flip_op(op: LowerOperator) -> LowerOperator {
    match op {
        LowerOperator::Eq => LowerOperator::Eq,
        LowerOperator::Lt => LowerOperator::Gt,
        LowerOperator::LtEq => LowerOperator::GtEq,
        LowerOperator::Gt => LowerOperator::Lt,
        LowerOperator::GtEq => LowerOperator::LtEq,
    }
}

fn is_top_level_scalar(logical: CoveLogicalType, physical: CovePhysicalKind) -> bool {
    !matches!(
        logical,
        CoveLogicalType::List | CoveLogicalType::Struct | CoveLogicalType::Map
    ) && !matches!(
        physical,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map
    )
}

#[cfg(test)]
mod tests {
    use super::{numeric_literal, LowerLiteral};
    use crate::planner::PredicateLiteral;

    #[test]
    fn numeric_lowering_rejects_nan_and_normalizes_negative_zero() {
        assert!(numeric_literal(&LowerLiteral::Float64(f64::NAN)).is_none());
        assert_eq!(
            numeric_literal(&LowerLiteral::Float64(-0.0)),
            Some(PredicateLiteral::Float64(0.0))
        );
    }
}
