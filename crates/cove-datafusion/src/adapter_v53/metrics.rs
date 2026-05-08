//! DataFusion 53.x metrics surfaces.

use datafusion::physical_plan::metrics::{Count, ExecutionPlanMetricsSet, MetricBuilder};

use crate::decode::DecodeStats;

#[derive(Debug, Clone)]
pub(crate) struct CoveFileMetrics {
    pub(crate) files_opened: Count,
    pub(crate) files_considered: Count,
    pub(crate) files_pruned: Count,
    pub(crate) files_validated: Count,
    pub(crate) overlay_files_hidden: Count,
    pub(crate) overlay_rows_hidden: Count,
    pub(crate) overlay_morsels_pruned: Count,
    pub(crate) covm_entries_stale: Count,
    pub(crate) manifest_fallbacks: Count,
    pub(crate) covx_sidecars_loaded: Count,
    pub(crate) covx_sidecars_stale: Count,
    pub(crate) covx_sidecars_ignored: Count,
    pub(crate) sidecar_index_fallbacks: Count,
    pub(crate) metadata_bytes_read: Count,
    pub(crate) data_bytes_read: Count,
    pub(crate) range_requests: Count,
    pub(crate) coalesced_range_requests: Count,
    pub(crate) scan_tasks: Count,
    pub(crate) scan_partitions: Count,
    pub(crate) dynamic_filter_snapshots: Count,
    pub(crate) dynamic_filter_pruned_tasks: Count,
    pub(crate) dynamic_filter_fallbacks: Count,
    pub(crate) pages_decoded: Count,
    pub(crate) rows_materialized: Count,
    pub(crate) rows_selected: Count,
    pub(crate) morsels_pruned: Count,
    pub(crate) morsels_considered: Count,
    pub(crate) residual_rows: Count,
    pub(crate) predicate_pages_checked: Count,
    pub(crate) lookup_index_hits: Count,
    pub(crate) lookup_index_misses: Count,
    pub(crate) inverted_index_hits: Count,
    pub(crate) index_rows_selected: Count,
    pub(crate) index_fallbacks: Count,
    pub(crate) execution_code_profiles_used: Count,
    pub(crate) execution_code_profile_fallbacks: Count,
    pub(crate) execution_code_literal_resolutions: Count,
    pub(crate) exact_predicates: Count,
    pub(crate) residual_predicates: Count,
    pub(crate) exactness_fallbacks: Count,
    pub(crate) lookup_rowref_tasks: Count,
    pub(crate) selection_all_rows: Count,
    pub(crate) selection_none: Count,
    pub(crate) selection_bitsets: Count,
    pub(crate) selection_row_indices: Count,
    pub(crate) range_plan_sparse: Count,
    pub(crate) range_plan_mixed: Count,
    pub(crate) range_plan_dense: Count,
    pub(crate) kernel_fallbacks: Count,
    pub(crate) arrow_export_direct_varbytes_rows: Count,
    pub(crate) arrow_export_direct_varbytes_bytes: Count,
    pub(crate) arrow_export_direct_numcode_rows: Count,
    pub(crate) arrow_export_direct_plainfixed_rows: Count,
    pub(crate) arrow_export_direct_filecode_dictionary_rows: Count,
    pub(crate) arrow_export_direct_transform_rows: Count,
    pub(crate) arrow_export_direct_constant_plainvarint_rows: Count,
    pub(crate) arrow_export_fallback_rows: Count,
    pub(crate) filecode_dictionary_keys_rows: Count,
    pub(crate) filecode_dictionary_values_bytes: Count,
    pub(crate) filecode_dictionary_value_cache_hits: Count,
    pub(crate) filecode_dictionary_value_cache_misses: Count,
    pub(crate) filecode_dictionary_decoded_fallback_rows: Count,
}

impl CoveFileMetrics {
    pub(crate) fn new(metrics: &ExecutionPlanMetricsSet, partition: usize) -> Self {
        Self {
            files_opened: MetricBuilder::new(metrics).counter("cove_files_opened", partition),
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
            pages_decoded: MetricBuilder::new(metrics).counter("cove_pages_decoded", partition),
            rows_materialized: MetricBuilder::new(metrics)
                .counter("cove_rows_materialized", partition),
            rows_selected: MetricBuilder::new(metrics).counter("cove_rows_selected", partition),
            morsels_pruned: MetricBuilder::new(metrics).counter("cove_morsels_pruned", partition),
            morsels_considered: MetricBuilder::new(metrics)
                .counter("cove_morsels_considered", partition),
            residual_rows: MetricBuilder::new(metrics).counter("cove_residual_rows", partition),
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

    pub(crate) fn record_decode(&self, stats: DecodeStats) {
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
        self.filecode_dictionary_values_bytes
            .add(stats.filecode_dictionary_values_bytes);
        self.filecode_dictionary_value_cache_hits
            .add(stats.filecode_dictionary_value_cache_hits);
        self.filecode_dictionary_value_cache_misses
            .add(stats.filecode_dictionary_value_cache_misses);
        self.filecode_dictionary_decoded_fallback_rows
            .add(stats.filecode_dictionary_decoded_fallback_rows);
    }
}
