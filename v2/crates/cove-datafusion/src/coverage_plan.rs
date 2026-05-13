//! DataFusion-side planning for COVE-COVERAGE predicate metadata.

use std::collections::BTreeSet;

use cove_core::{
    canonical::CanonicalValue,
    constants::{CoveLogicalType, CovePhysicalKind},
};
use cove_coverage::{
    PredicateAstPayloadV2, PredicateNullPolicyV2, PredicateOpV2,
    COVERAGE_PLAN_FLAG_MAY_UNDER_INCLUDE, COVERAGE_PLAN_FLAG_PRUNING_CANDIDATE,
};

use crate::{
    dataset_state::DatasetState,
    planner::{
        CovePredicate, FilterPlan, NullPredicateKind, NumericPredicateOp, PredicateLiteral,
        ScanPlan,
    },
};

const ABSENT_ID: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoveragePredicateExpr {
    Atom(CoveragePredicateAtom),
    And(Vec<CoveragePredicateExpr>),
    Or(Vec<CoveragePredicateExpr>),
}

impl CoveragePredicateExpr {
    fn atom(atom: CoveragePredicateAtom) -> Self {
        Self::Atom(atom)
    }

    fn and(mut conjuncts: Vec<Self>) -> Option<Self> {
        conjuncts.retain(|expr| match expr {
            Self::And(children) => !children.is_empty(),
            _ => true,
        });
        match conjuncts.len() {
            0 => None,
            1 => conjuncts.into_iter().next(),
            _ => Some(Self::And(conjuncts)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveragePredicateAtom {
    pub predicate_form_ref: u32,
    pub column_index: usize,
    pub column_id: u32,
    pub cache_coverage_set_refs: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoveragePlanDecision {
    Pruned,
    Included,
    Unknown,
}

#[derive(Debug, Clone, Default)]
pub struct CoveragePlanningIndex {
    forms: Vec<PredicateFormMatch>,
    safe_candidate_provider_ids: BTreeSet<u32>,
    cache: crate::dataset_state::CoverageCacheMetadata,
}

impl CoveragePlanningIndex {
    pub fn build(state: &DatasetState) -> Self {
        let mut safe_candidate_provider_ids = BTreeSet::new();
        for candidate in state.pruning().coverage_plan_candidates.iter() {
            let pruning = candidate.flags & COVERAGE_PLAN_FLAG_PRUNING_CANDIDATE != 0;
            let under_include = candidate.flags & COVERAGE_PLAN_FLAG_MAY_UNDER_INCLUDE != 0;
            if pruning && !under_include {
                safe_candidate_provider_ids.insert(candidate.provider_id);
            }
        }

        let mut forms = Vec::new();
        for form_with_payload in state.pruning().predicate_forms_with_payloads.iter() {
            if form_with_payload.payload.is_empty() {
                continue;
            }
            if form_with_payload.form.logical_context_ref != state.table().table_id {
                continue;
            }
            let Ok(payload) = PredicateAstPayloadV2::parse(&form_with_payload.payload) else {
                continue;
            };
            let Some(form_match) = PredicateFormMatch::from_ast(
                state,
                form_with_payload.form.predicate_form_id,
                &payload,
                &form_with_payload.payload,
            ) else {
                continue;
            };
            forms.push(form_match);
        }

        forms.sort_by_key(|form| {
            (
                !form_has_safe_candidate_provider(
                    state,
                    form.predicate_form_ref,
                    &safe_candidate_provider_ids,
                ),
                form.predicate_form_ref,
            )
        });

        Self {
            forms,
            safe_candidate_provider_ids,
            cache: state.coverage_cache().clone(),
        }
    }

    pub fn attach_to_filters(&self, filters: &mut [FilterPlan]) -> Option<CoveragePredicateExpr> {
        let mut conjuncts = Vec::new();
        for filter in filters {
            let Some(atom) = self.atom_for_filter(filter) else {
                continue;
            };
            if self.cache.enabled() {
                if atom.cache_coverage_set_refs.is_empty() {
                    self.cache.record_miss();
                } else {
                    self.cache.record_hit();
                }
            }
            filter.coverage_predicate_form_ref = Some(atom.predicate_form_ref);
            conjuncts.push(CoveragePredicateExpr::atom(atom));
        }
        CoveragePredicateExpr::and(conjuncts)
    }

    pub fn atom_for_filter(&self, filter: &FilterPlan) -> Option<CoveragePredicateAtom> {
        let predicate = filter.predicate.as_ref()?;
        self.forms
            .iter()
            .find(|form| form.matches(predicate))
            .map(|form| CoveragePredicateAtom {
                predicate_form_ref: form.predicate_form_ref,
                column_index: form.column_index,
                column_id: form.column_id,
                cache_coverage_set_refs: self
                    .cache
                    .coverage_set_refs_for_predicate(form.predicate_form_ref),
            })
    }

    pub fn has_safe_candidate_provider(&self, provider_id: u32) -> bool {
        self.safe_candidate_provider_ids.contains(&provider_id)
    }
}

pub(crate) fn refresh_scan_plan_coverage(state: &DatasetState, plan: &mut ScanPlan) {
    let index = CoveragePlanningIndex::build(state);
    plan.coverage_expr = index.attach_to_filters(&mut plan.filters);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PredicateFormMatch {
    predicate_form_ref: u32,
    column_index: usize,
    column_id: u32,
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
    op: CoverageAtomOp,
    literal: Option<Vec<u8>>,
}

impl PredicateFormMatch {
    fn from_ast(
        state: &DatasetState,
        predicate_form_ref: u32,
        payload: &PredicateAstPayloadV2,
        raw_payload: &[u8],
    ) -> Option<Self> {
        let node = payload
            .nodes
            .iter()
            .find(|node| node.node_id == payload.header.root_node_id)?;
        if node.flags != 0
            || node.function_ref != ABSENT_ID
            || node.aux_ref != ABSENT_ID
            || node.null_policy != PredicateNullPolicyV2::SqlWhere
        {
            return None;
        }
        let op = CoverageAtomOp::from_predicate_op(node.op)?;
        let column_id = column_ref_for_node(payload, node)?;
        let column_index = state
            .table()
            .columns
            .iter()
            .position(|column| column.column_id == column_id)?;
        let column = &state.table().columns[column_index];
        if node.collation_id != column.collation_id {
            return None;
        }
        match op {
            CoverageAtomOp::IsNull | CoverageAtomOp::IsNotNull => {
                if node.literal_ref != ABSENT_ID {
                    return None;
                }
                Some(Self {
                    predicate_form_ref,
                    column_index,
                    column_id,
                    logical: column.logical,
                    physical: column.physical,
                    op,
                    literal: None,
                })
            }
            _ => {
                let literal = literal_ref_for_node(payload, node)?;
                if literal.logical_type != column.logical as u16 {
                    return None;
                }
                let start = usize::try_from(literal.canonical_value_offset).ok()?;
                let end = start.checked_add(literal.canonical_value_length as usize)?;
                let bytes = raw_payload.get(start..end)?.to_vec();
                Some(Self {
                    predicate_form_ref,
                    column_index,
                    column_id,
                    logical: column.logical,
                    physical: column.physical,
                    op,
                    literal: Some(bytes),
                })
            }
        }
    }

    fn matches(&self, predicate: &CovePredicate) -> bool {
        match predicate {
            CovePredicate::Null { column_index, kind } => {
                self.column_index == *column_index
                    && self.literal.is_none()
                    && self.op == CoverageAtomOp::from_null_kind(*kind)
            }
            CovePredicate::Numeric {
                column_index,
                op,
                literal,
            } => {
                self.column_index == *column_index
                    && self.op == CoverageAtomOp::from_numeric_op(*op)
                    && self.literal.as_deref()
                        == canonical_numeric_literal(self.logical, *literal).as_deref()
            }
            CovePredicate::FileCodeIn {
                column_index,
                canonical_values,
                ..
            } => {
                self.column_index == *column_index
                    && self.op == CoverageAtomOp::Eq
                    && canonical_values.len() == 1
                    && self.literal.as_deref() == Some(canonical_values[0].as_slice())
            }
            CovePredicate::VarBytesEq {
                column_index,
                literal,
            } => {
                let literal = match (self.logical, self.physical) {
                    (CoveLogicalType::Utf8, CovePhysicalKind::VarBytes) => {
                        let value = std::str::from_utf8(literal).ok();
                        value.and_then(|value| CanonicalValue::Utf8(value).encode().ok())
                    }
                    (CoveLogicalType::Binary, CovePhysicalKind::VarBytes) => {
                        CanonicalValue::Bytes(literal).encode().ok()
                    }
                    _ => None,
                };
                self.column_index == *column_index
                    && self.op == CoverageAtomOp::Eq
                    && self.literal.as_deref() == literal.as_deref()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoverageAtomOp {
    Eq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    IsNull,
    IsNotNull,
}

impl CoverageAtomOp {
    fn from_predicate_op(op: PredicateOpV2) -> Option<Self> {
        match op {
            PredicateOpV2::Eq => Some(Self::Eq),
            PredicateOpV2::Lt => Some(Self::Lt),
            PredicateOpV2::LtEq => Some(Self::LtEq),
            PredicateOpV2::Gt => Some(Self::Gt),
            PredicateOpV2::GtEq => Some(Self::GtEq),
            PredicateOpV2::IsNull => Some(Self::IsNull),
            PredicateOpV2::IsNotNull => Some(Self::IsNotNull),
            _ => None,
        }
    }

    fn from_numeric_op(op: NumericPredicateOp) -> Self {
        match op {
            NumericPredicateOp::Eq => Self::Eq,
            NumericPredicateOp::Lt => Self::Lt,
            NumericPredicateOp::LtEq => Self::LtEq,
            NumericPredicateOp::Gt => Self::Gt,
            NumericPredicateOp::GtEq => Self::GtEq,
        }
    }

    fn from_null_kind(kind: NullPredicateKind) -> Self {
        match kind {
            NullPredicateKind::IsNull => Self::IsNull,
            NullPredicateKind::IsNotNull => Self::IsNotNull,
        }
    }
}

fn form_has_safe_candidate_provider(
    state: &DatasetState,
    predicate_form_ref: u32,
    safe_candidate_provider_ids: &BTreeSet<u32>,
) -> bool {
    state.pruning().coverage_proofs.iter().any(|proof| {
        proof.predicate_form_ref == predicate_form_ref
            && safe_candidate_provider_ids.contains(&proof.provider_id)
    })
}

fn column_ref_for_node(
    payload: &PredicateAstPayloadV2,
    node: &cove_coverage::PredicateAstNodeV2,
) -> Option<u32> {
    if node.column_or_path_ref != ABSENT_ID {
        return Some(node.column_or_path_ref);
    }
    payload
        .operand_refs
        .iter()
        .find(|operand| {
            operand.parent_node_id == node.node_id
                && operand.operand_kind == cove_coverage::PredicateOperandKindV2::ColumnOrPath
        })
        .map(|operand| operand.ref_id)
}

fn literal_ref_for_node<'a>(
    payload: &'a PredicateAstPayloadV2,
    node: &cove_coverage::PredicateAstNodeV2,
) -> Option<&'a cove_coverage::PredicateLiteralV2> {
    let literal_ref = if node.literal_ref != ABSENT_ID {
        node.literal_ref
    } else {
        payload
            .operand_refs
            .iter()
            .find(|operand| {
                operand.parent_node_id == node.node_id
                    && operand.operand_kind == cove_coverage::PredicateOperandKindV2::Literal
            })?
            .ref_id
    };
    payload
        .literals
        .iter()
        .find(|literal| literal.literal_id == literal_ref)
}

fn canonical_numeric_literal(
    logical: CoveLogicalType,
    literal: PredicateLiteral,
) -> Option<Vec<u8>> {
    match (logical, literal) {
        (CoveLogicalType::Int64, PredicateLiteral::Int64(value)) => CanonicalValue::Int {
            width: 8,
            value: i128::from(value),
        }
        .encode()
        .ok(),
        (CoveLogicalType::UInt64, PredicateLiteral::UInt64(value)) => CanonicalValue::Uint {
            width: 8,
            value: u128::from(value),
        }
        .encode()
        .ok(),
        (CoveLogicalType::Float64, PredicateLiteral::Float64(value)) => {
            CanonicalValue::Float64(value).encode().ok()
        }
        _ => None,
    }
}
