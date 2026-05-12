//! Exact metadata aggregate proofs shared by native adapter code.

use std::collections::{BTreeMap, BTreeSet};

use cove_core::{
    constants::{CoveLogicalType, CovePhysicalKind},
    dictionary::DictionaryValue,
    index::lookup::LookupKeyKind,
    row_ref::RowRef,
    wire, CoveError,
};

use crate::dataset_state::DatasetState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataAggregateProofKind {
    CountRows,
    CountColumn,
    FileCodeLookupCount,
    FileCodeLookupGroupCount,
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
pub enum MetadataAggregatePlan {
    ScalarCounts {
        counts: Vec<u64>,
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
            Self::ScalarCounts { proof, .. } | Self::FileCodeGroupCounts { proof, .. } => proof,
        }
    }

    pub fn output_rows(&self) -> usize {
        match self {
            Self::ScalarCounts { .. } => 1,
            Self::FileCodeGroupCounts { groups, .. } => groups.len(),
        }
    }
}

pub fn exact_unfiltered_counts(
    state: &DatasetState,
    column_indexes: &[Option<usize>],
) -> Result<Option<MetadataAggregatePlan>, CoveError> {
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

pub fn exact_filecode_filtered_count(
    state: &DatasetState,
    column_index: usize,
    canonical_values: &[Vec<u8>],
) -> Result<Option<MetadataAggregatePlan>, CoveError> {
    if canonical_values.is_empty() || !filecode_fast_paths_are_safe(state, column_index)? {
        return Ok(None);
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
