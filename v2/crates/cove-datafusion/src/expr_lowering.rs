//! DataFusion-agnostic expression lowering into COVE-native predicate programs.

use cove_core::{
    canonical::CanonicalValue,
    constants::{CoveLogicalType, CovePhysicalKind},
    CoveError,
};

use crate::{
    dataset_state::DatasetState,
    planner::{
        CovePredicateExpr, FilterPlan, NullPredicateKind, NumericPredicateOp, PredicateLiteral,
    },
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
    Binary(Vec<u8>),
    Int128(i128),
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
    And(Vec<LowerExpr>),
    Or(Vec<LowerExpr>),
    Binary {
        left: Box<LowerExpr>,
        op: LowerOperator,
        right: Box<LowerExpr>,
    },
    Between {
        expr: Box<LowerExpr>,
        low: Box<LowerExpr>,
        high: Box<LowerExpr>,
    },
    InList {
        expr: Box<LowerExpr>,
        list: Vec<LowerExpr>,
        negated: bool,
    },
    Unsupported(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterLowering {
    pub filters: Vec<FilterPlan>,
    pub fallbacks: usize,
}

pub fn lower_filter(
    state: &DatasetState,
    expr: &LowerExpr,
    display: impl Into<String>,
) -> FilterPlan {
    let display = display.into();
    let lowering = lower_filters(state, expr, display.clone());
    if lowering.fallbacks == 0 && lowering.filters.len() == 1 {
        return lowering.filters.into_iter().next().expect("single filter");
    }
    FilterPlan::unsupported(display)
}

pub fn lower_filters(
    state: &DatasetState,
    expr: &LowerExpr,
    display: impl Into<String>,
) -> FilterLowering {
    let display = display.into();
    let mut lowering = FilterLowering::default();
    lower_filters_into(state, expr, &display, &mut lowering);
    lowering
}

fn lower_filters_into(
    state: &DatasetState,
    expr: &LowerExpr,
    display: &str,
    lowering: &mut FilterLowering,
) {
    match expr {
        LowerExpr::And(children) => {
            for child in children {
                lower_filters_into(state, child, display, lowering);
            }
        }
        LowerExpr::Between { expr, low, high } => {
            push_lowered_atom(
                state,
                &LowerExpr::Binary {
                    left: expr.clone(),
                    op: LowerOperator::GtEq,
                    right: low.clone(),
                },
                format!("{display} lower"),
                lowering,
            );
            push_lowered_atom(
                state,
                &LowerExpr::Binary {
                    left: expr.clone(),
                    op: LowerOperator::LtEq,
                    right: high.clone(),
                },
                format!("{display} upper"),
                lowering,
            );
        }
        LowerExpr::Or(children) => {
            let mut exprs = Vec::with_capacity(children.len());
            let mut predicate_columns = Vec::new();
            for child in children {
                let child_lowering = lower_filters(state, child, display.to_string());
                if child_lowering.fallbacks != 0 || child_lowering.filters.is_empty() {
                    lowering.fallbacks += 1;
                    return;
                }
                let Some(expr) = predicate_expr_for_filters(&child_lowering.filters) else {
                    lowering.fallbacks += 1;
                    return;
                };
                for filter in &child_lowering.filters {
                    predicate_columns.extend(filter.predicate_columns.iter().copied());
                }
                exprs.push(expr);
            }
            predicate_columns.sort_unstable();
            predicate_columns.dedup();
            lowering.filters.push(FilterPlan::pruning_expr(
                predicate_columns,
                CovePredicateExpr::Or(exprs),
                display.to_string(),
            ));
        }
        LowerExpr::Unsupported(_) => {
            lowering.fallbacks += 1;
        }
        _ => push_lowered_atom(state, expr, display.to_string(), lowering),
    }
}

fn predicate_expr_for_filters(filters: &[FilterPlan]) -> Option<CovePredicateExpr> {
    let mut exprs = Vec::with_capacity(filters.len());
    for filter in filters {
        exprs.push(filter.predicate_expr.clone()?);
    }
    match exprs.len() {
        0 => None,
        1 => exprs.into_iter().next(),
        _ => Some(CovePredicateExpr::And(exprs)),
    }
}

fn push_lowered_atom(
    state: &DatasetState,
    expr: &LowerExpr,
    display: String,
    lowering: &mut FilterLowering,
) {
    let filter = lower_atom_filter(state, expr, display);
    if filter.use_kind == crate::planner::CoveFilterUse::Unsupported {
        lowering.fallbacks += 1;
    } else {
        lowering.filters.push(filter);
    }
}

fn lower_atom_filter(state: &DatasetState, expr: &LowerExpr, display: String) -> FilterPlan {
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
        (CoveLogicalType::Binary, LowerLiteral::Binary(value)) => {
            CanonicalValue::Bytes(value).encode().map(Some)
        }
        (CoveLogicalType::Json, LowerLiteral::Utf8(value)) => {
            CanonicalValue::Json(value).encode().map(Some)
        }
        (CoveLogicalType::Uuid, LowerLiteral::Utf8(value)) => {
            let uuid = parse_uuid_literal(value)?;
            CanonicalValue::Uuid(uuid).encode().map(Some)
        }
        (CoveLogicalType::Uuid, LowerLiteral::Binary(value)) if value.len() == 16 => {
            let mut uuid = [0u8; 16];
            uuid.copy_from_slice(value);
            CanonicalValue::Uuid(uuid).encode().map(Some)
        }
        (CoveLogicalType::Bool, LowerLiteral::Boolean(value)) => {
            CanonicalValue::Bool(*value).encode().map(Some)
        }
        (
            CoveLogicalType::Int8
            | CoveLogicalType::Int16
            | CoveLogicalType::Int32
            | CoveLogicalType::Int64,
            literal,
        ) => CanonicalValue::Int {
            width: integer_width(logical),
            value: signed_literal(literal)?,
        }
        .encode()
        .map(Some),
        (
            CoveLogicalType::UInt8
            | CoveLogicalType::UInt16
            | CoveLogicalType::UInt32
            | CoveLogicalType::UInt64,
            literal,
        ) => CanonicalValue::Uint {
            width: integer_width(logical),
            value: unsigned_literal(literal)?,
        }
        .encode()
        .map(Some),
        (CoveLogicalType::Float32, LowerLiteral::Float64(value)) => {
            CanonicalValue::Float32(*value as f32).encode().map(Some)
        }
        (CoveLogicalType::Float64, LowerLiteral::Float64(value)) => {
            CanonicalValue::Float64(*value).encode().map(Some)
        }
        (CoveLogicalType::Decimal64, literal) => {
            let value = i64::try_from(signed_literal(literal)?).map_err(|_| {
                CoveError::UnsupportedEncoding("decimal64 literal out of range".into())
            })?;
            CanonicalValue::Decimal64(value).encode().map(Some)
        }
        (CoveLogicalType::Decimal128, literal) => {
            CanonicalValue::Decimal128(signed_literal(literal)?)
                .encode()
                .map(Some)
        }
        (CoveLogicalType::DateDays, literal) => {
            let value = i32::try_from(signed_literal(literal)?).map_err(|_| {
                CoveError::UnsupportedEncoding("date_days literal out of range".into())
            })?;
            CanonicalValue::DateDays(value).encode().map(Some)
        }
        (CoveLogicalType::TimestampMicros, literal) => {
            let value = i64::try_from(signed_literal(literal)?).map_err(|_| {
                CoveError::UnsupportedEncoding("timestamp_micros literal out of range".into())
            })?;
            CanonicalValue::TimestampMicros(value).encode().map(Some)
        }
        (CoveLogicalType::TimestampNanos, literal) => {
            let value = i64::try_from(signed_literal(literal)?).map_err(|_| {
                CoveError::UnsupportedEncoding("timestamp_nanos literal out of range".into())
            })?;
            CanonicalValue::TimestampNanos(value).encode().map(Some)
        }
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "unsupported FileCode literal for {logical:?}"
        ))),
    }
}

fn integer_width(logical: CoveLogicalType) -> u8 {
    match logical {
        CoveLogicalType::Int8 | CoveLogicalType::UInt8 => 1,
        CoveLogicalType::Int16 | CoveLogicalType::UInt16 => 2,
        CoveLogicalType::Int32 | CoveLogicalType::UInt32 => 4,
        _ => 8,
    }
}

fn signed_literal(literal: &LowerLiteral) -> Result<i128, CoveError> {
    match literal {
        LowerLiteral::Int128(value) => Ok(*value),
        LowerLiteral::Int64(value) => Ok(i128::from(*value)),
        LowerLiteral::UInt64(value) => Ok(i128::from(*value)),
        LowerLiteral::Utf8(value) => value.parse::<i128>().map_err(|_| {
            CoveError::UnsupportedEncoding("literal cannot be parsed as signed integer".into())
        }),
        _ => Err(CoveError::UnsupportedEncoding(
            "literal is not a signed integer".into(),
        )),
    }
}

fn unsigned_literal(literal: &LowerLiteral) -> Result<u128, CoveError> {
    match literal {
        LowerLiteral::Int128(value) => u128::try_from(*value).map_err(|_| {
            CoveError::UnsupportedEncoding("negative literal for unsigned type".into())
        }),
        LowerLiteral::Int64(value) => u128::try_from(*value).map_err(|_| {
            CoveError::UnsupportedEncoding("negative literal for unsigned type".into())
        }),
        LowerLiteral::UInt64(value) => Ok(u128::from(*value)),
        LowerLiteral::Utf8(value) => value.parse::<u128>().map_err(|_| {
            CoveError::UnsupportedEncoding("literal cannot be parsed as unsigned integer".into())
        }),
        _ => Err(CoveError::UnsupportedEncoding(
            "literal is not an unsigned integer".into(),
        )),
    }
}

fn parse_uuid_literal(value: &str) -> Result<[u8; 16], CoveError> {
    let value = value.trim();
    let compact = value.replace('-', "");
    if compact.len() != 32 {
        return Err(CoveError::UnsupportedEncoding(
            "uuid literal must contain 32 hex characters".into(),
        ));
    }
    let mut out = [0u8; 16];
    for (index, chunk) in compact.as_bytes().chunks_exact(2).enumerate() {
        out[index] = (hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?;
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Result<u8, CoveError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(CoveError::UnsupportedEncoding(
            "uuid literal contains invalid hex".into(),
        )),
    }
}

fn numeric_literal(literal: &LowerLiteral) -> Option<PredicateLiteral> {
    match literal {
        LowerLiteral::Int128(value) => i64::try_from(*value).ok().map(PredicateLiteral::Int64),
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
