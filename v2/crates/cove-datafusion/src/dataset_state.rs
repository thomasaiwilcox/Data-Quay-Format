//! Immutable dataset metadata state shared across scans.

mod construct;
mod pruning;
mod pruning_sections;
mod schema;
mod segment;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use arrow_schema::SchemaRef;
use cove_arrow::arrow::ArrowExportOptions;
use cove_cache::CoverageCacheEntryV2;
use cove_core::{
    codec::CodecExtensionDescriptorV2,
    constants::{CompressionCodec, CovePhysicalKind, PrimaryProfile, SectionKind},
    domain::ColumnDomain,
    feature_binding::OperationKindV2,
    feature_scope::{FeatureScopeTable, FeatureUseRequestV2},
    footer::CoveFooter,
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    mount::MountedCoveFile,
    nested_schema::{NestedSchemaNodeV1, NestedSchemaSectionV1},
    page::{page_flag_codec, ColumnPageIndexEntryV1},
    segment::TableSegmentIndexEntryV1,
    table::{ColumnEntry, TableEntry},
    zone_stats::ZoneStatsSection,
    CoveError,
};
use cove_coverage::{
    CoveragePlanCandidateV2, CoverageProofRecordV2, CoverageProviderDescriptorV2, CoverageSetV2,
    PredicateNormalFormV2, PredicateNormalFormWithPayloadV2,
};
#[cfg(feature = "covi")]
use cove_index::execution::ValidatedCoviArtifactV2;
use cove_layout::{
    FastMetadataIndexV2, LayoutPlanV2, PageClusterDirectoryV2, PageClusterEntryV2,
    ScanSplitIndexV2, ZeroCopyBufferMapV2, ZeroCopyCompatibilityContext, ZeroCopyCompatibilityV2,
    ZeroCopyDictionarySemanticsV2, ZeroCopyLifetimeScopeV2, ZeroCopyNestedLayoutKindV2,
};

use crate::{
    coverage_plan::refresh_scan_plan_coverage,
    decode::Utf8ProofCache,
    execution_code::{self, ExecutionCodePlanStats},
    options::{ExecutionCodePolicy, LocalFileReadPolicy, PagePayloadValidationPolicy},
    overlay::RowVisibility,
    planner::{covi_candidates_for_filters, CovePredicate, ScanPlan},
    range_reader::RangeClusterHint,
    range_reader::RangeCoalescingOptions,
    scan_program::compile_scan_program,
};

pub use pruning_sections::{
    parse_aggregates_from_sections, parse_blooms_from_sections,
    parse_codec_descriptors_from_sections, parse_column_domains_from_sections,
    parse_composites_from_sections, parse_coverage_plan_candidates_from_sections,
    parse_coverage_proofs_from_sections, parse_coverage_providers_from_sections,
    parse_coverage_sets_from_sections, parse_exact_sets_from_sections,
    parse_inverted_from_sections, parse_lookups_from_sections, parse_predicate_forms_from_sections,
    parse_predicate_forms_with_payloads_from_sections, parse_topn_from_sections,
    parse_zone_stats_from_sections,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    pub source: Arc<str>,
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: u32,
}

#[derive(Debug, Clone)]
pub struct CoverageCacheMetadata {
    enabled: bool,
    entries: Arc<Vec<CoverageCacheEntryV2>>,
    counters: Arc<CoverageCacheCounters>,
}

#[derive(Debug, Default)]
struct CoverageCacheCounters {
    hits: AtomicUsize,
    misses: AtomicUsize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoverageCacheRuntimeStats {
    pub enabled: bool,
    pub entries: usize,
    pub hits: usize,
    pub misses: usize,
}

impl CoverageCacheMetadata {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            entries: Arc::new(Vec::new()),
            counters: Arc::new(CoverageCacheCounters::default()),
        }
    }

    pub fn enabled_with_entries(entries: Vec<CoverageCacheEntryV2>) -> Self {
        Self {
            enabled: true,
            entries: Arc::new(entries),
            counters: Arc::new(CoverageCacheCounters::default()),
        }
    }

    pub fn enabled_empty() -> Self {
        Self::enabled_with_entries(Vec::new())
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn entries(&self) -> &[CoverageCacheEntryV2] {
        self.entries.as_slice()
    }

    pub fn coverage_set_refs_for_predicate(&self, predicate_form_ref: u32) -> Vec<u32> {
        let mut refs = self
            .entries
            .iter()
            .filter(|entry| entry.predicate_normal_form_ref == predicate_form_ref)
            .map(|entry| entry.coverage_set_ref)
            .collect::<Vec<_>>();
        refs.sort_unstable();
        refs.dedup();
        refs
    }

    pub fn record_hit(&self) {
        if self.enabled {
            self.counters.hits.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_miss(&self) {
        if self.enabled {
            self.counters.misses.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn runtime_stats(&self) -> CoverageCacheRuntimeStats {
        CoverageCacheRuntimeStats {
            enabled: self.enabled,
            entries: self.entries.len(),
            hits: self.counters.hits.load(Ordering::Relaxed),
            misses: self.counters.misses.load(Ordering::Relaxed),
        }
    }
}

impl Default for CoverageCacheMetadata {
    fn default() -> Self {
        Self::disabled()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PruningMetadata {
    pub nested_schemas: Arc<Vec<NestedSchemaSectionV1>>,
    pub codec_descriptors: Arc<Vec<CodecExtensionDescriptorV2>>,
    pub column_domains: Arc<Vec<ColumnDomain>>,
    pub zone_stats: Arc<Vec<ZoneStatsSection>>,
    pub exact_sets: Arc<Vec<ExactSetIndex>>,
    pub blooms: Arc<Vec<BloomFilterIndex>>,
    pub lookups: Arc<Vec<LookupIndex>>,
    pub inverted: Arc<Vec<InvertedMorselIndex>>,
    pub aggregates: Arc<Vec<AggregateSynopsis>>,
    pub composites: Arc<Vec<CompositeIndex>>,
    pub topn: Arc<Vec<TopNSummary>>,
    pub coverage_providers: Arc<Vec<CoverageProviderDescriptorV2>>,
    pub coverage_sets: Arc<Vec<CoverageSetV2>>,
    pub coverage_proofs: Arc<Vec<CoverageProofRecordV2>>,
    pub coverage_plan_candidates: Arc<Vec<CoveragePlanCandidateV2>>,
    pub predicate_forms: Arc<Vec<PredicateNormalFormV2>>,
    pub predicate_forms_with_payloads: Arc<Vec<PredicateNormalFormWithPayloadV2>>,
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
    pub covi_sidecars_loaded: usize,
    pub covi_sidecars_stale: usize,
    pub covi_sidecars_ignored: usize,
    pub covi_candidate_pruned: usize,
    pub covi_index_only_answers: usize,
    pub sidecar_index_fallbacks: usize,
    pub covel_sections_loaded: usize,
    pub covel_sections_ignored: usize,
    pub covel_scan_splits_loaded: usize,
    pub covel_zero_copy_maps_loaded: usize,
    pub coverage_cache_entries_loaded: usize,
    pub coverage_cache_entries_stale: usize,
    pub coverage_cache_entries_ignored: usize,
    pub coverage_cache_invalidations: usize,
}

impl DatasetBootstrapStats {
    pub(crate) fn add_layout_metadata(&mut self, layout: &LayoutPlanningMetadataV2) {
        self.covel_sections_loaded += layout.covel_sections_loaded;
        self.covel_sections_ignored += layout.covel_sections_ignored;
        self.covel_scan_splits_loaded += usize::from(layout.scan_splits.is_some());
        self.covel_zero_copy_maps_loaded += layout.zero_copy_maps.len();
    }
}

#[derive(Debug, Clone, Default)]
pub struct LayoutPlanningMetadataV2 {
    pub layout_plans: Arc<Vec<LayoutPlanV2>>,
    pub scan_splits: Option<Arc<ScanSplitIndexV2>>,
    pub fast_metadata: Option<Arc<FastMetadataIndexV2>>,
    pub page_clusters: Option<Arc<PageClusterDirectoryV2>>,
    pub zero_copy_maps: Arc<Vec<ZeroCopyBufferMapV2>>,
    pub covel_sections_loaded: usize,
    pub covel_sections_ignored: usize,
}

impl LayoutPlanningMetadataV2 {
    pub(crate) fn record_loaded(&mut self) {
        self.covel_sections_loaded += 1;
    }

    pub(crate) fn record_ignored(&mut self) {
        self.covel_sections_ignored += 1;
    }
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    identity: FileIdentity,
    file_bytes: Option<Arc<Vec<u8>>>,
    mounted: Arc<MountedCoveFile>,
    table: Arc<TableEntry>,
    segments: Arc<Vec<TableSegmentIndexEntryV1>>,
    pruning: PruningMetadata,
    layout: LayoutPlanningMetadataV2,
    coverage_cache: CoverageCacheMetadata,
    feature_scope_table: Option<FeatureScopeTable>,
    #[cfg(feature = "covi")]
    covi: Option<Arc<ValidatedCoviArtifactV2>>,
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
    layout: LayoutPlanningMetadataV2,
    planning_cache: pruning::PlanningCache,
    coverage_cache: CoverageCacheMetadata,
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

    pub fn coverage_cache(&self) -> &CoverageCacheMetadata {
        &self.coverage_cache
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
        refresh_scan_plan_coverage(&file_state, &mut plan);
        plan.scan_program = compile_scan_program(&file_state, &plan.filters);
        plan.covi_candidates = covi_candidates_for_filters(&file_state, &plan.filters);
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

    #[inline]
    pub fn mounted(&self) -> &MountedCoveFile {
        self.mounted.as_ref()
    }

    #[inline]
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

    #[inline]
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

    pub(crate) fn range_cluster_hint(
        &self,
        segment_id: u32,
        morsel_id: u32,
        range_start: u64,
        range_end: u64,
    ) -> Option<RangeClusterHint> {
        self.layout
            .page_clusters
            .as_deref()
            .and_then(|directory| {
                cluster_for_range(directory, segment_id, morsel_id, range_start, range_end)
            })
            .map(|cluster| RangeClusterHint {
                cluster_start: cluster.offset,
                cluster_end: cluster.offset.saturating_add(cluster.length),
                preferred_coalesce_distance: u64::from(cluster.preferred_coalesce_distance),
            })
    }

    pub(crate) fn zero_copy_compatibility_for_page(
        &self,
        segment_id: u32,
        column: &ColumnEntry,
        page: &ColumnPageIndexEntryV1,
    ) -> Option<ZeroCopyCompatibilityV2> {
        let dictionary_semantics = match column.physical {
            CovePhysicalKind::FileCode => ZeroCopyDictionarySemanticsV2::FileCodeDictionary,
            _ => ZeroCopyDictionarySemanticsV2::NoDictionary,
        };
        let nested_layout_kind = match column.physical {
            CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
                ZeroCopyNestedLayoutKindV2::CoveNativeNested
            }
            _ => ZeroCopyNestedLayoutKindV2::NotNested,
        };
        let context = ZeroCopyCompatibilityContext {
            active_visibility_overlay: !self.file(0).ok()?.visibility().is_all(),
            accepts_cove_null_bitmap_polarity: true,
            expected_dictionary_semantics: dictionary_semantics,
            expected_nested_layout_kind: nested_layout_kind,
            required_lifetime_scope: ZeroCopyLifetimeScopeV2::ReaderSession,
        };
        let page_compressed = page_flag_codec(page.flags).ok()? != CompressionCodec::None;
        self.layout.zero_copy_maps.iter().find_map(|map| {
            map.entries
                .iter()
                .find(|entry| {
                    entry.table_id == self.table.table_id
                        && entry.column_id == column.column_id
                        && entry.segment_id == segment_id
                        && entry.morsel_id == page.morsel_id
                })
                .map(|entry| {
                    if page_compressed && entry.compression_required_none != 0 {
                        return ZeroCopyCompatibilityV2::MaterializeRequired(
                            cove_layout::ZeroCopyMaterializationReasonV2::CompressedBuffer,
                        );
                    }
                    entry.compatibility(&context)
                })
        })
    }

    #[cfg(feature = "covi")]
    pub(crate) fn covi(&self) -> Option<&ValidatedCoviArtifactV2> {
        self.files.first().and_then(|file| file.covi.as_deref())
    }

    #[cfg(feature = "covi")]
    pub(crate) fn with_file_covi(
        &self,
        file_ordinal: usize,
        covi: Option<Arc<ValidatedCoviArtifactV2>>,
        stats: DatasetBootstrapStats,
    ) -> Result<Self, CoveError> {
        let mut next = self.clone();
        let mut files = self.files.as_ref().clone();
        let file = files.get_mut(file_ordinal).ok_or_else(|| {
            CoveError::BadSection(format!("file ordinal {file_ordinal} is out of bounds"))
        })?;
        file.covi = covi;
        next.files = Arc::new(files);
        next.bootstrap_stats.covi_sidecars_loaded += stats.covi_sidecars_loaded;
        next.bootstrap_stats.covi_sidecars_stale += stats.covi_sidecars_stale;
        next.bootstrap_stats.covi_sidecars_ignored += stats.covi_sidecars_ignored;
        Ok(next)
    }

    pub(crate) fn with_coverage_cache(
        &self,
        coverage_cache: CoverageCacheMetadata,
        stats: DatasetBootstrapStats,
    ) -> Result<Self, CoveError> {
        let mut next = self.clone();
        let mut files = self.files.as_ref().clone();
        if files.len() == 1 {
            files[0].coverage_cache = coverage_cache.clone();
            next.files = Arc::new(files);
        }
        next.coverage_cache = coverage_cache;
        next.bootstrap_stats.coverage_cache_entries_loaded += stats.coverage_cache_entries_loaded;
        next.bootstrap_stats.coverage_cache_entries_stale += stats.coverage_cache_entries_stale;
        next.bootstrap_stats.coverage_cache_entries_ignored += stats.coverage_cache_entries_ignored;
        next.bootstrap_stats.coverage_cache_invalidations += stats.coverage_cache_invalidations;
        Ok(next)
    }

    pub fn dynamic_filters_enabled(&self) -> bool {
        self.dynamic_filters_enabled
    }

    #[inline]
    pub fn pruning(&self) -> &PruningMetadata {
        &self.pruning
    }

    pub(crate) fn nested_schema_for_column(&self, column_id: u32) -> Option<&NestedSchemaNodeV1> {
        self.pruning
            .nested_schemas
            .iter()
            .find_map(|section| section.entry(self.table.table_id, column_id))
            .map(|entry| &entry.root)
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

    pub(crate) fn reject_table_scan_page_feature_use(
        &self,
        segment: &TableSegmentIndexEntryV1,
        page: &cove_core::page::ColumnPageIndexEntryV1,
    ) -> Result<(), CoveError> {
        let section_id = self.segment_data_section_id(segment)?;
        let request = FeatureUseRequestV2::new()
            .with_profile(PrimaryProfile::TableScan as u8)
            .with_operation(OperationKindV2::OrdinaryTableScan)
            .with_cove_t_column_page(section_id, page.column_id, page.morsel_id);
        self.file(0)?.reject_feature_use(&request)
    }

    fn segment_data_section_id(
        &self,
        segment: &TableSegmentIndexEntryV1,
    ) -> Result<u32, CoveError> {
        let segment_end = segment
            .offset
            .checked_add(segment.length)
            .ok_or(CoveError::ArithOverflow)?;
        for section in &self.mounted.footer.sections {
            if section.section_kind != SectionKind::TableSegmentData as u16 {
                continue;
            }
            let section_end = section.end_offset()?;
            if segment.offset >= section.offset && segment_end <= section_end {
                return Ok(section.section_id);
            }
        }
        Err(CoveError::BadSection(
            "table segment payload is not covered by a TABLE_SEGMENT_DATA section".into(),
        ))
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
        Self::new_with_visibility_and_feature_scope(
            identity,
            file_bytes,
            mounted,
            table,
            segments,
            pruning,
            None,
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
        Self::new_with_visibility_and_feature_scope(
            identity, file_bytes, mounted, table, segments, pruning, None, visibility, flags,
        )
    }

    pub fn new_with_visibility_and_feature_scope(
        identity: FileIdentity,
        file_bytes: Option<Arc<Vec<u8>>>,
        mounted: Arc<MountedCoveFile>,
        table: Arc<TableEntry>,
        segments: Arc<Vec<TableSegmentIndexEntryV1>>,
        pruning: PruningMetadata,
        feature_scope_table: Option<FeatureScopeTable>,
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
            layout: LayoutPlanningMetadataV2::default(),
            coverage_cache: CoverageCacheMetadata::disabled(),
            feature_scope_table,
            #[cfg(feature = "covi")]
            covi: None,
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

    pub fn layout(&self) -> &LayoutPlanningMetadataV2 {
        &self.layout
    }

    pub fn coverage_cache(&self) -> &CoverageCacheMetadata {
        &self.coverage_cache
    }

    #[cfg(feature = "covm")]
    pub(crate) fn with_layout(mut self, layout: LayoutPlanningMetadataV2) -> Self {
        self.layout = layout;
        self
    }

    #[cfg(feature = "covm")]
    pub(crate) fn with_coverage_cache(mut self, coverage_cache: CoverageCacheMetadata) -> Self {
        self.coverage_cache = coverage_cache;
        self
    }

    #[cfg(all(feature = "covi", feature = "covm"))]
    pub(crate) fn with_covi(mut self, covi: Option<Arc<ValidatedCoviArtifactV2>>) -> Self {
        self.covi = covi;
        self
    }

    pub fn visibility(&self) -> &RowVisibility {
        &self.visibility
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }

    pub(crate) fn reject_feature_use(
        &self,
        request: &FeatureUseRequestV2,
    ) -> Result<(), CoveError> {
        if let Some(scope_table) = self.feature_scope_table.as_ref() {
            scope_table.reject_unknowns_for_request(request)?;
        }
        Ok(())
    }
}

fn cluster_for_range(
    directory: &PageClusterDirectoryV2,
    segment_id: u32,
    morsel_id: u32,
    range_start: u64,
    range_end: u64,
) -> Option<&PageClusterEntryV2> {
    directory.entries.iter().find(|cluster| {
        cluster.segment_id == segment_id
            && morsel_id >= cluster.first_morsel_id
            && morsel_id < cluster.first_morsel_id.saturating_add(cluster.morsel_count)
            && range_start >= cluster.offset
            && range_end <= cluster.offset.saturating_add(cluster.length)
    })
}

pub(crate) fn ordinary_table_scan_feature_use_request(footer: &CoveFooter) -> FeatureUseRequestV2 {
    let mut request = FeatureUseRequestV2::new()
        .with_profile(PrimaryProfile::TableScan as u8)
        .with_operation(OperationKindV2::OrdinaryTableScan);
    for section in &footer.sections {
        let Some(kind) = SectionKind::from_u16(section.section_kind) else {
            continue;
        };
        if matches!(
            kind,
            SectionKind::TableCatalog
                | SectionKind::TableSegmentIndex
                | SectionKind::TableSegmentData
                | SectionKind::FileDictionaryIndex
                | SectionKind::FileDictionaryPayload
        ) {
            request.needed_section_ids.insert(section.section_id);
        }
    }
    request
}
