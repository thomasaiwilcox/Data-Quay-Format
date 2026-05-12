//! DataFusion-agnostic scan planning.

use arrow_schema::SchemaRef;
use cove_core::{constants::CovePhysicalKind, CoveError};

use crate::{
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
pub struct FilterPlan {
    pub use_kind: CoveFilterUse,
    pub predicate_columns: Vec<usize>,
    pub display: String,
    pub predicate: Option<CovePredicate>,
}

pub type PredicateProgram = Vec<FilterPlan>;

impl FilterPlan {
    pub fn unsupported(display: impl Into<String>) -> Self {
        Self {
            use_kind: CoveFilterUse::Unsupported,
            predicate_columns: Vec::new(),
            display: display.into(),
            predicate: None,
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
        Self {
            use_kind: CoveFilterUse::PruningOnly,
            predicate_columns: vec![column_index],
            display: display.into(),
            predicate: Some(CovePredicate::FileCodeIn {
                column_index,
                file_codes,
                canonical_values,
            }),
        }
    }

    pub fn pruning_varbytes_eq(
        column_index: usize,
        literal: Vec<u8>,
        display: impl Into<String>,
    ) -> Self {
        Self {
            use_kind: CoveFilterUse::PruningOnly,
            predicate_columns: vec![column_index],
            display: display.into(),
            predicate: Some(CovePredicate::VarBytesEq {
                column_index,
                literal,
            }),
        }
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
    pub scan_program: CoveScanProgram,
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
    let predicate_ordered = order_filters_by_cost(&mut filters);
    execution_code::validate_policy_for_filters(state, &filters)?;
    let mut scan_program = compile_scan_program(state, &filters);
    scan_program.predicate_ordered = predicate_ordered;
    Ok(ScanPlan {
        scan_projection,
        output_schema,
        filters,
        predicate_columns,
        column_plan,
        topn_hint: None,
        scan_program,
    })
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
