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
    pub split_id: Option<u32>,
    pub cluster_id: Option<u32>,
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
    pub scan_splits_used: usize,
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
    let mut split_partitions = Vec::new();
    let mut fallback_tasks = Vec::new();
    for file_ordinal in 0..state.file_count() {
        let file_state = state.single_file_view(file_ordinal)?;
        let file_plan = state.resolved_plan_for_file(plan, file_ordinal)?;
        if let Some(splits) = file_state.file(0)?.layout().scan_splits.as_deref() {
            let segment_by_id = file_state
                .segments()
                .iter()
                .enumerate()
                .map(|(segment_index, segment)| (segment.segment_id, segment_index))
                .collect::<BTreeMap<_, _>>();
            for split in &splits.entries {
                let mut partition = TaskPartition::default();
                let segment_end = split
                    .first_segment_id
                    .checked_add(split.segment_count)
                    .ok_or(CoveError::ArithOverflow)?;
                let morsel_end = split
                    .first_morsel_id
                    .checked_add(split.morsel_count)
                    .ok_or(CoveError::ArithOverflow)?;
                for segment_id in split.first_segment_id..segment_end {
                    let Some(segment_index) = segment_by_id.get(&segment_id).copied() else {
                        continue;
                    };
                    let segment = file_state
                        .segments()
                        .get(segment_index)
                        .ok_or(CoveError::SegmentCorrupt)?;
                    for morsel_id in split.first_morsel_id..morsel_end.min(segment.morsel_count()) {
                        maybe_push_task(
                            &file_state,
                            &file_plan,
                            file_ordinal,
                            segment_index,
                            segment,
                            morsel_id,
                            Some(split.split_id),
                            single_cluster_id(split.first_cluster_id, split.cluster_count),
                            &mut graph,
                            Some(&mut partition),
                            None,
                        )?;
                    }
                }
                if !partition.tasks.is_empty() {
                    graph.scan_splits_used += 1;
                    split_partitions.push(partition);
                }
            }
        } else {
            for (segment_index, segment) in file_state.segments().iter().enumerate() {
                for morsel_id in 0..segment.morsel_count() {
                    maybe_push_task(
                        &file_state,
                        &file_plan,
                        file_ordinal,
                        segment_index,
                        segment,
                        morsel_id,
                        None,
                        None,
                        &mut graph,
                        None,
                        Some(&mut fallback_tasks),
                    )?;
                }
            }
        }
    }
    if split_partitions.is_empty() {
        finalize_partitions(&mut graph, state.target_morsels_per_partition());
    } else {
        graph.partitions.extend(split_partitions);
        graph.partitions.extend(partition_tasks(
            &fallback_tasks,
            state.target_morsels_per_partition(),
        ));
        if graph.partitions.is_empty() {
            graph.partitions.push(TaskPartition::default());
        }
    }
    Ok(graph)
}

#[allow(clippy::too_many_arguments)]
fn maybe_push_task(
    file_state: &DatasetState,
    file_plan: &ScanPlan,
    file_ordinal: usize,
    segment_index: usize,
    segment: &cove_core::segment::TableSegmentIndexEntryV1,
    morsel_id: u32,
    split_id: Option<u32>,
    cluster_id: Option<u32>,
    graph: &mut TaskGraph,
    partition: Option<&mut TaskPartition>,
    fallback_tasks: Option<&mut Vec<ScanTask>>,
) -> Result<(), CoveError> {
    graph.morsels_considered += 1;
    let row_start = u64::from(segment.row_start)
        .checked_add(
            u64::from(morsel_id)
                .checked_mul(u64::from(segment.morsel_row_count))
                .ok_or(CoveError::ArithOverflow)?,
        )
        .ok_or(CoveError::ArithOverflow)?;
    let row_count = segment.morsel_row_count_for(morsel_id)?;
    if file_state.file(0)?.visibility().morsel_all_hidden(
        row_start,
        row_count,
        file_state.table().row_count,
    )? || prune::morsel_pruned(file_state, segment.segment_id, morsel_id, file_plan)?
    {
        graph.morsels_pruned += 1;
        return Ok(());
    }
    let task = ScanTask {
        file_ordinal,
        segment_index,
        segment_id: segment.segment_id,
        morsel_id,
        row_start,
        row_count,
        split_id,
        cluster_id,
        output_columns: file_plan.column_plan.output_columns.clone(),
        predicate_columns: file_plan.column_plan.predicate_columns.clone(),
        row_selection: None,
    };
    if let Some(partition) = partition {
        partition.tasks.push(task.clone());
    }
    if let Some(fallback_tasks) = fallback_tasks {
        fallback_tasks.push(task.clone());
    }
    graph.tasks.push(task);
    Ok(())
}

fn single_cluster_id(first_cluster_id: u32, cluster_count: u32) -> Option<u32> {
    if cluster_count == 1 {
        Some(first_cluster_id)
    } else {
        None
    }
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
                split_id: None,
                cluster_id: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use cove_core::{
        constants::{
            CoveEncodingKind, CoveLogicalType, CovePhysicalKind, PrimaryProfile, SectionKind,
        },
        segment::TableSegmentIndexEntryV1,
        table::{ColumnEntry, TableCatalog, TableEntry},
        writer::{ScanPageSpec, ScanProfileCoveWriter, ScanSegment, SectionPayload},
    };
    use cove_layout::build_default_scan_split_index;

    use crate::planner::plan_scan;

    fn split_test_table() -> TableCatalog {
        TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "events".into(),
                row_count: 4,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![ColumnEntry {
                    column_id: 1,
                    name: "id".into(),
                    logical: CoveLogicalType::Int64,
                    physical: CovePhysicalKind::NumCode,
                    nullable: false,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        }
    }

    fn split_authority_segments() -> Vec<TableSegmentIndexEntryV1> {
        vec![
            TableSegmentIndexEntryV1 {
                table_id: 1,
                segment_id: 1,
                row_start: 0,
                row_count: 2,
                morsel_count: 2,
                morsel_row_count: 1,
                column_count: 1,
                offset: 0,
                length: 1,
                stats_ref: 0,
                flags: 0,
                checksum: 0,
            },
            TableSegmentIndexEntryV1 {
                table_id: 1,
                segment_id: 2,
                row_start: 2,
                row_count: 2,
                morsel_count: 2,
                morsel_row_count: 1,
                column_count: 1,
                offset: 0,
                length: 1,
                stats_ref: 0,
                flags: 0,
                checksum: 0,
            },
        ]
    }

    fn split_test_page(value: i64) -> ScanPageSpec {
        ScanPageSpec::new(1, (value as u64).to_le_bytes().to_vec())
            .with_encoding_root(CoveEncodingKind::NumCode as u32)
    }

    fn split_test_file(mut split_payload: Option<Vec<u8>>) -> Vec<u8> {
        let mut writer = ScanProfileCoveWriter::new(split_test_table());
        let mut first = ScanSegment::new(1, 1, 0, 2, 1);
        first.morsel_row_count = 1;
        first.set_column_pages(1, vec![split_test_page(1), split_test_page(2)]);
        writer.push_segment(first);

        let mut second = ScanSegment::new(1, 2, 2, 2, 1);
        second.morsel_row_count = 1;
        second.set_column_pages(1, vec![split_test_page(3), split_test_page(4)]);
        writer.push_segment(second);

        if let Some(data) = split_payload.take() {
            writer.push_extra_section(SectionPayload {
                section_kind: SectionKind::ScanSplitIndex as u16,
                profile: PrimaryProfile::LayoutPlanning as u8,
                flags: 0,
                item_count: 2,
                row_count: 4,
                compression: cove_core::constants::CompressionCodec::None as u8,
                alignment_log2: 0,
                required_features: 0,
                optional_features: cove_core::constants::FEATURE_SCAN_SPLIT_INDEX,
                data,
            });
        }

        writer.write().unwrap()
    }

    #[test]
    fn valid_scan_splits_drive_one_partition_per_split() {
        let catalog = split_test_table();
        let table = &catalog.tables[0];
        let split_index = build_default_scan_split_index(table, &split_authority_segments())
            .unwrap()
            .serialize()
            .unwrap();
        let state =
            DatasetState::from_bytes("split-test", split_test_file(Some(split_index))).unwrap();
        let plan = plan_scan(&state, None, Vec::new()).unwrap();

        let graph = build_task_graph(&state, &plan).unwrap();

        assert_eq!(graph.partitions.len(), 2);
        assert_eq!(graph.scan_splits_used, 2);
        assert_eq!(graph.tasks.len(), 4);
        assert!(graph.partitions[0]
            .tasks
            .iter()
            .all(|task| task.split_id == Some(1)));
        assert!(graph.partitions[1]
            .tasks
            .iter()
            .all(|task| task.split_id == Some(2)));
    }

    #[test]
    fn invalid_optional_scan_splits_fall_back_to_default_partitioning() {
        let catalog = split_test_table();
        let table = &catalog.tables[0];
        let mut split_index =
            build_default_scan_split_index(table, &split_authority_segments()).unwrap();
        split_index.entries[0].row_count = 99;
        let state = DatasetState::from_bytes(
            "bad-split-test",
            split_test_file(Some(split_index.serialize().unwrap())),
        )
        .unwrap();
        let plan = plan_scan(&state, None, Vec::new()).unwrap();

        let graph = build_task_graph(&state, &plan).unwrap();

        assert_eq!(graph.partitions.len(), 1);
        assert_eq!(graph.scan_splits_used, 0);
        assert!(graph.tasks.iter().all(|task| task.split_id.is_none()));
        assert_eq!(state.bootstrap_stats().covel_sections_ignored, 1);
    }
}
