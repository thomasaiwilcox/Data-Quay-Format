//! Stable scan task generation from planned candidates.

use std::collections::{BTreeMap, BTreeSet};

use cove_core::index::lookup::LookupKeyKind;
use cove_core::CoveError;

use crate::{
    dataset_state::DatasetState,
    decode::numeric_lookup_key,
    planner::{CovePredicate, NumericPredicateOp, ScanPlan},
    prune,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanTask {
    pub file_ordinal: usize,
    pub segment_index: usize,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub row_start: u64,
    pub row_count: u32,
    pub output_columns: Vec<usize>,
    pub predicate_columns: Vec<usize>,
    pub row_selection: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskGraph {
    pub tasks: Vec<ScanTask>,
    pub partitions: Vec<TaskPartition>,
    pub morsels_considered: usize,
    pub morsels_pruned: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskPartition {
    pub tasks: Vec<ScanTask>,
}

/// Build a stable task list using metadata-only pruning. This function must
/// not read or decode page payloads; execution remains responsible for row
/// predicate evaluation and materialization.
pub fn build_task_graph(state: &DatasetState, plan: &ScanPlan) -> Result<TaskGraph, CoveError> {
    if plan.scan_program.lookup_rowref_eligible {
        if let Some(graph) = build_lookup_rowref_task_graph(state, plan)? {
            return Ok(graph);
        }
    }

    let mut graph = TaskGraph::default();
    for file_ordinal in 0..state.file_count() {
        let file_state = state.single_file_view(file_ordinal)?;
        let file_plan = state.resolved_plan_for_file(plan, file_ordinal)?;
        for (segment_index, segment) in file_state.segments().iter().enumerate() {
            let mut row_start = segment.row_start as u64;
            let morsel_count = segment.morsel_count();
            for morsel_id in 0..morsel_count {
                graph.morsels_considered += 1;
                let row_count = segment.morsel_row_count_for(morsel_id)?;
                if file_state.file(0)?.visibility().morsel_all_hidden(
                    row_start,
                    row_count,
                    file_state.table().row_count,
                )? || prune::morsel_pruned(
                    &file_state,
                    segment.segment_id,
                    morsel_id,
                    &file_plan,
                )? {
                    graph.morsels_pruned += 1;
                    row_start = row_start
                        .checked_add(u64::from(row_count))
                        .ok_or(CoveError::ArithOverflow)?;
                    continue;
                }
                graph.tasks.push(ScanTask {
                    file_ordinal,
                    segment_index,
                    segment_id: segment.segment_id,
                    morsel_id,
                    row_start,
                    row_count,
                    output_columns: file_plan.column_plan.output_columns.clone(),
                    predicate_columns: file_plan.column_plan.predicate_columns.clone(),
                    row_selection: None,
                });
                row_start = row_start
                    .checked_add(u64::from(row_count))
                    .ok_or(CoveError::ArithOverflow)?;
            }
        }
    }
    finalize_partitions(&mut graph, state.target_morsels_per_partition());
    Ok(graph)
}

fn build_lookup_rowref_task_graph(
    state: &DatasetState,
    plan: &ScanPlan,
) -> Result<Option<TaskGraph>, CoveError> {
    let mut graph = TaskGraph::default();
    for file_ordinal in 0..state.file_count() {
        let file_state = state.single_file_view(file_ordinal)?;
        let file_plan = state.resolved_plan_for_file(plan, file_ordinal)?;
        let Some((column_index, key_kind, keys)) = lookup_keys_for_plan(&file_plan) else {
            return Ok(None);
        };
        let Some(column) = file_state.table().columns.get(column_index) else {
            return Ok(None);
        };
        let Some(index) = file_state.lookup_for(column.column_id) else {
            return Ok(None);
        };
        if index.header.key_kind != key_kind {
            return Ok(None);
        }

        let segment_by_id = file_state
            .segments()
            .iter()
            .enumerate()
            .map(|(segment_index, segment)| (segment.segment_id, segment_index))
            .collect::<BTreeMap<_, _>>();
        let mut rows_by_morsel: BTreeMap<RowrefMorselKey, BTreeSet<u32>> = BTreeMap::new();
        for key in keys {
            let Some(rows) = index.rows_for(key) else {
                continue;
            };
            for row in rows {
                if row.table_id != file_state.table().table_id {
                    continue;
                }
                let Some(segment_index) = segment_by_id.get(&row.segment_id).copied() else {
                    return Ok(None);
                };
                let segment = file_state
                    .segments()
                    .get(segment_index)
                    .ok_or(CoveError::SegmentCorrupt)?;
                let row_count = segment.morsel_row_count_for(row.morsel_id)?;
                let row_in_morsel = u32::from(row.row_in_morsel);
                if row_in_morsel >= row_count {
                    return Ok(None);
                }
                let row_start = u64::from(segment.row_start)
                    .checked_add(
                        u64::from(row.morsel_id)
                            .checked_mul(u64::from(segment.morsel_row_count))
                            .ok_or(CoveError::ArithOverflow)?,
                    )
                    .ok_or(CoveError::ArithOverflow)?;
                let morsel_key = RowrefMorselKey {
                    file_ordinal,
                    segment_index,
                    segment_id: segment.segment_id,
                    morsel_id: row.morsel_id,
                    row_start,
                    row_count,
                };
                rows_by_morsel
                    .entry(morsel_key)
                    .or_default()
                    .insert(row_in_morsel);
            }
        }

        for (key, rows) in rows_by_morsel {
            if file_state.file(0)?.visibility().morsel_all_hidden(
                key.row_start,
                key.row_count,
                file_state.table().row_count,
            )? {
                graph.morsels_pruned += 1;
                continue;
            }
            graph.morsels_considered += 1;
            graph.tasks.push(ScanTask {
                file_ordinal: key.file_ordinal,
                segment_index: key.segment_index,
                segment_id: key.segment_id,
                morsel_id: key.morsel_id,
                row_start: key.row_start,
                row_count: key.row_count,
                output_columns: file_plan.column_plan.output_columns.clone(),
                predicate_columns: file_plan.column_plan.predicate_columns.clone(),
                row_selection: Some(rows.into_iter().collect()),
            });
        }
    }
    finalize_partitions(&mut graph, state.target_morsels_per_partition());
    Ok(Some(graph))
}

fn lookup_keys_for_plan(plan: &ScanPlan) -> Option<(usize, LookupKeyKind, Vec<u64>)> {
    if plan.filters.len() != 1 {
        return None;
    }
    match plan.filters.first()?.predicate.as_ref()? {
        CovePredicate::FileCodeIn {
            column_index,
            file_codes,
            ..
        } => Some((
            *column_index,
            LookupKeyKind::FileCode,
            file_codes.iter().copied().map(u64::from).collect(),
        )),
        CovePredicate::Numeric {
            column_index,
            op: NumericPredicateOp::Eq,
            literal,
        } => Some((
            *column_index,
            LookupKeyKind::NumCode,
            vec![numeric_lookup_key(*literal)?],
        )),
        _ => None,
    }
}

fn finalize_partitions(graph: &mut TaskGraph, target_morsels_per_partition: usize) {
    graph.partitions = partition_tasks(&graph.tasks, target_morsels_per_partition);
    if graph.partitions.is_empty() {
        graph.partitions.push(TaskPartition::default());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RowrefMorselKey {
    file_ordinal: usize,
    segment_index: usize,
    segment_id: u32,
    morsel_id: u32,
    row_start: u64,
    row_count: u32,
}

fn partition_tasks(tasks: &[ScanTask], target_morsels_per_partition: usize) -> Vec<TaskPartition> {
    let target = target_morsels_per_partition.max(1);
    if tasks.is_empty() {
        return Vec::new();
    }
    tasks
        .chunks(target)
        .map(|chunk| TaskPartition {
            tasks: chunk.to_vec(),
        })
        .collect()
}

trait SegmentTaskExt {
    fn morsel_count(&self) -> u32;
    fn morsel_row_count_for(&self, morsel_id: u32) -> Result<u32, CoveError>;
}

impl SegmentTaskExt for cove_core::segment::TableSegmentIndexEntryV1 {
    fn morsel_count(&self) -> u32 {
        self.morsel_count
    }

    fn morsel_row_count_for(&self, morsel_id: u32) -> Result<u32, CoveError> {
        let start = morsel_id
            .checked_mul(self.morsel_row_count)
            .ok_or(CoveError::ArithOverflow)?;
        if start >= self.row_count {
            return Err(CoveError::OffsetRange);
        }
        let remaining = self.row_count - start;
        Ok(remaining.min(self.morsel_row_count))
    }
}
