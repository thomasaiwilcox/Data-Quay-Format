//! Decode and materialization kernels shared by execution modes.

use std::{
    borrow::Cow,
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use arrow_array::{RecordBatch, RecordBatchOptions};
use arrow_schema::SchemaRef;
use cove_arrow::arrow::{
    arrow_buffer_owner, encoded_columns_to_arrow_arrays_with_owners_options, ArrowEncodedColumn,
    ArrowExportOptions, ArrowRowSelection, ArrowVarBytesExportPolicy,
};
use cove_core::{
    array::{CoveArrayValue, EncodedArray, PreparedEncodedArray},
    compression,
    constants::{CoveEncodingKind, CovePhysicalKind},
    index::{lookup::LookupKeyKind, topn::TopNDirection},
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

use crate::{
    dataset_state::{DatasetBootstrapStats, DatasetState},
    options::{LocalFileReadPolicy, PagePayloadValidationPolicy},
    planner::{
        CoveFilterUse, CovePredicate, NullPredicateKind, NumericPredicateOp, PredicateLiteral,
        ScanPlan,
    },
    prune,
    range_reader::{
        coalesced_range_count, read_coalesced_range_buffers_with_options, CoveRangeReader,
        LocalFileRangeReader, MemoryRangeReader, MmapFileRangeReader, RangeReadKind, RangeReadMode,
        RangeReadPlan,
    },
    task_graph::ScanTask,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeStats {
    pub files_considered: usize,
    pub files_pruned: usize,
    pub files_validated: usize,
    pub overlay_files_hidden: usize,
    pub overlay_rows_hidden: usize,
    pub overlay_morsels_pruned: usize,
    pub covm_entries_stale: usize,
    pub manifest_fallbacks: usize,
    pub covx_sidecars_loaded: usize,
    pub covx_sidecars_stale: usize,
    pub covx_sidecars_ignored: usize,
    pub sidecar_index_fallbacks: usize,
    pub pages_decoded: usize,
    pub rows_materialized: usize,
    pub rows_selected: usize,
    pub morsels_pruned: usize,
    pub morsels_considered: usize,
    pub predicate_pages_checked: usize,
    pub residual_rows: usize,
    pub metadata_bytes_read: usize,
    pub data_bytes_read: usize,
    pub range_requests: usize,
    pub coalesced_range_requests: usize,
    pub scan_tasks: usize,
    pub scan_partitions: usize,
    pub dynamic_filter_snapshots: usize,
    pub dynamic_filter_pruned_tasks: usize,
    pub dynamic_filter_fallbacks: usize,
    pub lookup_index_hits: usize,
    pub lookup_index_misses: usize,
    pub inverted_index_hits: usize,
    pub index_rows_selected: usize,
    pub index_fallbacks: usize,
    pub execution_code_profiles_used: usize,
    pub execution_code_profile_fallbacks: usize,
    pub execution_code_literal_resolutions: usize,
    pub exact_predicates: usize,
    pub residual_predicates: usize,
    pub exactness_fallbacks: usize,
    pub lookup_rowref_tasks: usize,
    pub selection_all_rows: usize,
    pub selection_none: usize,
    pub selection_bitsets: usize,
    pub selection_row_indices: usize,
    pub range_plan_sparse: usize,
    pub range_plan_mixed: usize,
    pub range_plan_dense: usize,
    pub kernel_fallbacks: usize,
}

impl DecodeStats {
    fn record_bootstrap(&mut self, stats: DatasetBootstrapStats) {
        self.files_considered += stats.files_considered;
        self.files_pruned += stats.files_pruned;
        self.files_validated += stats.files_validated;
        self.overlay_files_hidden += stats.overlay_files_hidden;
        self.overlay_rows_hidden += stats.overlay_rows_hidden;
        self.covm_entries_stale += stats.covm_entries_stale;
        self.manifest_fallbacks += stats.manifest_fallbacks;
        self.covx_sidecars_loaded += stats.covx_sidecars_loaded;
        self.covx_sidecars_stale += stats.covx_sidecars_stale;
        self.covx_sidecars_ignored += stats.covx_sidecars_ignored;
        self.sidecar_index_fallbacks += stats.sidecar_index_fallbacks;
    }

    pub(crate) fn add_decode(&mut self, other: Self) {
        self.files_considered += other.files_considered;
        self.files_pruned += other.files_pruned;
        self.files_validated += other.files_validated;
        self.overlay_files_hidden += other.overlay_files_hidden;
        self.overlay_rows_hidden += other.overlay_rows_hidden;
        self.overlay_morsels_pruned += other.overlay_morsels_pruned;
        self.covm_entries_stale += other.covm_entries_stale;
        self.manifest_fallbacks += other.manifest_fallbacks;
        self.covx_sidecars_loaded += other.covx_sidecars_loaded;
        self.covx_sidecars_stale += other.covx_sidecars_stale;
        self.covx_sidecars_ignored += other.covx_sidecars_ignored;
        self.sidecar_index_fallbacks += other.sidecar_index_fallbacks;
        self.pages_decoded += other.pages_decoded;
        self.rows_materialized += other.rows_materialized;
        self.rows_selected += other.rows_selected;
        self.morsels_pruned += other.morsels_pruned;
        self.morsels_considered += other.morsels_considered;
        self.predicate_pages_checked += other.predicate_pages_checked;
        self.residual_rows += other.residual_rows;
        self.metadata_bytes_read += other.metadata_bytes_read;
        self.data_bytes_read += other.data_bytes_read;
        self.range_requests += other.range_requests;
        self.coalesced_range_requests += other.coalesced_range_requests;
        self.scan_tasks += other.scan_tasks;
        self.scan_partitions += other.scan_partitions;
        self.dynamic_filter_snapshots += other.dynamic_filter_snapshots;
        self.dynamic_filter_pruned_tasks += other.dynamic_filter_pruned_tasks;
        self.dynamic_filter_fallbacks += other.dynamic_filter_fallbacks;
        self.lookup_index_hits += other.lookup_index_hits;
        self.lookup_index_misses += other.lookup_index_misses;
        self.inverted_index_hits += other.inverted_index_hits;
        self.index_rows_selected += other.index_rows_selected;
        self.index_fallbacks += other.index_fallbacks;
        self.execution_code_profiles_used += other.execution_code_profiles_used;
        self.execution_code_profile_fallbacks += other.execution_code_profile_fallbacks;
        self.execution_code_literal_resolutions += other.execution_code_literal_resolutions;
        self.exact_predicates += other.exact_predicates;
        self.residual_predicates += other.residual_predicates;
        self.exactness_fallbacks += other.exactness_fallbacks;
        self.lookup_rowref_tasks += other.lookup_rowref_tasks;
        self.selection_all_rows += other.selection_all_rows;
        self.selection_none += other.selection_none;
        self.selection_bitsets += other.selection_bitsets;
        self.selection_row_indices += other.selection_row_indices;
        self.range_plan_sparse += other.range_plan_sparse;
        self.range_plan_mixed += other.range_plan_mixed;
        self.range_plan_dense += other.range_plan_dense;
        self.kernel_fallbacks += other.kernel_fallbacks;
    }
}

#[derive(Debug)]
pub struct DecodedScan {
    pub batches: Vec<RecordBatch>,
    pub stats: DecodeStats,
}

#[derive(Debug, Default)]
pub(crate) struct ScanExecutionCache {
    local_readers: Mutex<HashMap<LocalReaderCacheKey, Arc<dyn CoveRangeReader>>>,
    segment_metadata: Mutex<HashMap<SegmentMetadataCacheKey, Arc<SegmentMetadata>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LocalReaderCacheKey {
    file_ordinal: usize,
    policy: LocalFileReadPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SegmentMetadataCacheKey {
    file_ordinal: usize,
    table_id: u32,
    segment_id: u32,
    row_start: u64,
    offset: u64,
    length: u64,
}

impl SegmentMetadataCacheKey {
    fn new(file_ordinal: usize, segment_ref: &TableSegmentIndexEntryV1) -> Self {
        Self {
            file_ordinal,
            table_id: segment_ref.table_id,
            segment_id: segment_ref.segment_id,
            row_start: u64::from(segment_ref.row_start),
            offset: segment_ref.offset,
            length: segment_ref.length,
        }
    }
}

impl ScanExecutionCache {
    fn local_reader(
        &self,
        file_ordinal: usize,
        policy: LocalFileReadPolicy,
        path: impl AsRef<Path>,
    ) -> Result<Arc<dyn CoveRangeReader>, CoveError> {
        let path = path.as_ref().to_path_buf();
        let mut readers = self.local_readers.lock().map_err(|_| {
            CoveError::BadSection("scan execution local-reader cache lock poisoned".into())
        })?;
        let key = LocalReaderCacheKey {
            file_ordinal,
            policy,
        };
        Ok(Arc::clone(readers.entry(key).or_insert_with(
            || match policy {
                LocalFileReadPolicy::PositionedReads => Arc::new(LocalFileRangeReader::new(&path)),
                LocalFileReadPolicy::Mmap => Arc::new(MmapFileRangeReader::new(&path)),
            },
        )))
    }

    fn get_segment_metadata(
        &self,
        key: SegmentMetadataCacheKey,
    ) -> Result<Option<Arc<SegmentMetadata>>, CoveError> {
        let metadata = self.segment_metadata.lock().map_err(|_| {
            CoveError::BadSection("scan execution segment-metadata cache lock poisoned".into())
        })?;
        Ok(metadata.get(&key).cloned())
    }

    fn insert_segment_metadata(
        &self,
        key: SegmentMetadataCacheKey,
        segment: Arc<SegmentMetadata>,
    ) -> Result<Arc<SegmentMetadata>, CoveError> {
        let mut metadata = self.segment_metadata.lock().map_err(|_| {
            CoveError::BadSection("scan execution segment-metadata cache lock poisoned".into())
        })?;
        Ok(Arc::clone(metadata.entry(key).or_insert(segment)))
    }
}

#[derive(Debug, Default)]
struct DecodeScratch {
    selected_mask: SelectionMask,
    filter_mask: SelectionMask,
    selected_rows: Vec<u32>,
    selection: Selection,
}

#[derive(Debug, Default, Clone)]
struct SelectionMask {
    words: Vec<u64>,
    len: usize,
}

#[derive(Debug, Clone, Default)]
enum Selection {
    #[default]
    None,
    AllRows {
        len: usize,
    },
    Bitset(SelectionMask),
    RowIndices(Vec<u32>),
}

impl Selection {
    fn len(&self) -> usize {
        match self {
            Self::None => 0,
            Self::AllRows { len } => *len,
            Self::Bitset(mask) => mask.count_ones(),
            Self::RowIndices(rows) => rows.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn from_mask(mask: &SelectionMask, rows: &mut Vec<u32>) -> Result<Self, CoveError> {
        let selected = mask.count_ones();
        if selected == 0 {
            return Ok(Self::None);
        }
        if selected == mask.len {
            return Ok(Self::AllRows { len: mask.len });
        }
        if selected * 5 <= mask.len {
            mask.write_selected_rows(rows)?;
            return Ok(Self::RowIndices(rows.clone()));
        }
        Ok(Self::Bitset(mask.clone()))
    }

    fn from_rows(rows: &[u32], row_count: usize) -> Self {
        if rows.is_empty() {
            return Self::None;
        }
        if rows.len() == row_count
            && rows
                .iter()
                .enumerate()
                .all(|(index, row)| u32::try_from(index).ok() == Some(*row))
        {
            return Self::AllRows { len: row_count };
        }
        if rows.len() * 5 <= row_count {
            Self::RowIndices(rows.to_vec())
        } else {
            let mut mask = SelectionMask::default();
            mask.fill_none(row_count);
            for row in rows {
                let index = *row as usize;
                if index < row_count {
                    mask.set(index);
                }
            }
            Self::Bitset(mask)
        }
    }

    fn write_rows(&self, rows: &mut Vec<u32>) -> Result<(), CoveError> {
        rows.clear();
        match self {
            Self::None => Ok(()),
            Self::AllRows { len } => {
                rows.reserve(*len);
                for row in 0..*len {
                    rows.push(u32::try_from(row).map_err(|_| CoveError::ArithOverflow)?);
                }
                Ok(())
            }
            Self::Bitset(mask) => mask.write_selected_rows(rows),
            Self::RowIndices(values) => {
                rows.extend_from_slice(values);
                Ok(())
            }
        }
    }

    fn record(&self, stats: &mut DecodeStats) {
        match self {
            Self::None => stats.selection_none += 1,
            Self::AllRows { .. } => stats.selection_all_rows += 1,
            Self::Bitset(_) => stats.selection_bitsets += 1,
            Self::RowIndices(_) => stats.selection_row_indices += 1,
        }
    }
}

impl SelectionMask {
    fn fill_all(&mut self, len: usize) {
        self.len = len;
        let word_len = len.div_ceil(64);
        self.words.clear();
        self.words.resize(word_len, u64::MAX);
        self.mask_tail();
    }

    fn fill_none(&mut self, len: usize) {
        self.len = len;
        let word_len = len.div_ceil(64);
        self.words.clear();
        self.words.resize(word_len, 0);
    }

    fn set(&mut self, index: usize) {
        debug_assert!(index < self.len);
        self.words[index / 64] |= 1u64 << (index % 64);
    }

    fn clear_bit(&mut self, index: usize) {
        debug_assert!(index < self.len);
        self.words[index / 64] &= !(1u64 << (index % 64));
    }

    fn and_inplace(&mut self, other: &Self) {
        debug_assert_eq!(self.len, other.len);
        for (left, right) in self.words.iter_mut().zip(other.words.iter()) {
            *left &= *right;
        }
    }

    fn all_zero(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    fn count_ones(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    fn write_selected_rows(&self, rows: &mut Vec<u32>) -> Result<(), CoveError> {
        rows.clear();
        rows.reserve(self.count_ones());
        for (word_index, word) in self.words.iter().copied().enumerate() {
            let mut remaining = word;
            while remaining != 0 {
                let bit = remaining.trailing_zeros() as usize;
                let index = word_index
                    .checked_mul(64)
                    .and_then(|base| base.checked_add(bit))
                    .ok_or(CoveError::ArithOverflow)?;
                if index < self.len {
                    rows.push(u32::try_from(index).map_err(|_| CoveError::ArithOverflow)?);
                }
                remaining &= remaining - 1;
            }
        }
        Ok(())
    }

    fn mask_tail(&mut self) {
        let tail_bits = self.len % 64;
        if tail_bits == 0 {
            return;
        }
        if let Some(last) = self.words.last_mut() {
            *last &= (1u64 << tail_bits) - 1;
        }
    }
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
    let mut decoded = DecodedScan {
        batches: Vec::new(),
        stats: DecodeStats {
            scan_tasks: tasks.len(),
            scan_partitions: usize::from(partition_index == 0) * partition_count,
            ..DecodeStats::default()
        },
    };
    if partition_index == 0 {
        decoded.stats.record_bootstrap(state.bootstrap_stats());
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
        decoded.stats.execution_code_profiles_used += execution_stats.supported_files;
        decoded.stats.execution_code_profile_fallbacks += execution_stats.fallback_files;
        decoded.stats.execution_code_literal_resolutions += execution_stats.literal_resolutions;
        if plan_selects_no_rows(&file_plan) {
            decoded.stats.files_pruned += 1;
            task_start = task_end;
            continue;
        }
        let file_state = state.single_file_view(file_ordinal)?;
        let file_decoded = if let Some(bytes) = file_state
            .files()
            .first()
            .and_then(|file| file.full_file_bytes_arc())
        {
            let reader = MemoryRangeReader::from_arc(bytes);
            futures::executor::block_on(decode_scan_with_reader_tasks_cached(
                &file_state,
                &file_plan,
                &reader,
                file_tasks,
                Some(cache.as_ref()),
                file_ordinal,
            ))?
        } else if Path::new(file_state.source()).is_file() {
            let reader = cache.local_reader(
                file_ordinal,
                file_state.local_file_read_policy(),
                file_state.source(),
            )?;
            futures::executor::block_on(decode_scan_with_reader_tasks_cached(
                &file_state,
                &file_plan,
                reader.as_ref(),
                file_tasks,
                Some(cache.as_ref()),
                file_ordinal,
            ))?
        } else {
            decode_scan(&file_state, &file_plan)?
        };
        decoded.stats.add_decode(file_decoded.stats);
        decoded.batches.extend(file_decoded.batches);
        task_start = task_end;
    }
    Ok(decoded)
}

/// Decode a planned native single-file scan into Arrow record batches.
///
/// INVARIANT: this routine emits rows in segment order and morsel order, and it
/// delegates scalar COVE-to-Arrow representation rules to `cove-arrow`. FileCode
/// predicates are resolved against this concrete single-file view before
/// pruning or residual filtering begins.
pub fn decode_scan(state: &DatasetState, plan: &ScanPlan) -> Result<DecodedScan, CoveError> {
    let plan = state.resolved_plan_for_current_state(plan)?;
    validate_scan_plan(state, &plan)?;
    let mut batches = Vec::new();
    let mut stats = DecodeStats::default();
    record_plan_predicates(&plan, &mut stats);
    let mut scratch = DecodeScratch::default();

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
            &prepared_segment.morsels.entries,
            &plan,
        ) {
            stats.morsels_considered += 1;
            let row_start = u64::from(segment.header.row_start)
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
            if prune::morsel_pruned(state, segment.header.segment_id, morsel.morsel_id, &plan)?
                || should_prune_morsel(
                    state,
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

            if plan.scan_projection.is_empty() {
                let options = RecordBatchOptions::new().with_row_count(Some(selected_len));
                batches.push(
                    RecordBatch::try_new_with_options(
                        plan.output_schema.clone(),
                        Vec::new(),
                        &options,
                    )
                    .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))?,
                );
                stats.rows_materialized += selected_len;
                continue;
            }

            let mut page_payloads = Vec::with_capacity(plan.scan_projection.len());
            let mut page_indexes = Vec::with_capacity(plan.scan_projection.len());
            let mut columns = Vec::with_capacity(plan.scan_projection.len());
            for projection_index in &plan.scan_projection {
                let column = &state.table().columns[*projection_index];
                let segment_column = prepared_segment.column(column.column_id)?;
                let page = prepared_segment.page_for_morsel(segment_column, morsel.morsel_id)?;
                let payload = materialize_page_payload(
                    segment_bytes,
                    column,
                    &page,
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
            let column_refs =
                arrow_encoded_columns_for_payloads(&encoded_columns, &page_payloads, arrow_options);
            let batch = record_batch_for_selection(
                &column_refs,
                &scratch.selection,
                plan.output_schema.clone(),
                arrow_options,
            )?
            .value;
            stats.rows_materialized += batch.num_rows();
            batches.push(batch);
        }
    }

    Ok(DecodedScan { batches, stats })
}

fn plan_selects_no_rows(plan: &ScanPlan) -> bool {
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
}

fn record_range_plan(plan: RangeReadPlan, stats: &mut DecodeStats) {
    match plan.mode {
        RangeReadMode::Sparse => stats.range_plan_sparse += 1,
        RangeReadMode::Mixed => stats.range_plan_mixed += 1,
        RangeReadMode::Dense => stats.range_plan_dense += 1,
    }
}

fn apply_overlay_to_rows(
    state: &DatasetState,
    morsel_row_start: u64,
    selected_rows: &mut Vec<u32>,
    _stats: &mut DecodeStats,
) -> Result<(), CoveError> {
    let visibility = state.file(0)?.visibility();
    if visibility.is_all() || selected_rows.is_empty() {
        return Ok(());
    }
    let mut write = 0usize;
    for read in 0..selected_rows.len() {
        let row = selected_rows[read];
        let absolute = morsel_row_start
            .checked_add(u64::from(row))
            .ok_or(CoveError::ArithOverflow)?;
        if visibility.is_row_visible(absolute, state.table().row_count)? {
            selected_rows[write] = row;
            write += 1;
        }
    }
    selected_rows.truncate(write);
    Ok(())
}

fn apply_overlay_to_selection(
    state: &DatasetState,
    morsel_row_start: u64,
    row_count: u32,
    scratch: &mut DecodeScratch,
    stats: &mut DecodeStats,
) -> Result<(), CoveError> {
    let visibility = state.file(0)?.visibility();
    if visibility.is_all() || scratch.selection.is_empty() {
        return Ok(());
    }
    scratch.selection.write_rows(&mut scratch.selected_rows)?;
    apply_overlay_to_rows(state, morsel_row_start, &mut scratch.selected_rows, stats)?;
    scratch.selection = Selection::from_rows(&scratch.selected_rows, row_count as usize);
    Ok(())
}

fn arrow_encoded_columns_for_payloads<'name, 'array, 'data>(
    encoded_columns: &'array [(&'name str, EncodedArray<'data>)],
    page_payloads: &[RetainedColumnPagePayloadV1],
    options: ArrowExportOptions,
) -> Vec<ArrowEncodedColumn<'name, 'array, 'data>> {
    debug_assert_eq!(encoded_columns.len(), page_payloads.len());
    encoded_columns
        .iter()
        .zip(page_payloads.iter())
        .map(|((name, array), payload)| {
            let data_owner = if options.varbytes_policy == ArrowVarBytesExportPolicy::View
                && array.physical == CovePhysicalKind::VarBytes
            {
                Some(arrow_buffer_owner(payload.data.owner()))
            } else {
                None
            };
            ArrowEncodedColumn::with_data_owner(*name, array, data_owner)
        })
        .collect()
}

fn record_batch_for_selection(
    columns: &[ArrowEncodedColumn<'_, '_, '_>],
    selection: &Selection,
    schema: SchemaRef,
    options: ArrowExportOptions,
) -> Result<cove_arrow::arrow::ArrowExportResult<RecordBatch>, CoveError> {
    let arrow_selection = match selection {
        Selection::None => ArrowRowSelection::Rows(&[]),
        Selection::AllRows { .. } => ArrowRowSelection::All,
        Selection::RowIndices(rows) => ArrowRowSelection::Rows(rows),
        Selection::Bitset(mask) => ArrowRowSelection::Bitset {
            words: &mask.words,
            len: mask.len,
        },
    };
    let result =
        encoded_columns_to_arrow_arrays_with_owners_options(columns, arrow_selection, options)?;
    let batch = RecordBatch::try_new(schema, result.value)
        .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))?;
    Ok(cove_arrow::arrow::ArrowExportResult {
        value: batch,
        report: result.report,
    })
}

fn ordered_morsels<'a>(
    state: &DatasetState,
    segment_id: u32,
    entries: &'a [RowMorselEntryV1],
    plan: &ScanPlan,
) -> Vec<&'a RowMorselEntryV1> {
    let mut ordered = entries.iter().collect::<Vec<_>>();
    let Some(hint) = plan.topn_hint else {
        return ordered;
    };
    let Some(column) = state.table().columns.get(hint.column_index) else {
        return ordered;
    };
    let wanted_direction = if hint.descending {
        TopNDirection::Largest
    } else {
        TopNDirection::Smallest
    };
    ordered.sort_by_key(|morsel| {
        let rank = state
            .topn_for(column.column_id)
            .into_iter()
            .find(|summary| {
                summary.segment_id == segment_id
                    && summary.morsel_id == morsel.morsel_id
                    && summary.direction == wanted_direction
            })
            .and_then(topn_score)
            .map(|score| {
                if hint.descending {
                    u64::MAX.saturating_sub(score)
                } else {
                    score
                }
            })
            .unwrap_or(u64::MAX);
        (rank, morsel.morsel_id)
    });
    ordered
}

fn topn_score(summary: &cove_core::index::topn::TopNSummary) -> Option<u64> {
    summary
        .payload
        .chunks_exact(16)
        .next()
        .and_then(|chunk| chunk[0..8].try_into().ok().map(u64::from_le_bytes))
}

pub async fn decode_scan_with_reader<R: CoveRangeReader + ?Sized>(
    state: &DatasetState,
    plan: &ScanPlan,
    reader: &R,
) -> Result<DecodedScan, CoveError> {
    decode_scan_with_reader_cached(state, plan, reader, None, 0).await
}

async fn decode_scan_with_reader_cached<R: CoveRangeReader + ?Sized>(
    state: &DatasetState,
    plan: &ScanPlan,
    reader: &R,
    cache: Option<&ScanExecutionCache>,
    file_ordinal: usize,
) -> Result<DecodedScan, CoveError> {
    let plan = state.resolved_plan_for_current_state(plan)?;
    validate_scan_plan(state, &plan)?;
    let mut batches = Vec::new();
    let mut stats = DecodeStats::default();
    record_plan_predicates(&plan, &mut stats);
    let mut scratch = DecodeScratch::default();

    for segment_ref in state.segments() {
        let segment =
            read_segment_metadata(reader, state, segment_ref, &mut stats, cache, file_ordinal)
                .await?;

        for morsel in ordered_morsels(
            state,
            segment_ref.segment_id,
            &segment.morsels.entries,
            &plan,
        ) {
            stats.morsels_considered += 1;
            let row_start = u64::from(segment_ref.row_start)
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
            if prune::morsel_pruned(state, segment_ref.segment_id, morsel.morsel_id, &plan)?
                || should_prune_morsel_metadata(
                    state,
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
                batches.push(
                    RecordBatch::try_new_with_options(
                        plan.output_schema.clone(),
                        Vec::new(),
                        &options,
                    )
                    .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))?,
                );
                stats.rows_materialized += selected_len;
                continue;
            }

            let mut page_indexes = Vec::with_capacity(plan.scan_projection.len());
            let mut columns = Vec::with_capacity(plan.scan_projection.len());
            let mut ranges = Vec::new();
            let mut range_slots = Vec::with_capacity(plan.scan_projection.len());
            for projection_index in &plan.scan_projection {
                let column = &state.table().columns[*projection_index];
                let segment_column = segment.column(column.column_id)?;
                let page = segment.page_for_morsel(segment_column, morsel.morsel_id)?;
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
                    ranges.push(start..end);
                }
                stats.pages_decoded += usize::from(page.page_length != 0);
                page_indexes.push(page.clone());
                columns.push(column);
            }

            let coalesced = coalesced_range_count(&ranges, state.range_coalescing())?;
            record_range_plan(
                RangeReadPlan::choose(
                    selected_len,
                    morsel.row_count as usize,
                    ranges.len(),
                    coalesced,
                ),
                &mut stats,
            );
            stats.range_requests += coalesced;
            if coalesced < ranges.len() {
                stats.coalesced_range_requests += coalesced;
            }
            let wires = read_coalesced_range_buffers_with_options(
                reader,
                &ranges,
                RangeReadKind::Data,
                state.range_coalescing(),
            )
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
            let column_refs =
                arrow_encoded_columns_for_payloads(&encoded_columns, &page_payloads, arrow_options);
            let batch = record_batch_for_selection(
                &column_refs,
                &scratch.selection,
                plan.output_schema.clone(),
                arrow_options,
            )?
            .value;
            stats.rows_materialized += batch.num_rows();
            batches.push(batch);
        }
    }

    Ok(DecodedScan { batches, stats })
}

pub async fn decode_scan_with_reader_tasks<R: CoveRangeReader + ?Sized>(
    state: &DatasetState,
    plan: &ScanPlan,
    reader: &R,
    tasks: &[ScanTask],
) -> Result<DecodedScan, CoveError> {
    decode_scan_with_reader_tasks_cached(state, plan, reader, tasks, None, 0).await
}

async fn decode_scan_with_reader_tasks_cached<R: CoveRangeReader + ?Sized>(
    state: &DatasetState,
    plan: &ScanPlan,
    reader: &R,
    tasks: &[ScanTask],
    cache: Option<&ScanExecutionCache>,
    file_ordinal: usize,
) -> Result<DecodedScan, CoveError> {
    let plan = state.resolved_plan_for_current_state(plan)?;
    validate_scan_plan(state, &plan)?;
    let mut batches = Vec::new();
    let mut stats = DecodeStats::default();
    record_plan_predicates(&plan, &mut stats);
    let mut scratch = DecodeScratch::default();

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
            let row_start = u64::from(segment_ref.row_start)
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
            if prune::morsel_pruned(state, segment_ref.segment_id, morsel.morsel_id, &plan)?
                || should_prune_morsel_metadata(
                    state,
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

            if plan.scan_projection.is_empty() {
                let options = RecordBatchOptions::new().with_row_count(Some(selected_len));
                batches.push(
                    RecordBatch::try_new_with_options(
                        plan.output_schema.clone(),
                        Vec::new(),
                        &options,
                    )
                    .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))?,
                );
                stats.rows_materialized += selected_len;
                continue;
            }

            let mut page_indexes = Vec::with_capacity(plan.scan_projection.len());
            let mut columns = Vec::with_capacity(plan.scan_projection.len());
            let mut ranges = Vec::new();
            let mut range_slots = Vec::with_capacity(plan.scan_projection.len());
            for projection_index in &plan.scan_projection {
                let column = &state.table().columns[*projection_index];
                let segment_column = segment.column(column.column_id)?;
                let page = segment.page_for_morsel(segment_column, morsel.morsel_id)?;
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
                    ranges.push(start..end);
                }
                stats.pages_decoded += usize::from(page.page_length != 0);
                page_indexes.push(page.clone());
                columns.push(column);
            }

            let coalesced = coalesced_range_count(&ranges, state.range_coalescing())?;
            record_range_plan(
                RangeReadPlan::choose(
                    selected_len,
                    morsel.row_count as usize,
                    ranges.len(),
                    coalesced,
                ),
                &mut stats,
            );
            stats.range_requests += coalesced;
            if coalesced < ranges.len() {
                stats.coalesced_range_requests += coalesced;
            }
            let wires = read_coalesced_range_buffers_with_options(
                reader,
                &ranges,
                RangeReadKind::Data,
                state.range_coalescing(),
            )
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
            let column_refs =
                arrow_encoded_columns_for_payloads(&encoded_columns, &page_payloads, arrow_options);
            let batch = record_batch_for_selection(
                &column_refs,
                &scratch.selection,
                plan.output_schema.clone(),
                arrow_options,
            )?
            .value;
            stats.rows_materialized += batch.num_rows();
            batches.push(batch);
        }
        task_start = task_end;
    }

    Ok(DecodedScan { batches, stats })
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

#[derive(Debug)]
struct SegmentMetadata {
    morsels: RowMorselDirectory,
    morsel_positions_by_id: Vec<Option<usize>>,
    columns: Vec<PreparedSegmentColumn>,
    column_positions: Vec<(u32, usize)>,
}

#[derive(Debug)]
struct PreparedSegmentColumn {
    page_index: ColumnPageIndex,
    page_positions_by_morsel: Vec<Option<usize>>,
}

impl SegmentMetadata {
    fn new(
        morsels: RowMorselDirectory,
        columns: Vec<TableColumnDirectoryEntryV1>,
        page_indexes: Vec<ColumnPageIndex>,
    ) -> Result<Self, CoveError> {
        if columns.len() != page_indexes.len() {
            return Err(CoveError::SegmentCorrupt);
        }
        let max_morsel_id = morsels
            .entries
            .iter()
            .map(|entry| entry.morsel_id as usize)
            .max()
            .unwrap_or(0);
        let mut morsel_positions_by_id = vec![None; max_morsel_id.saturating_add(1)];
        for (position, morsel) in morsels.entries.iter().enumerate() {
            let slot = morsel.morsel_id as usize;
            if morsel_positions_by_id[slot].replace(position).is_some() {
                return Err(CoveError::SegmentCorrupt);
            }
        }

        let mut prepared_columns = Vec::with_capacity(columns.len());
        let mut column_positions = Vec::with_capacity(columns.len());
        for (position, (directory, page_index)) in columns
            .into_iter()
            .zip(page_indexes.into_iter())
            .enumerate()
        {
            let mut page_positions_by_morsel = vec![None; morsel_positions_by_id.len()];
            for (page_position, page) in page_index.entries.iter().enumerate() {
                let Some(&Some(morsel_position)) =
                    morsel_positions_by_id.get(page.morsel_id as usize)
                else {
                    return Err(CoveError::PageCorrupt);
                };
                if morsels.entries[morsel_position].row_count != page.row_count {
                    return Err(CoveError::PageCorrupt);
                }
                let slot = &mut page_positions_by_morsel[page.morsel_id as usize];
                if slot.replace(page_position).is_some() {
                    return Err(CoveError::PageCorrupt);
                }
            }
            column_positions.push((directory.column_id, position));
            prepared_columns.push(PreparedSegmentColumn {
                page_index,
                page_positions_by_morsel,
            });
        }
        column_positions.sort_unstable_by_key(|(column_id, _)| *column_id);
        for pair in column_positions.windows(2) {
            if pair[0].0 == pair[1].0 {
                return Err(CoveError::SegmentCorrupt);
            }
        }
        Ok(Self {
            morsels,
            morsel_positions_by_id,
            columns: prepared_columns,
            column_positions,
        })
    }

    fn morsel(&self, morsel_id: u32) -> Result<&RowMorselEntryV1, CoveError> {
        let Some(&Some(position)) = self.morsel_positions_by_id.get(morsel_id as usize) else {
            return Err(CoveError::SegmentCorrupt);
        };
        self.morsels
            .entries
            .get(position)
            .ok_or(CoveError::SegmentCorrupt)
    }

    fn column(&self, column_id: u32) -> Result<&PreparedSegmentColumn, CoveError> {
        let position = self
            .column_positions
            .binary_search_by_key(&column_id, |(candidate, _)| *candidate)
            .map_err(|_| CoveError::SegmentCorrupt)?;
        self.columns
            .get(self.column_positions[position].1)
            .ok_or(CoveError::SegmentCorrupt)
    }

    fn page_for_morsel<'a>(
        &'a self,
        column: &'a PreparedSegmentColumn,
        morsel_id: u32,
    ) -> Result<&'a ColumnPageIndexEntryV1, CoveError> {
        let Some(&Some(page_position)) = column.page_positions_by_morsel.get(morsel_id as usize)
        else {
            return Err(CoveError::PageCorrupt);
        };
        column
            .page_index
            .entries
            .get(page_position)
            .ok_or(CoveError::PageCorrupt)
    }
}

async fn read_segment_metadata<R: CoveRangeReader + ?Sized>(
    reader: &R,
    state: &DatasetState,
    segment_ref: &TableSegmentIndexEntryV1,
    stats: &mut DecodeStats,
    cache: Option<&ScanExecutionCache>,
    file_ordinal: usize,
) -> Result<Arc<SegmentMetadata>, CoveError> {
    let key = SegmentMetadataCacheKey::new(file_ordinal, segment_ref);
    if let Some(cache) = cache {
        if let Some(segment) = cache.get_segment_metadata(key)? {
            return Ok(segment);
        }
    }

    let header_end = segment_ref
        .offset
        .checked_add(TABLE_SEGMENT_HEADER_LEN as u64)
        .ok_or(CoveError::ArithOverflow)?;
    let header_bytes = reader
        .read_range(segment_ref.offset..header_end, RangeReadKind::Metadata)
        .await?;
    stats.metadata_bytes_read = stats
        .metadata_bytes_read
        .checked_add(header_bytes.len())
        .ok_or(CoveError::ArithOverflow)?;
    let header = TableSegmentHeaderV1::parse(&header_bytes)?;
    if header.table_id != segment_ref.table_id
        || header.segment_id != segment_ref.segment_id
        || header.row_start != segment_ref.row_start
        || header.row_count != segment_ref.row_count
        || header.column_count != segment_ref.column_count
    {
        return Err(CoveError::SegmentCorrupt);
    }
    if header.data_offset > segment_ref.length {
        return Err(CoveError::SegmentCorrupt);
    }
    let metadata_end = segment_ref
        .offset
        .checked_add(header.data_offset)
        .ok_or(CoveError::ArithOverflow)?;
    let metadata = reader
        .read_range(segment_ref.offset..metadata_end, RangeReadKind::Metadata)
        .await?;
    stats.metadata_bytes_read = stats
        .metadata_bytes_read
        .checked_add(metadata.len())
        .ok_or(CoveError::ArithOverflow)?;
    let segment = Arc::new(parse_segment_metadata(
        &metadata,
        segment_ref.length,
        state.mounted().header.required_features,
    )?);
    if let Some(cache) = cache {
        cache.insert_segment_metadata(key, segment)
    } else {
        Ok(segment)
    }
}

fn parse_segment_metadata(
    bytes: &[u8],
    segment_len: u64,
    required_features: u64,
) -> Result<SegmentMetadata, CoveError> {
    let header = TableSegmentHeaderV1::parse(bytes)?;
    if header.row_count == 0 && header.morsel_count != 0 {
        return Err(CoveError::SegmentCorrupt);
    }
    if header.row_count != 0 && header.morsel_row_count == 0 {
        return Err(CoveError::SegmentCorrupt);
    }
    let morsel_offset =
        usize::try_from(header.morsel_directory_offset).map_err(|_| CoveError::OffsetRange)?;
    if morsel_offset < TABLE_SEGMENT_HEADER_LEN || morsel_offset > bytes.len() {
        return Err(CoveError::SegmentCorrupt);
    }
    let morsel_dir_len = (header.morsel_count as usize)
        .checked_mul(cove_core::segment::ROW_MORSEL_ENTRY_LEN)
        .ok_or(CoveError::ArithOverflow)?;
    let morsel_end = morsel_offset
        .checked_add(morsel_dir_len)
        .ok_or(CoveError::ArithOverflow)?;
    if morsel_end > bytes.len() {
        return Err(CoveError::SegmentCorrupt);
    }
    let column_directory_offset =
        usize::try_from(header.column_directory_offset).map_err(|_| CoveError::OffsetRange)?;
    let page_index_offset =
        usize::try_from(header.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
    let data_offset = usize::try_from(header.data_offset).map_err(|_| CoveError::OffsetRange)?;
    if column_directory_offset < morsel_end
        || page_index_offset < column_directory_offset
        || data_offset < page_index_offset
        || data_offset > bytes.len()
    {
        return Err(CoveError::SegmentCorrupt);
    }
    let morsels =
        RowMorselDirectory::parse(&bytes[morsel_offset..morsel_end], header.morsel_count)?;
    if morsels.sum_rows() != header.row_count as u64 {
        return Err(CoveError::SegmentCorrupt);
    }
    let column_dir_len = (header.column_count as usize)
        .checked_mul(TABLE_COLUMN_DIRECTORY_ENTRY_LEN)
        .ok_or(CoveError::ArithOverflow)?;
    let column_dir_end = column_directory_offset
        .checked_add(column_dir_len)
        .ok_or(CoveError::ArithOverflow)?;
    if column_dir_end > page_index_offset {
        return Err(CoveError::SegmentCorrupt);
    }
    let mut columns = Vec::with_capacity(header.column_count as usize);
    let mut page_indexes = Vec::with_capacity(header.column_count as usize);
    let mut pos = column_directory_offset;
    for _ in 0..header.column_count {
        columns.push(TableColumnDirectoryEntryV1::parse(
            &bytes[pos..pos + TABLE_COLUMN_DIRECTORY_ENTRY_LEN],
        )?);
        pos += TABLE_COLUMN_DIRECTORY_ENTRY_LEN;
    }
    for column in &columns {
        let column_page_index_offset =
            usize::try_from(column.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
        let column_page_index_length =
            usize::try_from(column.page_index_length).map_err(|_| CoveError::OffsetRange)?;
        let column_page_index_end = column_page_index_offset
            .checked_add(column_page_index_length)
            .ok_or(CoveError::ArithOverflow)?;
        if column_page_index_offset < page_index_offset || column_page_index_end > data_offset {
            return Err(CoveError::SegmentCorrupt);
        }
        let column_data_end = column
            .data_offset
            .checked_add(column.data_length)
            .ok_or(CoveError::ArithOverflow)?;
        if column.data_offset < header.data_offset || column_data_end > segment_len {
            return Err(CoveError::PageCorrupt);
        }
        let page_index =
            ColumnPageIndex::parse(&bytes[column_page_index_offset..column_page_index_end])?;
        for page in &page_index.entries {
            if page.column_id != column.column_id {
                return Err(CoveError::PageCorrupt);
            }
            let morsel = morsels
                .entries
                .get(page.morsel_id as usize)
                .ok_or(CoveError::SegmentCorrupt)?;
            if page.row_count != morsel.row_count {
                return Err(CoveError::PageCorrupt);
            }
            if page_uses_payload_elision(page.flags)
                && required_features & cove_core::constants::FEATURE_PAGE_PAYLOAD_ELISION == 0
            {
                return Err(CoveError::BadSection(
                    "page payload-elision flags require FEATURE_PAGE_PAYLOAD_ELISION in required_features"
                        .into(),
                ));
            }
            if page.page_length != 0 {
                let page_end = page
                    .page_offset
                    .checked_add(page.page_length)
                    .ok_or(CoveError::ArithOverflow)?;
                if page.page_offset < column.data_offset || page_end > column_data_end {
                    return Err(CoveError::PageCorrupt);
                }
            }
        }
        page_indexes.push(page_index);
    }
    SegmentMetadata::new(morsels, columns, page_indexes)
}

fn prepare_segment_payload(
    segment_bytes: &[u8],
    segment: &TableSegmentPayloadV1,
) -> Result<SegmentMetadata, CoveError> {
    let mut page_indexes = Vec::with_capacity(segment.columns.len());
    for column in &segment.columns {
        page_indexes.push(column_page_index(segment_bytes, column)?);
    }
    SegmentMetadata::new(
        segment.morsels.clone(),
        segment.columns.clone(),
        page_indexes,
    )
}

fn selected_rows_for_morsel(
    state: &DatasetState,
    segment_bytes: &[u8],
    segment: &SegmentMetadata,
    segment_id: u32,
    morsel_id: u32,
    plan: &ScanPlan,
    stats: &mut DecodeStats,
    scratch: &mut DecodeScratch,
) -> Result<(), CoveError> {
    scratch.selected_rows.clear();
    scratch.selection = Selection::None;
    let morsel = segment.morsel(morsel_id)?;
    if !plan_has_row_predicate(plan) {
        scratch.selection = Selection::AllRows {
            len: morsel.row_count as usize,
        };
        return Ok(());
    }
    let skip_index_predicates = match lookup_selection_for_morsel(
        state,
        segment_id,
        morsel_id,
        morsel.row_count,
        plan,
        stats,
        scratch,
    )? {
        true => true,
        false => {
            scratch.selected_mask.fill_all(morsel.row_count as usize);
            false
        }
    };
    if scratch.selected_mask.all_zero() {
        scratch.selection = Selection::None;
        return Ok(());
    }
    for filter in &plan.filters {
        let Some(predicate) = &filter.predicate else {
            continue;
        };
        if matches!(predicate, CovePredicate::Null { .. }) {
            continue;
        }
        if skip_index_predicates && predicate_is_index_covered(state, predicate) {
            continue;
        }
        if matches!(
            predicate,
            CovePredicate::FileCodeIn { file_codes, .. } if file_codes.is_empty()
        ) {
            scratch.selection = Selection::None;
            return Ok(());
        }
        let Some(column_index) = predicate_column_index(predicate) else {
            continue;
        };
        let column = &state.table().columns[column_index];
        let segment_column = segment.column(column.column_id)?;
        let page = segment.page_for_morsel(segment_column, morsel_id)?;
        let payload = match materialize_page_payload(
            segment_bytes,
            column,
            &page,
            state.page_payload_validation_policy(),
        ) {
            Ok(payload) => payload,
            Err(CoveError::UnsupportedEncoding(_)) => {
                if filter.use_kind == CoveFilterUse::FullRowPredicateExact {
                    stats.exactness_fallbacks += 1;
                    return Err(CoveError::UnsupportedEncoding(format!(
                        "exact predicate {} cannot be evaluated for page encoding",
                        filter.display
                    )));
                }
                stats.kernel_fallbacks += 1;
                scratch.selection = Selection::AllRows {
                    len: morsel.row_count as usize,
                };
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        stats.pages_decoded += usize::from(page.page_length != 0);
        stats.data_bytes_read = stats
            .data_bytes_read
            .checked_add(usize::try_from(page.page_length).map_err(|_| CoveError::OffsetRange)?)
            .ok_or(CoveError::ArithOverflow)?;
        let dictionary = if matches!(predicate, CovePredicate::FileCodeIn { .. }) {
            None
        } else {
            state.mounted().dictionary.as_ref()
        };
        let array = encoded_array_for_page(&payload, &page, dictionary)?;
        let prepared = array.prepare()?;
        if !try_apply_predicate_to_selection(predicate, &prepared, &mut scratch.selected_mask)? {
            if filter.use_kind == CoveFilterUse::FullRowPredicateExact {
                stats.exactness_fallbacks += 1;
                return Err(CoveError::UnsupportedEncoding(format!(
                    "exact predicate {} cannot be evaluated by Cove",
                    filter.display
                )));
            }
            stats.kernel_fallbacks += 1;
            scratch.selection = Selection::AllRows {
                len: morsel.row_count as usize,
            };
            return Ok(());
        }
        if scratch.selected_mask.all_zero() {
            scratch.selection = Selection::None;
            return Ok(());
        }
    }
    scratch.selection = Selection::from_mask(&scratch.selected_mask, &mut scratch.selected_rows)?;
    Ok(())
}

async fn selected_rows_for_morsel_metadata<R: CoveRangeReader + ?Sized>(
    state: &DatasetState,
    segment: &SegmentMetadata,
    segment_ref: &TableSegmentIndexEntryV1,
    morsel_id: u32,
    plan: &ScanPlan,
    reader: &R,
    stats: &mut DecodeStats,
    scratch: &mut DecodeScratch,
) -> Result<(), CoveError> {
    scratch.selected_rows.clear();
    scratch.selection = Selection::None;
    let morsel = segment.morsel(morsel_id)?;
    if !plan_has_row_predicate(plan) {
        scratch.selection = Selection::AllRows {
            len: morsel.row_count as usize,
        };
        return Ok(());
    }
    let skip_index_predicates = match lookup_selection_for_morsel(
        state,
        segment_ref.segment_id,
        morsel_id,
        morsel.row_count,
        plan,
        stats,
        scratch,
    )? {
        true => true,
        false => {
            scratch.selected_mask.fill_all(morsel.row_count as usize);
            false
        }
    };
    if scratch.selected_mask.all_zero() {
        scratch.selection = Selection::None;
        return Ok(());
    }
    for filter in &plan.filters {
        let Some(predicate) = &filter.predicate else {
            continue;
        };
        if matches!(predicate, CovePredicate::Null { .. }) {
            continue;
        }
        if skip_index_predicates && predicate_is_index_covered(state, predicate) {
            continue;
        }
        if matches!(
            predicate,
            CovePredicate::FileCodeIn { file_codes, .. } if file_codes.is_empty()
        ) {
            scratch.selection = Selection::None;
            return Ok(());
        }
        let Some(column_index) = predicate_column_index(predicate) else {
            continue;
        };
        let column = &state.table().columns[column_index];
        let segment_column = segment.column(column.column_id)?;
        let page = segment.page_for_morsel(segment_column, morsel_id)?;
        let page_wire = if page.page_length == 0 {
            None
        } else {
            let start = segment_ref
                .offset
                .checked_add(page.page_offset)
                .ok_or(CoveError::ArithOverflow)?;
            let end = start
                .checked_add(page.page_length)
                .ok_or(CoveError::ArithOverflow)?;
            let bytes = reader
                .read_range_buffer(start..end, RangeReadKind::Data)
                .await?;
            stats.data_bytes_read = stats
                .data_bytes_read
                .checked_add(bytes.len())
                .ok_or(CoveError::ArithOverflow)?;
            Some(bytes)
        };
        stats.pages_decoded += usize::from(page.page_length != 0);
        let payload = match materialize_page_payload_from_wire(
            column,
            &page,
            page_wire,
            state.page_payload_validation_policy(),
        ) {
            Ok(payload) => payload,
            Err(CoveError::UnsupportedEncoding(_)) => {
                if filter.use_kind == CoveFilterUse::FullRowPredicateExact {
                    stats.exactness_fallbacks += 1;
                    return Err(CoveError::UnsupportedEncoding(format!(
                        "exact predicate {} cannot be evaluated for page encoding",
                        filter.display
                    )));
                }
                stats.kernel_fallbacks += 1;
                scratch.selection = Selection::AllRows {
                    len: morsel.row_count as usize,
                };
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let dictionary = if matches!(predicate, CovePredicate::FileCodeIn { .. }) {
            None
        } else {
            state.mounted().dictionary.as_ref()
        };
        let array = encoded_array_for_page(&payload, &page, dictionary)?;
        let prepared = array.prepare()?;
        if !try_apply_predicate_to_selection(predicate, &prepared, &mut scratch.selected_mask)? {
            if filter.use_kind == CoveFilterUse::FullRowPredicateExact {
                stats.exactness_fallbacks += 1;
                return Err(CoveError::UnsupportedEncoding(format!(
                    "exact predicate {} cannot be evaluated by Cove",
                    filter.display
                )));
            }
            stats.kernel_fallbacks += 1;
            scratch.selection = Selection::AllRows {
                len: morsel.row_count as usize,
            };
            return Ok(());
        }
        if scratch.selected_mask.all_zero() {
            scratch.selection = Selection::None;
            return Ok(());
        }
    }
    scratch.selection = Selection::from_mask(&scratch.selected_mask, &mut scratch.selected_rows)?;
    Ok(())
}

fn plan_has_row_predicate(plan: &ScanPlan) -> bool {
    plan.filters.iter().any(|filter| {
        matches!(
            filter.predicate,
            Some(CovePredicate::Numeric { .. } | CovePredicate::FileCodeIn { .. })
        )
    })
}

fn lookup_selection_for_morsel(
    state: &DatasetState,
    segment_id: u32,
    morsel_id: u32,
    row_count: u32,
    plan: &ScanPlan,
    stats: &mut DecodeStats,
    scratch: &mut DecodeScratch,
) -> Result<bool, CoveError> {
    let mut saw_lookup_filter = false;
    scratch.selected_mask.fill_all(row_count as usize);
    for filter in &plan.filters {
        let (column_index, key_kind, keys) = match &filter.predicate {
            Some(CovePredicate::FileCodeIn {
                column_index,
                file_codes,
                ..
            }) => (
                *column_index,
                LookupKeyKind::FileCode,
                file_codes
                    .iter()
                    .copied()
                    .map(u64::from)
                    .collect::<Vec<_>>(),
            ),
            Some(CovePredicate::Numeric {
                column_index,
                op: NumericPredicateOp::Eq,
                literal,
            }) => {
                let Some(key) = numeric_lookup_key(*literal) else {
                    continue;
                };
                (*column_index, LookupKeyKind::NumCode, vec![key])
            }
            _ => continue,
        };
        let column = &state.table().columns[column_index];
        let Some(index) = state.lookup_for(column.column_id) else {
            if saw_lookup_filter && key_kind == LookupKeyKind::FileCode {
                stats.index_fallbacks += 1;
                return Ok(false);
            }
            continue;
        };
        if index.header.key_kind != key_kind {
            stats.index_fallbacks += 1;
            return Ok(false);
        }
        saw_lookup_filter = true;
        scratch.filter_mask.fill_none(row_count as usize);
        for key in keys {
            match index.rows_for(key) {
                Some(rows) if !rows.is_empty() => {
                    stats.lookup_index_hits += 1;
                    for row in rows {
                        if row.table_id != state.table().table_id
                            || row.segment_id != segment_id
                            || row.morsel_id != morsel_id
                        {
                            continue;
                        }
                        let row_index = usize::try_from(row.row_in_morsel)
                            .map_err(|_| CoveError::ArithOverflow)?;
                        if row_index >= scratch.filter_mask.len {
                            stats.index_fallbacks += 1;
                            return Ok(false);
                        }
                        scratch.filter_mask.set(row_index);
                    }
                }
                _ => stats.lookup_index_misses += 1,
            }
        }
        scratch.selected_mask.and_inplace(&scratch.filter_mask);
        if scratch.selected_mask.all_zero() {
            break;
        }
    }
    if saw_lookup_filter {
        stats.index_rows_selected += scratch.selected_mask.count_ones();
        Ok(true)
    } else {
        Ok(false)
    }
}

fn predicate_is_index_covered(state: &DatasetState, predicate: &CovePredicate) -> bool {
    match predicate {
        CovePredicate::FileCodeIn { column_index, .. } => {
            let column = &state.table().columns[*column_index];
            state
                .lookup_for(column.column_id)
                .map(|index| index.header.key_kind == LookupKeyKind::FileCode)
                .unwrap_or(false)
        }
        CovePredicate::Numeric {
            column_index,
            op: NumericPredicateOp::Eq,
            literal,
        } if numeric_lookup_key(*literal).is_some() => {
            let column = &state.table().columns[*column_index];
            state
                .lookup_for(column.column_id)
                .map(|index| index.header.key_kind == LookupKeyKind::NumCode)
                .unwrap_or(false)
        }
        _ => false,
    }
}

pub(crate) fn numeric_lookup_key(literal: PredicateLiteral) -> Option<u64> {
    match literal {
        PredicateLiteral::Int64(value) => u64::try_from(value).ok(),
        PredicateLiteral::UInt64(value) => Some(value),
        PredicateLiteral::Float64(value) if value.is_finite() && value.fract() == 0.0 => {
            if value >= 0.0 && value <= u64::MAX as f64 {
                Some(value as u64)
            } else {
                None
            }
        }
        PredicateLiteral::Float64(_) => None,
    }
}

fn plan_has_residual(plan: &ScanPlan) -> bool {
    plan.filters
        .iter()
        .any(|filter| filter.use_kind == CoveFilterUse::PruningOnly)
}

fn predicate_column_index(predicate: &CovePredicate) -> Option<usize> {
    match predicate {
        CovePredicate::Null { column_index, .. }
        | CovePredicate::Numeric { column_index, .. }
        | CovePredicate::FileCodeIn { column_index, .. } => Some(*column_index),
    }
}

fn apply_predicate_to_selection(
    predicate: &CovePredicate,
    prepared: &PreparedEncodedArray<'_>,
    selected: &mut SelectionMask,
) -> Result<bool, CoveError> {
    if let Some(()) =
        try_apply_numcode_predicate_to_selection(predicate, prepared.array(), selected)?
    {
        return Ok(true);
    }
    for word_index in 0..selected.words.len() {
        let mut remaining = selected.words[word_index];
        while remaining != 0 {
            let bit = remaining.trailing_zeros() as usize;
            let index = word_index
                .checked_mul(64)
                .and_then(|base| base.checked_add(bit))
                .ok_or(CoveError::ArithOverflow)?;
            if index >= selected.len {
                break;
            }
            let row = u64::try_from(index).map_err(|_| CoveError::ArithOverflow)?;
            let array = prepared.array();
            let keep = match predicate {
                CovePredicate::Null { kind, .. } => {
                    let is_null = array.is_null(row)?;
                    match kind {
                        NullPredicateKind::IsNull => is_null,
                        NullPredicateKind::IsNotNull => !is_null,
                    }
                }
                CovePredicate::Numeric { op, literal, .. } => {
                    let value = prepared.decode_row(row)?;
                    match compare_numeric_value(&value, *op, *literal)? {
                        Some(value) => value,
                        None => return Ok(false),
                    }
                }
                CovePredicate::FileCodeIn { file_codes, .. } => {
                    if array.is_null(row)? {
                        false
                    } else {
                        let code = match raw_file_code_at(array, row)? {
                            Some(code) => Some(code),
                            None => match prepared.decode_row(row)? {
                                CoveArrayValue::FileCode(code) => Some(code),
                                _ => None,
                            },
                        };
                        match code {
                            Some(code) => file_codes.binary_search(&code).is_ok(),
                            None => return Ok(false),
                        }
                    }
                }
            };
            if !keep {
                selected.clear_bit(index);
            }
            remaining &= remaining - 1;
        }
    }
    Ok(true)
}

fn try_apply_numcode_predicate_to_selection(
    predicate: &CovePredicate,
    array: &EncodedArray<'_>,
    selected: &mut SelectionMask,
) -> Result<Option<()>, CoveError> {
    let CovePredicate::Numeric { op, literal, .. } = predicate else {
        return Ok(None);
    };
    if array.encoding != CoveEncodingKind::NumCode || array.physical != CovePhysicalKind::NumCode {
        return Ok(None);
    }

    let mut next = selected.clone();
    for word_index in 0..selected.words.len() {
        let mut remaining = selected.words[word_index];
        while remaining != 0 {
            let bit = remaining.trailing_zeros() as usize;
            let index = word_index
                .checked_mul(64)
                .and_then(|base| base.checked_add(bit))
                .ok_or(CoveError::ArithOverflow)?;
            if index >= selected.len {
                break;
            }
            let row = u64::try_from(index).map_err(|_| CoveError::ArithOverflow)?;
            let keep = if array.is_null(row)? {
                false
            } else {
                let code = raw_numcode_at(array, row)?;
                match compare_numcode_value(code, *op, *literal) {
                    Ok(value) => value,
                    Err(CoveError::UnsupportedEncoding(_)) => return Ok(None),
                    Err(error) => return Err(error),
                }
            };
            if !keep {
                next.clear_bit(index);
            }
            remaining &= remaining - 1;
        }
    }
    *selected = next;
    Ok(Some(()))
}

fn try_apply_predicate_to_selection(
    predicate: &CovePredicate,
    prepared: &PreparedEncodedArray<'_>,
    selected: &mut SelectionMask,
) -> Result<bool, CoveError> {
    match apply_predicate_to_selection(predicate, prepared, selected) {
        Ok(value) => Ok(value),
        Err(CoveError::UnsupportedEncoding(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

fn compare_numcode_value(
    value: u64,
    op: NumericPredicateOp,
    literal: PredicateLiteral,
) -> Result<bool, CoveError> {
    match literal {
        PredicateLiteral::Int64(literal) => {
            let value = i64::try_from(value)
                .map_err(|_| CoveError::UnsupportedEncoding("NumCode value exceeds i64".into()))?;
            Ok(compare_ordered(value, op, literal))
        }
        PredicateLiteral::UInt64(literal) => Ok(compare_ordered(value, op, literal)),
        PredicateLiteral::Float64(literal) => Ok(compare_ordered(value as f64, op, literal)),
    }
}

fn compare_numeric_value(
    value: &CoveArrayValue<'_>,
    op: NumericPredicateOp,
    literal: PredicateLiteral,
) -> Result<Option<bool>, CoveError> {
    if matches!(value, CoveArrayValue::Null) {
        return Ok(Some(false));
    }
    match literal {
        PredicateLiteral::Int64(literal) => {
            let Some(value) = value_as_i64(value)? else {
                return Ok(None);
            };
            Ok(Some(compare_ordered(value, op, literal)))
        }
        PredicateLiteral::UInt64(literal) => {
            let Some(value) = value_as_u64(value)? else {
                return Ok(None);
            };
            Ok(Some(compare_ordered(value, op, literal)))
        }
        PredicateLiteral::Float64(literal) => {
            let Some(value) = value_as_f64(value)? else {
                return Ok(None);
            };
            Ok(Some(compare_ordered(value, op, literal)))
        }
    }
}

fn compare_ordered<T: PartialOrd + PartialEq>(left: T, op: NumericPredicateOp, right: T) -> bool {
    match op {
        NumericPredicateOp::Eq => left == right,
        NumericPredicateOp::Lt => left < right,
        NumericPredicateOp::LtEq => left <= right,
        NumericPredicateOp::Gt => left > right,
        NumericPredicateOp::GtEq => left >= right,
    }
}

fn value_as_i64(value: &CoveArrayValue<'_>) -> Result<Option<i64>, CoveError> {
    match value {
        CoveArrayValue::NumCode(value) | CoveArrayValue::Varint(value) => i64::try_from(*value)
            .map(Some)
            .map_err(|_| CoveError::UnsupportedEncoding("NumCode value exceeds i64".into())),
        CoveArrayValue::Int64(value) => Ok(Some(*value)),
        CoveArrayValue::Bytes(bytes) if bytes.len() == 8 => {
            Ok(Some(i64::from_le_bytes((*bytes).try_into().unwrap())))
        }
        _ => Ok(None),
    }
}

fn value_as_u64(value: &CoveArrayValue<'_>) -> Result<Option<u64>, CoveError> {
    match value {
        CoveArrayValue::NumCode(value) | CoveArrayValue::Varint(value) => Ok(Some(*value)),
        CoveArrayValue::Int64(value) => u64::try_from(*value).map(Some).map_err(|_| {
            CoveError::UnsupportedEncoding("negative value cannot compare as u64".into())
        }),
        CoveArrayValue::Bytes(bytes) if bytes.len() == 8 => {
            Ok(Some(u64::from_le_bytes((*bytes).try_into().unwrap())))
        }
        _ => Ok(None),
    }
}

fn value_as_f64(value: &CoveArrayValue<'_>) -> Result<Option<f64>, CoveError> {
    match value {
        CoveArrayValue::NumCode(value) | CoveArrayValue::Varint(value) => Ok(Some(*value as f64)),
        CoveArrayValue::Int64(value) => Ok(Some(*value as f64)),
        CoveArrayValue::Bytes(bytes) if bytes.len() == 8 => {
            let value = f64::from_bits(u64::from_le_bytes((*bytes).try_into().unwrap()));
            if value.is_nan() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        }
        _ => Ok(None),
    }
}

fn raw_file_code_at(array: &EncodedArray<'_>, row: u64) -> Result<Option<u32>, CoveError> {
    if array.encoding != CoveEncodingKind::FileCode {
        return Ok(None);
    }
    let offset = usize::try_from(row)
        .map_err(|_| CoveError::ArithOverflow)?
        .checked_mul(4)
        .ok_or(CoveError::ArithOverflow)?;
    let bytes = wire::read_range_checked(array.data, offset, 4)?;
    Ok(Some(u32::from_le_bytes(bytes.try_into().unwrap())))
}

fn raw_numcode_at(array: &EncodedArray<'_>, row: u64) -> Result<u64, CoveError> {
    let offset = usize::try_from(row)
        .map_err(|_| CoveError::ArithOverflow)?
        .checked_mul(8)
        .ok_or(CoveError::ArithOverflow)?;
    let bytes = wire::read_range_checked(array.data, offset, 8)?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn should_prune_morsel(
    state: &DatasetState,
    segment: &SegmentMetadata,
    morsel_id: u32,
    plan: &ScanPlan,
    stats: &mut DecodeStats,
) -> Result<bool, CoveError> {
    for filter in &plan.filters {
        if filter.use_kind != CoveFilterUse::PruningOnly {
            continue;
        }
        let Some(CovePredicate::Null { column_index, kind }) = filter.predicate.as_ref() else {
            continue;
        };
        let column = &state.table().columns[*column_index];
        let segment_column = segment.column(column.column_id)?;
        let page = segment.page_for_morsel(segment_column, morsel_id)?;
        stats.predicate_pages_checked += 1;
        match *kind {
            NullPredicateKind::IsNull if page.null_count == 0 => return Ok(true),
            NullPredicateKind::IsNotNull if page.non_null_count == 0 => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

fn should_prune_morsel_metadata(
    state: &DatasetState,
    segment: &SegmentMetadata,
    morsel_id: u32,
    plan: &ScanPlan,
    stats: &mut DecodeStats,
) -> Result<bool, CoveError> {
    for filter in &plan.filters {
        if filter.use_kind != CoveFilterUse::PruningOnly {
            continue;
        }
        let Some(CovePredicate::Null { column_index, kind }) = filter.predicate.as_ref() else {
            continue;
        };
        let column = &state.table().columns[*column_index];
        let segment_column = segment.column(column.column_id)?;
        let page = segment.page_for_morsel(segment_column, morsel_id)?;
        stats.predicate_pages_checked += 1;
        match *kind {
            NullPredicateKind::IsNull if page.null_count == 0 => return Ok(true),
            NullPredicateKind::IsNotNull if page.non_null_count == 0 => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

fn column_page_index(
    segment_bytes: &[u8],
    column: &cove_core::segment::TableColumnDirectoryEntryV1,
) -> Result<ColumnPageIndex, CoveError> {
    let start = usize::try_from(column.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
    let len = usize::try_from(column.page_index_length).map_err(|_| CoveError::OffsetRange)?;
    let bytes = wire::read_range_checked(segment_bytes, start, len)?;
    ColumnPageIndex::parse(bytes)
}

fn materialize_page_payload(
    segment_bytes: &[u8],
    column: &ColumnEntry,
    page: &ColumnPageIndexEntryV1,
    validation_policy: PagePayloadValidationPolicy,
) -> Result<RetainedColumnPagePayloadV1, CoveError> {
    if page.flags & PAGE_FLAG_STATS_ONLY_CONSTANT != 0 {
        return materialize_stats_only_page(column, page);
    }

    let start = usize::try_from(page.page_offset).map_err(|_| CoveError::OffsetRange)?;
    let len = usize::try_from(page.page_length).map_err(|_| CoveError::OffsetRange)?;
    let page_wire = wire::read_range_checked(segment_bytes, start, len)?;
    let decoded = compression::column_page_payload_with_checksum_validation(
        page_wire,
        page,
        page_checksum_validation(validation_policy),
    )?;
    let decoded = match decoded {
        Cow::Borrowed(bytes) => bytes.to_vec(),
        Cow::Owned(bytes) => bytes,
    };
    RetainedColumnPagePayloadV1::parse_with_buffer_checksum_validation(
        RetainedBytes::from_vec(decoded),
        buffer_checksum_validation(validation_policy),
    )
}

fn materialize_page_payload_from_wire(
    column: &ColumnEntry,
    page: &ColumnPageIndexEntryV1,
    page_wire: Option<RetainedBytes>,
    validation_policy: PagePayloadValidationPolicy,
) -> Result<RetainedColumnPagePayloadV1, CoveError> {
    if page.flags & PAGE_FLAG_STATS_ONLY_CONSTANT != 0 {
        return materialize_stats_only_page(column, page);
    }
    let Some(page_wire) = page_wire else {
        return Err(CoveError::PageCorrupt);
    };
    let decoded = compression::column_page_payload_retained_with_checksum_validation(
        page_wire,
        page,
        page_checksum_validation(validation_policy),
    )?;
    RetainedColumnPagePayloadV1::parse_with_buffer_checksum_validation(
        decoded,
        buffer_checksum_validation(validation_policy),
    )
}

fn page_checksum_validation(
    validation_policy: PagePayloadValidationPolicy,
) -> compression::PageChecksumValidation {
    match validation_policy {
        PagePayloadValidationPolicy::Trusted => compression::PageChecksumValidation::Trusted,
        PagePayloadValidationPolicy::Strict => compression::PageChecksumValidation::Verify,
    }
}

fn buffer_checksum_validation(
    validation_policy: PagePayloadValidationPolicy,
) -> cove_core::page_payload::BufferChecksumValidation {
    match validation_policy {
        PagePayloadValidationPolicy::Trusted => {
            cove_core::page_payload::BufferChecksumValidation::Trusted
        }
        PagePayloadValidationPolicy::Strict => {
            cove_core::page_payload::BufferChecksumValidation::Verify
        }
    }
}

fn materialize_stats_only_page(
    column: &ColumnEntry,
    page: &ColumnPageIndexEntryV1,
) -> Result<RetainedColumnPagePayloadV1, CoveError> {
    if page.flags & PAGE_FLAG_ALL_NULL != 0 {
        let bitmap_len = (page.row_count as usize)
            .checked_add(7)
            .ok_or(CoveError::ArithOverflow)?
            / 8;
        let mut bitmap = vec![0xff; bitmap_len];
        if page.row_count % 8 != 0 && !bitmap.is_empty() {
            let valid_bits = page.row_count % 8;
            bitmap[bitmap_len - 1] = (1u8 << valid_bits) - 1;
        }
        let payload = ColumnPagePayloadV1::build_single_node(
            page.row_count,
            default_encoding_kind(column.physical),
            column.logical,
            column.physical,
            Some(bitmap),
            Vec::new(),
        )?;
        return RetainedColumnPagePayloadV1::parse(RetainedBytes::from_vec(payload));
    }
    if page.flags & PAGE_FLAG_ALL_NON_NULL != 0 {
        return Err(CoveError::UnsupportedEncoding(
            "native decoder cannot decode stats-only non-null constant pages without materialized values"
                .into(),
        ));
    }
    Err(CoveError::PageCorrupt)
}

fn encoded_array_for_page<'a>(
    payload: &'a RetainedColumnPagePayloadV1,
    page: &ColumnPageIndexEntryV1,
    dictionary: Option<&'a cove_core::dictionary::FileDictionary>,
) -> Result<EncodedArray<'a>, CoveError> {
    let root = payload
        .nodes
        .iter()
        .find(|node| node.node_id == payload.header.root_node_id)
        .ok_or(CoveError::PageCorrupt)?;
    let validity = buffer_slice(payload, PageBufferKind::NullBitmap)?
        .map(|bytes| ValidityBitmap::new(bytes, page.row_count as u64));
    let values = buffer_slice(payload, PageBufferKind::Values)?.unwrap_or(&[]);
    Ok(EncodedArray::new(
        root.logical_type,
        root.physical_kind,
        page.row_count as u64,
        root.encoding_kind,
        validity,
        values,
        dictionary,
    ))
}

fn buffer_slice(
    payload: &RetainedColumnPagePayloadV1,
    kind: PageBufferKind,
) -> Result<Option<&[u8]>, CoveError> {
    let mut matches = payload.buffers.iter().filter(|buffer| buffer.kind == kind);
    let Some(buffer) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(CoveError::PageCorrupt);
    }
    let start = usize::try_from(buffer.offset).map_err(|_| CoveError::OffsetRange)?;
    let len = usize::try_from(buffer.length).map_err(|_| CoveError::OffsetRange)?;
    wire::read_range_checked(payload.data.as_slice(), start, len).map(Some)
}

fn default_encoding_kind(physical: CovePhysicalKind) -> CoveEncodingKind {
    match physical {
        CovePhysicalKind::FileCode => CoveEncodingKind::FileCode,
        CovePhysicalKind::NumCode => CoveEncodingKind::NumCode,
        CovePhysicalKind::Boolean | CovePhysicalKind::FixedBytes => CoveEncodingKind::PlainFixed,
        CovePhysicalKind::VarBytes => CoveEncodingKind::VarBytes,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            CoveEncodingKind::Canonical
        }
        _ => CoveEncodingKind::Canonical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::CoveTableOptions;
    use arrow_array::Array;
    use cove_arrow::arrow::ArrowStringValidationPolicy;
    use cove_core::{
        array::EncodedArray,
        checksum,
        constants::{CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
        page_payload::{COLUMN_PAGE_PAYLOAD_HEADER_LEN, COVE_ENCODING_NODE_LEN},
        wire,
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
    fn cove_table_options_default_to_strict_arrow_string_validation() {
        assert_eq!(
            CoveTableOptions::default()
                .arrow_export_options()
                .string_validation_policy,
            ArrowStringValidationPolicy::Strict
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
    }

    #[test]
    fn cove_table_options_default_to_positioned_local_file_reads() {
        assert_eq!(
            CoveTableOptions::default().local_file_read_policy(),
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
            PagePayloadValidationPolicy::Trusted,
        )
        .is_ok());
        assert!(matches!(
            materialize_page_payload_from_wire(
                &column,
                &page,
                Some(RetainedBytes::from_vec(payload)),
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
            PagePayloadValidationPolicy::Trusted,
        )
        .is_ok());
        assert!(matches!(
            materialize_page_payload_from_wire(
                &column,
                &page,
                Some(RetainedBytes::from_vec(payload)),
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
        assert!(apply_predicate_to_selection(&predicate, &prepared, &mut selected).unwrap());

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
        assert!(apply_predicate_to_selection(&predicate, &prepared, &mut selected).unwrap());

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

        let result = record_batch_for_selection(
            &[ArrowEncodedColumn::borrowed("word", &array)],
            &selection,
            schema,
            ArrowExportOptions::default(),
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
}
