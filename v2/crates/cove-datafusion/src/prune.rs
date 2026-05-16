//! Candidate pruning for files, segments, morsels, and pages.

use cove_core::{
    index::{bloom::BloomHashDomain, inverted::InvertedKeyKind, lookup::LookupKeyKind},
    predicate::PredicateZoneOutcome,
    pruning::{
        explain_bloom_membership, explain_file_code_equality, explain_is_not_null, explain_is_null,
        explain_numcode_range,
    },
    zone_stats::NumericStatValue,
    CoveError,
};
use cove_coverage::{
    can_use_proof_for_pruning, coverage_set_payload_checksum, CoverageExactnessV2,
    CoverageGranularityV2, CoverageProofRecordV2, CoverageProofStrengthV2,
    CoverageProviderDescriptorV2, CoverageSetEntryV2, CoverageSetV2,
};

use crate::{
    coverage_plan::{CoveragePlanDecision, CoveragePredicateAtom, CoveragePredicateExpr},
    dataset_state::DatasetState,
    planner::{
        CovePredicate, FilterPlan, NullPredicateKind, NumericPredicateOp, PredicateLiteral,
        ScanPlan,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateSet {
    All {
        len: usize,
    },
    /// Sorted, deduplicated row indexes. Helpers normalize defensive copies
    /// when callers construct non-canonical sparse sets manually.
    Sparse(Vec<u32>),
    Bitmap {
        len: usize,
        bits: Vec<u64>,
    },
}

impl CandidateSet {
    pub fn all(len: usize) -> Self {
        Self::All { len }
    }

    pub fn empty() -> Self {
        Self::Sparse(Vec::new())
    }

    pub fn contains(&self, index: usize) -> bool {
        match self {
            CandidateSet::All { len } => index < *len,
            CandidateSet::Sparse(values) => sparse_contains(values, index as u32),
            CandidateSet::Bitmap { len, bits } => {
                if index >= *len {
                    return false;
                }
                candidate_bitmap_contains(bits, index)
            }
        }
    }

    pub fn len(&self) -> usize {
        match self {
            CandidateSet::All { len } => *len,
            CandidateSet::Sparse(values) => sparse_len(values),
            CandidateSet::Bitmap { len, bits } => bitmap_len(*len, bits),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn intersect(&self, other: &Self) -> Self {
        let max_len = self.domain_len().min(other.domain_len());
        if self.is_empty() || other.is_empty() || max_len == 0 {
            return Self::empty();
        }
        match (self, other) {
            (CandidateSet::All { .. }, _) => other.clamp(max_len),
            (_, CandidateSet::All { .. }) => self.clamp(max_len),
            (CandidateSet::Sparse(left), CandidateSet::Sparse(right)) => {
                Self::Sparse(intersect_sparse_sparse(left, right, max_len))
            }
            (
                CandidateSet::Bitmap {
                    bits: left_bits, ..
                },
                CandidateSet::Bitmap {
                    bits: right_bits, ..
                },
            ) => Self::Bitmap {
                len: max_len,
                bits: intersect_bitmap_bitmap(left_bits, right_bits, max_len),
            },
            (CandidateSet::Sparse(values), CandidateSet::Bitmap { len, bits })
            | (CandidateSet::Bitmap { len, bits }, CandidateSet::Sparse(values)) => {
                Self::Sparse(intersect_sparse_bitmap(values, *len, bits, max_len))
            }
        }
    }

    pub fn union(&self, other: &Self) -> Self {
        let max_len = self.domain_len().max(other.domain_len());
        match (self, other) {
            (CandidateSet::All { .. }, _) | (_, CandidateSet::All { .. }) => {
                Self::All { len: max_len }
            }
            (CandidateSet::Sparse(left), CandidateSet::Sparse(right)) => {
                Self::Sparse(union_sparse_sparse(left, right, max_len))
            }
            (
                CandidateSet::Bitmap {
                    bits: left_bits, ..
                },
                CandidateSet::Bitmap {
                    bits: right_bits, ..
                },
            ) => Self::Bitmap {
                len: max_len,
                bits: union_bitmap_bitmap(left_bits, right_bits, max_len),
            },
            (CandidateSet::Sparse(values), CandidateSet::Bitmap { bits, .. })
            | (CandidateSet::Bitmap { bits, .. }, CandidateSet::Sparse(values)) => Self::Bitmap {
                len: max_len,
                bits: union_bitmap_sparse(bits, values, max_len),
            },
        }
    }

    fn domain_len(&self) -> usize {
        match self {
            CandidateSet::All { len } | CandidateSet::Bitmap { len, .. } => *len,
            CandidateSet::Sparse(values) => sparse_domain_len(values),
        }
    }

    fn clamp(&self, len: usize) -> Self {
        match self {
            CandidateSet::All { .. } => Self::All { len },
            CandidateSet::Sparse(values) => Self::Sparse(clamp_sparse(values, len)),
            CandidateSet::Bitmap { bits, .. } => Self::Bitmap {
                len,
                bits: clamp_bitmap(bits, len),
            },
        }
    }
}

fn sparse_contains(values: &[u32], needle: u32) -> bool {
    if sparse_is_normalized(values) {
        values.binary_search(&needle).is_ok()
    } else {
        values.contains(&needle)
    }
}

fn sparse_len(values: &[u32]) -> usize {
    if sparse_is_normalized(values) {
        values.len()
    } else {
        normalize_sparse(values).len()
    }
}

fn sparse_domain_len(values: &[u32]) -> usize {
    if sparse_is_normalized(values) {
        values.last().map(|value| *value as usize + 1).unwrap_or(0)
    } else {
        values
            .iter()
            .copied()
            .max()
            .map(|value| value as usize + 1)
            .unwrap_or(0)
    }
}

fn sparse_is_normalized(values: &[u32]) -> bool {
    values.windows(2).all(|window| window[0] < window[1])
}

fn normalize_sparse(values: &[u32]) -> Vec<u32> {
    let mut normalized = values.to_vec();
    normalized.sort_unstable();
    normalized.dedup();
    normalized
}

fn clamp_sparse(values: &[u32], len: usize) -> Vec<u32> {
    let normalized = normalize_sparse(values);
    normalized
        .into_iter()
        .filter(|value| (*value as usize) < len)
        .collect()
}

fn intersect_sparse_sparse(left: &[u32], right: &[u32], max_len: usize) -> Vec<u32> {
    let left = normalize_sparse(left);
    let right = normalize_sparse(right);
    let mut out = Vec::new();
    let mut left_index = 0usize;
    let mut right_index = 0usize;
    while let (Some(left_value), Some(right_value)) = (left.get(left_index), right.get(right_index))
    {
        match left_value.cmp(right_value) {
            std::cmp::Ordering::Less => left_index += 1,
            std::cmp::Ordering::Greater => right_index += 1,
            std::cmp::Ordering::Equal => {
                if (*left_value as usize) < max_len {
                    out.push(*left_value);
                }
                left_index += 1;
                right_index += 1;
            }
        }
    }
    out
}

fn union_sparse_sparse(left: &[u32], right: &[u32], max_len: usize) -> Vec<u32> {
    let left = normalize_sparse(left);
    let right = normalize_sparse(right);
    let mut out = Vec::new();
    let mut left_index = 0usize;
    let mut right_index = 0usize;
    while let (Some(left_value), Some(right_value)) = (left.get(left_index), right.get(right_index))
    {
        match left_value.cmp(right_value) {
            std::cmp::Ordering::Less => {
                if (*left_value as usize) < max_len {
                    out.push(*left_value);
                }
                left_index += 1;
            }
            std::cmp::Ordering::Greater => {
                if (*right_value as usize) < max_len {
                    out.push(*right_value);
                }
                right_index += 1;
            }
            std::cmp::Ordering::Equal => {
                if (*left_value as usize) < max_len {
                    out.push(*left_value);
                }
                left_index += 1;
                right_index += 1;
            }
        }
    }
    out.extend(
        left[left_index..]
            .iter()
            .copied()
            .filter(|value| (*value as usize) < max_len),
    );
    out.extend(
        right[right_index..]
            .iter()
            .copied()
            .filter(|value| (*value as usize) < max_len),
    );
    out
}

fn intersect_sparse_bitmap(
    values: &[u32],
    bitmap_len: usize,
    bits: &[u64],
    max_len: usize,
) -> Vec<u32> {
    normalize_sparse(values)
        .into_iter()
        .filter(|value| {
            let index = *value as usize;
            index < max_len && index < bitmap_len && candidate_bitmap_contains(bits, index)
        })
        .collect()
}

fn bitmap_len(len: usize, bits: &[u64]) -> usize {
    let full_words = len / 64;
    let tail_bits = len % 64;
    let mut total = bits
        .iter()
        .take(full_words)
        .map(|word| word.count_ones() as usize)
        .sum::<usize>();
    if tail_bits != 0 {
        if let Some(word) = bits.get(full_words) {
            let mask = (1u64 << tail_bits) - 1;
            total += (word & mask).count_ones() as usize;
        }
    }
    total
}

fn candidate_bitmap_contains(bits: &[u64], index: usize) -> bool {
    let word = index / 64;
    let bit = index % 64;
    bits.get(word)
        .map(|value| value & (1u64 << bit) != 0)
        .unwrap_or(false)
}

fn bitmap_word_len(len: usize) -> usize {
    len.div_ceil(64)
}

fn clamp_bitmap(bits: &[u64], len: usize) -> Vec<u64> {
    let word_len = bitmap_word_len(len);
    let mut clamped = bits.iter().copied().take(word_len).collect::<Vec<_>>();
    if let Some(last) = clamped.last_mut() {
        let tail_bits = len % 64;
        if tail_bits != 0 {
            *last &= (1u64 << tail_bits) - 1;
        }
    }
    clamped
}

fn intersect_bitmap_bitmap(left_bits: &[u64], right_bits: &[u64], len: usize) -> Vec<u64> {
    let word_len = bitmap_word_len(len);
    let mut out = Vec::with_capacity(word_len);
    for word in 0..word_len {
        let left = left_bits.get(word).copied().unwrap_or(0);
        let right = right_bits.get(word).copied().unwrap_or(0);
        out.push(left & right);
    }
    clamp_bitmap(&out, len)
}

fn union_bitmap_bitmap(left_bits: &[u64], right_bits: &[u64], len: usize) -> Vec<u64> {
    let word_len = bitmap_word_len(len);
    let mut out = Vec::with_capacity(word_len);
    for word in 0..word_len {
        let left = left_bits.get(word).copied().unwrap_or(0);
        let right = right_bits.get(word).copied().unwrap_or(0);
        out.push(left | right);
    }
    clamp_bitmap(&out, len)
}

fn union_bitmap_sparse(bits: &[u64], values: &[u32], len: usize) -> Vec<u64> {
    let mut out = clamp_bitmap(bits, len);
    let word_len = bitmap_word_len(len);
    if out.len() < word_len {
        out.resize(word_len, 0);
    }
    for value in normalize_sparse(values) {
        let index = value as usize;
        if index >= len {
            continue;
        }
        let word = index / 64;
        let bit = index % 64;
        out[word] |= 1u64 << bit;
    }
    out
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneStats {
    pub morsels_considered: usize,
    pub morsels_pruned: usize,
}

pub fn morsel_pruned(
    state: &DatasetState,
    segment_id: u32,
    morsel_id: u32,
    plan: &ScanPlan,
) -> Result<bool, CoveError> {
    if composite_prunes_morsel(state, segment_id, morsel_id, plan) {
        return Ok(true);
    }
    if let Some(expr) = &plan.coverage_expr {
        if coverage_expr_decision(state, segment_id, morsel_id, expr)
            == CoveragePlanDecision::Pruned
        {
            return Ok(true);
        }
    }
    for filter in &plan.filters {
        if filter_prunes_morsel(state, segment_id, morsel_id, filter)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn composite_prunes_morsel(
    state: &DatasetState,
    segment_id: u32,
    morsel_id: u32,
    plan: &ScanPlan,
) -> bool {
    for index in state.composite_indexes() {
        let mut code_sets: Vec<&[u32]> = Vec::with_capacity(index.key_columns.len());
        for column_id in &index.key_columns {
            let Some(column_index) = state
                .table()
                .columns
                .iter()
                .position(|column| column.column_id == *column_id)
            else {
                code_sets.clear();
                break;
            };
            let Some(codes) = file_code_filter_for_column(plan, column_index) else {
                code_sets.clear();
                break;
            };
            if codes.is_empty() {
                return true;
            }
            code_sets.push(codes);
        }
        if code_sets.len() != index.key_columns.len() {
            continue;
        }
        match composite_tuple_has_match(index, &code_sets, segment_id, morsel_id) {
            Some(true) => return false,
            Some(false) => return true,
            None => continue,
        }
    }
    false
}

fn file_code_filter_for_column(plan: &ScanPlan, column_index: usize) -> Option<&[u32]> {
    plan.filters
        .iter()
        .find_map(|filter| match &filter.predicate {
            Some(CovePredicate::FileCodeIn {
                column_index: candidate,
                file_codes,
                ..
            }) if *candidate == column_index => Some(file_codes.as_slice()),
            _ => None,
        })
}

fn composite_tuple_has_match(
    index: &cove_core::index::composite::CompositeIndex,
    code_sets: &[&[u32]],
    segment_id: u32,
    morsel_id: u32,
) -> Option<bool> {
    let key_width = index.key_columns.len().checked_mul(8)?;
    let entry_width = key_width.checked_add(8)?;
    if entry_width == 8 || !index.entries.len().is_multiple_of(entry_width) {
        return None;
    }
    for entry in index.entries.chunks_exact(entry_width) {
        let entry_segment = u32::from_le_bytes(entry[key_width..key_width + 4].try_into().ok()?);
        let entry_morsel = u32::from_le_bytes(entry[key_width + 4..key_width + 8].try_into().ok()?);
        if entry_segment != segment_id || entry_morsel != morsel_id {
            continue;
        }
        let mut all_match = true;
        for (idx, codes) in code_sets.iter().enumerate() {
            let start = idx.checked_mul(8)?;
            let value = u64::from_le_bytes(entry[start..start + 8].try_into().ok()?);
            let Ok(code) = u32::try_from(value) else {
                all_match = false;
                break;
            };
            if codes.binary_search(&code).is_err() {
                all_match = false;
                break;
            }
        }
        if all_match {
            return Some(true);
        }
    }
    Some(false)
}

fn filter_prunes_morsel(
    state: &DatasetState,
    segment_id: u32,
    morsel_id: u32,
    filter: &FilterPlan,
) -> Result<bool, CoveError> {
    let Some(predicate) = &filter.predicate else {
        return Ok(false);
    };
    if coverage_proof_prunes_morsel(state, segment_id, morsel_id, filter, predicate) {
        return Ok(true);
    }
    match predicate {
        CovePredicate::Null { column_index, kind } => {
            let column = &state.table().columns[*column_index];
            let zone = state.zone_stats_for(segment_id, morsel_id, column.column_id);
            let outcome = match kind {
                NullPredicateKind::IsNull => explain_is_null(zone).final_outcome,
                NullPredicateKind::IsNotNull => explain_is_not_null(zone).final_outcome,
            };
            Ok(outcome == PredicateZoneOutcome::NoMatch)
        }
        CovePredicate::Numeric {
            column_index,
            op,
            literal,
        } => {
            let column = &state.table().columns[*column_index];
            let zone = state.zone_stats_for(segment_id, morsel_id, column.column_id);
            let (lower, lower_inclusive, upper, upper_inclusive) = numeric_bounds(*op, *literal)?;
            Ok(
                explain_numcode_range(lower, lower_inclusive, upper, upper_inclusive, zone)
                    .final_outcome
                    == PredicateZoneOutcome::NoMatch,
            )
        }
        CovePredicate::FileCodeIn {
            column_index,
            file_codes,
            ..
        } => {
            if file_codes.is_empty() {
                return Ok(true);
            }
            let column = &state.table().columns[*column_index];
            if lookup_prunes_file_codes(state, segment_id, morsel_id, column.column_id, file_codes)
            {
                return Ok(true);
            }
            if inverted_prunes_file_codes(
                state,
                segment_id,
                morsel_id,
                column.column_id,
                file_codes,
            ) {
                return Ok(true);
            }
            let zone = state.zone_stats_for(segment_id, morsel_id, column.column_id);
            let domain = state.column_domain_for(column.column_id);
            let exact_set = state.exact_set_for(column.column_id);
            let bloom = state.bloom_for(column.column_id);
            for file_code in file_codes {
                let explained =
                    explain_file_code_equality(*file_code, zone, domain, exact_set).final_outcome;
                if explained != PredicateZoneOutcome::NoMatch
                    && !bloom_excludes_file_code(state, column.column_id, *file_code, bloom)?
                {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        CovePredicate::VarBytesEq { .. } => Ok(false),
    }
}

fn coverage_proof_prunes_morsel(
    state: &DatasetState,
    segment_id: u32,
    morsel_id: u32,
    filter: &FilterPlan,
    predicate: &CovePredicate,
) -> bool {
    let Some(predicate_form_ref) = filter.coverage_predicate_form_ref else {
        return false;
    };
    let Some(column_index) = predicate_column_index(predicate) else {
        return false;
    };
    coverage_atom_prunes_morsel(
        state,
        segment_id,
        morsel_id,
        &CoveragePredicateAtom {
            predicate_form_ref,
            column_index,
            column_id: state
                .table()
                .columns
                .get(column_index)
                .map(|column| column.column_id)
                .unwrap_or(u32::MAX),
            cache_coverage_set_refs: Vec::new(),
        },
    ) == CoveragePlanDecision::Pruned
}

fn coverage_expr_decision(
    state: &DatasetState,
    segment_id: u32,
    morsel_id: u32,
    expr: &CoveragePredicateExpr,
) -> CoveragePlanDecision {
    match expr {
        CoveragePredicateExpr::Atom(atom) => {
            coverage_atom_prunes_morsel(state, segment_id, morsel_id, atom)
        }
        CoveragePredicateExpr::And(children) => {
            let mut saw_unknown = false;
            for child in children {
                match coverage_expr_decision(state, segment_id, morsel_id, child) {
                    CoveragePlanDecision::Pruned => return CoveragePlanDecision::Pruned,
                    CoveragePlanDecision::Included => {}
                    CoveragePlanDecision::Unknown => saw_unknown = true,
                }
            }
            if saw_unknown {
                CoveragePlanDecision::Unknown
            } else {
                CoveragePlanDecision::Included
            }
        }
        CoveragePredicateExpr::Or(children) => {
            let mut saw_unknown = false;
            for child in children {
                match coverage_expr_decision(state, segment_id, morsel_id, child) {
                    CoveragePlanDecision::Pruned => {}
                    CoveragePlanDecision::Included => return CoveragePlanDecision::Included,
                    CoveragePlanDecision::Unknown => saw_unknown = true,
                }
            }
            if saw_unknown {
                CoveragePlanDecision::Unknown
            } else {
                CoveragePlanDecision::Pruned
            }
        }
    }
}

fn coverage_atom_prunes_morsel(
    state: &DatasetState,
    segment_id: u32,
    morsel_id: u32,
    atom: &CoveragePredicateAtom,
) -> CoveragePlanDecision {
    let Ok(file) = state.file(0) else {
        return CoveragePlanDecision::Unknown;
    };
    match morsel_visibility_decision(state, file.visibility(), segment_id, morsel_id) {
        CoveragePlanDecision::Pruned => return CoveragePlanDecision::Pruned,
        CoveragePlanDecision::Included => {}
        CoveragePlanDecision::Unknown => return CoveragePlanDecision::Unknown,
    }
    if !state
        .pruning()
        .predicate_forms
        .iter()
        .any(|form| form.predicate_form_id == atom.predicate_form_ref)
        && !state
            .pruning()
            .predicate_forms_with_payloads
            .iter()
            .any(|form| form.form.predicate_form_id == atom.predicate_form_ref)
    {
        return CoveragePlanDecision::Unknown;
    };
    let Some(column) = state.table().columns.get(atom.column_index) else {
        return CoveragePlanDecision::Unknown;
    };
    let table_id = state.table().table_id;
    let column_id = column.column_id;
    if atom.column_id != column_id {
        return CoveragePlanDecision::Unknown;
    }

    match cached_coverage_atom_decision(state, table_id, segment_id, morsel_id, atom) {
        CoveragePlanDecision::Unknown => {}
        decision => return decision,
    }

    let mut saw_usable = false;
    for record in state
        .pruning()
        .coverage_proofs
        .iter()
        .filter(|record| record.predicate_form_ref == atom.predicate_form_ref)
    {
        if !coverage_record_usable_for_pruning(record) {
            continue;
        }
        let Some(provider) = state.pruning().coverage_providers.iter().find(|provider| {
            coverage_provider_matches_filter(
                provider,
                record,
                table_id,
                column_id,
                atom.predicate_form_ref,
            )
        }) else {
            continue;
        };
        let Some(set) = state
            .pruning()
            .coverage_sets
            .iter()
            .find(|set| set.header.coverage_set_id == record.coverage_set_id)
        else {
            continue;
        };
        if !coverage_record_matches_set(record, provider, set, atom.predicate_form_ref) {
            continue;
        }
        if !coverage_set_payload_matches(record, set) {
            continue;
        }
        if !coverage_set_supports_morsel_pruning(set) {
            continue;
        }
        saw_usable = true;
        if !coverage_set_covers_morsel(set, 0, table_id, segment_id, morsel_id) {
            return CoveragePlanDecision::Pruned;
        }
    }
    if saw_usable {
        CoveragePlanDecision::Included
    } else {
        CoveragePlanDecision::Unknown
    }
}

fn morsel_visibility_decision(
    state: &DatasetState,
    visibility: &crate::overlay::RowVisibility,
    segment_id: u32,
    morsel_id: u32,
) -> CoveragePlanDecision {
    if visibility.is_all() {
        return CoveragePlanDecision::Included;
    }
    let Some(segment) = state
        .segments()
        .iter()
        .find(|segment| segment.segment_id == segment_id)
    else {
        return CoveragePlanDecision::Unknown;
    };
    if segment.morsel_row_count == 0 || morsel_id >= segment.morsel_count {
        return CoveragePlanDecision::Unknown;
    }
    let first_row_in_segment = u64::from(morsel_id) * u64::from(segment.morsel_row_count);
    let remaining = u64::from(segment.row_count).saturating_sub(first_row_in_segment);
    if remaining == 0 {
        return CoveragePlanDecision::Unknown;
    }
    let row_count = remaining.min(u64::from(segment.morsel_row_count));
    let Ok(row_count) = u32::try_from(row_count) else {
        return CoveragePlanDecision::Unknown;
    };
    let Ok(absolute_start) = segment
        .row_start
        .checked_add(first_row_in_segment)
        .ok_or(CoveError::ArithOverflow)
    else {
        return CoveragePlanDecision::Unknown;
    };
    let total_rows = state.table().row_count;
    match visibility.hidden_rows_in_range(absolute_start, row_count, total_rows) {
        Ok(hidden) if hidden == row_count => CoveragePlanDecision::Pruned,
        Ok(0) => CoveragePlanDecision::Included,
        Ok(_) => CoveragePlanDecision::Unknown,
        Err(_) => CoveragePlanDecision::Unknown,
    }
}

fn cached_coverage_atom_decision(
    state: &DatasetState,
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,
    atom: &CoveragePredicateAtom,
) -> CoveragePlanDecision {
    if atom.cache_coverage_set_refs.is_empty() {
        return CoveragePlanDecision::Unknown;
    }
    let mut saw_usable = false;
    for coverage_set_ref in &atom.cache_coverage_set_refs {
        let Some(set) = state
            .pruning()
            .coverage_sets
            .iter()
            .find(|set| set.header.coverage_set_id == *coverage_set_ref)
        else {
            continue;
        };
        if set.header.predicate_form_ref != atom.predicate_form_ref
            || set.header.exactness.may_under_include()
            || !set.header.proof_strength.allows_pruning()
            || !coverage_set_supports_morsel_pruning(set)
        {
            continue;
        }
        saw_usable = true;
        if !coverage_set_covers_morsel(set, 0, table_id, segment_id, morsel_id) {
            return CoveragePlanDecision::Pruned;
        }
    }
    if saw_usable {
        CoveragePlanDecision::Included
    } else {
        CoveragePlanDecision::Unknown
    }
}

fn predicate_column_index(predicate: &CovePredicate) -> Option<usize> {
    match predicate {
        CovePredicate::Null { column_index, .. }
        | CovePredicate::Numeric { column_index, .. }
        | CovePredicate::FileCodeIn { column_index, .. }
        | CovePredicate::VarBytesEq { column_index, .. } => Some(*column_index),
    }
}

fn coverage_record_usable_for_pruning(record: &CoverageProofRecordV2) -> bool {
    can_use_proof_for_pruning(record)
        && coverage_strength_is_exact(record.proof_strength)
        && matches!(
            record.exactness,
            CoverageExactnessV2::Exact | CoverageExactnessV2::ApproximateOverInclusiveOnly
        )
}

fn coverage_provider_matches_filter(
    provider: &CoverageProviderDescriptorV2,
    record: &CoverageProofRecordV2,
    table_id: u32,
    column_id: u32,
    predicate_form_ref: u32,
) -> bool {
    provider.provider_id == record.provider_id
        && provider.referenced_table_id == table_id
        && provider.referenced_column_id == column_id
        && provider.snapshot_validity_ref == record.snapshot_validity_ref
        && provider.granularity == record.granularity
        && provider.proof_strength == record.proof_strength
        && provider.exactness == record.exactness
        && provider.null_semantics == record.null_semantics
        && provider.null_semantics != 255
        && coverage_strength_is_exact(provider.proof_strength)
        && !provider.exactness.may_under_include()
        && (provider.predicate_form_ref == u32::MAX
            || provider.predicate_form_ref == predicate_form_ref)
}

fn coverage_record_matches_set(
    record: &CoverageProofRecordV2,
    provider: &CoverageProviderDescriptorV2,
    set: &CoverageSetV2,
    predicate_form_ref: u32,
) -> bool {
    set.header.provider_id == provider.provider_id
        && set.header.provider_id == record.provider_id
        && set.header.coverage_set_id == record.coverage_set_id
        && set.header.predicate_form_ref == predicate_form_ref
        && set.header.snapshot_validity_ref == record.snapshot_validity_ref
        && set.header.snapshot_validity_ref == provider.snapshot_validity_ref
        && set.header.proof_strength == record.proof_strength
        && set.header.exactness == record.exactness
        && set.header.granularity == record.granularity
}

fn coverage_set_payload_matches(record: &CoverageProofRecordV2, set: &CoverageSetV2) -> bool {
    let Ok(bytes) = set.serialize() else {
        return false;
    };
    let checksum = coverage_set_payload_checksum(&bytes);
    record.validate_against_coverage_set(set, checksum).is_ok()
}

fn coverage_strength_is_exact(strength: CoverageProofStrengthV2) -> bool {
    matches!(
        strength,
        CoverageProofStrengthV2::ExactTight | CoverageProofStrengthV2::ExactConservative
    )
}

fn coverage_set_supports_morsel_pruning(set: &CoverageSetV2) -> bool {
    coverage_granularity_maps_to_morsels(set.header.granularity)
        && set
            .entries
            .iter()
            .all(|entry| coverage_granularity_maps_to_morsels(entry.target_kind))
}

fn coverage_granularity_maps_to_morsels(granularity: CoverageGranularityV2) -> bool {
    matches!(
        granularity,
        CoverageGranularityV2::Dataset
            | CoverageGranularityV2::File
            | CoverageGranularityV2::Segment
            | CoverageGranularityV2::Page
            | CoverageGranularityV2::Morsel
            | CoverageGranularityV2::RowRange
            | CoverageGranularityV2::RowOrdinalSet
    )
}

fn coverage_set_covers_morsel(
    set: &CoverageSetV2,
    current_file_ref: u32,
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,
) -> bool {
    set.entries.iter().any(|entry| {
        coverage_entry_covers_morsel(entry, current_file_ref, table_id, segment_id, morsel_id)
    })
}

fn coverage_entry_covers_morsel(
    entry: &CoverageSetEntryV2,
    current_file_ref: u32,
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,
) -> bool {
    match entry.target_kind {
        CoverageGranularityV2::Dataset => true,
        CoverageGranularityV2::File => entry.file_ref == current_file_ref,
        CoverageGranularityV2::Segment => {
            entry.file_ref == current_file_ref
                && entry.table_id == table_id
                && entry.segment_id == segment_id
        }
        CoverageGranularityV2::Morsel => {
            entry.file_ref == current_file_ref
                && entry.table_id == table_id
                && entry.segment_id == segment_id
                && entry.morsel_id == morsel_id
        }
        CoverageGranularityV2::Page | CoverageGranularityV2::RowRange => {
            entry.file_ref == current_file_ref
                && entry.table_id == table_id
                && entry.segment_id == segment_id
        }
        CoverageGranularityV2::RowOrdinalSet => {
            entry.file_ref == current_file_ref && entry.table_id == table_id
        }
        _ => false,
    }
}

fn lookup_prunes_file_codes(
    state: &DatasetState,
    segment_id: u32,
    morsel_id: u32,
    column_id: u32,
    file_codes: &[u32],
) -> bool {
    let Some(index) = state.lookup_for(column_id) else {
        return false;
    };
    if index.header.key_kind != LookupKeyKind::FileCode {
        return false;
    }
    for file_code in file_codes {
        let Some(rows) = index.rows_for(u64::from(*file_code)) else {
            continue;
        };
        for row in rows {
            if row.table_id == state.table().table_id
                && row.segment_id == segment_id
                && row.morsel_id == morsel_id
            {
                return false;
            }
        }
    }
    true
}

fn inverted_prunes_file_codes(
    state: &DatasetState,
    segment_id: u32,
    morsel_id: u32,
    column_id: u32,
    file_codes: &[u32],
) -> bool {
    let Some(index) = state.inverted_for(column_id) else {
        return false;
    };
    if index.header.key_kind != InvertedKeyKind::FileCode {
        return false;
    }
    let Some(global_morsel_ordinal) = global_morsel_ordinal(state, segment_id, morsel_id) else {
        return false;
    };
    for file_code in file_codes {
        let Ok(entry_index) = index
            .entries
            .binary_search_by_key(&u64::from(*file_code), |entry| entry.key)
        else {
            continue;
        };
        let entry = &index.entries[entry_index];
        if bitmap_contains(
            &index.bitmap_data,
            entry.morsel_bitmap_offset,
            entry.morsel_bitmap_length,
            global_morsel_ordinal,
        ) {
            return false;
        }
    }
    true
}

fn global_morsel_ordinal(state: &DatasetState, segment_id: u32, morsel_id: u32) -> Option<u32> {
    let mut ordinal = 0u32;
    for segment in state.segments() {
        if segment.segment_id == segment_id {
            return (morsel_id < segment.morsel_count)
                .then(|| ordinal.checked_add(morsel_id))
                .flatten();
        }
        ordinal = ordinal.checked_add(segment.morsel_count)?;
    }
    None
}

fn bitmap_contains(bitmap_data: &[u8], offset: u64, length: u32, bit_index: u32) -> bool {
    let Ok(start) = usize::try_from(offset) else {
        return true;
    };
    let Some(end) = start.checked_add(length as usize) else {
        return true;
    };
    if end > bitmap_data.len() {
        return true;
    }
    let byte_index = bit_index as usize / 8;
    if byte_index >= length as usize {
        return false;
    }
    let bit = bit_index % 8;
    bitmap_data[start + byte_index] & (1u8 << bit) != 0
}

fn bloom_excludes_file_code(
    state: &DatasetState,
    column_id: u32,
    file_code: u32,
    bloom: Option<&cove_core::index::bloom::BloomFilterIndex>,
) -> Result<bool, CoveError> {
    let Some(bloom) = bloom else {
        return Ok(false);
    };
    let value = match bloom.header.hash_domain {
        BloomHashDomain::FileCode => file_code.to_le_bytes().to_vec(),
        BloomHashDomain::CanonicalValueHash => {
            let Some(dictionary) = state.mounted().dictionary.as_ref() else {
                return Ok(false);
            };
            match dictionary.decode_value(file_code)? {
                cove_core::dictionary::DictionaryValue::RawBytes(bytes) => bytes,
                cove_core::dictionary::DictionaryValue::RedactedPresent => return Ok(false),
                _ => return Ok(false),
            }
        }
        BloomHashDomain::NumCode => return Ok(false),
        _ => return Ok(false),
    };
    let column_matches =
        bloom.header.table_id == state.table().table_id && bloom.header.column_id == column_id;
    if !column_matches {
        return Ok(false);
    }
    Ok(
        explain_bloom_membership(&value, Some(bloom), false).final_outcome
            == PredicateZoneOutcome::NoMatch,
    )
}

fn numeric_bounds(
    op: NumericPredicateOp,
    literal: PredicateLiteral,
) -> Result<
    (
        Option<NumericStatValue>,
        bool,
        Option<NumericStatValue>,
        bool,
    ),
    CoveError,
> {
    let value = numeric_stat_value(literal)?;
    Ok(match op {
        NumericPredicateOp::Eq => (Some(value), true, Some(value), true),
        NumericPredicateOp::Lt => (None, false, Some(value), false),
        NumericPredicateOp::LtEq => (None, false, Some(value), true),
        NumericPredicateOp::Gt => (Some(value), false, None, false),
        NumericPredicateOp::GtEq => (Some(value), true, None, false),
    })
}

fn numeric_stat_value(literal: PredicateLiteral) -> Result<NumericStatValue, CoveError> {
    match literal {
        PredicateLiteral::Int64(value) => Ok(NumericStatValue::Int64(value)),
        PredicateLiteral::UInt64(value) => Ok(NumericStatValue::UInt64(value)),
        PredicateLiteral::Float64(value) if !value.is_nan() => Ok(NumericStatValue::Float64(value)),
        PredicateLiteral::Float64(_) => Err(CoveError::BadStats),
    }
}

#[cfg(test)]
mod tests {
    use super::{coverage_entry_covers_morsel, coverage_granularity_maps_to_morsels, CandidateSet};
    use cove_coverage::{CoverageGranularityV2, CoverageSetEntryV2};

    #[test]
    fn candidate_sets_intersect_all_sparse_and_bitmap() {
        let all = CandidateSet::all(8);
        let sparse = CandidateSet::Sparse(vec![1, 3, 5, 7]);
        let bitmap = CandidateSet::Bitmap {
            len: 8,
            bits: vec![0b0010_1010],
        };

        assert_eq!(all.intersect(&sparse), sparse);
        assert_eq!(
            sparse.intersect(&bitmap),
            CandidateSet::Sparse(vec![1, 3, 5])
        );
        assert!(sparse.intersect(&CandidateSet::empty()).is_empty());
    }

    #[test]
    fn candidate_sets_union_preserves_sparse_domain_membership() {
        let left = CandidateSet::Sparse(vec![0, 2, 4]);
        let right = CandidateSet::Bitmap {
            len: 6,
            bits: vec![0b0000_1010],
        };

        let union = left.union(&right);

        assert_eq!(union.len(), 5);
        for index in [0, 1, 2, 3, 4] {
            assert!(union.contains(index));
        }
        assert!(!union.contains(5));
        assert_eq!(union.union(&CandidateSet::all(3)), CandidateSet::all(6));
    }

    #[test]
    fn candidate_sets_handle_unnormalized_sparse_inputs() {
        let left = CandidateSet::Sparse(vec![4, 2, 2, 0]);
        let right = CandidateSet::Sparse(vec![3, 4, 1]);

        assert_eq!(left.len(), 3);
        assert_eq!(left.intersect(&right), CandidateSet::Sparse(vec![4]));
        assert_eq!(
            left.union(&right),
            CandidateSet::Sparse(vec![0, 1, 2, 3, 4])
        );
    }

    #[test]
    fn candidate_sets_bitmap_operations_respect_len_tail_bits() {
        let left = CandidateSet::Bitmap {
            len: 70,
            bits: vec![u64::MAX, 0b11, u64::MAX],
        };
        let right = CandidateSet::Bitmap {
            len: 70,
            bits: vec![0b1010, 0b10],
        };

        assert_eq!(left.len(), 66);
        assert_eq!(
            left.intersect(&right),
            CandidateSet::Bitmap {
                len: 70,
                bits: vec![0b1010, 0b10],
            }
        );
        assert_eq!(
            right.union(&CandidateSet::Sparse(vec![68])),
            CandidateSet::Bitmap {
                len: 70,
                bits: vec![0b1010, 0b10010],
            }
        );
    }

    #[test]
    fn coverage_entries_map_only_safe_granularities_to_morsels() {
        let segment = coverage_entry(CoverageGranularityV2::Segment, 0, 7, 2, u32::MAX);
        let morsel = coverage_entry(CoverageGranularityV2::Morsel, 0, 7, 2, 3);
        let page = coverage_entry(CoverageGranularityV2::Page, 0, 7, 2, u32::MAX);
        let row_range = coverage_entry(CoverageGranularityV2::RowRange, 0, 7, 2, u32::MAX);

        assert!(coverage_entry_covers_morsel(&segment, 0, 7, 2, 0));
        assert!(!coverage_entry_covers_morsel(&segment, 0, 7, 3, 0));
        assert!(coverage_entry_covers_morsel(&morsel, 0, 7, 2, 3));
        assert!(!coverage_entry_covers_morsel(&morsel, 0, 7, 2, 4));
        assert!(coverage_entry_covers_morsel(&page, 0, 7, 2, 99));
        assert!(coverage_entry_covers_morsel(&row_range, 0, 7, 2, 99));
        assert!(!coverage_entry_covers_morsel(&row_range, 1, 7, 2, 99));
        assert!(coverage_granularity_maps_to_morsels(
            CoverageGranularityV2::RowRange
        ));
        assert!(!coverage_granularity_maps_to_morsels(
            CoverageGranularityV2::RowGroup
        ));
    }

    fn coverage_entry(
        target_kind: CoverageGranularityV2,
        file_ref: u32,
        table_id: u32,
        segment_id: u32,
        morsel_id: u32,
    ) -> CoverageSetEntryV2 {
        CoverageSetEntryV2 {
            target_kind,
            flags: 0,
            file_ref,
            table_id,
            segment_id,
            morsel_id,
            page_ref: u32::MAX,
            object_type_id: u32::MAX,
            path_ref: u32::MAX,
            dimensional_bucket_ref: u32::MAX,
            row_start: 0,
            row_count: 1,
            row_ordinal_bitmap_ref: u32::MAX,
            byte_range_ref: u32::MAX,
            checksum: 0,
        }
    }
}
