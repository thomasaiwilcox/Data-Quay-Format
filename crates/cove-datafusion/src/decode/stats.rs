use arrow_array::RecordBatch;

use super::*;

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
    pub original_range_requests: usize,
    pub range_bytes_requested: usize,
    pub range_bytes_used: usize,
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
    pub predicate_orderings: usize,
    pub exactness_fallbacks: usize,
    pub late_materialization_morsels: usize,
    pub late_materialization_rows_skipped: usize,
    pub late_materialization_cells_skipped: usize,
    pub lookup_rowref_tasks: usize,
    pub selection_all_rows: usize,
    pub selection_none: usize,
    pub selection_bitsets: usize,
    pub selection_row_indices: usize,
    pub range_plan_sparse: usize,
    pub range_plan_mixed: usize,
    pub range_plan_dense: usize,
    pub kernel_fallbacks: usize,
    pub arrow_export_direct_varbytes_rows: usize,
    pub arrow_export_direct_varbytes_bytes: usize,
    pub arrow_export_direct_numcode_rows: usize,
    pub arrow_export_direct_plainfixed_rows: usize,
    pub arrow_export_direct_filecode_dictionary_rows: usize,
    pub arrow_export_direct_transform_rows: usize,
    pub arrow_export_direct_constant_plainvarint_rows: usize,
    pub arrow_export_fallback_rows: usize,
    pub filecode_dictionary_keys_rows: usize,
    pub filecode_dictionary_values_bytes: usize,
    pub filecode_dictionary_value_cache_hits: usize,
    pub filecode_dictionary_value_cache_misses: usize,
    pub filecode_dictionary_decoded_fallback_rows: usize,
    pub utf8_proof_hits: usize,
    pub utf8_proof_misses: usize,
    pub utf8_proofs_earned: usize,
}

impl DecodeStats {
    pub(crate) fn record_bootstrap(&mut self, stats: DatasetBootstrapStats) {
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
        self.original_range_requests += other.original_range_requests;
        self.range_bytes_requested += other.range_bytes_requested;
        self.range_bytes_used += other.range_bytes_used;
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
        self.predicate_orderings += other.predicate_orderings;
        self.exactness_fallbacks += other.exactness_fallbacks;
        self.late_materialization_morsels += other.late_materialization_morsels;
        self.late_materialization_rows_skipped += other.late_materialization_rows_skipped;
        self.late_materialization_cells_skipped += other.late_materialization_cells_skipped;
        self.lookup_rowref_tasks += other.lookup_rowref_tasks;
        self.selection_all_rows += other.selection_all_rows;
        self.selection_none += other.selection_none;
        self.selection_bitsets += other.selection_bitsets;
        self.selection_row_indices += other.selection_row_indices;
        self.range_plan_sparse += other.range_plan_sparse;
        self.range_plan_mixed += other.range_plan_mixed;
        self.range_plan_dense += other.range_plan_dense;
        self.kernel_fallbacks += other.kernel_fallbacks;
        self.arrow_export_direct_varbytes_rows += other.arrow_export_direct_varbytes_rows;
        self.arrow_export_direct_varbytes_bytes += other.arrow_export_direct_varbytes_bytes;
        self.arrow_export_direct_numcode_rows += other.arrow_export_direct_numcode_rows;
        self.arrow_export_direct_plainfixed_rows += other.arrow_export_direct_plainfixed_rows;
        self.arrow_export_direct_filecode_dictionary_rows +=
            other.arrow_export_direct_filecode_dictionary_rows;
        self.arrow_export_direct_transform_rows += other.arrow_export_direct_transform_rows;
        self.arrow_export_direct_constant_plainvarint_rows +=
            other.arrow_export_direct_constant_plainvarint_rows;
        self.arrow_export_fallback_rows += other.arrow_export_fallback_rows;
        self.filecode_dictionary_keys_rows += other.filecode_dictionary_keys_rows;
        self.filecode_dictionary_values_bytes += other.filecode_dictionary_values_bytes;
        self.filecode_dictionary_value_cache_hits += other.filecode_dictionary_value_cache_hits;
        self.filecode_dictionary_value_cache_misses += other.filecode_dictionary_value_cache_misses;
        self.filecode_dictionary_decoded_fallback_rows +=
            other.filecode_dictionary_decoded_fallback_rows;
        self.utf8_proof_hits += other.utf8_proof_hits;
        self.utf8_proof_misses += other.utf8_proof_misses;
        self.utf8_proofs_earned += other.utf8_proofs_earned;
    }
}

#[derive(Debug)]
pub struct DecodedScan {
    pub batches: Vec<RecordBatch>,
    pub stats: DecodeStats,
}
