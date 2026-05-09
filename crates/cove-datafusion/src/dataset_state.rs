//! Immutable dataset metadata state shared across scans.

mod construct;
mod pruning;
mod pruning_sections;
mod schema;
mod segment;

use std::sync::Arc;

use arrow_schema::SchemaRef;
use cove_arrow::arrow::ArrowExportOptions;
use cove_core::{
    domain::ColumnDomain,
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    mount::MountedCoveFile,
    segment::TableSegmentIndexEntryV1,
    table::TableEntry,
    zone_stats::ZoneStatsSection,
    CoveError,
};

use crate::{
    decode::Utf8ProofCache,
    execution_code::{self, ExecutionCodePlanStats},
    options::{ExecutionCodePolicy, LocalFileReadPolicy, PagePayloadValidationPolicy},
    overlay::RowVisibility,
    planner::{CovePredicate, ScanPlan},
    range_reader::RangeCoalescingOptions,
    scan_program::compile_scan_program,
};

pub use pruning_sections::{
    parse_aggregates_from_sections, parse_blooms_from_sections, parse_column_domains_from_sections,
    parse_composites_from_sections, parse_exact_sets_from_sections, parse_inverted_from_sections,
    parse_lookups_from_sections, parse_topn_from_sections, parse_zone_stats_from_sections,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    pub source: Arc<str>,
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: u32,
}

#[derive(Debug, Clone, Default)]
pub struct PruningMetadata {
    pub column_domains: Arc<Vec<ColumnDomain>>,
    pub zone_stats: Arc<Vec<ZoneStatsSection>>,
    pub exact_sets: Arc<Vec<ExactSetIndex>>,
    pub blooms: Arc<Vec<BloomFilterIndex>>,
    pub lookups: Arc<Vec<LookupIndex>>,
    pub inverted: Arc<Vec<InvertedMorselIndex>>,
    pub aggregates: Arc<Vec<AggregateSynopsis>>,
    pub composites: Arc<Vec<CompositeIndex>>,
    pub topn: Arc<Vec<TopNSummary>>,
}

#[derive(Debug, Clone, Default)]
pub struct FileTable {
    pub source: Vec<Arc<str>>,
    pub file_id: Vec<[u8; 16]>,
    pub file_len: Vec<u64>,
    pub footer_crc32c: Vec<u32>,
    pub row_count: Vec<u64>,
    pub segment_count: Vec<u32>,
    pub flags: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DatasetBootstrapStats {
    pub files_considered: usize,
    pub files_pruned: usize,
    pub files_validated: usize,
    pub overlay_files_hidden: usize,
    pub overlay_rows_hidden: usize,
    pub covm_entries_stale: usize,
    pub manifest_fallbacks: usize,
    pub covx_sidecars_loaded: usize,
    pub covx_sidecars_stale: usize,
    pub covx_sidecars_ignored: usize,
    pub sidecar_index_fallbacks: usize,
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    identity: FileIdentity,
    file_bytes: Option<Arc<Vec<u8>>>,
    mounted: Arc<MountedCoveFile>,
    table: Arc<TableEntry>,
    segments: Arc<Vec<TableSegmentIndexEntryV1>>,
    pruning: PruningMetadata,
    visibility: RowVisibility,
    flags: u32,
}

/// Metadata for a single COVE file scan path.
///
/// INVARIANT: every `DatasetState` is built only after `cove-core` has
/// validated the host file and mount metadata. DataFusion adapter code must
/// treat this as immutable query-planning state and must not reinterpret COVE
/// file features independently.
#[derive(Debug, Clone)]
pub struct DatasetState {
    identity: FileIdentity,
    file_bytes: Option<Arc<Vec<u8>>>,
    mounted: Arc<MountedCoveFile>,
    table: Arc<TableEntry>,
    schema: SchemaRef,
    segments: Arc<Vec<TableSegmentIndexEntryV1>>,
    arrow_export_options: ArrowExportOptions,
    execution_code_policy: ExecutionCodePolicy,
    page_payload_validation_policy: PagePayloadValidationPolicy,
    local_file_read_policy: LocalFileReadPolicy,
    target_morsels_per_partition: usize,
    range_coalescing: RangeCoalescingOptions,
    dynamic_filters_enabled: bool,
    pruning: PruningMetadata,
    planning_cache: pruning::PlanningCache,
    file_table: FileTable,
    files: Arc<Vec<FileMetadata>>,
    utf8_proof_cache: Arc<Utf8ProofCache>,
    bootstrap_stats: DatasetBootstrapStats,
}

impl DatasetState {
    pub fn file_table(&self) -> &FileTable {
        &self.file_table
    }

    pub fn files(&self) -> &[FileMetadata] {
        self.files.as_slice()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn file(&self, file_ordinal: usize) -> Result<&FileMetadata, CoveError> {
        self.files.get(file_ordinal).ok_or_else(|| {
            CoveError::BadSection(format!("file ordinal {file_ordinal} is out of bounds"))
        })
    }

    pub fn bootstrap_stats(&self) -> DatasetBootstrapStats {
        self.bootstrap_stats
    }

    pub fn file_code_for_canonical(
        &self,
        file_ordinal: usize,
        canonical: &[u8],
    ) -> Result<Option<u32>, CoveError> {
        let Some(reverse_lookup) = self.file(file_ordinal)?.mounted.reverse_lookup.as_ref() else {
            return Ok(None);
        };
        Ok(reverse_lookup.by_canonical_value.get(canonical).copied())
    }

    pub fn resolved_plan_for_file(
        &self,
        plan: &ScanPlan,
        file_ordinal: usize,
    ) -> Result<ScanPlan, CoveError> {
        self.resolved_plan_for_file_with_stats(plan, file_ordinal)
            .map(|(plan, _)| plan)
    }

    pub fn resolved_plan_for_file_with_stats(
        &self,
        plan: &ScanPlan,
        file_ordinal: usize,
    ) -> Result<(ScanPlan, ExecutionCodePlanStats), CoveError> {
        let mut plan = plan.clone();
        let mut stats = ExecutionCodePlanStats::default();
        for filter in &mut plan.filters {
            let Some(CovePredicate::FileCodeIn {
                file_codes,
                canonical_values,
                ..
            }) = filter.predicate.as_mut()
            else {
                continue;
            };
            if canonical_values.is_empty() {
                continue;
            }
            let (resolved, resolution_stats) =
                execution_code::resolve_file_code_predicate_for_file(
                    self,
                    file_ordinal,
                    canonical_values,
                )?;
            stats.supported_files += resolution_stats.supported_files;
            stats.fallback_files += resolution_stats.fallback_files;
            stats.literal_resolutions += resolution_stats.literal_resolutions;
            *file_codes = resolved;
        }
        let file_state = self.single_file_view(file_ordinal)?;
        plan.scan_program = compile_scan_program(&file_state, &plan.filters);
        Ok((plan, stats))
    }

    /// Resolve a scan plan against this exact dataset state.
    ///
    /// INVARIANT: direct native decode/prune entrypoints operate on single-file
    /// dataset views, so FileCode predicates must be resolved against that
    /// concrete file before any pruning or residual evaluation touches them.
    pub fn resolved_plan_for_current_state(&self, plan: &ScanPlan) -> Result<ScanPlan, CoveError> {
        self.resolved_plan_for_current_state_with_stats(plan)
            .map(|(plan, _)| plan)
    }

    pub fn resolved_plan_for_current_state_with_stats(
        &self,
        plan: &ScanPlan,
    ) -> Result<(ScanPlan, ExecutionCodePlanStats), CoveError> {
        if self.file_count() != 1 {
            return Err(CoveError::BadSchema(format!(
                "direct scan execution requires a single-file dataset view, found {} files",
                self.file_count()
            )));
        }
        self.resolved_plan_for_file_with_stats(plan, 0)
    }

    pub fn identity(&self) -> &FileIdentity {
        &self.identity
    }

    pub(crate) fn utf8_proof_cache(&self) -> &Arc<Utf8ProofCache> {
        &self.utf8_proof_cache
    }

    pub fn source(&self) -> &str {
        self.identity.source.as_ref()
    }

    pub fn file_len(&self) -> u64 {
        self.identity.file_len
    }

    pub fn file_id(&self) -> &[u8; 16] {
        &self.identity.file_id
    }

    pub fn footer_crc32c(&self) -> u32 {
        self.identity.footer_crc32c
    }

    pub fn full_file_bytes(&self) -> Result<&[u8], CoveError> {
        self.file_bytes
            .as_deref()
            .map(Vec::as_slice)
            .ok_or_else(|| {
                CoveError::BadSection(
                    "COVE DataFusion scan requires range execution for metadata-only state".into(),
                )
            })
    }

    pub fn has_full_file_bytes(&self) -> bool {
        self.file_bytes.is_some()
    }

    pub fn mounted(&self) -> &MountedCoveFile {
        self.mounted.as_ref()
    }

    pub fn table(&self) -> &TableEntry {
        self.table.as_ref()
    }

    pub fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    pub fn segments(&self) -> &[TableSegmentIndexEntryV1] {
        self.segments.as_slice()
    }

    pub fn arrow_export_options(&self) -> ArrowExportOptions {
        self.arrow_export_options
    }

    pub fn execution_code_policy(&self) -> ExecutionCodePolicy {
        self.execution_code_policy
    }

    pub fn page_payload_validation_policy(&self) -> PagePayloadValidationPolicy {
        self.page_payload_validation_policy
    }

    pub fn local_file_read_policy(&self) -> LocalFileReadPolicy {
        self.local_file_read_policy
    }

    pub fn target_morsels_per_partition(&self) -> usize {
        self.target_morsels_per_partition
    }

    pub fn range_coalescing(&self) -> RangeCoalescingOptions {
        self.range_coalescing
    }

    pub fn dynamic_filters_enabled(&self) -> bool {
        self.dynamic_filters_enabled
    }

    pub fn pruning(&self) -> &PruningMetadata {
        &self.pruning
    }

    pub fn full_projection(&self) -> Vec<usize> {
        (0..self.table.columns.len()).collect()
    }

    pub fn projected_schema(&self, projection: &[usize]) -> Result<SchemaRef, CoveError> {
        self.schema
            .project(projection)
            .map(Arc::new)
            .map_err(|err| CoveError::BadSchema(format!("Arrow schema projection: {err}")))
    }
}

impl FileMetadata {
    pub fn new(
        identity: FileIdentity,
        file_bytes: Option<Arc<Vec<u8>>>,
        mounted: Arc<MountedCoveFile>,
        table: Arc<TableEntry>,
        segments: Arc<Vec<TableSegmentIndexEntryV1>>,
        pruning: PruningMetadata,
        flags: u32,
    ) -> Self {
        Self::new_with_visibility(
            identity,
            file_bytes,
            mounted,
            table,
            segments,
            pruning,
            RowVisibility::All,
            flags,
        )
    }

    pub fn new_with_visibility(
        identity: FileIdentity,
        file_bytes: Option<Arc<Vec<u8>>>,
        mounted: Arc<MountedCoveFile>,
        table: Arc<TableEntry>,
        segments: Arc<Vec<TableSegmentIndexEntryV1>>,
        pruning: PruningMetadata,
        visibility: RowVisibility,
        flags: u32,
    ) -> Self {
        Self {
            identity,
            file_bytes,
            mounted,
            table,
            segments,
            pruning,
            visibility,
            flags,
        }
    }

    pub fn identity(&self) -> &FileIdentity {
        &self.identity
    }

    pub fn source(&self) -> &str {
        self.identity.source.as_ref()
    }

    pub fn has_full_file_bytes(&self) -> bool {
        self.file_bytes.is_some()
    }

    pub fn full_file_bytes(&self) -> Option<&[u8]> {
        self.file_bytes.as_deref().map(Vec::as_slice)
    }

    pub fn full_file_bytes_arc(&self) -> Option<Arc<Vec<u8>>> {
        self.file_bytes.clone()
    }

    pub fn mounted(&self) -> &MountedCoveFile {
        self.mounted.as_ref()
    }

    pub fn table(&self) -> &TableEntry {
        self.table.as_ref()
    }

    pub fn segments(&self) -> &[TableSegmentIndexEntryV1] {
        self.segments.as_slice()
    }

    pub fn pruning(&self) -> &PruningMetadata {
        &self.pruning
    }

    pub fn visibility(&self) -> &RowVisibility {
        &self.visibility
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }
}
