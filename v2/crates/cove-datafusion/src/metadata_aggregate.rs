//! Exact metadata aggregate proofs shared by native adapter code.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use cove_core::{
    canonical::validate_canonical_payload,
    constants::{CoveLogicalType, CovePhysicalKind, ValueTag},
    dictionary::DictionaryValue,
    index::{
        aggregate::{
            AggregateEntry, AggregatePayloadV2, NumericAggregateOverflowPolicy, SynopsisAccuracy,
            SynopsisKind, TaggedCanonicalValue,
        },
        lookup::LookupKeyKind,
    },
    row_ref::RowRef,
    wire, CoveError,
};

use crate::dataset_state::{DatasetState, FileMetadata};
#[cfg(feature = "covi")]
use cove_index::execution::{CoviAggregateKindV2, CoviIndexOnlyRequestV2};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataAggregateProofKind {
    CountRows,
    CountColumn,
    FileCodeLookupCount,
    FileCodeLookupGroupCount,
    FileCodeHistogramCount,
    FileCodeHistogramGroupCount,
    CoviIndexOnlyCount,
    CoviIndexOnlyValue,
    AggregateSynopsisValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataAggregateProof {
    pub kind: MetadataAggregateProofKind,
    pub reason: String,
    pub dictionary_group_labels_decoded: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataGroupCount {
    pub canonical_value: Vec<u8>,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataAggregateValue {
    pub logical: CoveLogicalType,
    pub canonical_value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataSynopsisAggregateKind {
    Min,
    Max,
    Sum,
    Avg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataAggregatePlan {
    ScalarCounts {
        counts: Vec<u64>,
        proof: MetadataAggregateProof,
    },
    ScalarValues {
        values: Vec<MetadataAggregateValue>,
        proof: MetadataAggregateProof,
    },
    FileCodeGroupCounts {
        column_index: usize,
        groups: Vec<MetadataGroupCount>,
        proof: MetadataAggregateProof,
    },
}

impl MetadataAggregatePlan {
    pub fn proof(&self) -> &MetadataAggregateProof {
        match self {
            Self::ScalarCounts { proof, .. }
            | Self::ScalarValues { proof, .. }
            | Self::FileCodeGroupCounts { proof, .. } => proof,
        }
    }

    pub fn output_rows(&self) -> usize {
        match self {
            Self::ScalarCounts { .. } | Self::ScalarValues { .. } => 1,
            Self::FileCodeGroupCounts { groups, .. } => groups.len(),
        }
    }
}

pub fn exact_unfiltered_counts(
    state: &DatasetState,
    column_indexes: &[Option<usize>],
) -> Result<Option<MetadataAggregatePlan>, CoveError> {
    #[cfg(feature = "covi")]
    if let Some(plan) = exact_covi_unfiltered_counts(state, column_indexes)? {
        return Ok(Some(plan));
    }
    let mut counts = Vec::with_capacity(column_indexes.len());
    let mut saw_column = false;
    for column_index in column_indexes {
        if column_index.is_some() {
            saw_column = true;
        }
        let Some(count) = state.exact_global_count(*column_index)? else {
            return Ok(None);
        };
        counts.push(count);
    }
    Ok(Some(MetadataAggregatePlan::ScalarCounts {
        counts,
        proof: MetadataAggregateProof {
            kind: if saw_column {
                MetadataAggregateProofKind::CountColumn
            } else {
                MetadataAggregateProofKind::CountRows
            },
            reason: "exact row counts from validated COVE metadata".into(),
            dictionary_group_labels_decoded: 0,
        },
    }))
}

pub fn exact_unfiltered_aggregate_synopses(
    state: &DatasetState,
    requests: &[(usize, MetadataSynopsisAggregateKind)],
) -> Result<Option<MetadataAggregatePlan>, CoveError> {
    if requests.is_empty() || !aggregate_synopsis_fast_paths_are_safe(state)? {
        return Ok(None);
    }
    let mut values = Vec::with_capacity(requests.len());
    for (column_index, aggregate_kind) in requests {
        let column = state
            .table()
            .columns
            .get(*column_index)
            .ok_or_else(|| CoveError::BadSchema("aggregate column out of bounds".into()))?;
        let value = match aggregate_kind {
            MetadataSynopsisAggregateKind::Min | MetadataSynopsisAggregateKind::Max => {
                exact_synopsis_min_max(state, *column_index, *aggregate_kind)?
            }
            MetadataSynopsisAggregateKind::Sum => exact_synopsis_sum(state, *column_index, false)?,
            MetadataSynopsisAggregateKind::Avg => exact_synopsis_sum(state, *column_index, true)?,
        };
        let Some(canonical_value) = value else {
            return Ok(None);
        };
        values.push(MetadataAggregateValue {
            logical: if matches!(aggregate_kind, MetadataSynopsisAggregateKind::Avg) {
                CoveLogicalType::Float64
            } else {
                column.logical
            },
            canonical_value,
        });
    }
    Ok(Some(MetadataAggregatePlan::ScalarValues {
        values,
        proof: MetadataAggregateProof {
            kind: MetadataAggregateProofKind::AggregateSynopsisValue,
            reason: "exact aggregate synopsis payloads from validated COVE metadata".into(),
            dictionary_group_labels_decoded: 0,
        },
    }))
}

fn exact_synopsis_min_max(
    state: &DatasetState,
    column_index: usize,
    aggregate_kind: MetadataSynopsisAggregateKind,
) -> Result<Option<Option<Vec<u8>>>, CoveError> {
    let column = state
        .table()
        .columns
        .get(column_index)
        .ok_or_else(|| CoveError::BadSchema("min/max column out of bounds".into()))?;
    if column.logical == CoveLogicalType::Bool {
        return Ok(None);
    }
    let mut best: Option<TaggedCanonicalValue> = None;
    let mut covered_rows = 0u64;
    let mut non_null_rows = 0u64;
    for file in state.files() {
        let entries = exact_payload_entries_for_file(file, column.column_id, SynopsisKind::MinMax)?;
        if entries.is_empty() {
            return Ok(None);
        }
        for (entry, payload) in entries {
            covered_rows = covered_rows
                .checked_add(u64::from(entry.row_count))
                .ok_or(CoveError::ArithOverflow)?;
            non_null_rows = non_null_rows
                .checked_add(entry.non_null_count()?)
                .ok_or(CoveError::ArithOverflow)?;
            let AggregatePayloadV2::MinMax { min, max } = payload else {
                return Ok(None);
            };
            let candidate = match aggregate_kind {
                MetadataSynopsisAggregateKind::Min => min,
                MetadataSynopsisAggregateKind::Max => max,
                _ => return Ok(None),
            };
            let Some(candidate) = candidate else {
                continue;
            };
            validate_canonical_payload(candidate.value_tag, &candidate.payload)?;
            match &best {
                None => best = Some(candidate.clone()),
                Some(current) => {
                    let ordering = compare_tagged_canonical(column.logical, candidate, current)?;
                    let replace = match aggregate_kind {
                        MetadataSynopsisAggregateKind::Min => ordering == Ordering::Less,
                        MetadataSynopsisAggregateKind::Max => ordering == Ordering::Greater,
                        _ => false,
                    };
                    if replace {
                        best = Some(candidate.clone());
                    }
                }
            }
        }
    }
    if covered_rows != state.table().row_count {
        return Ok(None);
    }
    Ok(Some(if non_null_rows == 0 {
        None
    } else {
        Some(best.ok_or(CoveError::BadIndex)?.payload)
    }))
}

fn exact_synopsis_sum(
    state: &DatasetState,
    column_index: usize,
    as_avg: bool,
) -> Result<Option<Option<Vec<u8>>>, CoveError> {
    let column = state
        .table()
        .columns
        .get(column_index)
        .ok_or_else(|| CoveError::BadSchema("sum column out of bounds".into()))?;
    let kind = if as_avg {
        SynopsisKind::SumAndCount
    } else {
        SynopsisKind::Sum
    };
    let mut covered_rows = 0u64;
    let mut non_null_rows = 0u64;
    let mut accumulator = SumAccumulator::default();
    for file in state.files() {
        let entries = exact_payload_entries_for_file(file, column.column_id, kind)?;
        if entries.is_empty() {
            return Ok(None);
        }
        for (entry, payload) in entries {
            covered_rows = covered_rows
                .checked_add(u64::from(entry.row_count))
                .ok_or(CoveError::ArithOverflow)?;
            let (declared_count, sum) = match payload {
                AggregatePayloadV2::Sum {
                    overflow_policy,
                    sum,
                } if !as_avg
                    && *overflow_policy == NumericAggregateOverflowPolicy::CheckedExact =>
                {
                    (entry.non_null_count()?, sum)
                }
                AggregatePayloadV2::SumAndCount {
                    overflow_policy,
                    non_null_count,
                    sum,
                } if as_avg && *overflow_policy == NumericAggregateOverflowPolicy::CheckedExact => {
                    (*non_null_count, sum)
                }
                _ => return Ok(None),
            };
            if declared_count != entry.non_null_count()? {
                return Ok(None);
            }
            non_null_rows = non_null_rows
                .checked_add(declared_count)
                .ok_or(CoveError::ArithOverflow)?;
            validate_canonical_payload(sum.value_tag, &sum.payload)?;
            accumulator.add(sum)?;
        }
    }
    if covered_rows != state.table().row_count {
        return Ok(None);
    }
    if as_avg {
        if non_null_rows == 0 {
            return Ok(Some(None));
        }
        let avg = accumulator.as_f64()? / non_null_rows as f64;
        return Ok(Some(Some(avg.to_bits().to_le_bytes().to_vec())));
    }
    if non_null_rows == 0 {
        return Ok(Some(None));
    }
    accumulator.finish().map(Some)
}

fn exact_payload_entries_for_file<'a>(
    file: &'a FileMetadata,
    column_id: u32,
    kind: SynopsisKind,
) -> Result<Vec<(&'a AggregateEntry, &'a AggregatePayloadV2)>, CoveError> {
    let mut out = Vec::new();
    for synopsis in file.pruning().aggregates.iter() {
        for (index, entry) in synopsis.entries.iter().enumerate() {
            if entry.table_id != file.table().table_id
                || entry.column_id != column_id
                || entry.synopsis_kind != kind
                || entry.accuracy != SynopsisAccuracy::Exact
            {
                continue;
            }
            let Some(payload) = synopsis.payload_for_entry(index) else {
                return Err(CoveError::BadIndex);
            };
            out.push((entry, payload));
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Default)]
enum SumAccumulator {
    #[default]
    Empty,
    Int(i128),
    UInt(u128),
    Decimal(i128),
    Float(f64),
}

impl SumAccumulator {
    fn add(&mut self, value: &TaggedCanonicalValue) -> Result<(), CoveError> {
        match value.value_tag {
            ValueTag::Int64 => {
                let value = i128::from(i64::from_le_bytes(fixed_value_bytes(&value.payload)?));
                match self {
                    Self::Empty => *self = Self::Int(value),
                    Self::Int(total) => {
                        *total = total.checked_add(value).ok_or(CoveError::ArithOverflow)?
                    }
                    _ => return Err(CoveError::BadIndex),
                }
            }
            ValueTag::UInt64 => {
                let value = u128::from(u64::from_le_bytes(fixed_value_bytes(&value.payload)?));
                match self {
                    Self::Empty => *self = Self::UInt(value),
                    Self::UInt(total) => {
                        *total = total.checked_add(value).ok_or(CoveError::ArithOverflow)?
                    }
                    _ => return Err(CoveError::BadIndex),
                }
            }
            ValueTag::Decimal128 => {
                let value = i128::from_le_bytes(fixed_value_bytes(&value.payload)?);
                match self {
                    Self::Empty => *self = Self::Decimal(value),
                    Self::Decimal(total) => {
                        *total = total.checked_add(value).ok_or(CoveError::ArithOverflow)?
                    }
                    _ => return Err(CoveError::BadIndex),
                }
            }
            ValueTag::Float32Bits => {
                let value =
                    f32::from_bits(u32::from_le_bytes(fixed_value_bytes(&value.payload)?)) as f64;
                match self {
                    Self::Empty => *self = Self::Float(value),
                    Self::Float(total) => *total += value,
                    _ => return Err(CoveError::BadIndex),
                }
            }
            ValueTag::Float64Bits => {
                let value = f64::from_bits(u64::from_le_bytes(fixed_value_bytes(&value.payload)?));
                match self {
                    Self::Empty => *self = Self::Float(value),
                    Self::Float(total) => *total += value,
                    _ => return Err(CoveError::BadIndex),
                }
            }
            _ => return Err(CoveError::BadIndex),
        }
        Ok(())
    }

    fn finish(self) -> Result<Option<Vec<u8>>, CoveError> {
        match self {
            Self::Empty => Ok(None),
            Self::Int(value) => Ok(Some(
                i64::try_from(value)
                    .map_err(|_| CoveError::ArithOverflow)?
                    .to_le_bytes()
                    .to_vec(),
            )),
            Self::UInt(value) => Ok(Some(
                u64::try_from(value)
                    .map_err(|_| CoveError::ArithOverflow)?
                    .to_le_bytes()
                    .to_vec(),
            )),
            Self::Decimal(value) => Ok(Some(value.to_le_bytes().to_vec())),
            Self::Float(value) => Ok(Some(value.to_bits().to_le_bytes().to_vec())),
        }
    }

    fn as_f64(&self) -> Result<f64, CoveError> {
        match self {
            Self::Empty => Ok(0.0),
            Self::Int(value) => Ok(*value as f64),
            Self::UInt(value) => Ok(*value as f64),
            Self::Decimal(value) => Ok(*value as f64),
            Self::Float(value) => Ok(*value),
        }
    }
}

fn fixed_value_bytes<const N: usize>(bytes: &[u8]) -> Result<[u8; N], CoveError> {
    bytes.try_into().map_err(|_| CoveError::BadIndex)
}

#[cfg(feature = "covi")]
fn exact_covi_unfiltered_counts(
    state: &DatasetState,
    column_indexes: &[Option<usize>],
) -> Result<Option<MetadataAggregatePlan>, CoveError> {
    let mut counts = Vec::with_capacity(column_indexes.len());
    for column_index in column_indexes {
        let Some(answer) =
            exact_covi_unfiltered_answer(state, *column_index, CoviAggregateKindV2::Count)?
        else {
            return Ok(None);
        };
        counts.push(if column_index.is_some() {
            answer.non_null_count
        } else {
            answer.row_count
        });
    }
    Ok(Some(MetadataAggregatePlan::ScalarCounts {
        counts,
        proof: MetadataAggregateProof {
            kind: MetadataAggregateProofKind::CoviIndexOnlyCount,
            reason: "exact COVE-I index-only count answer".into(),
            dictionary_group_labels_decoded: 0,
        },
    }))
}

#[cfg(feature = "covi")]
pub fn exact_covi_unfiltered_min_max(
    state: &DatasetState,
    requests: &[(usize, CoviAggregateKindV2)],
) -> Result<Option<MetadataAggregatePlan>, CoveError> {
    let mut values = Vec::with_capacity(requests.len());
    for (column_index, aggregate_kind) in requests {
        if !matches!(
            aggregate_kind,
            CoviAggregateKindV2::Min | CoviAggregateKindV2::Max
        ) {
            return Ok(None);
        }
        let column = state
            .table()
            .columns
            .get(*column_index)
            .ok_or_else(|| CoveError::BadSchema("min/max column out of bounds".into()))?;
        let Some(value_tag) = value_tag_for_covi_index_only(column.logical) else {
            return Ok(None);
        };
        let Some(answer) =
            exact_covi_unfiltered_answer(state, Some(*column_index), *aggregate_kind)?
        else {
            return Ok(None);
        };
        let canonical_value = match answer.value {
            Some(value) => {
                validate_canonical_payload(value_tag, &value)?;
                Some(value)
            }
            None if answer.non_null_count == 0 => None,
            None => return Ok(None),
        };
        values.push(MetadataAggregateValue {
            logical: column.logical,
            canonical_value,
        });
    }
    Ok(Some(MetadataAggregatePlan::ScalarValues {
        values,
        proof: MetadataAggregateProof {
            kind: MetadataAggregateProofKind::CoviIndexOnlyValue,
            reason: "exact COVE-I index-only min/max answer".into(),
            dictionary_group_labels_decoded: 0,
        },
    }))
}

#[cfg(feature = "covi")]
fn exact_covi_unfiltered_answer(
    state: &DatasetState,
    column_index: Option<usize>,
    aggregate_kind: CoviAggregateKindV2,
) -> Result<Option<cove_index::execution::CoviIndexOnlyAnswerV2>, CoveError> {
    if state.file_count() != 1
        || !state.file(0)?.visibility().is_all()
        || state.file(0)?.has_redaction()
    {
        return Ok(None);
    }
    let Some(covi) = state.covi() else {
        return Ok(None);
    };
    let column_id = match column_index {
        Some(index) => Some(
            state
                .table()
                .columns
                .get(index)
                .ok_or_else(|| CoveError::BadSchema("aggregate column out of bounds".into()))?
                .column_id,
        ),
        None => None,
    };
    let request = CoviIndexOnlyRequestV2 {
        table_id: state.table().table_id,
        column_id,
        aggregate_kind,
        predicate_form_ref: None,
        require_exact: true,
    };
    covi.index_only_answer(&request)
}

#[cfg(feature = "covi")]
fn value_tag_for_covi_index_only(logical: CoveLogicalType) -> Option<ValueTag> {
    match logical {
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
        CoveLogicalType::Utf8 => Some(ValueTag::Utf8),
        _ => None,
    }
}

pub fn exact_filecode_filtered_count(
    state: &DatasetState,
    column_index: usize,
    canonical_values: &[Vec<u8>],
) -> Result<Option<MetadataAggregatePlan>, CoveError> {
    if canonical_values.is_empty() || !filecode_fast_paths_are_safe(state, column_index)? {
        return Ok(None);
    }
    if let Some(plan) =
        exact_filecode_histogram_filtered_count(state, column_index, canonical_values)?
    {
        return Ok(Some(plan));
    }
    let mut total = 0u64;
    for file_ordinal in 0..state.file_count() {
        let file_state = state.single_file_view(file_ordinal)?;
        let column = file_state
            .table()
            .columns
            .get(column_index)
            .ok_or_else(|| CoveError::BadSchema("FileCode filter column out of bounds".into()))?;
        let Some(lookup) = file_state.lookup_for(column.column_id) else {
            return Ok(None);
        };
        if lookup.header.key_kind != LookupKeyKind::FileCode {
            return Ok(None);
        }
        let mut selected = BTreeSet::new();
        for canonical in canonical_values {
            // INVARIANT: `file_state` is a single-file dataset view, so file
            // ordinal 0 refers to that concrete file only.
            let Some(file_code) = file_state.file_code_for_canonical(0, canonical)? else {
                continue;
            };
            if let Some(rows) = lookup.rows_for(u64::from(file_code)) {
                for row in rows {
                    if row.table_id == file_state.table().table_id {
                        selected.insert(*row);
                    }
                }
            }
        }
        total = total
            .checked_add(u64::try_from(selected.len()).map_err(|_| CoveError::ArithOverflow)?)
            .ok_or(CoveError::ArithOverflow)?;
    }
    Ok(Some(MetadataAggregatePlan::ScalarCounts {
        counts: vec![total],
        proof: MetadataAggregateProof {
            kind: MetadataAggregateProofKind::FileCodeLookupCount,
            reason: "exact FileCode lookup index count for equality/IN filter".into(),
            dictionary_group_labels_decoded: 0,
        },
    }))
}

fn exact_filecode_histogram_filtered_count(
    state: &DatasetState,
    column_index: usize,
    canonical_values: &[Vec<u8>],
) -> Result<Option<MetadataAggregatePlan>, CoveError> {
    let mut total = 0u64;
    for file_ordinal in 0..state.file_count() {
        let file_state = state.single_file_view(file_ordinal)?;
        let column = file_state
            .table()
            .columns
            .get(column_index)
            .ok_or_else(|| CoveError::BadSchema("FileCode filter column out of bounds".into()))?;
        let mut selected_codes = BTreeSet::new();
        for canonical in canonical_values {
            if let Some(file_code) = file_state.file_code_for_canonical(0, canonical)? {
                selected_codes.insert(u64::from(file_code));
            }
        }
        let entries = exact_payload_entries_for_file(
            file_state.file(0)?,
            column.column_id,
            SynopsisKind::FileCodeHistogram,
        )?;
        if entries.is_empty() {
            return Ok(None);
        }
        let mut covered_rows = 0u64;
        for (entry, payload) in entries {
            covered_rows = covered_rows
                .checked_add(u64::from(entry.row_count))
                .ok_or(CoveError::ArithOverflow)?;
            let AggregatePayloadV2::FileCodeHistogram { buckets } = payload else {
                return Ok(None);
            };
            for bucket in buckets {
                if selected_codes.contains(&bucket.key) {
                    total = total
                        .checked_add(bucket.count)
                        .ok_or(CoveError::ArithOverflow)?;
                }
            }
        }
        if covered_rows != file_state.table().row_count {
            return Ok(None);
        }
    }
    Ok(Some(MetadataAggregatePlan::ScalarCounts {
        counts: vec![total],
        proof: MetadataAggregateProof {
            kind: MetadataAggregateProofKind::FileCodeHistogramCount,
            reason: "exact FileCode histogram count for equality/IN filter".into(),
            dictionary_group_labels_decoded: 0,
        },
    }))
}

pub fn exact_filecode_group_counts(
    state: &DatasetState,
    column_index: usize,
) -> Result<Option<MetadataAggregatePlan>, CoveError> {
    if !filecode_fast_paths_are_safe(state, column_index)? {
        return Ok(None);
    }
    let column = state
        .table()
        .columns
        .get(column_index)
        .ok_or_else(|| CoveError::BadSchema("FileCode group column out of bounds".into()))?;
    if column.logical != CoveLogicalType::Utf8 {
        return Ok(None);
    }
    if let Some(plan) = exact_filecode_histogram_group_counts(state, column_index)? {
        return Ok(Some(plan));
    }

    let mut grouped: BTreeMap<Vec<u8>, u64> = BTreeMap::new();
    let mut labels_decoded = 0usize;
    for file_ordinal in 0..state.file_count() {
        let file_state = state.single_file_view(file_ordinal)?;
        let column = &file_state.table().columns[column_index];
        let Some(lookup) = file_state.lookup_for(column.column_id) else {
            return Ok(None);
        };
        if lookup.header.key_kind != LookupKeyKind::FileCode {
            return Ok(None);
        }
        let Some(dictionary) = file_state.mounted().dictionary.as_ref() else {
            return Ok(None);
        };
        let mut covered = BTreeSet::<RowRef>::new();
        for entry in &lookup.entries {
            let Ok(file_code) = u32::try_from(entry.key) else {
                return Ok(None);
            };
            let canonical = match dictionary.decode_value(file_code)? {
                DictionaryValue::RawBytes(bytes) => bytes,
                DictionaryValue::RedactedPresent => return Ok(None),
                _ => return Ok(None),
            };
            labels_decoded += 1;
            let mut entry_rows = BTreeSet::new();
            for row in &entry.rows {
                if row.table_id == file_state.table().table_id {
                    entry_rows.insert(*row);
                    covered.insert(*row);
                }
            }
            let count = u64::try_from(entry_rows.len()).map_err(|_| CoveError::ArithOverflow)?;
            let slot = grouped.entry(canonical).or_default();
            *slot = slot.checked_add(count).ok_or(CoveError::ArithOverflow)?;
        }
        if u64::try_from(covered.len()).map_err(|_| CoveError::ArithOverflow)?
            != file_state.table().row_count
        {
            return Ok(None);
        }
    }

    Ok(Some(MetadataAggregatePlan::FileCodeGroupCounts {
        column_index,
        groups: grouped
            .into_iter()
            .map(|(canonical_value, count)| MetadataGroupCount {
                canonical_value,
                count,
            })
            .collect(),
        proof: MetadataAggregateProof {
            kind: MetadataAggregateProofKind::FileCodeLookupGroupCount,
            reason: "exact FileCode lookup index coverage for GROUP BY count".into(),
            dictionary_group_labels_decoded: labels_decoded,
        },
    }))
}

fn exact_filecode_histogram_group_counts(
    state: &DatasetState,
    column_index: usize,
) -> Result<Option<MetadataAggregatePlan>, CoveError> {
    let mut grouped: BTreeMap<Vec<u8>, u64> = BTreeMap::new();
    let mut labels_decoded = 0usize;
    for file_ordinal in 0..state.file_count() {
        let file_state = state.single_file_view(file_ordinal)?;
        let column = file_state
            .table()
            .columns
            .get(column_index)
            .ok_or_else(|| CoveError::BadSchema("FileCode group column out of bounds".into()))?;
        let Some(dictionary) = file_state.mounted().dictionary.as_ref() else {
            return Ok(None);
        };
        let entries = exact_payload_entries_for_file(
            file_state.file(0)?,
            column.column_id,
            SynopsisKind::FileCodeHistogram,
        )?;
        if entries.is_empty() {
            return Ok(None);
        }
        let mut covered_rows = 0u64;
        let mut null_rows = 0u64;
        for (entry, payload) in entries {
            covered_rows = covered_rows
                .checked_add(u64::from(entry.row_count))
                .ok_or(CoveError::ArithOverflow)?;
            null_rows = null_rows
                .checked_add(u64::from(entry.null_count))
                .ok_or(CoveError::ArithOverflow)?;
            let AggregatePayloadV2::FileCodeHistogram { buckets } = payload else {
                return Ok(None);
            };
            for bucket in buckets {
                let file_code = u32::try_from(bucket.key).map_err(|_| CoveError::BadIndex)?;
                let canonical = match dictionary.decode_value(file_code)? {
                    DictionaryValue::RawBytes(bytes) => bytes,
                    DictionaryValue::RedactedPresent => return Ok(None),
                    _ => return Ok(None),
                };
                labels_decoded += 1;
                let slot = grouped.entry(canonical).or_default();
                *slot = slot
                    .checked_add(bucket.count)
                    .ok_or(CoveError::ArithOverflow)?;
            }
        }
        if covered_rows != file_state.table().row_count || null_rows != 0 {
            return Ok(None);
        }
    }
    Ok(Some(MetadataAggregatePlan::FileCodeGroupCounts {
        column_index,
        groups: grouped
            .into_iter()
            .map(|(canonical_value, count)| MetadataGroupCount {
                canonical_value,
                count,
            })
            .collect(),
        proof: MetadataAggregateProof {
            kind: MetadataAggregateProofKind::FileCodeHistogramGroupCount,
            reason: "exact FileCode histogram coverage for GROUP BY count".into(),
            dictionary_group_labels_decoded: labels_decoded,
        },
    }))
}

fn compare_tagged_canonical(
    logical: CoveLogicalType,
    left: &TaggedCanonicalValue,
    right: &TaggedCanonicalValue,
) -> Result<Ordering, CoveError> {
    let both_bool = matches!(left.value_tag, ValueTag::BoolFalse | ValueTag::BoolTrue)
        && matches!(right.value_tag, ValueTag::BoolFalse | ValueTag::BoolTrue);
    if left.value_tag != right.value_tag && !both_bool {
        return Err(CoveError::BadIndex);
    }
    let ordering = match left.value_tag {
        ValueTag::BoolFalse | ValueTag::BoolTrue => {
            bool_tag_value(left.value_tag).cmp(&bool_tag_value(right.value_tag))
        }
        ValueTag::Int64 => i64::from_le_bytes(fixed_value_bytes(&left.payload)?)
            .cmp(&i64::from_le_bytes(fixed_value_bytes(&right.payload)?)),
        ValueTag::UInt64 => u64::from_le_bytes(fixed_value_bytes(&left.payload)?)
            .cmp(&u64::from_le_bytes(fixed_value_bytes(&right.payload)?)),
        ValueTag::Decimal128 => i128::from_le_bytes(fixed_value_bytes(&left.payload)?)
            .cmp(&i128::from_le_bytes(fixed_value_bytes(&right.payload)?)),
        ValueTag::DateDays => i32::from_le_bytes(fixed_value_bytes(&left.payload)?)
            .cmp(&i32::from_le_bytes(fixed_value_bytes(&right.payload)?)),
        ValueTag::TimestampMicros | ValueTag::TimestampNanos => {
            i64::from_le_bytes(fixed_value_bytes(&left.payload)?)
                .cmp(&i64::from_le_bytes(fixed_value_bytes(&right.payload)?))
        }
        ValueTag::Float32Bits if logical == CoveLogicalType::Float32 => {
            let left = f32::from_bits(u32::from_le_bytes(fixed_value_bytes(&left.payload)?));
            let right = f32::from_bits(u32::from_le_bytes(fixed_value_bytes(&right.payload)?));
            left.total_cmp(&right)
        }
        ValueTag::Float64Bits if logical == CoveLogicalType::Float64 => {
            let left = f64::from_bits(u64::from_le_bytes(fixed_value_bytes(&left.payload)?));
            let right = f64::from_bits(u64::from_le_bytes(fixed_value_bytes(&right.payload)?));
            left.total_cmp(&right)
        }
        ValueTag::Utf8 => canonical_utf8(&left.payload)?.cmp(&canonical_utf8(&right.payload)?),
        ValueTag::Binary => left.payload.cmp(&right.payload),
        _ => return Err(CoveError::BadIndex),
    };
    Ok(ordering)
}

fn bool_tag_value(tag: ValueTag) -> bool {
    matches!(tag, ValueTag::BoolTrue)
}

pub fn canonical_utf8(canonical: &[u8]) -> Result<String, CoveError> {
    let (len, used) = wire::decode_u64_leb128(canonical)?;
    let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
    let end = used.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end != canonical.len() {
        return Err(CoveError::BadSection(
            "canonical Utf8 payload length mismatch".into(),
        ));
    }
    std::str::from_utf8(&canonical[used..end])
        .map(str::to_owned)
        .map_err(|_| CoveError::BadSection("canonical Utf8 payload is not UTF-8".into()))
}

fn filecode_fast_paths_are_safe(
    state: &DatasetState,
    column_index: usize,
) -> Result<bool, CoveError> {
    for file in state.files() {
        let Some(column) = file.table().columns.get(column_index) else {
            return Ok(false);
        };
        if column.physical != CovePhysicalKind::FileCode
            || !file.visibility().is_all()
            || file.has_redaction()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn aggregate_synopsis_fast_paths_are_safe(state: &DatasetState) -> Result<bool, CoveError> {
    for file in state.files() {
        if !file.visibility().is_all() || file.has_redaction() {
            return Ok(false);
        }
    }
    Ok(true)
}
