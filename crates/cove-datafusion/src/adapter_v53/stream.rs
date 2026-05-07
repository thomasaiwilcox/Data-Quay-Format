//! DataFusion 53.x stream glue.

use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use arrow_schema::SchemaRef;
use datafusion::{
    common::{DataFusionError, Result},
    execution::RecordBatchStream,
    physical_plan::metrics::{Count, MetricBuilder},
};
use futures::Stream;

use crate::{
    adapter_v53::cove_to_datafusion,
    dataset_state::DatasetState,
    decode::{decode_local_dataset_scan_tasks, DecodeStats, DecodedScan},
    planner::ScanPlan,
    task_graph::ScanTask,
};

#[cfg(feature = "dynamic-filters")]
use crate::adapter_v53::dynamic_filter::snapshot_dynamic_filters;

#[cfg(feature = "dynamic-filters")]
use datafusion::physical_expr::PhysicalExpr;

#[derive(Debug)]
pub(crate) struct CoveStreamMetrics {
    output_batches: Count,
    output_rows: Count,
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
}

impl CoveStreamMetrics {
    pub(crate) fn new(
        metrics: &datafusion::physical_plan::metrics::ExecutionPlanMetricsSet,
        partition: usize,
    ) -> Self {
        Self {
            output_batches: MetricBuilder::new(metrics).output_batches(partition),
            output_rows: MetricBuilder::new(metrics).output_rows(partition),
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
    }

    fn record_batch(&self, rows: usize) {
        self.output_batches.add(1);
        self.output_rows.add(rows);
    }
}

#[derive(Debug)]
pub(crate) struct CoveRecordBatchStream {
    schema: SchemaRef,
    state: Option<Arc<DatasetState>>,
    plan: ScanPlan,
    tasks: Vec<ScanTask>,
    partition_index: usize,
    partition_count: usize,
    #[cfg(feature = "dynamic-filters")]
    dynamic_filters: Vec<Arc<dyn PhysicalExpr>>,
    batches: VecDeque<arrow_array::RecordBatch>,
    decode_task:
        Option<tokio::task::JoinHandle<std::result::Result<DecodedScan, cove_core::CoveError>>>,
    metrics: CoveStreamMetrics,
    done: bool,
}

impl CoveRecordBatchStream {
    pub(crate) fn new(
        schema: SchemaRef,
        state: Arc<DatasetState>,
        plan: ScanPlan,
        tasks: Vec<ScanTask>,
        partition_index: usize,
        partition_count: usize,
        #[cfg(feature = "dynamic-filters")] dynamic_filters: Vec<Arc<dyn PhysicalExpr>>,
        metrics: CoveStreamMetrics,
    ) -> Self {
        Self {
            schema,
            state: Some(state),
            plan,
            tasks,
            partition_index,
            partition_count,
            #[cfg(feature = "dynamic-filters")]
            dynamic_filters,
            batches: VecDeque::new(),
            decode_task: None,
            metrics,
            done: false,
        }
    }
}

impl Stream for CoveRecordBatchStream {
    type Item = Result<arrow_array::RecordBatch>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(batch) = this.batches.pop_front() {
            this.metrics.record_batch(batch.num_rows());
            return Poll::Ready(Some(Ok(batch)));
        }
        if this.done {
            return Poll::Ready(None);
        }
        if this.decode_task.is_none() {
            let Some(state) = this.state.take() else {
                this.done = true;
                return Poll::Ready(None);
            };
            #[cfg(feature = "dynamic-filters")]
            let mut plan = this.plan.clone();
            #[cfg(not(feature = "dynamic-filters"))]
            let plan = this.plan.clone();
            #[cfg(feature = "dynamic-filters")]
            let mut dynamic_stats = DecodeStats::default();
            #[cfg(not(feature = "dynamic-filters"))]
            let dynamic_stats = DecodeStats::default();
            #[cfg(feature = "dynamic-filters")]
            if !this.dynamic_filters.is_empty() {
                match snapshot_dynamic_filters(&state, &this.dynamic_filters) {
                    Ok(snapshot) => {
                        dynamic_stats.dynamic_filter_snapshots += snapshot.snapshots;
                        dynamic_stats.dynamic_filter_fallbacks += snapshot.fallbacks;
                        plan.filters.extend(snapshot.filters);
                    }
                    Err(_) => {
                        dynamic_stats.dynamic_filter_fallbacks += this.dynamic_filters.len();
                    }
                }
            }
            let tasks = this.tasks.clone();
            let partition_index = this.partition_index;
            let partition_count = this.partition_count;
            this.decode_task = Some(tokio::task::spawn_blocking(move || {
                let mut decoded = decode_local_dataset_scan_tasks(
                    &state,
                    &plan,
                    &tasks,
                    partition_index,
                    partition_count,
                )?;
                decoded.stats.add_decode(dynamic_stats);
                Ok(decoded)
            }));
        }

        let Some(task) = this.decode_task.as_mut() else {
            this.done = true;
            return Poll::Ready(None);
        };
        match Pin::new(task).poll(_cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(joined) => {
                this.decode_task = None;
                match joined {
                    Ok(Ok(decoded)) => {
                        this.metrics.record_decode(decoded.stats);
                        this.batches = decoded.batches.into();
                        if let Some(batch) = this.batches.pop_front() {
                            this.metrics.record_batch(batch.num_rows());
                            Poll::Ready(Some(Ok(batch)))
                        } else {
                            this.done = true;
                            Poll::Ready(None)
                        }
                    }
                    Ok(Err(error)) => {
                        this.done = true;
                        Poll::Ready(Some(Err(cove_to_datafusion(error))))
                    }
                    Err(error) => {
                        this.done = true;
                        Poll::Ready(Some(Err(DataFusionError::Execution(format!(
                            "CoveRecordBatchStream decode task failed: {error}"
                        )))))
                    }
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
