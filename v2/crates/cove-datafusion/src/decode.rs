//! Decode and materialization kernels shared by execution modes.

mod cache;
mod materialize;
mod morsels;
mod predicates;
mod pruning;
mod selection;
mod stats;

use std::{borrow::Cow, collections::BTreeSet, path::Path, sync::Arc};

use arrow_array::{
    types::UInt32Type, Array, ArrayRef, DictionaryArray, RecordBatch, RecordBatchOptions,
};
use arrow_schema::SchemaRef;
use cove_arrow::arrow::{
    arrow_buffer_owner, encoded_array_to_arrow_with_row_selection_options_and_owner,
    encoded_filecode_array_to_arrow_dictionary_remapped,
    encoded_filecode_array_to_arrow_dictionary_with_values,
    file_dictionary_entries_compatible_with_logical, file_dictionary_values_to_arrow,
    filecode_dictionary_value_export_options, nested_page_payload_to_arrow_array, ArrowBufferOwner,
    ArrowDictionaryPolicy, ArrowExportOptions, ArrowRowSelection, ArrowStringValidationPolicy,
    ArrowVarBytesExportPolicy,
};
use cove_core::{
    array::{CoveArrayValue, EncodedArray, PreparedEncodedArray},
    compression,
    constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
    index::{lookup::LookupKeyKind, topn::TopNDirection},
    nested_schema::NestedSchemaNodeV1,
    page::{
        page_uses_payload_elision, ColumnPageIndex, ColumnPageIndexEntryV1, PAGE_FLAG_ALL_NON_NULL,
        PAGE_FLAG_ALL_NULL, PAGE_FLAG_STATS_ONLY_CONSTANT,
    },
    page_payload::{ColumnPagePayloadV1, PageBufferKind, RetainedColumnPagePayloadV1},
    retained_bytes::RetainedBytes,
    segment::{
        RowMorselDirectory, RowMorselEntryV1, TableColumnDirectoryEntryV1, TableSegmentHeaderV1,
        TableSegmentIndexEntryV1, TableSegmentPayloadV1, TABLE_COLUMN_DIRECTORY_ENTRY_LEN,
        TABLE_SEGMENT_HEADER_LEN,
    },
    table::ColumnEntry,
    validity::ValidityBitmap,
    wire, CoveError,
};
use cove_layout::{ZeroCopyCompatibilityV2, ZeroCopyMaterializationReasonV2};

use crate::{
    dataset_state::{DatasetBootstrapStats, DatasetState},
    options::{LocalFileReadPolicy, PagePayloadValidationPolicy},
    planner::{
        CoveFilterUse, CovePredicate, NullPredicateKind, NumericPredicateOp, PredicateLiteral,
        ScanPlan,
    },
    prune,
    range_reader::{
        build_layout_aware_coalesced_range_plan, read_coalesced_range_buffers_for_plan,
        CoveRangeReader, LocalFileRangeReader, MemoryRangeReader, MmapFileRangeReader,
        RangeReadKind, RangeReadMode, RangeReadPlan,
    },
    task_graph::ScanTask,
};
pub use stats::{DecodeStats, DecodedScan};

use cache::{ArrowDictionaryValuesCacheKey, SegmentMetadataCacheKey, Utf8ProofKey};
pub(crate) use cache::{ScanExecutionCache, Utf8ProofCache};
#[cfg(test)]
use materialize::DecodedArrowColumn;
use materialize::{
    arrow_encoded_columns_for_payloads, encoded_array_for_page, materialize_page_payload,
    materialize_page_payload_from_wire, record_batch_for_selection,
};
use morsels::{ordered_morsels, prepare_segment_payload, read_segment_metadata};
#[cfg(test)]
use predicates::apply_predicate_to_selection;
pub(crate) use predicates::numeric_lookup_key;
use predicates::{plan_has_exact_row_predicate, plan_has_residual};
use pruning::{
    apply_overlay_to_selection, covi_morsel_pruned, selected_rows_for_morsel,
    selected_rows_for_morsel_metadata, should_prune_morsel, should_prune_morsel_metadata,
};
use selection::{DecodeScratch, Selection, SelectionMask};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecodeControl {
    Continue,
    Stop,
}

pub(crate) trait DecodeSink {
    fn emit_batch(
        &mut self,
        batch: RecordBatch,
        stats: &mut DecodeStats,
    ) -> Result<DecodeControl, CoveError>;

    fn should_stop(&self) -> bool {
        false
    }
}

#[derive(Debug, Default)]
struct VecDecodeSink {
    batches: Vec<RecordBatch>,
}

impl VecDecodeSink {
    fn finish(self, stats: DecodeStats) -> DecodedScan {
        DecodedScan {
            batches: self.batches,
            stats,
        }
    }
}

impl DecodeSink for VecDecodeSink {
    fn emit_batch(
        &mut self,
        batch: RecordBatch,
        stats: &mut DecodeStats,
    ) -> Result<DecodeControl, CoveError> {
        stats.rows_materialized += batch.num_rows();
        self.batches.push(batch);
        Ok(DecodeControl::Continue)
    }
}

#[derive(Debug)]
pub(crate) struct FetchLimitedDecodeSink<S> {
    inner: S,
    remaining: Option<usize>,
    stopped: bool,
}

impl<S> FetchLimitedDecodeSink<S> {
    pub(crate) fn new(inner: S, fetch: Option<usize>) -> Self {
        Self {
            inner,
            remaining: fetch,
            stopped: fetch == Some(0),
        }
    }
}

impl<S: DecodeSink> DecodeSink for FetchLimitedDecodeSink<S> {
    fn emit_batch(
        &mut self,
        batch: RecordBatch,
        stats: &mut DecodeStats,
    ) -> Result<DecodeControl, CoveError> {
        if self.stopped {
            return Ok(DecodeControl::Stop);
        }

        let batch = match self.remaining {
            Some(0) => {
                self.stopped = true;
                return Ok(DecodeControl::Stop);
            }
            Some(remaining) if batch.num_rows() > remaining => batch.slice(0, remaining),
            _ => batch,
        };
        let emitted_rows = batch.num_rows();
        let control = self.inner.emit_batch(batch, stats)?;
        if let Some(remaining) = self.remaining.as_mut() {
            *remaining = remaining.saturating_sub(emitted_rows);
            if *remaining == 0 {
                self.stopped = true;
                return Ok(DecodeControl::Stop);
            }
        }
        if control == DecodeControl::Stop {
            self.stopped = true;
        }
        Ok(control)
    }

    fn should_stop(&self) -> bool {
        self.stopped || self.inner.should_stop()
    }
}

fn emit_batch<S: DecodeSink + ?Sized>(
    sink: &mut S,
    stats: &mut DecodeStats,
    batch: RecordBatch,
) -> Result<DecodeControl, CoveError> {
    sink.emit_batch(batch, stats)
}

pub fn decode_local_dataset_scan(
    state: &DatasetState,
    plan: &ScanPlan,
) -> Result<DecodedScan, CoveError> {
    let cache = Arc::new(ScanExecutionCache::default());
    decode_local_dataset_scan_with_cache(state, plan, cache)
}

fn decode_local_dataset_scan_with_cache(
    state: &DatasetState,
    plan: &ScanPlan,
    cache: Arc<ScanExecutionCache>,
) -> Result<DecodedScan, CoveError> {
    let mut decoded = DecodedScan {
        batches: Vec::new(),
        stats: DecodeStats::default(),
    };
    decoded.stats.record_bootstrap(state.bootstrap_stats());

    for file_ordinal in 0..state.file_count() {
        let (file_plan, execution_stats) =
            state.resolved_plan_for_file_with_stats(plan, file_ordinal)?;
        decoded.stats.execution_code_profiles_used += execution_stats.supported_files;
        decoded.stats.execution_code_profile_fallbacks += execution_stats.fallback_files;
        decoded.stats.execution_code_literal_resolutions += execution_stats.literal_resolutions;
        if plan_selects_no_rows(&file_plan) {
            decoded.stats.files_pruned += 1;
            continue;
        }
        let file_state = state.single_file_view(file_ordinal)?;
        let file_decoded = if file_state.has_full_file_bytes() {
            decode_scan(&file_state, &file_plan)?
        } else if Path::new(file_state.source()).is_file() {
            let reader = cache.local_reader(
                file_ordinal,
                file_state.local_file_read_policy(),
                file_state.source(),
            )?;
            futures::executor::block_on(decode_scan_with_reader_cached(
                &file_state,
                &file_plan,
                reader.as_ref(),
                Some(cache.as_ref()),
                file_ordinal,
            ))?
        } else {
            decode_scan(&file_state, &file_plan)?
        };
        decoded.stats.add_decode(file_decoded.stats);
        decoded.batches.extend(file_decoded.batches);
    }
    Ok(decoded)
}

pub fn decode_local_dataset_scan_tasks(
    state: &DatasetState,
    plan: &ScanPlan,
    tasks: &[ScanTask],
    partition_index: usize,
    partition_count: usize,
) -> Result<DecodedScan, CoveError> {
    let cache = Arc::new(ScanExecutionCache::default());
    decode_local_dataset_scan_tasks_with_cache(
        state,
        plan,
        tasks,
        partition_index,
        partition_count,
        cache,
    )
}

pub(crate) fn decode_local_dataset_scan_tasks_with_cache(
    state: &DatasetState,
    plan: &ScanPlan,
    tasks: &[ScanTask],
    partition_index: usize,
    partition_count: usize,
    cache: Arc<ScanExecutionCache>,
) -> Result<DecodedScan, CoveError> {
    let mut sink = VecDecodeSink::default();
    let stats = decode_local_dataset_scan_tasks_with_sink(
        state,
        plan,
        tasks,
        partition_index,
        partition_count,
        cache,
        &mut sink,
    )?;
    Ok(sink.finish(stats))
}

pub(crate) fn decode_local_dataset_scan_tasks_with_sink<S: DecodeSink + ?Sized>(
    state: &DatasetState,
    plan: &ScanPlan,
    tasks: &[ScanTask],
    partition_index: usize,
    partition_count: usize,
    cache: Arc<ScanExecutionCache>,
    sink: &mut S,
) -> Result<DecodeStats, CoveError> {
    let mut stats = DecodeStats {
        scan_tasks: tasks.len(),
        scan_partitions: usize::from(partition_index == 0) * partition_count,
        covel_scan_splits_used: tasks
            .iter()
            .filter_map(|task| task.split_id.map(|split_id| (task.file_ordinal, split_id)))
            .collect::<BTreeSet<_>>()
            .len(),
        ..DecodeStats::default()
    };
    if partition_index == 0 {
        stats.record_bootstrap(state.bootstrap_stats());
    }
    if sink.should_stop() {
        return Ok(stats);
    }

    let mut task_start = 0usize;
    while task_start < tasks.len() {
        let file_ordinal = tasks[task_start].file_ordinal;
        let task_end = tasks[task_start..]
            .iter()
            .position(|task| task.file_ordinal != file_ordinal)
            .map(|offset| task_start + offset)
            .unwrap_or(tasks.len());
        let file_tasks = &tasks[task_start..task_end];
        let (file_plan, execution_stats) =
            state.resolved_plan_for_file_with_stats(plan, file_ordinal)?;
        stats.execution_code_profiles_used += execution_stats.supported_files;
        stats.execution_code_profile_fallbacks += execution_stats.fallback_files;
        stats.execution_code_literal_resolutions += execution_stats.literal_resolutions;
        if plan_selects_no_rows(&file_plan) {
            stats.files_pruned += 1;
            task_start = task_end;
            continue;
        }
        let file_state = state.single_file_view(file_ordinal)?;
        let file_stats = if let Some(bytes) = file_state
            .files()
            .first()
            .and_then(|file| file.full_file_bytes_arc())
        {
            let reader = MemoryRangeReader::from_arc(bytes);
            futures::executor::block_on(decode_scan_with_reader_tasks_to_sink_cached(
                &file_state,
                &file_plan,
                &reader,
                file_tasks,
                Some(cache.as_ref()),
                file_ordinal,
                sink,
            ))?
        } else if Path::new(file_state.source()).is_file() {
            let reader = cache.local_reader(
                file_ordinal,
                file_state.local_file_read_policy(),
                file_state.source(),
            )?;
            futures::executor::block_on(decode_scan_with_reader_tasks_to_sink_cached(
                &file_state,
                &file_plan,
                reader.as_ref(),
                file_tasks,
                Some(cache.as_ref()),
                file_ordinal,
                sink,
            ))?
        } else {
            decode_scan_to_sink(&file_state, &file_plan, sink)?
        };
        stats.add_decode(file_stats);
        if sink.should_stop() {
            return Ok(stats);
        }
        task_start = task_end;
    }
    Ok(stats)
}

/// Decode a planned native single-file scan into Arrow record batches.
///
/// INVARIANT: this routine emits rows in segment order and morsel order, and it
/// delegates scalar COVE-to-Arrow representation rules to `cove-arrow`. FileCode
/// predicates are resolved against this concrete single-file view before
/// pruning or residual filtering begins.
pub fn decode_scan(state: &DatasetState, plan: &ScanPlan) -> Result<DecodedScan, CoveError> {
    let mut sink = VecDecodeSink::default();
    let stats = decode_scan_to_sink(state, plan, &mut sink)?;
    Ok(sink.finish(stats))
}

pub(crate) fn decode_scan_to_sink<S: DecodeSink + ?Sized>(
    state: &DatasetState,
    plan: &ScanPlan,
    sink: &mut S,
) -> Result<DecodeStats, CoveError> {
    let plan = state.resolved_plan_for_current_state(plan)?;
    validate_scan_plan(state, &plan)?;
    let mut stats = DecodeStats::default();
    record_plan_predicates(&plan, &mut stats);
    let mut scratch = DecodeScratch::default();
    if sink.should_stop() {
        return Ok(stats);
    }

    for segment_ref in state.segments() {
        let segment_bytes = wire::read_range_checked(
            state.full_file_bytes()?,
            usize::try_from(segment_ref.offset).map_err(|_| CoveError::OffsetRange)?,
            usize::try_from(segment_ref.length).map_err(|_| CoveError::OffsetRange)?,
        )?;
        let segment = TableSegmentPayloadV1::parse_with_required_features(
            segment_bytes,
            state.mounted().header.required_features,
        )?;
        let prepared_segment = prepare_segment_payload(segment_bytes, &segment)?;

        for morsel in ordered_morsels(
            state,
            segment.header.segment_id,
            prepared_segment.morsel_entries(),
            &plan,
        ) {
            stats.morsels_considered += 1;
            let row_start = segment
                .header
                .row_start
                .checked_add(u64::from(morsel.first_row_in_segment))
                .ok_or(CoveError::ArithOverflow)?;
            if state.file(0)?.visibility().morsel_all_hidden(
                row_start,
                morsel.row_count,
                state.table().row_count,
            )? {
                stats.morsels_pruned += 1;
                stats.overlay_morsels_pruned += 1;
                continue;
            }
            if covi_morsel_pruned(
                &plan,
                segment.header.segment_id,
                morsel.morsel_id,
                row_start,
                morsel.row_count,
            ) {
                stats.morsels_pruned += 1;
                stats.covi_candidate_pruned += 1;
                continue;
            }
            if prune::morsel_pruned(state, segment.header.segment_id, morsel.morsel_id, &plan)?
                || should_prune_morsel(
                    state,
                    segment_ref,
                    &prepared_segment,
                    morsel.morsel_id,
                    &plan,
                    &mut stats,
                )?
            {
                stats.morsels_pruned += 1;
                continue;
            }

            selected_rows_for_morsel(
                state,
                segment_bytes,
                &prepared_segment,
                segment_ref,
                segment.header.segment_id,
                morsel.morsel_id,
                &plan,
                &mut stats,
                &mut scratch,
            )?;
            apply_overlay_to_selection(
                state,
                row_start,
                morsel.row_count,
                &mut scratch,
                &mut stats,
            )?;
            scratch.selection.record(&mut stats);
            if scratch.selection.is_empty() {
                stats.rows_selected += 0;
                continue;
            }
            let selected_len = scratch.selection.len();
            stats.rows_selected += selected_len;
            if plan_has_residual(&plan) {
                stats.residual_rows += selected_len;
            }
            record_late_materialization(
                &plan,
                morsel.row_count as usize,
                selected_len,
                &mut stats,
            )?;

            if plan.scan_projection.is_empty() {
                let options = RecordBatchOptions::new().with_row_count(Some(selected_len));
                let batch = RecordBatch::try_new_with_options(
                    plan.output_schema.clone(),
                    Vec::new(),
                    &options,
                )
                .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))?;
                if emit_batch(sink, &mut stats, batch)? == DecodeControl::Stop {
                    return Ok(stats);
                }
                continue;
            }

            let mut page_payloads = Vec::with_capacity(plan.scan_projection.len());
            let mut page_indexes = Vec::with_capacity(plan.scan_projection.len());
            let mut columns = Vec::with_capacity(plan.scan_projection.len());
            for projection_index in &plan.scan_projection {
                let column = &state.table().columns[*projection_index];
                let segment_column = prepared_segment.column(column.column_id)?;
                let page = prepared_segment.page_for_morsel(segment_column, morsel.morsel_id)?;
                state.reject_table_scan_page_feature_use(segment_ref, page)?;
                let payload = materialize_page_payload(
                    segment_bytes,
                    column,
                    page,
                    state.pruning().codec_descriptors.as_slice(),
                    state
                        .mounted()
                        .dictionary
                        .as_ref()
                        .map(|dictionary| dictionary.len()),
                    state.page_payload_validation_policy(),
                )?;
                stats.pages_decoded += usize::from(page.page_length != 0);
                stats.data_bytes_read = stats
                    .data_bytes_read
                    .checked_add(
                        usize::try_from(page.page_length).map_err(|_| CoveError::OffsetRange)?,
                    )
                    .ok_or(CoveError::ArithOverflow)?;
                page_payloads.push(payload);
                page_indexes.push(page.clone());
                columns.push(column);
            }

            let mut encoded_columns = Vec::with_capacity(columns.len());
            for ((column, page), payload) in columns
                .iter()
                .zip(page_indexes.iter())
                .zip(page_payloads.iter())
            {
                encoded_columns.push((
                    column.name.as_str(),
                    encoded_array_for_page(payload, page, state.mounted().dictionary.as_ref())?,
                ));
            }
            let arrow_options = state.arrow_export_options();
            let column_refs = arrow_encoded_columns_for_payloads(
                state,
                segment.header.segment_id,
                &columns,
                &encoded_columns,
                &page_indexes,
                &page_payloads,
                arrow_options,
            );
            let batch = record_batch_for_selection(
                state,
                &column_refs,
                &scratch.selection,
                plan.output_schema.clone(),
                arrow_options,
                None,
                &mut stats,
            )?
            .value;
            if emit_batch(sink, &mut stats, batch)? == DecodeControl::Stop {
                return Ok(stats);
            }
        }
    }

    Ok(stats)
}

fn plan_selects_no_rows(plan: &ScanPlan) -> bool {
    if plan
        .covi_candidates
        .as_ref()
        .map(Vec::is_empty)
        .unwrap_or(false)
    {
        return true;
    }
    plan.filters.iter().any(|filter| {
        matches!(
            filter.predicate,
            Some(CovePredicate::FileCodeIn { ref file_codes, .. }) if file_codes.is_empty()
        )
    })
}

fn record_plan_predicates(plan: &ScanPlan, stats: &mut DecodeStats) {
    stats.exact_predicates += plan.scan_program.exact_filters;
    stats.residual_predicates += plan.scan_program.inexact_filters;
    stats.predicate_orderings += usize::from(plan.scan_program.predicate_ordered);
}

fn record_range_plan(plan: RangeReadPlan, stats: &mut DecodeStats) {
    match plan.mode {
        RangeReadMode::Sparse => stats.range_plan_sparse += 1,
        RangeReadMode::Mixed => stats.range_plan_mixed += 1,
        RangeReadMode::Dense => stats.range_plan_dense += 1,
    }
}

fn record_late_materialization(
    plan: &ScanPlan,
    row_count: usize,
    selected_len: usize,
    stats: &mut DecodeStats,
) -> Result<(), CoveError> {
    if plan.scan_projection.is_empty()
        || selected_len >= row_count
        || !plan_has_exact_row_predicate(plan)
    {
        return Ok(());
    }
    let skipped_rows = row_count
        .checked_sub(selected_len)
        .ok_or(CoveError::ArithOverflow)?;
    let skipped_cells = skipped_rows
        .checked_mul(plan.scan_projection.len())
        .ok_or(CoveError::ArithOverflow)?;
    stats.late_materialization_morsels += 1;
    stats.late_materialization_rows_skipped = stats
        .late_materialization_rows_skipped
        .checked_add(skipped_rows)
        .ok_or(CoveError::ArithOverflow)?;
    stats.late_materialization_cells_skipped = stats
        .late_materialization_cells_skipped
        .checked_add(skipped_cells)
        .ok_or(CoveError::ArithOverflow)?;
    Ok(())
}

pub async fn decode_scan_with_reader<R: CoveRangeReader + ?Sized>(
    state: &DatasetState,
    plan: &ScanPlan,
    reader: &R,
) -> Result<DecodedScan, CoveError> {
    let mut sink = VecDecodeSink::default();
    let stats =
        decode_scan_with_reader_to_sink_cached(state, plan, reader, None, 0, &mut sink).await?;
    Ok(sink.finish(stats))
}

async fn decode_scan_with_reader_cached<R: CoveRangeReader + ?Sized>(
    state: &DatasetState,
    plan: &ScanPlan,
    reader: &R,
    cache: Option<&ScanExecutionCache>,
    file_ordinal: usize,
) -> Result<DecodedScan, CoveError> {
    let mut sink = VecDecodeSink::default();
    let stats =
        decode_scan_with_reader_to_sink_cached(state, plan, reader, cache, file_ordinal, &mut sink)
            .await?;
    Ok(sink.finish(stats))
}

pub(crate) async fn decode_scan_with_reader_to_sink<
    R: CoveRangeReader + ?Sized,
    S: DecodeSink + ?Sized,
>(
    state: &DatasetState,
    plan: &ScanPlan,
    reader: &R,
    sink: &mut S,
) -> Result<DecodeStats, CoveError> {
    decode_scan_with_reader_to_sink_cached(state, plan, reader, None, 0, sink).await
}

async fn decode_scan_with_reader_to_sink_cached<
    R: CoveRangeReader + ?Sized,
    S: DecodeSink + ?Sized,
>(
    state: &DatasetState,
    plan: &ScanPlan,
    reader: &R,
    cache: Option<&ScanExecutionCache>,
    file_ordinal: usize,
    sink: &mut S,
) -> Result<DecodeStats, CoveError> {
    let plan = state.resolved_plan_for_current_state(plan)?;
    validate_scan_plan(state, &plan)?;
    let mut stats = DecodeStats::default();
    record_plan_predicates(&plan, &mut stats);
    let mut scratch = DecodeScratch::default();
    if sink.should_stop() {
        return Ok(stats);
    }

    for segment_ref in state.segments() {
        let segment =
            read_segment_metadata(reader, state, segment_ref, &mut stats, cache, file_ordinal)
                .await?;

        for morsel in ordered_morsels(
            state,
            segment_ref.segment_id,
            segment.morsel_entries(),
            &plan,
        ) {
            stats.morsels_considered += 1;
            let row_start = segment_ref
                .row_start
                .checked_add(u64::from(morsel.first_row_in_segment))
                .ok_or(CoveError::ArithOverflow)?;
            if state.file(0)?.visibility().morsel_all_hidden(
                row_start,
                morsel.row_count,
                state.table().row_count,
            )? {
                stats.morsels_pruned += 1;
                stats.overlay_morsels_pruned += 1;
                continue;
            }
            if covi_morsel_pruned(
                &plan,
                segment_ref.segment_id,
                morsel.morsel_id,
                row_start,
                morsel.row_count,
            ) {
                stats.morsels_pruned += 1;
                stats.covi_candidate_pruned += 1;
                continue;
            }
            if prune::morsel_pruned(state, segment_ref.segment_id, morsel.morsel_id, &plan)?
                || should_prune_morsel_metadata(
                    state,
                    segment_ref,
                    &segment,
                    morsel.morsel_id,
                    &plan,
                    &mut stats,
                )?
            {
                stats.morsels_pruned += 1;
                continue;
            }

            selected_rows_for_morsel_metadata(
                state,
                &segment,
                segment_ref,
                morsel.morsel_id,
                &plan,
                reader,
                &mut stats,
                &mut scratch,
            )
            .await?;
            apply_overlay_to_selection(
                state,
                row_start,
                morsel.row_count,
                &mut scratch,
                &mut stats,
            )?;
            scratch.selection.record(&mut stats);
            if scratch.selection.is_empty() {
                continue;
            }
            let selected_len = scratch.selection.len();
            stats.rows_selected += selected_len;
            if plan_has_residual(&plan) {
                stats.residual_rows += selected_len;
            }

            if plan.scan_projection.is_empty() {
                let options = RecordBatchOptions::new().with_row_count(Some(selected_len));
                let batch = RecordBatch::try_new_with_options(
                    plan.output_schema.clone(),
                    Vec::new(),
                    &options,
                )
                .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))?;
                if emit_batch(sink, &mut stats, batch)? == DecodeControl::Stop {
                    return Ok(stats);
                }
                continue;
            }

            let mut page_indexes = Vec::with_capacity(plan.scan_projection.len());
            let mut columns = Vec::with_capacity(plan.scan_projection.len());
            let mut ranges = Vec::new();
            let mut range_hints = Vec::new();
            let mut range_slots = Vec::with_capacity(plan.scan_projection.len());
            for projection_index in &plan.scan_projection {
                let column = &state.table().columns[*projection_index];
                let segment_column = segment.column(column.column_id)?;
                let page = segment.page_for_morsel(segment_column, morsel.morsel_id)?;
                state.reject_table_scan_page_feature_use(segment_ref, page)?;
                if page.page_length == 0 {
                    range_slots.push(None);
                } else {
                    let start = segment_ref
                        .offset
                        .checked_add(page.page_offset)
                        .ok_or(CoveError::ArithOverflow)?;
                    let end = start
                        .checked_add(page.page_length)
                        .ok_or(CoveError::ArithOverflow)?;
                    range_slots.push(Some(ranges.len()));
                    range_hints.push(state.range_cluster_hint(
                        segment_ref.segment_id,
                        morsel.morsel_id,
                        start,
                        end,
                    ));
                    ranges.push(start..end);
                }
                stats.pages_decoded += usize::from(page.page_length != 0);
                page_indexes.push(page.clone());
                columns.push(column);
            }

            let coalesced_plan = build_layout_aware_coalesced_range_plan(
                &ranges,
                &range_hints,
                state.range_coalescing(),
            )?;
            let range_stats = coalesced_plan.stats();
            record_range_plan(
                RangeReadPlan::choose(
                    selected_len,
                    morsel.row_count as usize,
                    range_stats.original_ranges,
                    range_stats.coalesced_ranges,
                ),
                &mut stats,
            );
            stats.original_range_requests += range_stats.original_ranges;
            stats.range_requests += range_stats.coalesced_ranges;
            stats.range_bytes_requested = stats
                .range_bytes_requested
                .checked_add(range_stats.coalesced_bytes)
                .ok_or(CoveError::ArithOverflow)?;
            stats.range_bytes_used = stats
                .range_bytes_used
                .checked_add(range_stats.original_bytes)
                .ok_or(CoveError::ArithOverflow)?;
            if range_stats.coalesced_ranges < range_stats.original_ranges {
                stats.coalesced_range_requests += range_stats.coalesced_ranges;
            }
            let wires =
                read_coalesced_range_buffers_for_plan(reader, RangeReadKind::Data, &coalesced_plan)
                    .await?;
            stats.data_bytes_read = stats
                .data_bytes_read
                .checked_add(wires.iter().map(RetainedBytes::len).sum::<usize>())
                .ok_or(CoveError::ArithOverflow)?;
            let mut wire_slots = wires.into_iter().map(Some).collect::<Vec<_>>();
            let mut page_payloads = Vec::with_capacity(page_indexes.len());
            for ((column, page), slot) in columns.iter().zip(page_indexes.iter()).zip(range_slots) {
                let wire = slot.and_then(|index| wire_slots[index].take());
                page_payloads.push(materialize_page_payload_from_wire(
                    column,
                    page,
                    wire,
                    state.pruning().codec_descriptors.as_slice(),
                    state
                        .mounted()
                        .dictionary
                        .as_ref()
                        .map(|dictionary| dictionary.len()),
                    state.page_payload_validation_policy(),
                )?);
            }

            let mut encoded_columns = Vec::with_capacity(columns.len());
            for ((column, page), payload) in columns
                .iter()
                .zip(page_indexes.iter())
                .zip(page_payloads.iter())
            {
                encoded_columns.push((
                    column.name.as_str(),
                    encoded_array_for_page(payload, page, state.mounted().dictionary.as_ref())?,
                ));
            }
            let arrow_options = state.arrow_export_options();
            let column_refs = arrow_encoded_columns_for_payloads(
                state,
                segment_ref.segment_id,
                &columns,
                &encoded_columns,
                &page_indexes,
                &page_payloads,
                arrow_options,
            );
            let batch = record_batch_for_selection(
                state,
                &column_refs,
                &scratch.selection,
                plan.output_schema.clone(),
                arrow_options,
                cache,
                &mut stats,
            )?
            .value;
            if emit_batch(sink, &mut stats, batch)? == DecodeControl::Stop {
                return Ok(stats);
            }
        }
    }

    Ok(stats)
}

pub async fn decode_scan_with_reader_tasks<R: CoveRangeReader + ?Sized>(
    state: &DatasetState,
    plan: &ScanPlan,
    reader: &R,
    tasks: &[ScanTask],
) -> Result<DecodedScan, CoveError> {
    let mut sink = VecDecodeSink::default();
    let stats = decode_scan_with_reader_tasks_to_sink_cached(
        state, plan, reader, tasks, None, 0, &mut sink,
    )
    .await?;
    Ok(sink.finish(stats))
}

async fn decode_scan_with_reader_tasks_to_sink_cached<
    R: CoveRangeReader + ?Sized,
    S: DecodeSink + ?Sized,
>(
    state: &DatasetState,
    plan: &ScanPlan,
    reader: &R,
    tasks: &[ScanTask],
    cache: Option<&ScanExecutionCache>,
    file_ordinal: usize,
    sink: &mut S,
) -> Result<DecodeStats, CoveError> {
    let plan = state.resolved_plan_for_current_state(plan)?;
    validate_scan_plan(state, &plan)?;
    let mut stats = DecodeStats::default();
    record_plan_predicates(&plan, &mut stats);
    let mut scratch = DecodeScratch::default();
    if sink.should_stop() {
        return Ok(stats);
    }

    let mut task_start = 0usize;
    while task_start < tasks.len() {
        let segment_index = tasks[task_start].segment_index;
        let segment_ref = state
            .segments()
            .get(segment_index)
            .ok_or(CoveError::SegmentCorrupt)?;
        let segment =
            read_segment_metadata(reader, state, segment_ref, &mut stats, cache, file_ordinal)
                .await?;
        let task_end = tasks[task_start..]
            .iter()
            .position(|task| task.segment_index != segment_index)
            .map(|offset| task_start + offset)
            .unwrap_or(tasks.len());

        for task in &tasks[task_start..task_end] {
            stats.morsels_considered += 1;
            let morsel = segment.morsel(task.morsel_id)?;
            let row_start = segment_ref
                .row_start
                .checked_add(u64::from(morsel.first_row_in_segment))
                .ok_or(CoveError::ArithOverflow)?;
            if state.file(0)?.visibility().morsel_all_hidden(
                row_start,
                morsel.row_count,
                state.table().row_count,
            )? {
                stats.morsels_pruned += 1;
                stats.overlay_morsels_pruned += 1;
                continue;
            }
            if covi_morsel_pruned(
                &plan,
                segment_ref.segment_id,
                morsel.morsel_id,
                row_start,
                morsel.row_count,
            ) {
                stats.morsels_pruned += 1;
                stats.covi_candidate_pruned += 1;
                continue;
            }
            if prune::morsel_pruned(state, segment_ref.segment_id, morsel.morsel_id, &plan)?
                || should_prune_morsel_metadata(
                    state,
                    segment_ref,
                    &segment,
                    morsel.morsel_id,
                    &plan,
                    &mut stats,
                )?
            {
                stats.morsels_pruned += 1;
                continue;
            }

            if let Some(rows) = &task.row_selection {
                scratch.selection = Selection::from_rows(rows, morsel.row_count as usize);
                scratch.selected_rows.clear();
                stats.lookup_rowref_tasks += 1;
            } else {
                selected_rows_for_morsel_metadata(
                    state,
                    &segment,
                    segment_ref,
                    morsel.morsel_id,
                    &plan,
                    reader,
                    &mut stats,
                    &mut scratch,
                )
                .await?;
            }
            apply_overlay_to_selection(
                state,
                row_start,
                morsel.row_count,
                &mut scratch,
                &mut stats,
            )?;
            scratch.selection.record(&mut stats);
            if scratch.selection.is_empty() {
                continue;
            }
            let selected_len = scratch.selection.len();
            stats.rows_selected += selected_len;
            if plan_has_residual(&plan) {
                stats.residual_rows += selected_len;
            }
            record_late_materialization(
                &plan,
                morsel.row_count as usize,
                selected_len,
                &mut stats,
            )?;

            if plan.scan_projection.is_empty() {
                let options = RecordBatchOptions::new().with_row_count(Some(selected_len));
                let batch = RecordBatch::try_new_with_options(
                    plan.output_schema.clone(),
                    Vec::new(),
                    &options,
                )
                .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))?;
                if emit_batch(sink, &mut stats, batch)? == DecodeControl::Stop {
                    return Ok(stats);
                }
                continue;
            }

            let mut page_indexes = Vec::with_capacity(plan.scan_projection.len());
            let mut columns = Vec::with_capacity(plan.scan_projection.len());
            let mut ranges = Vec::new();
            let mut range_hints = Vec::new();
            let mut range_slots = Vec::with_capacity(plan.scan_projection.len());
            for projection_index in &plan.scan_projection {
                let column = &state.table().columns[*projection_index];
                let segment_column = segment.column(column.column_id)?;
                let page = segment.page_for_morsel(segment_column, morsel.morsel_id)?;
                state.reject_table_scan_page_feature_use(segment_ref, page)?;
                if page.page_length == 0 {
                    range_slots.push(None);
                } else {
                    let start = segment_ref
                        .offset
                        .checked_add(page.page_offset)
                        .ok_or(CoveError::ArithOverflow)?;
                    let end = start
                        .checked_add(page.page_length)
                        .ok_or(CoveError::ArithOverflow)?;
                    range_slots.push(Some(ranges.len()));
                    range_hints.push(state.range_cluster_hint(
                        segment_ref.segment_id,
                        morsel.morsel_id,
                        start,
                        end,
                    ));
                    ranges.push(start..end);
                }
                stats.pages_decoded += usize::from(page.page_length != 0);
                page_indexes.push(page.clone());
                columns.push(column);
            }

            let coalesced_plan = build_layout_aware_coalesced_range_plan(
                &ranges,
                &range_hints,
                state.range_coalescing(),
            )?;
            let range_stats = coalesced_plan.stats();
            record_range_plan(
                RangeReadPlan::choose(
                    selected_len,
                    morsel.row_count as usize,
                    range_stats.original_ranges,
                    range_stats.coalesced_ranges,
                ),
                &mut stats,
            );
            stats.original_range_requests += range_stats.original_ranges;
            stats.range_requests += range_stats.coalesced_ranges;
            stats.range_bytes_requested = stats
                .range_bytes_requested
                .checked_add(range_stats.coalesced_bytes)
                .ok_or(CoveError::ArithOverflow)?;
            stats.range_bytes_used = stats
                .range_bytes_used
                .checked_add(range_stats.original_bytes)
                .ok_or(CoveError::ArithOverflow)?;
            if range_stats.coalesced_ranges < range_stats.original_ranges {
                stats.coalesced_range_requests += range_stats.coalesced_ranges;
            }
            let wires =
                read_coalesced_range_buffers_for_plan(reader, RangeReadKind::Data, &coalesced_plan)
                    .await?;
            stats.data_bytes_read = stats
                .data_bytes_read
                .checked_add(wires.iter().map(RetainedBytes::len).sum::<usize>())
                .ok_or(CoveError::ArithOverflow)?;
            let mut wire_slots = wires.into_iter().map(Some).collect::<Vec<_>>();
            let mut page_payloads = Vec::with_capacity(page_indexes.len());
            for ((column, page), slot) in columns.iter().zip(page_indexes.iter()).zip(range_slots) {
                let wire = slot.and_then(|index| wire_slots[index].take());
                page_payloads.push(materialize_page_payload_from_wire(
                    column,
                    page,
                    wire,
                    state.pruning().codec_descriptors.as_slice(),
                    state
                        .mounted()
                        .dictionary
                        .as_ref()
                        .map(|dictionary| dictionary.len()),
                    state.page_payload_validation_policy(),
                )?);
            }

            let mut encoded_columns = Vec::with_capacity(columns.len());
            for ((column, page), payload) in columns
                .iter()
                .zip(page_indexes.iter())
                .zip(page_payloads.iter())
            {
                encoded_columns.push((
                    column.name.as_str(),
                    encoded_array_for_page(payload, page, state.mounted().dictionary.as_ref())?,
                ));
            }
            let arrow_options = state.arrow_export_options();
            let column_refs = arrow_encoded_columns_for_payloads(
                state,
                segment_ref.segment_id,
                &columns,
                &encoded_columns,
                &page_indexes,
                &page_payloads,
                arrow_options,
            );
            let batch = record_batch_for_selection(
                state,
                &column_refs,
                &scratch.selection,
                plan.output_schema.clone(),
                arrow_options,
                cache,
                &mut stats,
            )?
            .value;
            if emit_batch(sink, &mut stats, batch)? == DecodeControl::Stop {
                return Ok(stats);
            }
        }
        if sink.should_stop() {
            return Ok(stats);
        }
        task_start = task_end;
    }

    Ok(stats)
}

fn validate_scan_plan(state: &DatasetState, plan: &ScanPlan) -> Result<(), CoveError> {
    for index in &plan.scan_projection {
        if *index >= state.table().columns.len() {
            return Err(CoveError::BadSchema(format!(
                "scan projection index {index} is out of bounds for {} columns",
                state.table().columns.len()
            )));
        }
    }
    for index in &plan.predicate_columns {
        if *index >= state.table().columns.len() {
            return Err(CoveError::BadSchema(format!(
                "predicate column index {index} is out of bounds for {} columns",
                state.table().columns.len()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        options::CoveTableOptions,
        planner::{plan_scan, FilterPlan},
    };
    use arrow_array::Array;
    use cove_arrow::arrow::ArrowStringValidationPolicy;
    use cove_core::{
        array::EncodedArray,
        checksum,
        constants::{CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
        page_payload::{COLUMN_PAGE_PAYLOAD_HEADER_LEN, COVE_ENCODING_NODE_LEN},
        table::{TableCatalog, TableEntry},
        wire,
        writer::{ScanPageSpec, ScanProfileCoveWriter, ScanSegment},
    };
    use std::sync::Arc;

    fn bool_test_column() -> ColumnEntry {
        ColumnEntry {
            column_id: 0,
            name: "flag".to_string(),
            logical: CoveLogicalType::Bool,
            physical: CovePhysicalKind::Boolean,
            nullable: false,
            sort_order: 0,
            collation_id: 0,
            precision: 0,
            scale: 0,
            flags: 0,
        }
    }

    fn bool_test_page(payload: &[u8], checksum: u32) -> ColumnPageIndexEntryV1 {
        ColumnPageIndexEntryV1 {
            column_id: 0,
            morsel_id: 0,
            row_count: 3,
            non_null_count: 3,
            null_count: 0,
            encoding_root: 0,
            page_offset: 0,
            page_length: payload.len() as u64,
            uncompressed_length: payload.len() as u64,
            stats_ref: 0,
            flags: CompressionCodec::None as u32,
            checksum,
        }
    }

    fn bool_test_payload() -> Vec<u8> {
        ColumnPagePayloadV1::build_single_node(
            3,
            CoveEncodingKind::PlainFixed,
            CoveLogicalType::Bool,
            CovePhysicalKind::Boolean,
            None,
            vec![0, 1, 0],
        )
        .unwrap()
    }

    fn column(
        column_id: u32,
        name: &str,
        logical: CoveLogicalType,
        physical: CovePhysicalKind,
        nullable: bool,
    ) -> ColumnEntry {
        ColumnEntry {
            column_id,
            name: name.into(),
            logical,
            physical,
            nullable,
            sort_order: 0,
            collation_id: 0,
            precision: 0,
            scale: 0,
            flags: 0,
        }
    }

    fn numcode_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
        ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::NumCode as u32)
    }

    fn varbytes_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
        ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::VarBytes as u32)
    }

    fn bool_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
        ScanPageSpec::new(row_count, payload)
            .with_encoding_root(CoveEncodingKind::PlainFixed as u32)
    }

    fn numcode_i64(values: &[i64]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| (*value as u64).to_le_bytes())
            .collect()
    }

    fn varbytes(values: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for value in values {
            out.extend_from_slice(&(value.len() as u32).to_le_bytes());
            out.extend_from_slice(value.as_bytes());
        }
        out
    }

    fn bools(values: &[bool]) -> Vec<u8> {
        values.iter().map(|value| u8::from(*value)).collect()
    }

    fn primitive_events_file() -> Vec<u8> {
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "events".into(),
                row_count: 3,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![
                    column(
                        1,
                        "id",
                        CoveLogicalType::Int64,
                        CovePhysicalKind::NumCode,
                        false,
                    ),
                    column(
                        2,
                        "name",
                        CoveLogicalType::Utf8,
                        CovePhysicalKind::VarBytes,
                        false,
                    ),
                    column(
                        3,
                        "active",
                        CoveLogicalType::Bool,
                        CovePhysicalKind::Boolean,
                        false,
                    ),
                ],
            }],
        };
        let mut first = ScanSegment::new(1, 0, 0, 2, 3);
        first.set_column_pages(1, vec![numcode_page(2, numcode_i64(&[1, 2]))]);
        first.set_column_pages(2, vec![varbytes_page(2, varbytes(&["alpha", "beta"]))]);
        first.set_column_pages(3, vec![bool_page(2, bools(&[true, false]))]);

        let mut second = ScanSegment::new(1, 1, 2, 1, 3);
        second.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[3]))]);
        second.set_column_pages(2, vec![varbytes_page(1, varbytes(&["gamma"]))]);
        second.set_column_pages(3, vec![bool_page(1, bools(&[true]))]);

        let mut writer = ScanProfileCoveWriter::new(catalog);
        writer.push_segment(first);
        writer.push_segment(second);
        writer.write().unwrap()
    }

    #[test]
    fn cove_table_options_default_to_trusted_page_payload_validation() {
        assert_eq!(
            CoveTableOptions::default().page_payload_validation_policy(),
            PagePayloadValidationPolicy::Trusted
        );
        assert_eq!(
            CoveTableOptions::default()
                .with_strict_page_payload_validation()
                .page_payload_validation_policy(),
            PagePayloadValidationPolicy::Strict
        );
    }

    #[test]
    fn cove_table_options_default_to_cached_proof_arrow_string_validation() {
        assert_eq!(
            CoveTableOptions::default()
                .arrow_export_options()
                .string_validation_policy,
            ArrowStringValidationPolicy::StrictOrCachedProof
        );
        assert_eq!(
            CoveTableOptions::default()
                .with_trusted_arrow_string_validation()
                .arrow_export_options()
                .string_validation_policy,
            ArrowStringValidationPolicy::TrustedPageProof
        );
        assert_eq!(
            CoveTableOptions::default()
                .with_trusted_arrow_string_validation()
                .with_strict_arrow_string_validation()
                .arrow_export_options()
                .string_validation_policy,
            ArrowStringValidationPolicy::Strict
        );
        assert_eq!(
            CoveTableOptions::default()
                .with_strict_arrow_string_validation()
                .with_cached_proof_arrow_string_validation()
                .arrow_export_options()
                .string_validation_policy,
            ArrowStringValidationPolicy::StrictOrCachedProof
        );
    }

    #[test]
    fn cove_table_options_default_to_mmap_local_file_reads() {
        assert_eq!(
            CoveTableOptions::default().local_file_read_policy(),
            LocalFileReadPolicy::Mmap
        );
        assert_eq!(
            CoveTableOptions::default()
                .with_positioned_local_file_reads()
                .local_file_read_policy(),
            LocalFileReadPolicy::PositionedReads
        );
        assert_eq!(
            CoveTableOptions::default()
                .with_local_file_mmap_reads()
                .local_file_read_policy(),
            LocalFileReadPolicy::Mmap
        );
        assert_eq!(
            CoveTableOptions::default()
                .with_local_file_mmap_reads()
                .with_positioned_local_file_reads()
                .local_file_read_policy(),
            LocalFileReadPolicy::PositionedReads
        );
    }

    #[test]
    fn trusted_arrow_string_validation_and_local_file_mmap_reads_sets_both_knobs() {
        let options = CoveTableOptions::default()
            .with_trusted_arrow_string_validation_and_local_file_mmap_reads();
        assert_eq!(
            options.arrow_export_options().string_validation_policy,
            ArrowStringValidationPolicy::TrustedPageProof
        );
        assert_eq!(options.local_file_read_policy(), LocalFileReadPolicy::Mmap);
        assert_eq!(
            options
                .clone()
                .with_strict_arrow_string_validation()
                .arrow_export_options()
                .string_validation_policy,
            ArrowStringValidationPolicy::Strict
        );
        assert_eq!(
            options
                .with_positioned_local_file_reads()
                .local_file_read_policy(),
            LocalFileReadPolicy::PositionedReads
        );
    }

    #[test]
    fn utf8_proof_cache_reuses_verified_pages_within_one_dataset_state() {
        let state = DatasetState::from_bytes("events", primitive_events_file()).unwrap();
        let plan = plan_scan(&state, None, Vec::new()).unwrap();

        let first = decode_scan(&state, &plan).unwrap();
        assert_eq!(first.stats.utf8_proof_hits, 0);
        assert_eq!(first.stats.utf8_proof_misses, 2);
        assert_eq!(first.stats.utf8_proofs_earned, 2);

        let second = decode_scan(&state, &plan).unwrap();
        assert_eq!(second.stats.utf8_proof_hits, 2);
        assert_eq!(second.stats.utf8_proof_misses, 0);
        assert_eq!(second.stats.utf8_proofs_earned, 0);

        let other_state = DatasetState::from_bytes("events-copy", primitive_events_file()).unwrap();
        let other_plan = plan_scan(&other_state, None, Vec::new()).unwrap();
        let third = decode_scan(&other_state, &other_plan).unwrap();
        assert_eq!(third.stats.utf8_proof_hits, 0);
        assert_eq!(third.stats.utf8_proof_misses, 2);
        assert_eq!(third.stats.utf8_proofs_earned, 2);
    }

    #[derive(Debug, Default)]
    struct StopAfterFirstBatchSink {
        batches: usize,
        rows: usize,
    }

    impl DecodeSink for StopAfterFirstBatchSink {
        fn emit_batch(
            &mut self,
            batch: RecordBatch,
            stats: &mut DecodeStats,
        ) -> Result<DecodeControl, CoveError> {
            let rows = batch.num_rows();
            stats.rows_materialized += rows;
            self.batches += 1;
            self.rows += rows;
            Ok(DecodeControl::Stop)
        }
    }

    #[test]
    fn decode_sink_stop_after_first_batch_stops_partition_cleanly() {
        let state = DatasetState::from_bytes("events", primitive_events_file()).unwrap();
        let plan = plan_scan(&state, None, Vec::new()).unwrap();
        let full = decode_scan(&state, &plan).unwrap();
        let mut sink = StopAfterFirstBatchSink::default();

        let stats = decode_scan_to_sink(&state, &plan, &mut sink).unwrap();

        assert_eq!(sink.batches, 1);
        assert_eq!(sink.rows, 2);
        assert_eq!(stats.rows_materialized, 2);
        assert_eq!(stats.morsels_considered, 1);
        assert!(stats.morsels_considered < full.stats.morsels_considered);
    }

    #[test]
    fn fetch_limited_sink_slices_final_batch_and_stops() {
        let state = DatasetState::from_bytes("events", primitive_events_file()).unwrap();
        let projection = vec![0];
        let plan = plan_scan(&state, Some(&projection), Vec::new()).unwrap();
        let inner = VecDecodeSink::default();
        let mut sink = FetchLimitedDecodeSink::new(inner, Some(1));

        let stats = decode_scan_to_sink(&state, &plan, &mut sink).unwrap();

        assert_eq!(stats.rows_materialized, 1);
        assert_eq!(stats.rows_selected, 2);
        assert_eq!(stats.morsels_considered, 1);
        assert_eq!(sink.inner.batches.len(), 1);
        assert_eq!(sink.inner.batches[0].num_rows(), 1);
    }

    #[test]
    fn exact_numeric_filter_records_late_materialization_for_projected_strings() {
        let state = DatasetState::from_bytes("events", primitive_events_file()).unwrap();
        let projection = vec![1];
        let plan = plan_scan(
            &state,
            Some(&projection),
            vec![FilterPlan::pruning_numeric(
                0,
                NumericPredicateOp::Gt,
                PredicateLiteral::UInt64(1),
                "id > 1",
            )],
        )
        .unwrap();

        let decoded = decode_scan(&state, &plan).unwrap();

        assert_eq!(decoded.stats.exact_predicates, 1);
        assert_eq!(decoded.stats.rows_selected, 2);
        assert_eq!(decoded.stats.rows_materialized, 2);
        assert_eq!(decoded.stats.late_materialization_morsels, 1);
        assert_eq!(decoded.stats.late_materialization_rows_skipped, 1);
        assert_eq!(decoded.stats.late_materialization_cells_skipped, 1);
        assert_eq!(decoded.stats.utf8_proof_misses, 2);
        assert_eq!(decoded.stats.utf8_proofs_earned, 2);
        assert_eq!(
            decoded
                .batches
                .iter()
                .map(|batch| batch.num_rows())
                .sum::<usize>(),
            2
        );
    }

    #[test]
    fn exact_numeric_filter_with_no_matches_does_not_materialize_projected_strings() {
        let state = DatasetState::from_bytes("events", primitive_events_file()).unwrap();
        let projection = vec![1];
        let plan = plan_scan(
            &state,
            Some(&projection),
            vec![FilterPlan::pruning_numeric(
                0,
                NumericPredicateOp::Gt,
                PredicateLiteral::UInt64(99),
                "id > 99",
            )],
        )
        .unwrap();

        let decoded = decode_scan(&state, &plan).unwrap();

        assert_eq!(decoded.stats.rows_selected, 0);
        assert_eq!(decoded.stats.rows_materialized, 0);
        assert_eq!(decoded.stats.utf8_proof_misses, 0);
        assert_eq!(decoded.stats.utf8_proofs_earned, 0);
        assert!(decoded.batches.is_empty());
    }

    #[test]
    fn exact_numeric_filter_still_works_when_predicate_column_is_projected() {
        let state = DatasetState::from_bytes("events", primitive_events_file()).unwrap();
        let projection = vec![0, 1];
        let plan = plan_scan(
            &state,
            Some(&projection),
            vec![FilterPlan::pruning_numeric(
                0,
                NumericPredicateOp::Gt,
                PredicateLiteral::UInt64(1),
                "id > 1",
            )],
        )
        .unwrap();

        let decoded = decode_scan(&state, &plan).unwrap();

        assert_eq!(decoded.stats.rows_selected, 2);
        assert_eq!(decoded.stats.rows_materialized, 2);
        assert_eq!(decoded.stats.late_materialization_rows_skipped, 1);
        assert_eq!(decoded.stats.late_materialization_cells_skipped, 2);
        assert!(decoded.batches.iter().all(|batch| batch.num_columns() == 2));
    }

    #[test]
    fn scan_execution_cache_reuses_local_reader_by_file_ordinal() {
        let cache = ScanExecutionCache::default();
        let first = cache
            .local_reader(
                4,
                LocalFileReadPolicy::PositionedReads,
                "/tmp/cove-cache-a.cove",
            )
            .unwrap();
        let second = cache
            .local_reader(
                4,
                LocalFileReadPolicy::PositionedReads,
                "/tmp/cove-cache-a.cove",
            )
            .unwrap();
        let mmap = cache
            .local_reader(4, LocalFileReadPolicy::Mmap, "/tmp/cove-cache-a.cove")
            .unwrap();
        let other = cache
            .local_reader(
                5,
                LocalFileReadPolicy::PositionedReads,
                "/tmp/cove-cache-a.cove",
            )
            .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &mmap));
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn trusted_page_payload_validation_skips_page_wire_crc_only() {
        let column = bool_test_column();
        let payload = bool_test_payload();
        let wrong_checksum = checksum::crc32c(&payload) ^ 1;
        let page = bool_test_page(&payload, wrong_checksum);

        assert!(materialize_page_payload_from_wire(
            &column,
            &page,
            Some(RetainedBytes::from_vec(payload.clone())),
            &[],
            None,
            PagePayloadValidationPolicy::Trusted,
        )
        .is_ok());
        assert!(matches!(
            materialize_page_payload_from_wire(
                &column,
                &page,
                Some(RetainedBytes::from_vec(payload)),
                &[],
                None,
                PagePayloadValidationPolicy::Strict,
            )
            .unwrap_err(),
            CoveError::ChecksumMismatch
        ));
    }

    #[test]
    fn trusted_page_payload_validation_skips_buffer_crc_only() {
        let column = bool_test_column();
        let mut payload = bool_test_payload();
        let checksum_offset = COLUMN_PAGE_PAYLOAD_HEADER_LEN + COVE_ENCODING_NODE_LEN + 24;
        payload[checksum_offset..checksum_offset + 4].copy_from_slice(&0u32.to_le_bytes());
        let page = bool_test_page(&payload, checksum::crc32c(&payload));

        assert!(materialize_page_payload_from_wire(
            &column,
            &page,
            Some(RetainedBytes::from_vec(payload.clone())),
            &[],
            None,
            PagePayloadValidationPolicy::Trusted,
        )
        .is_ok());
        assert!(matches!(
            materialize_page_payload_from_wire(
                &column,
                &page,
                Some(RetainedBytes::from_vec(payload)),
                &[],
                None,
                PagePayloadValidationPolicy::Strict,
            )
            .unwrap_err(),
            CoveError::ChecksumMismatch
        ));
    }

    #[test]
    fn selection_mask_and_row_extraction() {
        let mut selected = SelectionMask::default();
        selected.fill_all(10);
        selected.clear_bit(1);
        selected.clear_bit(8);

        let mut filter = SelectionMask::default();
        filter.fill_none(10);
        filter.set(0);
        filter.set(8);
        filter.set(9);

        selected.and_inplace(&filter);
        assert_eq!(selected.count_ones(), 2);

        let mut rows = Vec::new();
        selected.write_selected_rows(&mut rows).unwrap();
        assert_eq!(rows, vec![0, 9]);
    }

    #[test]
    fn numeric_predicate_filters_prepared_varint_rows() {
        let bytes = [
            wire::encode_u64_leb128(3),
            wire::encode_u64_leb128(17),
            wire::encode_u64_leb128(255),
        ]
        .concat();
        let array = EncodedArray::new(
            CoveLogicalType::UInt64,
            CovePhysicalKind::NumCode,
            3,
            CoveEncodingKind::PlainVarint,
            None,
            &bytes,
            None,
        );
        let prepared = array.prepare().unwrap();
        let predicate = CovePredicate::Numeric {
            column_index: 0,
            op: NumericPredicateOp::Eq,
            literal: PredicateLiteral::UInt64(17),
        };
        let mut selected = SelectionMask::default();
        selected.fill_all(3);
        let mut scratch = SelectionMask::default();
        assert!(
            apply_predicate_to_selection(&predicate, &prepared, &mut selected, &mut scratch)
                .unwrap()
        );

        let mut rows = Vec::new();
        selected.write_selected_rows(&mut rows).unwrap();
        assert_eq!(rows, vec![1]);
    }

    #[test]
    fn numeric_predicate_filters_numcode_rows_directly() {
        let bytes = [
            3u64.to_le_bytes(),
            17u64.to_le_bytes(),
            255u64.to_le_bytes(),
        ]
        .concat();
        let array = EncodedArray::new(
            CoveLogicalType::UInt64,
            CovePhysicalKind::NumCode,
            3,
            CoveEncodingKind::NumCode,
            None,
            &bytes,
            None,
        );
        let prepared = array.prepare().unwrap();
        let predicate = CovePredicate::Numeric {
            column_index: 0,
            op: NumericPredicateOp::Gt,
            literal: PredicateLiteral::UInt64(16),
        };
        let mut selected = SelectionMask::default();
        selected.fill_all(3);
        let mut scratch = SelectionMask::default();
        assert!(
            apply_predicate_to_selection(&predicate, &prepared, &mut selected, &mut scratch)
                .unwrap()
        );

        let mut rows = Vec::new();
        selected.write_selected_rows(&mut rows).unwrap();
        assert_eq!(rows, vec![1, 2]);
    }

    #[test]
    fn record_batch_for_selection_exports_bitset_without_row_materialization() {
        let mut values = Vec::new();
        values.extend_from_slice(&1u32.to_le_bytes());
        values.extend_from_slice(b"a");
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"bb");
        values.extend_from_slice(&3u32.to_le_bytes());
        values.extend_from_slice(b"ccc");
        let array = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            3,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );
        let mut mask = SelectionMask::default();
        mask.fill_none(3);
        mask.set(0);
        mask.set(2);
        let selection = Selection::Bitset(mask);
        let schema = Arc::new(arrow_schema::Schema::new(vec![arrow_schema::Field::new(
            "word",
            arrow_schema::DataType::Utf8,
            false,
        )]));
        let state = DatasetState::from_bytes("selection", primitive_events_file()).unwrap();
        let mut stats = DecodeStats::default();

        let result = record_batch_for_selection(
            &state,
            &[DecodedArrowColumn {
                name: "word",
                array: &array,
                payload: None,
                nested_schema: None,
                data_owner: None,
                utf8_proof_key: None,
                zero_copy: None,
            }],
            &selection,
            schema,
            ArrowExportOptions::default(),
            None,
            &mut stats,
        )
        .unwrap();
        let strings = result
            .value
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(strings.len(), 2);
        assert_eq!(strings.value(0), "a");
        assert_eq!(strings.value(1), "ccc");
    }

    #[test]
    fn compatible_zero_copy_map_records_direct_export_metric() {
        let values = Arc::new(varbytes(&["alpha", "beta"]));
        let array = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            values.as_slice(),
            None,
        );
        let schema = Arc::new(arrow_schema::Schema::new(vec![arrow_schema::Field::new(
            "word",
            arrow_schema::DataType::Utf8View,
            false,
        )]));
        let state = DatasetState::from_bytes("zero-copy", primitive_events_file()).unwrap();
        let selection = Selection::AllRows { len: 2 };
        let mut stats = DecodeStats::default();

        let result = record_batch_for_selection(
            &state,
            &[DecodedArrowColumn {
                name: "word",
                array: &array,
                payload: None,
                nested_schema: None,
                data_owner: Some(arrow_buffer_owner(Arc::clone(&values))),
                utf8_proof_key: None,
                zero_copy: Some(ZeroCopyCompatibilityV2::Compatible),
            }],
            &selection,
            schema,
            arrow_view_options(),
            None,
            &mut stats,
        )
        .unwrap();

        assert_eq!(result.value.num_rows(), 2);
        assert_eq!(
            result.value.column(0).data_type(),
            &arrow_schema::DataType::Utf8View
        );
        assert_eq!(stats.zero_copy_compatible_buffers, 1);
        assert_eq!(stats.zero_copy_materialized_buffers, 0);
    }

    #[test]
    fn compatible_zero_copy_map_with_selection_materializes_owned_view() {
        let values = Arc::new(varbytes(&["alpha", "hidden", "beta"]));
        let array = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            3,
            CoveEncodingKind::VarBytes,
            None,
            values.as_slice(),
            None,
        );
        let schema = Arc::new(arrow_schema::Schema::new(vec![arrow_schema::Field::new(
            "word",
            arrow_schema::DataType::Utf8View,
            false,
        )]));
        let state = DatasetState::from_bytes("zero-copy", primitive_events_file()).unwrap();
        let selection = Selection::RowIndices(vec![0, 2]);
        let mut stats = DecodeStats::default();

        let result = record_batch_for_selection(
            &state,
            &[DecodedArrowColumn {
                name: "word",
                array: &array,
                payload: None,
                nested_schema: None,
                data_owner: Some(arrow_buffer_owner(Arc::clone(&values))),
                utf8_proof_key: None,
                zero_copy: Some(ZeroCopyCompatibilityV2::Compatible),
            }],
            &selection,
            schema,
            arrow_view_options(),
            None,
            &mut stats,
        )
        .unwrap();

        let strings = result
            .value
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::StringViewArray>()
            .unwrap();
        assert_eq!(strings.value(0), "alpha");
        assert_eq!(strings.value(1), "beta");
        assert_eq!(stats.zero_copy_compatible_buffers, 0);
        assert_eq!(stats.zero_copy_materialized_buffers, 1);
        assert_eq!(stats.zero_copy_materialized_selection_mismatch, 1);
    }

    #[test]
    fn incompatible_zero_copy_maps_record_materialization_reasons() {
        for reason in [
            ZeroCopyMaterializationReasonV2::CompressedBuffer,
            ZeroCopyMaterializationReasonV2::NullPolarityMismatch,
            ZeroCopyMaterializationReasonV2::DictionaryMismatch,
            ZeroCopyMaterializationReasonV2::NestedLayoutMismatch,
            ZeroCopyMaterializationReasonV2::InsufficientLifetime,
            ZeroCopyMaterializationReasonV2::UnknownRole,
            ZeroCopyMaterializationReasonV2::ActiveVisibilityOverlay,
        ] {
            let stats = export_with_zero_copy_decision(Some(
                ZeroCopyCompatibilityV2::MaterializeRequired(reason),
            ));
            assert_eq!(stats.zero_copy_compatible_buffers, 0, "{reason:?}");
            assert_eq!(stats.zero_copy_materialized_buffers, 1, "{reason:?}");
            assert_eq!(zero_copy_reason_count(&stats, reason), 1, "{reason:?}");
        }
    }

    #[test]
    fn no_zero_copy_map_view_output_is_not_counted_as_attempted() {
        let stats = export_with_zero_copy_decision(None);
        assert_eq!(stats.zero_copy_compatible_buffers, 0);
        assert_eq!(stats.zero_copy_materialized_buffers, 0);
    }

    fn export_with_zero_copy_decision(decision: Option<ZeroCopyCompatibilityV2>) -> DecodeStats {
        let values = Arc::new(varbytes(&["alpha", "beta"]));
        let array = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            values.as_slice(),
            None,
        );
        let schema = Arc::new(arrow_schema::Schema::new(vec![arrow_schema::Field::new(
            "word",
            arrow_schema::DataType::Utf8View,
            false,
        )]));
        let state = DatasetState::from_bytes("zero-copy", primitive_events_file()).unwrap();
        let selection = Selection::AllRows { len: 2 };
        let mut stats = DecodeStats::default();
        let result = record_batch_for_selection(
            &state,
            &[DecodedArrowColumn {
                name: "word",
                array: &array,
                payload: None,
                nested_schema: None,
                data_owner: Some(arrow_buffer_owner(Arc::clone(&values))),
                utf8_proof_key: None,
                zero_copy: decision,
            }],
            &selection,
            schema,
            arrow_view_options(),
            None,
            &mut stats,
        )
        .unwrap();
        let strings = result
            .value
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::StringViewArray>()
            .unwrap();
        assert_eq!(strings.value(0), "alpha");
        assert_eq!(strings.value(1), "beta");
        stats
    }

    fn arrow_view_options() -> ArrowExportOptions {
        ArrowExportOptions {
            varbytes_policy: ArrowVarBytesExportPolicy::View,
            ..ArrowExportOptions::default()
        }
    }

    fn zero_copy_reason_count(
        stats: &DecodeStats,
        reason: ZeroCopyMaterializationReasonV2,
    ) -> usize {
        match reason {
            ZeroCopyMaterializationReasonV2::UnknownRole => {
                stats.zero_copy_materialized_unknown_role
            }
            ZeroCopyMaterializationReasonV2::NullPolarityMismatch => {
                stats.zero_copy_materialized_null_polarity_mismatch
            }
            ZeroCopyMaterializationReasonV2::CompressedBuffer => {
                stats.zero_copy_materialized_compressed_buffer
            }
            ZeroCopyMaterializationReasonV2::DictionaryMismatch => {
                stats.zero_copy_materialized_dictionary_mismatch
            }
            ZeroCopyMaterializationReasonV2::NestedLayoutMismatch => {
                stats.zero_copy_materialized_nested_layout_mismatch
            }
            ZeroCopyMaterializationReasonV2::InsufficientLifetime => {
                stats.zero_copy_materialized_insufficient_lifetime
            }
            ZeroCopyMaterializationReasonV2::ActiveVisibilityOverlay => {
                stats.zero_copy_materialized_active_visibility_overlay
            }
        }
    }
}
