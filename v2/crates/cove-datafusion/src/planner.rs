//! DataFusion-agnostic scan planning.

use arrow_schema::SchemaRef;
#[cfg(feature = "covi")]
use cove_core::{
    canonical::CanonicalValue,
    constants::{CoveLogicalType, ValueTag},
};
use cove_core::{constants::CovePhysicalKind, CoveError};
#[cfg(feature = "covi")]
use cove_index::execution::{CoviLookupKeyV2, CoviLookupRequestV2};

use crate::{
    coverage_plan::{CoveragePlanningIndex, CoveragePredicateExpr},
    dataset_state::DatasetState,
    execution_code,
    scan_program::{
        compile_scan_program, order_filters_by_cost, promote_filter_exactness, CoveScanProgram,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoveFilterUse {
    Unsupported,
    PruningOnly,
    FullRowPredicateExact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NullPredicateKind {
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericPredicateOp {
    Eq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

#[derive(Debug, Clone, Copy)]
pub enum PredicateLiteral {
    Int64(i64),
    UInt64(u64),
    Float64(f64),
}

impl PredicateLiteral {
    pub fn normalized(self) -> Self {
        match self {
            Self::Float64(value) if value == 0.0 => Self::Float64(0.0),
            _ => self,
        }
    }
}

impl PartialEq for PredicateLiteral {
    fn eq(&self, other: &Self) -> bool {
        match (*self, *other) {
            (Self::Int64(left), Self::Int64(right)) => left == right,
            (Self::UInt64(left), Self::UInt64(right)) => left == right,
            (Self::Float64(left), Self::Float64(right)) => {
                left.normalized_for_comparison().to_bits()
                    == right.normalized_for_comparison().to_bits()
            }
            _ => false,
        }
    }
}

impl Eq for PredicateLiteral {}

trait NormalizePredicateFloat {
    fn normalized_for_comparison(self) -> Self;
}

impl NormalizePredicateFloat for f64 {
    fn normalized_for_comparison(self) -> Self {
        if self == 0.0 {
            0.0
        } else {
            self
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnPlan {
    pub output_columns: Vec<usize>,
    pub predicate_columns: Vec<usize>,
    pub materialization_columns: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopNScanHint {
    pub column_index: usize,
    pub descending: bool,
    pub fetch: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CovePredicate {
    Null {
        column_index: usize,
        kind: NullPredicateKind,
    },
    Numeric {
        column_index: usize,
        op: NumericPredicateOp,
        literal: PredicateLiteral,
    },
    FileCodeIn {
        column_index: usize,
        /// Derived per-file execution codes for the current dataset view.
        file_codes: Vec<u32>,
        /// Canonical dictionary values are the source of truth and must be
        /// resolved against each concrete file before pruning or execution.
        canonical_values: Vec<Vec<u8>>,
    },
    VarBytesEq {
        column_index: usize,
        literal: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CovePredicateExpr {
    Atom(CovePredicate),
    And(Vec<CovePredicateExpr>),
    Or(Vec<CovePredicateExpr>),
}

impl CovePredicateExpr {
    pub fn atom(predicate: CovePredicate) -> Self {
        Self::Atom(predicate)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterPlan {
    pub use_kind: CoveFilterUse,
    pub predicate_columns: Vec<usize>,
    pub display: String,
    pub predicate: Option<CovePredicate>,
    pub predicate_expr: Option<CovePredicateExpr>,
    pub coverage_predicate_form_ref: Option<u32>,
}

pub type PredicateProgram = Vec<FilterPlan>;

impl FilterPlan {
    pub fn unsupported(display: impl Into<String>) -> Self {
        Self {
            use_kind: CoveFilterUse::Unsupported,
            predicate_columns: Vec::new(),
            display: display.into(),
            predicate: None,
            predicate_expr: None,
            coverage_predicate_form_ref: None,
        }
    }

    pub fn pruning_expr(
        predicate_columns: Vec<usize>,
        expr: CovePredicateExpr,
        display: impl Into<String>,
    ) -> Self {
        Self {
            use_kind: CoveFilterUse::PruningOnly,
            predicate_columns,
            display: display.into(),
            predicate: None,
            predicate_expr: Some(expr),
            coverage_predicate_form_ref: None,
        }
    }

    pub fn pruning_null(
        column_index: usize,
        kind: NullPredicateKind,
        display: impl Into<String>,
    ) -> Self {
        Self {
            use_kind: CoveFilterUse::PruningOnly,
            predicate_columns: vec![column_index],
            display: display.into(),
            predicate: Some(CovePredicate::Null { column_index, kind }),
            predicate_expr: Some(CovePredicateExpr::atom(CovePredicate::Null {
                column_index,
                kind,
            })),
            coverage_predicate_form_ref: None,
        }
    }

    pub fn pruning_numeric(
        column_index: usize,
        op: NumericPredicateOp,
        literal: PredicateLiteral,
        display: impl Into<String>,
    ) -> Self {
        Self {
            use_kind: CoveFilterUse::PruningOnly,
            predicate_columns: vec![column_index],
            display: display.into(),
            predicate: Some(CovePredicate::Numeric {
                column_index,
                op,
                literal: literal.normalized(),
            }),
            predicate_expr: Some(CovePredicateExpr::atom(CovePredicate::Numeric {
                column_index,
                op,
                literal: literal.normalized(),
            })),
            coverage_predicate_form_ref: None,
        }
    }

    pub fn pruning_file_code_in(
        column_index: usize,
        file_codes: Vec<u32>,
        display: impl Into<String>,
    ) -> Self {
        Self::pruning_file_code_in_with_canonical(column_index, file_codes, Vec::new(), display)
    }

    pub fn pruning_file_code_in_with_canonical(
        column_index: usize,
        mut file_codes: Vec<u32>,
        mut canonical_values: Vec<Vec<u8>>,
        display: impl Into<String>,
    ) -> Self {
        file_codes.sort_unstable();
        file_codes.dedup();
        canonical_values.sort();
        canonical_values.dedup();
        let predicate = CovePredicate::FileCodeIn {
            column_index,
            file_codes,
            canonical_values,
        };
        Self {
            use_kind: CoveFilterUse::PruningOnly,
            predicate_columns: vec![column_index],
            display: display.into(),
            predicate: Some(predicate.clone()),
            predicate_expr: Some(CovePredicateExpr::atom(predicate)),
            coverage_predicate_form_ref: None,
        }
    }

    pub fn pruning_varbytes_eq(
        column_index: usize,
        literal: Vec<u8>,
        display: impl Into<String>,
    ) -> Self {
        let predicate = CovePredicate::VarBytesEq {
            column_index,
            literal,
        };
        Self {
            use_kind: CoveFilterUse::PruningOnly,
            predicate_columns: vec![column_index],
            display: display.into(),
            predicate: Some(predicate.clone()),
            predicate_expr: Some(CovePredicateExpr::atom(predicate)),
            coverage_predicate_form_ref: None,
        }
    }

    pub fn with_coverage_predicate_form_ref(mut self, predicate_form_ref: u32) -> Self {
        self.coverage_predicate_form_ref = Some(predicate_form_ref);
        self
    }
}

#[derive(Debug, Clone)]
pub struct ScanPlan {
    pub scan_projection: Vec<usize>,
    pub output_schema: SchemaRef,
    pub filters: PredicateProgram,
    pub predicate_columns: Vec<usize>,
    pub column_plan: ColumnPlan,
    pub topn_hint: Option<TopNScanHint>,
    pub coverage_expr: Option<CoveragePredicateExpr>,
    pub scan_program: CoveScanProgram,
    pub covi_candidates: Option<Vec<ScanCandidateRowRange>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanCandidateRowRange {
    pub segment_id: u32,
    pub morsel_id: u32,
    pub row_start: u64,
    pub row_count: u64,
}

/// Build a single-file native scan plan. The scan projection is the only set
/// of columns materialized by execution; predicate-only columns are available
/// to pruning code through page-index metadata.
pub fn plan_scan(
    state: &DatasetState,
    projection: Option<&Vec<usize>>,
    mut filters: PredicateProgram,
) -> Result<ScanPlan, CoveError> {
    let scan_projection = projection
        .cloned()
        .unwrap_or_else(|| state.full_projection());
    validate_column_indexes(state, "scan projection", &scan_projection)?;

    let mut predicate_columns = Vec::new();
    for filter in &filters {
        validate_column_indexes(state, "predicate columns", &filter.predicate_columns)?;
        for column in &filter.predicate_columns {
            if !predicate_columns.contains(column) {
                predicate_columns.push(*column);
            }
        }
    }

    let output_schema = state.projected_schema(&scan_projection)?;
    let mut materialization_columns = scan_projection.clone();
    for column in &predicate_columns {
        if !materialization_columns.contains(column) {
            materialization_columns.push(*column);
        }
    }
    let column_plan = ColumnPlan {
        output_columns: scan_projection.clone(),
        predicate_columns: predicate_columns.clone(),
        materialization_columns,
    };
    validate_filter_shapes(state, &filters)?;
    for filter in &mut filters {
        promote_filter_exactness(state, filter);
    }
    let coverage_index = CoveragePlanningIndex::build(state);
    let coverage_expr = coverage_index.attach_to_filters(&mut filters);
    let predicate_ordered = order_filters_by_cost(&mut filters);
    execution_code::validate_policy_for_filters(state, &filters)?;
    let mut scan_program = compile_scan_program(state, &filters);
    scan_program.predicate_ordered = predicate_ordered;
    let covi_candidates = covi_candidates_for_filters(state, &filters);
    Ok(ScanPlan {
        scan_projection,
        output_schema,
        filters,
        predicate_columns,
        column_plan,
        topn_hint: None,
        coverage_expr,
        scan_program,
        covi_candidates,
    })
}

#[cfg(feature = "covi")]
pub(crate) fn covi_candidates_for_filters(
    state: &DatasetState,
    filters: &[FilterPlan],
) -> Option<Vec<ScanCandidateRowRange>> {
    let covi = state.covi()?;
    for filter in filters {
        let Some(predicate) = &filter.predicate else {
            continue;
        };
        let Some((column_index, keys)) = lookup_keys_for_predicate(state, predicate) else {
            continue;
        };
        let column = &state.table().columns[column_index];
        let mut rows = Vec::new();
        let request = if keys.len() == 1 {
            CoviLookupRequestV2::eq(
                state.table().table_id,
                column.column_id,
                CoviLookupKeyV2::CanonicalValueBytes(keys[0].clone()),
            )
        } else {
            CoviLookupRequestV2::membership(
                state.table().table_id,
                column.column_id,
                keys.into_iter()
                    .map(CoviLookupKeyV2::CanonicalValueBytes)
                    .collect::<Vec<_>>(),
            )
        };
        let Ok(candidates) = covi.lookup(&request) else {
            continue;
        };
        rows.extend(
            candidates
                .row_ranges
                .into_iter()
                .map(|range| ScanCandidateRowRange {
                    segment_id: range.segment_id,
                    morsel_id: range.morsel_id,
                    row_start: range.row_start,
                    row_count: range.row_count,
                }),
        );
        normalize_scan_candidate_ranges(&mut rows).ok()?;
        return Some(rows);
    }
    None
}

#[cfg(not(feature = "covi"))]
pub(crate) fn covi_candidates_for_filters(
    _state: &DatasetState,
    _filters: &[FilterPlan],
) -> Option<Vec<ScanCandidateRowRange>> {
    None
}

#[cfg(feature = "covi")]
fn lookup_keys_for_predicate(
    state: &DatasetState,
    predicate: &CovePredicate,
) -> Option<(usize, Vec<Vec<u8>>)> {
    match predicate {
        CovePredicate::FileCodeIn {
            column_index,
            canonical_values,
            ..
        } => {
            let column = &state.table().columns[*column_index];
            let tag = value_tag_for_logical(column.logical)?;
            let keys = canonical_values
                .iter()
                .map(|value| tagged_key(tag, value.clone()))
                .collect::<Vec<_>>();
            Some((*column_index, keys))
        }
        CovePredicate::VarBytesEq {
            column_index,
            literal,
        } => {
            let column = &state.table().columns[*column_index];
            let key = match column.logical {
                CoveLogicalType::Utf8 => {
                    let value = std::str::from_utf8(literal).ok()?;
                    tagged_canonical(CanonicalValue::Utf8(value)).ok()?
                }
                CoveLogicalType::Binary => tagged_canonical(CanonicalValue::Bytes(literal)).ok()?,
                _ => return None,
            };
            Some((*column_index, vec![key]))
        }
        CovePredicate::Numeric {
            column_index,
            op: NumericPredicateOp::Eq,
            literal,
        } => {
            let column = &state.table().columns[*column_index];
            let key = numeric_canonical_key(column.logical, *literal).ok()?;
            Some((*column_index, vec![key]))
        }
        _ => None,
    }
}

#[cfg(feature = "covi")]
fn normalize_scan_candidate_ranges(rows: &mut Vec<ScanCandidateRowRange>) -> Result<(), CoveError> {
    rows.sort_by_key(|row| (row.segment_id, row.morsel_id, row.row_start));
    let mut out: Vec<ScanCandidateRowRange> = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        if row.row_count == 0 {
            return Err(CoveError::BadCovi);
        }
        if let Some(last) = out.last_mut() {
            let same_scope = last.segment_id == row.segment_id && last.morsel_id == row.morsel_id;
            let last_end = last
                .row_start
                .checked_add(last.row_count)
                .ok_or(CoveError::ArithOverflow)?;
            if same_scope && row.row_start <= last_end {
                let row_end = row
                    .row_start
                    .checked_add(row.row_count)
                    .ok_or(CoveError::ArithOverflow)?;
                last.row_count = row_end
                    .checked_sub(last.row_start)
                    .ok_or(CoveError::ArithOverflow)?;
                continue;
            }
        }
        out.push(row);
    }
    *rows = out;
    Ok(())
}

#[cfg(feature = "covi")]
fn value_tag_for_logical(logical: CoveLogicalType) -> Option<ValueTag> {
    match logical {
        CoveLogicalType::Utf8 => Some(ValueTag::Utf8),
        CoveLogicalType::Binary => Some(ValueTag::Binary),
        CoveLogicalType::Bool => Some(ValueTag::BoolTrue),
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64 => Some(ValueTag::Int64),
        CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64 => Some(ValueTag::UInt64),
        CoveLogicalType::Float32 => Some(ValueTag::Float32Bits),
        CoveLogicalType::Float64 => Some(ValueTag::Float64Bits),
        CoveLogicalType::DateDays => Some(ValueTag::DateDays),
        CoveLogicalType::TimestampMicros => Some(ValueTag::TimestampMicros),
        CoveLogicalType::TimestampNanos => Some(ValueTag::TimestampNanos),
        CoveLogicalType::Uuid => Some(ValueTag::Uuid),
        CoveLogicalType::Json => Some(ValueTag::Json),
        CoveLogicalType::Decimal64 => Some(ValueTag::Decimal64),
        CoveLogicalType::Decimal128 => Some(ValueTag::Decimal128),
        _ => None,
    }
}

#[cfg(feature = "covi")]
fn tagged_canonical(value: CanonicalValue<'_>) -> Result<Vec<u8>, CoveError> {
    let tag = value.value_tag();
    let payload = value.encode()?;
    Ok(tagged_key(tag, payload))
}

#[cfg(feature = "covi")]
fn tagged_key(tag: ValueTag, payload: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + payload.len());
    out.extend_from_slice(&(tag as u16).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

#[cfg(feature = "covi")]
fn numeric_canonical_key(
    logical: CoveLogicalType,
    literal: PredicateLiteral,
) -> Result<Vec<u8>, CoveError> {
    let value = match (logical, literal) {
        (CoveLogicalType::Int64, PredicateLiteral::Int64(value)) => CanonicalValue::Int {
            width: 8,
            value: i128::from(value),
        },
        (CoveLogicalType::UInt64, PredicateLiteral::UInt64(value)) => CanonicalValue::Uint {
            width: 8,
            value: u128::from(value),
        },
        (CoveLogicalType::Float64, PredicateLiteral::Float64(value)) => {
            CanonicalValue::Float64(value)
        }
        _ => return Err(CoveError::BadCovi),
    };
    tagged_canonical(value)
}

fn validate_column_indexes(
    state: &DatasetState,
    label: &str,
    indexes: &[usize],
) -> Result<(), CoveError> {
    for index in indexes {
        if *index >= state.table().columns.len() {
            return Err(CoveError::BadSchema(format!(
                "{label} index {index} is out of bounds for {} columns",
                state.table().columns.len()
            )));
        }
    }
    Ok(())
}

fn validate_filter_shapes(state: &DatasetState, filters: &[FilterPlan]) -> Result<(), CoveError> {
    for filter in filters {
        match &filter.predicate {
            Some(CovePredicate::FileCodeIn {
                column_index,
                file_codes,
                ..
            }) => {
                let column = &state.table().columns[*column_index];
                if column.physical != CovePhysicalKind::FileCode {
                    return Err(CoveError::BadSchema(format!(
                        "FileCode predicate planned for non-FileCode column {}",
                        column.name
                    )));
                }
                if file_codes.is_empty() {
                    // Empty IN is valid as an optimization: it selects no rows.
                    continue;
                }
            }
            Some(CovePredicate::Numeric { column_index, .. }) => {
                let column = &state.table().columns[*column_index];
                if column.physical != CovePhysicalKind::NumCode {
                    return Err(CoveError::BadSchema(format!(
                        "numeric predicate planned for non-NumCode column {}",
                        column.name
                    )));
                }
            }
            Some(CovePredicate::VarBytesEq { column_index, .. }) => {
                let column = &state.table().columns[*column_index];
                if column.physical != CovePhysicalKind::VarBytes {
                    return Err(CoveError::BadSchema(format!(
                        "VarBytes predicate planned for non-VarBytes column {}",
                        column.name
                    )));
                }
            }
            _ => {}
        }
        if let Some(CovePredicate::Numeric {
            literal: PredicateLiteral::Float64(value),
            ..
        }) = filter.predicate
        {
            if value.is_nan() {
                return Err(CoveError::BadSchema(
                    "numeric predicate literal must not be NaN".into(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::PredicateLiteral;

    #[test]
    fn float_predicate_literals_compare_deterministically() {
        assert_eq!(
            PredicateLiteral::Float64(-0.0),
            PredicateLiteral::Float64(0.0)
        );
        assert_eq!(
            PredicateLiteral::Float64(f64::from_bits(0x7ff8_0000_0000_0001)),
            PredicateLiteral::Float64(f64::from_bits(0x7ff8_0000_0000_0001))
        );
        assert_ne!(
            PredicateLiteral::Float64(f64::from_bits(0x7ff8_0000_0000_0001)),
            PredicateLiteral::Float64(f64::from_bits(0x7ff8_0000_0000_0002))
        );
    }
}
