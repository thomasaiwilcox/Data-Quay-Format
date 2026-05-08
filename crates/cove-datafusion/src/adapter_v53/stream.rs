//! DataFusion 53.x stream glue.

use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use datafusion::{
    common::{DataFusionError, Result},
    execution::RecordBatchStream,
    physical_plan::metrics::{Count, MetricBuilder},
};
use futures::Stream;
use tokio::sync::mpsc;

use crate::{
    adapter_v53::cove_to_datafusion,
    dataset_state::DatasetState,
    decode::{
        decode_local_dataset_scan_tasks_with_cache, decode_local_dataset_scan_tasks_with_sink,
        DecodeControl, DecodeSink, DecodeStats, FetchLimitedDecodeSink, ScanExecutionCache,
    },
    planner::ScanPlan,
    task_graph::ScanTask,
};

#[cfg(feature = "dynamic-filters")]
use crate::adapter_v53::dynamic_filter::snapshot_dynamic_filters;

#[cfg(feature = "dynamic-filters")]
use datafusion::physical_expr::PhysicalExpr;

const DECODE_BATCH_CHANNEL_CAPACITY: usize = 2;

#[derive(Debug)]
pub(crate) enum DecodeStreamEvent {
    Batch(RecordBatch),
    Finished,
    Failed(cove_core::CoveError),
}

#[derive(Debug)]
pub(crate) struct ChannelDecodeSink {
    sender: mpsc::Sender<DecodeStreamEvent>,
    stopped: bool,
}

impl ChannelDecodeSink {
    pub(crate) fn new(sender: mpsc::Sender<DecodeStreamEvent>) -> Self {
        Self {
            sender,
            stopped: false,
        }
    }
}

impl DecodeSink for ChannelDecodeSink {
    fn emit_batch(
        &mut self,
        batch: RecordBatch,
        stats: &mut DecodeStats,
    ) -> std::result::Result<DecodeControl, cove_core::CoveError> {
        let rows = batch.num_rows();
        if self
            .sender
            .blocking_send(DecodeStreamEvent::Batch(batch))
            .is_err()
        {
            self.stopped = true;
            return Ok(DecodeControl::Stop);
        }
        stats.rows_materialized += rows;
        Ok(DecodeControl::Continue)
    }

    fn should_stop(&self) -> bool {
        self.stopped
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CoveStreamMetrics {
    output_batches: Count,
    output_rows: Count,
    materialization_streaming_partitions: Count,
    materialization_buffered_partitions: Count,
    files_considered: Count,
    files_pruned: Count,
    files_validated: Count,
    overlay_files_hidden: Count,
    overlay_rows_hidden: Count,
    overlay_morsels_pruned: Count,
    covm_entries_stale: Count,
    manifest_fallbacks: Count,
    covx_sidecars_loaded: Count,
    covx_sidecars_stale: Count,
    covx_sidecars_ignored: Count,
    sidecar_index_fallbacks: Count,
    pages_decoded: Count,
    rows_materialized: Count,
    rows_selected: Count,
    morsels_pruned: Count,
    morsels_considered: Count,
    residual_rows: Count,
    metadata_bytes_read: Count,
    data_bytes_read: Count,
    range_requests: Count,
    coalesced_range_requests: Count,
    scan_tasks: Count,
    scan_partitions: Count,
    dynamic_filter_snapshots: Count,
    dynamic_filter_pruned_tasks: Count,
    dynamic_filter_fallbacks: Count,
    predicate_pages_checked: Count,
    lookup_index_hits: Count,
    lookup_index_misses: Count,
    inverted_index_hits: Count,
    index_rows_selected: Count,
    index_fallbacks: Count,
    execution_code_profiles_used: Count,
    execution_code_profile_fallbacks: Count,
    execution_code_literal_resolutions: Count,
    exact_predicates: Count,
    residual_predicates: Count,
    exactness_fallbacks: Count,
    lookup_rowref_tasks: Count,
    selection_all_rows: Count,
    selection_none: Count,
    selection_bitsets: Count,
    selection_row_indices: Count,
    range_plan_sparse: Count,
    range_plan_mixed: Count,
    range_plan_dense: Count,
    kernel_fallbacks: Count,
    arrow_export_direct_varbytes_rows: Count,
    arrow_export_direct_varbytes_bytes: Count,
    arrow_export_direct_numcode_rows: Count,
    arrow_export_direct_plainfixed_rows: Count,
    arrow_export_direct_filecode_dictionary_rows: Count,
    arrow_export_direct_transform_rows: Count,
    arrow_export_direct_constant_plainvarint_rows: Count,
    arrow_export_fallback_rows: Count,
    filecode_dictionary_keys_rows: Count,
    filecode_dictionary_remapped_rows: Count,
    filecode_dictionary_values_bytes: Count,
    filecode_dictionary_value_cache_hits: Count,
    filecode_dictionary_value_cache_misses: Count,
    filecode_dictionary_decoded_fallback_rows: Count,
}

impl CoveStreamMetrics {
    pub(crate) fn new(
        metrics: &datafusion::physical_plan::metrics::ExecutionPlanMetricsSet,
        partition: usize,
    ) -> Self {
        Self {
            output_batches: MetricBuilder::new(metrics).output_batches(partition),
            output_rows: MetricBuilder::new(metrics).output_rows(partition),
            materialization_streaming_partitions: MetricBuilder::new(metrics)
                .counter("cove_materialization_streaming_partitions", partition),
            materialization_buffered_partitions: MetricBuilder::new(metrics)
                .counter("cove_materialization_buffered_partitions", partition),
            files_considered: MetricBuilder::new(metrics)
                .counter("cove_files_considered", partition),
            files_pruned: MetricBuilder::new(metrics).counter("cove_files_pruned", partition),
            files_validated: MetricBuilder::new(metrics).counter("cove_files_validated", partition),
            overlay_files_hidden: MetricBuilder::new(metrics)
                .counter("cove_overlay_files_hidden", partition),
            overlay_rows_hidden: MetricBuilder::new(metrics)
                .counter("cove_overlay_rows_hidden", partition),
            overlay_morsels_pruned: MetricBuilder::new(metrics)
                .counter("cove_overlay_morsels_pruned", partition),
            covm_entries_stale: MetricBuilder::new(metrics)
                .counter("cove_covm_entries_stale", partition),
            manifest_fallbacks: MetricBuilder::new(metrics)
                .counter("cove_manifest_fallbacks", partition),
            covx_sidecars_loaded: MetricBuilder::new(metrics)
                .counter("cove_covx_sidecars_loaded", partition),
            covx_sidecars_stale: MetricBuilder::new(metrics)
                .counter("cove_covx_sidecars_stale", partition),
            covx_sidecars_ignored: MetricBuilder::new(metrics)
                .counter("cove_covx_sidecars_ignored", partition),
            sidecar_index_fallbacks: MetricBuilder::new(metrics)
                .counter("cove_sidecar_index_fallbacks", partition),
            pages_decoded: MetricBuilder::new(metrics).counter("cove_pages_decoded", partition),
            rows_materialized: MetricBuilder::new(metrics)
                .counter("cove_rows_materialized", partition),
            rows_selected: MetricBuilder::new(metrics).counter("cove_rows_selected", partition),
            morsels_pruned: MetricBuilder::new(metrics).counter("cove_morsels_pruned", partition),
            morsels_considered: MetricBuilder::new(metrics)
                .counter("cove_morsels_considered", partition),
            residual_rows: MetricBuilder::new(metrics).counter("cove_residual_rows", partition),
            metadata_bytes_read: MetricBuilder::new(metrics)
                .counter("cove_metadata_bytes_read", partition),
            data_bytes_read: MetricBuilder::new(metrics).counter("cove_data_bytes_read", partition),
            range_requests: MetricBuilder::new(metrics).counter("cove_range_requests", partition),
            coalesced_range_requests: MetricBuilder::new(metrics)
                .counter("cove_coalesced_range_requests", partition),
            scan_tasks: MetricBuilder::new(metrics).counter("cove_scan_tasks", partition),
            scan_partitions: MetricBuilder::new(metrics).counter("cove_scan_partitions", partition),
            dynamic_filter_snapshots: MetricBuilder::new(metrics)
                .counter("cove_dynamic_filter_snapshots", partition),
            dynamic_filter_pruned_tasks: MetricBuilder::new(metrics)
                .counter("cove_dynamic_filter_pruned_tasks", partition),
            dynamic_filter_fallbacks: MetricBuilder::new(metrics)
                .counter("cove_dynamic_filter_fallbacks", partition),
            predicate_pages_checked: MetricBuilder::new(metrics)
                .counter("cove_predicate_pages_checked", partition),
            lookup_index_hits: MetricBuilder::new(metrics)
                .counter("cove_lookup_index_hits", partition),
            lookup_index_misses: MetricBuilder::new(metrics)
                .counter("cove_lookup_index_misses", partition),
            inverted_index_hits: MetricBuilder::new(metrics)
                .counter("cove_inverted_index_hits", partition),
            index_rows_selected: MetricBuilder::new(metrics)
                .counter("cove_index_rows_selected", partition),
            index_fallbacks: MetricBuilder::new(metrics).counter("cove_index_fallbacks", partition),
            execution_code_profiles_used: MetricBuilder::new(metrics)
                .counter("cove_execution_code_profiles_used", partition),
            execution_code_profile_fallbacks: MetricBuilder::new(metrics)
                .counter("cove_execution_code_profile_fallbacks", partition),
            execution_code_literal_resolutions: MetricBuilder::new(metrics)
                .counter("cove_execution_code_literal_resolutions", partition),
            exact_predicates: MetricBuilder::new(metrics)
                .counter("cove_exact_predicates", partition),
            residual_predicates: MetricBuilder::new(metrics)
                .counter("cove_residual_predicates", partition),
            exactness_fallbacks: MetricBuilder::new(metrics)
                .counter("cove_exactness_fallbacks", partition),
            lookup_rowref_tasks: MetricBuilder::new(metrics)
                .counter("cove_lookup_rowref_tasks", partition),
            selection_all_rows: MetricBuilder::new(metrics)
                .counter("cove_selection_all_rows", partition),
            selection_none: MetricBuilder::new(metrics).counter("cove_selection_none", partition),
            selection_bitsets: MetricBuilder::new(metrics)
                .counter("cove_selection_bitsets", partition),
            selection_row_indices: MetricBuilder::new(metrics)
                .counter("cove_selection_row_indices", partition),
            range_plan_sparse: MetricBuilder::new(metrics)
                .counter("cove_range_plan_sparse", partition),
            range_plan_mixed: MetricBuilder::new(metrics)
                .counter("cove_range_plan_mixed", partition),
            range_plan_dense: MetricBuilder::new(metrics)
                .counter("cove_range_plan_dense", partition),
            kernel_fallbacks: MetricBuilder::new(metrics)
                .counter("cove_kernel_fallbacks", partition),
            arrow_export_direct_varbytes_rows: MetricBuilder::new(metrics)
                .counter("cove_arrow_export_direct_varbytes_rows", partition),
            arrow_export_direct_varbytes_bytes: MetricBuilder::new(metrics)
                .counter("cove_arrow_export_direct_varbytes_bytes", partition),
            arrow_export_direct_numcode_rows: MetricBuilder::new(metrics)
                .counter("cove_arrow_export_direct_numcode_rows", partition),
            arrow_export_direct_plainfixed_rows: MetricBuilder::new(metrics)
                .counter("cove_arrow_export_direct_plainfixed_rows", partition),
            arrow_export_direct_filecode_dictionary_rows: MetricBuilder::new(metrics).counter(
                "cove_arrow_export_direct_filecode_dictionary_rows",
                partition,
            ),
            arrow_export_direct_transform_rows: MetricBuilder::new(metrics)
                .counter("cove_arrow_export_direct_transform_rows", partition),
            arrow_export_direct_constant_plainvarint_rows: MetricBuilder::new(metrics).counter(
                "cove_arrow_export_direct_constant_plainvarint_rows",
                partition,
            ),
            arrow_export_fallback_rows: MetricBuilder::new(metrics)
                .counter("cove_arrow_export_fallback_rows", partition),
            filecode_dictionary_keys_rows: MetricBuilder::new(metrics)
                .counter("cove_filecode_dictionary_keys_rows", partition),
            filecode_dictionary_remapped_rows: MetricBuilder::new(metrics)
                .counter("cove_filecode_dictionary_remapped_rows", partition),
            filecode_dictionary_values_bytes: MetricBuilder::new(metrics)
                .counter("cove_filecode_dictionary_values_bytes", partition),
            filecode_dictionary_value_cache_hits: MetricBuilder::new(metrics)
                .counter("cove_filecode_dictionary_value_cache_hits", partition),
            filecode_dictionary_value_cache_misses: MetricBuilder::new(metrics)
                .counter("cove_filecode_dictionary_value_cache_misses", partition),
            filecode_dictionary_decoded_fallback_rows: MetricBuilder::new(metrics)
                .counter("cove_filecode_dictionary_decoded_fallback_rows", partition),
        }
    }

    fn record_decode(&self, stats: DecodeStats) {
        self.files_considered.add(stats.files_considered);
        self.files_pruned.add(stats.files_pruned);
        self.files_validated.add(stats.files_validated);
        self.overlay_files_hidden.add(stats.overlay_files_hidden);
        self.overlay_rows_hidden.add(stats.overlay_rows_hidden);
        self.overlay_morsels_pruned
            .add(stats.overlay_morsels_pruned);
        self.covm_entries_stale.add(stats.covm_entries_stale);
        self.manifest_fallbacks.add(stats.manifest_fallbacks);
        self.covx_sidecars_loaded.add(stats.covx_sidecars_loaded);
        self.covx_sidecars_stale.add(stats.covx_sidecars_stale);
        self.covx_sidecars_ignored.add(stats.covx_sidecars_ignored);
        self.sidecar_index_fallbacks
            .add(stats.sidecar_index_fallbacks);
        self.pages_decoded.add(stats.pages_decoded);
        self.rows_materialized.add(stats.rows_materialized);
        self.rows_selected.add(stats.rows_selected);
        self.morsels_pruned.add(stats.morsels_pruned);
        self.morsels_considered.add(stats.morsels_considered);
        self.residual_rows.add(stats.residual_rows);
        self.metadata_bytes_read.add(stats.metadata_bytes_read);
        self.data_bytes_read.add(stats.data_bytes_read);
        self.range_requests.add(stats.range_requests);
        self.coalesced_range_requests
            .add(stats.coalesced_range_requests);
        self.scan_tasks.add(stats.scan_tasks);
        self.scan_partitions.add(stats.scan_partitions);
        self.dynamic_filter_snapshots
            .add(stats.dynamic_filter_snapshots);
        self.dynamic_filter_pruned_tasks
            .add(stats.dynamic_filter_pruned_tasks);
        self.dynamic_filter_fallbacks
            .add(stats.dynamic_filter_fallbacks);
        self.predicate_pages_checked
            .add(stats.predicate_pages_checked);
        self.lookup_index_hits.add(stats.lookup_index_hits);
        self.lookup_index_misses.add(stats.lookup_index_misses);
        self.inverted_index_hits.add(stats.inverted_index_hits);
        self.index_rows_selected.add(stats.index_rows_selected);
        self.index_fallbacks.add(stats.index_fallbacks);
        self.execution_code_profiles_used
            .add(stats.execution_code_profiles_used);
        self.execution_code_profile_fallbacks
            .add(stats.execution_code_profile_fallbacks);
        self.execution_code_literal_resolutions
            .add(stats.execution_code_literal_resolutions);
        self.exact_predicates.add(stats.exact_predicates);
        self.residual_predicates.add(stats.residual_predicates);
        self.exactness_fallbacks.add(stats.exactness_fallbacks);
        self.lookup_rowref_tasks.add(stats.lookup_rowref_tasks);
        self.selection_all_rows.add(stats.selection_all_rows);
        self.selection_none.add(stats.selection_none);
        self.selection_bitsets.add(stats.selection_bitsets);
        self.selection_row_indices.add(stats.selection_row_indices);
        self.range_plan_sparse.add(stats.range_plan_sparse);
        self.range_plan_mixed.add(stats.range_plan_mixed);
        self.range_plan_dense.add(stats.range_plan_dense);
        self.kernel_fallbacks.add(stats.kernel_fallbacks);
        self.arrow_export_direct_varbytes_rows
            .add(stats.arrow_export_direct_varbytes_rows);
        self.arrow_export_direct_varbytes_bytes
            .add(stats.arrow_export_direct_varbytes_bytes);
        self.arrow_export_direct_numcode_rows
            .add(stats.arrow_export_direct_numcode_rows);
        self.arrow_export_direct_plainfixed_rows
            .add(stats.arrow_export_direct_plainfixed_rows);
        self.arrow_export_direct_filecode_dictionary_rows
            .add(stats.arrow_export_direct_filecode_dictionary_rows);
        self.arrow_export_direct_transform_rows
            .add(stats.arrow_export_direct_transform_rows);
        self.arrow_export_direct_constant_plainvarint_rows
            .add(stats.arrow_export_direct_constant_plainvarint_rows);
        self.arrow_export_fallback_rows
            .add(stats.arrow_export_fallback_rows);
        self.filecode_dictionary_keys_rows
            .add(stats.filecode_dictionary_keys_rows);
        self.filecode_dictionary_remapped_rows
            .add(stats.filecode_dictionary_remapped_rows);
        self.filecode_dictionary_values_bytes
            .add(stats.filecode_dictionary_values_bytes);
        self.filecode_dictionary_value_cache_hits
            .add(stats.filecode_dictionary_value_cache_hits);
        self.filecode_dictionary_value_cache_misses
            .add(stats.filecode_dictionary_value_cache_misses);
        self.filecode_dictionary_decoded_fallback_rows
            .add(stats.filecode_dictionary_decoded_fallback_rows);
    }

    fn record_batch(&self, rows: usize) {
        self.output_batches.add(1);
        self.output_rows.add(rows);
    }

    fn record_streaming_partition(&self) {
        self.materialization_streaming_partitions.add(1);
    }

    fn record_buffered_partition(&self) {
        self.materialization_buffered_partitions.add(1);
    }
}

#[cfg(feature = "dynamic-filters")]
fn prepare_plan_for_decode(
    state: &DatasetState,
    mut plan: ScanPlan,
    dynamic_filters: Vec<Arc<dyn PhysicalExpr>>,
) -> (ScanPlan, DecodeStats) {
    let mut dynamic_stats = DecodeStats::default();
    if !dynamic_filters.is_empty() {
        match snapshot_dynamic_filters(state, &dynamic_filters) {
            Ok(snapshot) => {
                dynamic_stats.dynamic_filter_snapshots += snapshot.snapshots;
                dynamic_stats.dynamic_filter_fallbacks += snapshot.fallbacks;
                plan.filters.extend(snapshot.filters);
            }
            Err(_) => {
                dynamic_stats.dynamic_filter_fallbacks += dynamic_filters.len();
            }
        }
    }
    (plan, dynamic_stats)
}

#[cfg(not(feature = "dynamic-filters"))]
fn prepare_plan_for_decode(_state: &DatasetState, plan: ScanPlan) -> (ScanPlan, DecodeStats) {
    (plan, DecodeStats::default())
}

type BufferedDecodeResult = std::result::Result<Vec<RecordBatch>, cove_core::CoveError>;

#[derive(Debug)]
pub(crate) struct CoveRecordBatchStream {
    schema: SchemaRef,
    metrics: CoveStreamMetrics,
    inner: CoveRecordBatchStreamInner,
}

#[derive(Debug)]
enum CoveRecordBatchStreamInner {
    Streaming {
        receiver: mpsc::Receiver<DecodeStreamEvent>,
        decode_task: Option<tokio::task::JoinHandle<()>>,
        done: bool,
    },
    Buffered {
        batches: VecDeque<RecordBatch>,
        decode_task: Option<tokio::task::JoinHandle<BufferedDecodeResult>>,
        done: bool,
    },
}

impl CoveRecordBatchStream {
    pub(crate) fn new_streaming(
        schema: SchemaRef,
        state: Arc<DatasetState>,
        scan_cache: Arc<ScanExecutionCache>,
        plan: ScanPlan,
        tasks: Vec<ScanTask>,
        partition_index: usize,
        partition_count: usize,
        fetch: Option<usize>,
        #[cfg(feature = "dynamic-filters")] dynamic_filters: Vec<Arc<dyn PhysicalExpr>>,
        metrics: CoveStreamMetrics,
    ) -> Self {
        metrics.record_streaming_partition();
        let (sender, receiver) = mpsc::channel(DECODE_BATCH_CHANNEL_CAPACITY);
        let decode_metrics = metrics.clone();
        let decode_task = tokio::task::spawn_blocking(move || {
            let (plan, dynamic_stats) = prepare_plan_for_decode(
                &state,
                plan,
                #[cfg(feature = "dynamic-filters")]
                dynamic_filters,
            );
            let batch_sink = ChannelDecodeSink::new(sender.clone());
            let mut sink = FetchLimitedDecodeSink::new(batch_sink, fetch);
            let result = decode_local_dataset_scan_tasks_with_sink(
                &state,
                &plan,
                &tasks,
                partition_index,
                partition_count,
                scan_cache,
                &mut sink,
            );
            match result {
                Ok(mut stats) => {
                    stats.add_decode(dynamic_stats);
                    decode_metrics.record_decode(stats);
                    let _ = sender.blocking_send(DecodeStreamEvent::Finished);
                }
                Err(error) => {
                    let _ = sender.blocking_send(DecodeStreamEvent::Failed(error));
                }
            }
        });
        Self {
            schema,
            metrics,
            inner: CoveRecordBatchStreamInner::Streaming {
                receiver,
                decode_task: Some(decode_task),
                done: false,
            },
        }
    }

    pub(crate) fn new_buffered(
        schema: SchemaRef,
        state: Arc<DatasetState>,
        scan_cache: Arc<ScanExecutionCache>,
        plan: ScanPlan,
        tasks: Vec<ScanTask>,
        partition_index: usize,
        partition_count: usize,
        metrics: CoveStreamMetrics,
    ) -> Self {
        metrics.record_buffered_partition();
        let decode_metrics = metrics.clone();
        let decode_task = tokio::task::spawn_blocking(move || {
            let (plan, dynamic_stats) = prepare_plan_for_decode(
                &state,
                plan,
                #[cfg(feature = "dynamic-filters")]
                Vec::new(),
            );
            let mut decoded = decode_local_dataset_scan_tasks_with_cache(
                &state,
                &plan,
                &tasks,
                partition_index,
                partition_count,
                scan_cache,
            )?;
            decoded.stats.add_decode(dynamic_stats);
            decode_metrics.record_decode(decoded.stats);
            Ok(decoded.batches)
        });
        Self {
            schema,
            metrics,
            inner: CoveRecordBatchStreamInner::Buffered {
                batches: VecDeque::new(),
                decode_task: Some(decode_task),
                done: false,
            },
        }
    }
}

impl Stream for CoveRecordBatchStream {
    type Item = Result<arrow_array::RecordBatch>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let metrics = this.metrics.clone();

        match &mut this.inner {
            CoveRecordBatchStreamInner::Streaming {
                receiver,
                decode_task,
                done,
            } => {
                if *done {
                    return Poll::Ready(None);
                }
                match Pin::new(receiver).poll_recv(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Some(DecodeStreamEvent::Batch(batch))) => {
                        metrics.record_batch(batch.num_rows());
                        Poll::Ready(Some(Ok(batch)))
                    }
                    Poll::Ready(Some(DecodeStreamEvent::Finished)) => {
                        *done = true;
                        *decode_task = None;
                        Poll::Ready(None)
                    }
                    Poll::Ready(Some(DecodeStreamEvent::Failed(error))) => {
                        *done = true;
                        *decode_task = None;
                        Poll::Ready(Some(Err(cove_to_datafusion(error))))
                    }
                    Poll::Ready(None) => {
                        if let Some(task) = decode_task.as_mut() {
                            match Pin::new(task).poll(cx) {
                                Poll::Pending => Poll::Pending,
                                Poll::Ready(Ok(())) => {
                                    *decode_task = None;
                                    *done = true;
                                    Poll::Ready(None)
                                }
                                Poll::Ready(Err(error)) => {
                                    *decode_task = None;
                                    *done = true;
                                    Poll::Ready(Some(Err(DataFusionError::Execution(format!(
                                        "CoveRecordBatchStream decode task failed: {error}"
                                    )))))
                                }
                            }
                        } else {
                            *done = true;
                            Poll::Ready(None)
                        }
                    }
                }
            }
            CoveRecordBatchStreamInner::Buffered {
                batches,
                decode_task,
                done,
            } => {
                if *done {
                    return Poll::Ready(None);
                }
                if let Some(batch) = batches.pop_front() {
                    metrics.record_batch(batch.num_rows());
                    return Poll::Ready(Some(Ok(batch)));
                }
                if let Some(task) = decode_task.as_mut() {
                    match Pin::new(task).poll(cx) {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(Ok(Ok(decoded_batches))) => {
                            *decode_task = None;
                            *batches = VecDeque::from(decoded_batches);
                            if let Some(batch) = batches.pop_front() {
                                metrics.record_batch(batch.num_rows());
                                Poll::Ready(Some(Ok(batch)))
                            } else {
                                *done = true;
                                Poll::Ready(None)
                            }
                        }
                        Poll::Ready(Ok(Err(error))) => {
                            *decode_task = None;
                            *done = true;
                            Poll::Ready(Some(Err(cove_to_datafusion(error))))
                        }
                        Poll::Ready(Err(error)) => {
                            *decode_task = None;
                            *done = true;
                            Poll::Ready(Some(Err(DataFusionError::Execution(format!(
                                "CoveRecordBatchStream buffered decode task failed: {error}"
                            )))))
                        }
                    }
                } else {
                    *done = true;
                    Poll::Ready(None)
                }
            }
        }
    }
}

impl RecordBatchStream for CoveRecordBatchStream {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }
}
