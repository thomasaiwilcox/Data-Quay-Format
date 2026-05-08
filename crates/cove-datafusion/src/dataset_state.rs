//! Immutable dataset metadata state shared across scans.

mod pruning_sections;

use std::{collections::BTreeMap, sync::Arc};

use arrow_schema::{Schema, SchemaRef};
use cove_arrow::arrow::{
    arrow_data_type_for_column_export_options, ArrowExportOptions, ArrowFidelitySeverity,
    ArrowStringValidationPolicy,
};
use cove_core::{
    compression,
    constants::{CovePhysicalKind, SectionKind},
    domain::ColumnDomain,
    footer::CoveFooter,
    header::CoveHeaderV1,
    index::{
        aggregate::{AggregateEntry, AggregateSynopsis, SynopsisAccuracy, SynopsisKind},
        bloom::BloomFilterIndex,
        composite::{CompositeIndex, CompositeTransformKind},
        exact_set::ExactSetIndex,
        inverted::InvertedMorselIndex,
        lookup::LookupIndex,
        topn::TopNSummary,
    },
    mount::{
        build_reverse_lookup, mount_cove_file, EngineMetadata, MountOptions, MountedColumn,
        MountedCoveFile, MountedTable, OutputRepresentation, SidecarValidationStatus,
    },
    postscript::CovePostscriptV1,
    segment::{TableSegmentIndex, TableSegmentIndexEntryV1},
    table::TableEntry,
    zone_stats::{ZoneStatsEntry, ZoneStatsSection},
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

#[derive(Debug, Clone, Default)]
struct PlanningCache {
    column_by_id: BTreeMap<u32, usize>,
    zone_stat_by_key: BTreeMap<(u32, u32, u32), (usize, usize)>,
    column_domain_by_id: BTreeMap<u32, usize>,
    exact_set_by_id: BTreeMap<u32, usize>,
    bloom_by_id: BTreeMap<u32, usize>,
    lookup_by_id: BTreeMap<u32, usize>,
    inverted_by_id: BTreeMap<u32, usize>,
}

impl PlanningCache {
    fn build(table: &TableEntry, pruning: &PruningMetadata) -> Self {
        let mut cache = Self::default();
        for (index, column) in table.columns.iter().enumerate() {
            cache.column_by_id.entry(column.column_id).or_insert(index);
        }
        for (section_index, section) in pruning.zone_stats.iter().enumerate() {
            for (entry_index, entry) in section.entries.iter().enumerate() {
                if entry.table_id == table.table_id {
                    cache
                        .zone_stat_by_key
                        .entry((entry.segment_id, entry.morsel_id, entry.column_id))
                        .or_insert((section_index, entry_index));
                }
            }
        }
        for (index, domain) in pruning.column_domains.iter().enumerate() {
            if domain.header.table_or_object_id == table.table_id && domain.is_safe() {
                cache
                    .column_domain_by_id
                    .entry(domain.header.column_or_property_id)
                    .or_insert(index);
            }
        }
        for (index, exact_set) in pruning.exact_sets.iter().enumerate() {
            if exact_set.header.table_id == table.table_id {
                cache
                    .exact_set_by_id
                    .entry(exact_set.header.column_id)
                    .or_insert(index);
            }
        }
        for (index, bloom) in pruning.blooms.iter().enumerate() {
            if bloom.header.table_id == table.table_id {
                cache
                    .bloom_by_id
                    .entry(bloom.header.column_id)
                    .or_insert(index);
            }
        }
        for (index, lookup) in pruning.lookups.iter().enumerate() {
            if lookup.header.table_id == table.table_id {
                cache
                    .lookup_by_id
                    .entry(lookup.header.column_id)
                    .or_insert(index);
            }
        }
        for (index, inverted) in pruning.inverted.iter().enumerate() {
            if inverted.header.table_id == table.table_id {
                cache
                    .inverted_by_id
                    .entry(inverted.header.column_id)
                    .or_insert(index);
            }
        }
        cache
    }
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
    planning_cache: PlanningCache,
    file_table: FileTable,
    files: Arc<Vec<FileMetadata>>,
    utf8_proof_cache: Arc<Utf8ProofCache>,
    bootstrap_stats: DatasetBootstrapStats,
}

impl DatasetState {
    /// Build single-file state from already-loaded bytes.
    pub fn from_bytes(source: impl Into<Arc<str>>, bytes: Vec<u8>) -> Result<Self, CoveError> {
        Self::from_bytes_with_options(
            source,
            bytes,
            ArrowExportOptions {
                string_validation_policy: ArrowStringValidationPolicy::StrictOrCachedProof,
                ..ArrowExportOptions::default()
            },
            ExecutionCodePolicy::Opportunistic,
            PagePayloadValidationPolicy::Trusted,
            LocalFileReadPolicy::PositionedReads,
            128,
            RangeCoalescingOptions::default(),
            false,
        )
    }

    /// Build single-file state from already-loaded bytes and explicit Arrow
    /// export options.
    pub fn from_bytes_with_options(
        source: impl Into<Arc<str>>,
        bytes: Vec<u8>,
        arrow_export_options: ArrowExportOptions,
        execution_code_policy: ExecutionCodePolicy,
        page_payload_validation_policy: PagePayloadValidationPolicy,
        local_file_read_policy: LocalFileReadPolicy,
        target_morsels_per_partition: usize,
        range_coalescing: RangeCoalescingOptions,
        dynamic_filters_enabled: bool,
    ) -> Result<Self, CoveError> {
        let source = source.into();
        let postscript = CovePostscriptV1::parse_from_tail(&bytes)?;
        let mounted = mount_cove_file(
            &bytes,
            MountOptions {
                representation: OutputRepresentation::DecodeToValue,
                ..MountOptions::default()
            },
            None,
        )?;
        let file_len = u64::try_from(bytes.len()).map_err(|_| CoveError::ArithOverflow)?;
        let table_catalog = mounted.table_catalog.as_ref().ok_or_else(|| {
            CoveError::BadSchema(
                "COVE DataFusion native path requires a COVE-T table catalog".into(),
            )
        })?;
        if table_catalog.tables.len() != 1 {
            return Err(CoveError::BadSchema(format!(
                "COVE DataFusion native path supports exactly one table per file, found {}",
                table_catalog.tables.len()
            )));
        }
        let table = table_catalog.tables[0].clone();
        let schema = Arc::new(schema_for_table(
            &table,
            mounted.dictionary.is_some(),
            arrow_export_options,
        )?);
        let segment_index = parse_segment_index(&bytes, &mounted)?;
        let segments = segment_index
            .entries
            .into_iter()
            .filter(|segment| segment.table_id == table.table_id)
            .collect::<Vec<_>>();
        validate_table_segments(&table, &segments)?;
        let pruning = PruningMetadata {
            column_domains: Arc::new(mounted.column_domains.clone()),
            zone_stats: Arc::new(mounted.zone_stats.clone()),
            exact_sets: Arc::new(parse_exact_sets_from_sections(&bytes, &mounted.footer)),
            blooms: Arc::new(parse_blooms_from_sections(&bytes, &mounted.footer)),
            lookups: Arc::new(parse_lookups_from_sections(&bytes, &mounted.footer)),
            inverted: Arc::new(parse_inverted_from_sections(&bytes, &mounted.footer)),
            aggregates: Arc::new(parse_aggregates_from_sections(&bytes, &mounted.footer)),
            composites: Arc::new(parse_composites_from_sections(&bytes, &mounted.footer)),
            topn: Arc::new(parse_topn_from_sections(&bytes, &mounted.footer)),
        };
        let identity = FileIdentity {
            source,
            file_id: mounted.header.file_id,
            file_len,
            footer_crc32c: postscript.footer.crc32c,
        };
        let mounted = Arc::new(mounted);
        let table = Arc::new(table);
        let segments = Arc::new(segments);
        let file_bytes = Some(Arc::new(bytes));
        let file = FileMetadata {
            identity: identity.clone(),
            file_bytes: file_bytes.clone(),
            mounted: Arc::clone(&mounted),
            table: Arc::clone(&table),
            segments: Arc::clone(&segments),
            pruning: pruning.clone(),
            visibility: RowVisibility::All,
            flags: 0,
        };
        let file_table = FileTable::from_files(std::slice::from_ref(&file))?;
        let planning_cache = PlanningCache::build(table.as_ref(), &pruning);

        Ok(Self {
            identity,
            file_bytes,
            mounted,
            table,
            schema,
            segments,
            arrow_export_options,
            execution_code_policy,
            page_payload_validation_policy,
            local_file_read_policy,
            target_morsels_per_partition: target_morsels_per_partition.max(1),
            range_coalescing,
            dynamic_filters_enabled,
            pruning,
            planning_cache,
            file_table,
            files: Arc::new(vec![file]),
            utf8_proof_cache: Arc::new(Utf8ProofCache::default()),
            bootstrap_stats: DatasetBootstrapStats {
                files_considered: 1,
                files_validated: 1,
                ..DatasetBootstrapStats::default()
            },
        })
    }

    pub fn from_metadata_with_options(
        source: impl Into<Arc<str>>,
        file_len: u64,
        footer_crc32c: u32,
        header: CoveHeaderV1,
        footer: CoveFooter,
        table: TableEntry,
        dictionary: Option<cove_core::dictionary::FileDictionary>,
        engine_metadata: EngineMetadata,
        segments: Vec<TableSegmentIndexEntryV1>,
        pruning: PruningMetadata,
        arrow_export_options: ArrowExportOptions,
        execution_code_policy: ExecutionCodePolicy,
        page_payload_validation_policy: PagePayloadValidationPolicy,
        local_file_read_policy: LocalFileReadPolicy,
        target_morsels_per_partition: usize,
        range_coalescing: RangeCoalescingOptions,
        dynamic_filters_enabled: bool,
    ) -> Result<Self, CoveError> {
        let source = source.into();
        let schema = Arc::new(schema_for_table(
            &table,
            dictionary.is_some(),
            arrow_export_options,
        )?);
        validate_table_segments(&table, &segments)?;
        let mounted = mounted_from_metadata(
            header.clone(),
            footer,
            table.clone(),
            dictionary,
            engine_metadata,
        )?;
        let identity = FileIdentity {
            source,
            file_id: header.file_id,
            file_len,
            footer_crc32c,
        };
        let mounted = Arc::new(mounted);
        let table = Arc::new(table);
        let segments = Arc::new(segments);
        let file = FileMetadata {
            identity: identity.clone(),
            file_bytes: None,
            mounted: Arc::clone(&mounted),
            table: Arc::clone(&table),
            segments: Arc::clone(&segments),
            pruning: pruning.clone(),
            visibility: RowVisibility::All,
            flags: 0,
        };
        let file_table = FileTable::from_files(std::slice::from_ref(&file))?;
        let planning_cache = PlanningCache::build(table.as_ref(), &pruning);

        Ok(Self {
            identity,
            file_bytes: None,
            mounted,
            table,
            schema,
            segments,
            arrow_export_options,
            execution_code_policy,
            page_payload_validation_policy,
            local_file_read_policy,
            target_morsels_per_partition: target_morsels_per_partition.max(1),
            range_coalescing,
            dynamic_filters_enabled,
            pruning,
            planning_cache,
            file_table,
            files: Arc::new(vec![file]),
            utf8_proof_cache: Arc::new(Utf8ProofCache::default()),
            bootstrap_stats: DatasetBootstrapStats {
                files_considered: 1,
                files_validated: 1,
                ..DatasetBootstrapStats::default()
            },
        })
    }

    pub fn from_file_metadata_with_options(
        source: impl Into<Arc<str>>,
        files: Vec<FileMetadata>,
        mut bootstrap_stats: DatasetBootstrapStats,
        arrow_export_options: ArrowExportOptions,
        execution_code_policy: ExecutionCodePolicy,
        page_payload_validation_policy: PagePayloadValidationPolicy,
        local_file_read_policy: LocalFileReadPolicy,
        target_morsels_per_partition: usize,
        range_coalescing: RangeCoalescingOptions,
        dynamic_filters_enabled: bool,
    ) -> Result<Self, CoveError> {
        let mut files = files;
        if files.is_empty() {
            return Err(CoveError::BadSchema(
                "COVE DataFusion dataset requires at least one file".into(),
            ));
        }
        for file in &mut files {
            file.visibility = file.visibility.clone().normalized(file.table.row_count)?;
        }
        validate_schema_compatible_files(&files)?;
        let row_count = files.iter().try_fold(0u64, |acc, file| {
            acc.checked_add(file.table.row_count)
                .ok_or(CoveError::ArithOverflow)
        })?;
        let mut logical_table = files[0].table.as_ref().clone();
        logical_table.row_count = row_count;
        let schema = Arc::new(schema_for_table(
            &logical_table,
            files.iter().any(|file| file.mounted.dictionary.is_some()),
            arrow_export_options,
        )?);
        let file_table = FileTable::from_files(&files)?;
        bootstrap_stats.files_considered = bootstrap_stats.files_considered.max(files.len());
        let primary = &files[0];
        let pruning = primary.pruning.clone();
        let planning_cache = PlanningCache::build(&logical_table, &pruning);
        Ok(Self {
            identity: FileIdentity {
                source: source.into(),
                file_id: primary.identity.file_id,
                file_len: primary.identity.file_len,
                footer_crc32c: primary.identity.footer_crc32c,
            },
            file_bytes: primary.file_bytes.clone(),
            mounted: Arc::clone(&primary.mounted),
            table: Arc::new(logical_table),
            schema,
            segments: Arc::clone(&primary.segments),
            arrow_export_options,
            execution_code_policy,
            page_payload_validation_policy,
            local_file_read_policy,
            target_morsels_per_partition: target_morsels_per_partition.max(1),
            range_coalescing,
            dynamic_filters_enabled,
            pruning,
            planning_cache,
            file_table,
            files: Arc::new(files),
            utf8_proof_cache: Arc::new(Utf8ProofCache::default()),
            bootstrap_stats,
        })
    }

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

    pub fn single_file_view(&self, file_ordinal: usize) -> Result<Self, CoveError> {
        let file = self.file(file_ordinal)?.clone();
        let file_table = FileTable::from_files(std::slice::from_ref(&file))?;
        let planning_cache = PlanningCache::build(file.table.as_ref(), &file.pruning);
        let schema = Arc::new(schema_for_table(
            file.table.as_ref(),
            file.mounted.dictionary.is_some(),
            self.arrow_export_options,
        )?);
        Ok(Self {
            identity: file.identity.clone(),
            file_bytes: file.file_bytes.clone(),
            mounted: Arc::clone(&file.mounted),
            table: Arc::clone(&file.table),
            schema,
            segments: Arc::clone(&file.segments),
            arrow_export_options: self.arrow_export_options,
            execution_code_policy: self.execution_code_policy,
            page_payload_validation_policy: self.page_payload_validation_policy,
            local_file_read_policy: self.local_file_read_policy,
            target_morsels_per_partition: self.target_morsels_per_partition,
            range_coalescing: self.range_coalescing,
            dynamic_filters_enabled: self.dynamic_filters_enabled,
            pruning: file.pruning.clone(),
            planning_cache,
            file_table,
            files: Arc::new(vec![file]),
            utf8_proof_cache: Arc::clone(&self.utf8_proof_cache),
            bootstrap_stats: DatasetBootstrapStats {
                files_considered: 1,
                files_validated: 1,
                ..DatasetBootstrapStats::default()
            },
        })
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

    pub fn zone_stats_for(
        &self,
        segment_id: u32,
        morsel_id: u32,
        column_id: u32,
    ) -> Option<&ZoneStatsEntry> {
        let (section_index, entry_index) = self
            .planning_cache
            .zone_stat_by_key
            .get(&(segment_id, morsel_id, column_id))
            .copied()?;
        self.pruning
            .zone_stats
            .get(section_index)?
            .entries
            .get(entry_index)
    }

    pub fn segment_zone_stats_for(
        &self,
        segment_id: u32,
        column_id: u32,
    ) -> Option<&ZoneStatsEntry> {
        self.zone_stats_for(segment_id, u32::MAX, column_id)
    }

    pub fn column_domain_for(&self, column_id: u32) -> Option<&ColumnDomain> {
        self.planning_cache
            .column_domain_by_id
            .get(&column_id)
            .and_then(|index| self.pruning.column_domains.get(*index))
    }

    pub fn exact_set_for(&self, column_id: u32) -> Option<&ExactSetIndex> {
        self.planning_cache
            .exact_set_by_id
            .get(&column_id)
            .and_then(|index| self.pruning.exact_sets.get(*index))
    }

    pub fn bloom_for(&self, column_id: u32) -> Option<&BloomFilterIndex> {
        self.planning_cache
            .bloom_by_id
            .get(&column_id)
            .and_then(|index| self.pruning.blooms.get(*index))
    }

    pub fn lookup_for(&self, column_id: u32) -> Option<&LookupIndex> {
        self.planning_cache
            .lookup_by_id
            .get(&column_id)
            .and_then(|index| self.pruning.lookups.get(*index))
    }

    pub fn inverted_for(&self, column_id: u32) -> Option<&InvertedMorselIndex> {
        self.planning_cache
            .inverted_by_id
            .get(&column_id)
            .and_then(|index| self.pruning.inverted.get(*index))
    }

    pub fn aggregate_entries_for(&self, column_id: u32) -> Vec<&AggregateEntry> {
        self.pruning
            .aggregates
            .iter()
            .flat_map(|synopsis| synopsis.entries.iter())
            .filter(|entry| entry.table_id == self.table.table_id && entry.column_id == column_id)
            .collect()
    }

    pub fn composite_indexes(&self) -> impl Iterator<Item = &CompositeIndex> {
        self.pruning.composites.iter().filter(|index| {
            index.header.table_id == self.table.table_id
                && index.header.transform_kind == CompositeTransformKind::Tuple
        })
    }

    pub fn topn_for(&self, column_id: u32) -> Vec<&TopNSummary> {
        self.pruning
            .topn
            .iter()
            .filter(|summary| {
                summary.table_id == self.table.table_id && summary.column_id == column_id
            })
            .collect()
    }

    pub fn exact_global_count(
        &self,
        column_index: Option<usize>,
    ) -> Result<Option<u64>, CoveError> {
        let mut total = 0u64;
        for file in self.files() {
            let visible = file.visibility().visible_count(file.table().row_count)?;
            if visible == 0 {
                continue;
            }
            match column_index {
                None => {
                    total = total.checked_add(visible).ok_or(CoveError::ArithOverflow)?;
                }
                Some(index) => {
                    let column = file.table().columns.get(index).ok_or_else(|| {
                        CoveError::BadSchema(format!(
                            "COUNT column index {index} is out of bounds for {} columns",
                            file.table().columns.len()
                        ))
                    })?;
                    if column.physical == CovePhysicalKind::FileCode && file_has_redaction(file) {
                        return Ok(None);
                    }
                    if !column.nullable {
                        total = total.checked_add(visible).ok_or(CoveError::ArithOverflow)?;
                        continue;
                    }
                    if !file.visibility().is_all() {
                        return Ok(None);
                    }
                    let Some(file_count) = exact_count_for_file_column(file, column.column_id)?
                    else {
                        return Ok(None);
                    };
                    total = total
                        .checked_add(file_count)
                        .ok_or(CoveError::ArithOverflow)?;
                }
            }
        }
        Ok(Some(total))
    }

    pub fn exact_visible_row_count(&self) -> Result<u64, CoveError> {
        self.exact_global_count(None)?
            .ok_or(CoveError::ArithOverflow)
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

impl FileTable {
    fn from_files(files: &[FileMetadata]) -> Result<Self, CoveError> {
        let mut table = Self {
            source: Vec::with_capacity(files.len()),
            file_id: Vec::with_capacity(files.len()),
            file_len: Vec::with_capacity(files.len()),
            footer_crc32c: Vec::with_capacity(files.len()),
            row_count: Vec::with_capacity(files.len()),
            segment_count: Vec::with_capacity(files.len()),
            flags: Vec::with_capacity(files.len()),
        };
        for file in files {
            table.source.push(Arc::clone(&file.identity.source));
            table.file_id.push(file.identity.file_id);
            table.file_len.push(file.identity.file_len);
            table.footer_crc32c.push(file.identity.footer_crc32c);
            table.row_count.push(file.table.row_count);
            table
                .segment_count
                .push(u32::try_from(file.segments.len()).map_err(|_| CoveError::ArithOverflow)?);
            table.flags.push(file.flags);
        }
        Ok(table)
    }
}

fn file_has_redaction(file: &FileMetadata) -> bool {
    file.mounted()
        .reverse_lookup
        .as_ref()
        .map(|lookup| !lookup.redacted_filecodes.is_empty())
        .unwrap_or(false)
}

fn exact_count_for_file_column(
    file: &FileMetadata,
    column_id: u32,
) -> Result<Option<u64>, CoveError> {
    let entries = file
        .pruning()
        .aggregates
        .iter()
        .flat_map(|synopsis| synopsis.entries.iter())
        .filter(|entry| {
            entry.table_id == file.table().table_id
                && entry.column_id == column_id
                && entry.synopsis_kind == SynopsisKind::Count
                && entry.accuracy == SynopsisAccuracy::Exact
        })
        .collect::<Vec<_>>();

    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.segment_id == u32::MAX && entry.morsel_id == u32::MAX)
    {
        return entry_count(entry).map(Some);
    }

    let segment_entries = entries
        .iter()
        .copied()
        .filter(|entry| entry.segment_id != u32::MAX && entry.morsel_id == u32::MAX)
        .collect::<Vec<_>>();
    if let Some(count) = exact_count_from_entries(file.table().row_count, &segment_entries)? {
        return Ok(Some(count));
    }

    let morsel_entries = entries
        .iter()
        .copied()
        .filter(|entry| entry.segment_id != u32::MAX && entry.morsel_id != u32::MAX)
        .collect::<Vec<_>>();
    exact_count_from_entries(file.table().row_count, &morsel_entries)
}

fn exact_count_from_entries(
    expected_rows: u64,
    entries: &[&AggregateEntry],
) -> Result<Option<u64>, CoveError> {
    if entries.is_empty() {
        return Ok(None);
    }
    let mut rows = 0u64;
    let mut count = 0u64;
    for entry in entries {
        rows = rows
            .checked_add(u64::from(entry.row_count))
            .ok_or(CoveError::ArithOverflow)?;
        count = count
            .checked_add(entry_count(entry)?)
            .ok_or(CoveError::ArithOverflow)?;
    }
    if rows == expected_rows {
        Ok(Some(count))
    } else {
        Ok(None)
    }
}

fn entry_count(entry: &AggregateEntry) -> Result<u64, CoveError> {
    entry
        .row_count
        .checked_sub(entry.null_count)
        .map(u64::from)
        .ok_or(CoveError::BadIndex)
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

    pub fn has_redaction(&self) -> bool {
        file_has_redaction(self)
    }
}

fn mounted_from_metadata(
    header: CoveHeaderV1,
    footer: CoveFooter,
    table: TableEntry,
    dictionary: Option<cove_core::dictionary::FileDictionary>,
    engine_metadata: EngineMetadata,
) -> Result<MountedCoveFile, CoveError> {
    let representation = OutputRepresentation::DecodeToValue;
    let reverse_lookup = dictionary.as_ref().map(build_reverse_lookup).transpose()?;
    let mounted_table = MountedTable {
        table_id: table.table_id,
        namespace: table.namespace.clone(),
        name: table.name.clone(),
        row_count: table.row_count,
        columns: table
            .columns
            .iter()
            .map(|column| MountedColumn {
                column_id: column.column_id,
                name: column.name.clone(),
                logical: column.logical,
                physical: column.physical,
                nullable: column.nullable,
                representation,
            })
            .collect(),
    };
    Ok(MountedCoveFile {
        header,
        footer,
        table_catalog: Some(cove_core::table::TableCatalog {
            flags: 0,
            tables: vec![table],
        }),
        tables: vec![mounted_table],
        dictionary,
        representation,
        reverse_lookup,
        execution_code_map: None,
        execution_descriptors: engine_metadata.execution_descriptors.clone(),
        execution_scopes: engine_metadata.execution_scopes.clone(),
        code_spaces: engine_metadata.code_spaces.clone(),
        engine_profile_registries: engine_metadata.engine_profile_registries.clone(),
        engine_mount_policies: engine_metadata.engine_mount_policies.clone(),
        engine_metadata,
        column_domains: Vec::new(),
        zone_stats: Vec::new(),
        scan_indexes: Vec::new(),
        ignored_optional_sections: Vec::new(),
        covx_status: SidecarValidationStatus::NotProvided,
        covm_status: SidecarValidationStatus::NotProvided,
    })
}

fn validate_schema_compatible_files(files: &[FileMetadata]) -> Result<(), CoveError> {
    let first = &files[0].table;
    for (ordinal, file) in files.iter().enumerate().skip(1) {
        let candidate = &file.table;
        if candidate.columns.len() != first.columns.len() {
            return Err(CoveError::BadSchema(format!(
                "COVE manifest schema mismatch for file {ordinal}: expected {} columns, found {}",
                first.columns.len(),
                candidate.columns.len()
            )));
        }
        for (column_index, (expected, actual)) in first
            .columns
            .iter()
            .zip(candidate.columns.iter())
            .enumerate()
        {
            if expected.name != actual.name
                || expected.logical != actual.logical
                || expected.physical != actual.physical
                || expected.nullable != actual.nullable
            {
                return Err(CoveError::BadSchema(format!(
                    "COVE manifest schema mismatch for file {ordinal}, column {column_index}: expected {} {:?}/{:?} nullable={}, found {} {:?}/{:?} nullable={}",
                    expected.name,
                    expected.logical,
                    expected.physical,
                    expected.nullable,
                    actual.name,
                    actual.logical,
                    actual.physical,
                    actual.nullable
                )));
            }
        }
    }
    Ok(())
}

fn schema_for_table(
    table: &TableEntry,
    has_file_dictionary: bool,
    arrow_export_options: ArrowExportOptions,
) -> Result<Schema, CoveError> {
    let fields = table
        .columns
        .iter()
        .map(|column| {
            let result = arrow_data_type_for_column_export_options(
                column.logical,
                column.physical,
                has_file_dictionary,
                arrow_export_options,
            )?;
            if result
                .report
                .issues
                .iter()
                .any(|issue| issue.severity == ArrowFidelitySeverity::Unsupported)
            {
                return Err(CoveError::UnsupportedEncoding(format!(
                    "Arrow schema export for {:?} is unsupported",
                    column.logical
                )));
            }
            if result.report.has_lossy_or_unsupported() {
                return Err(CoveError::UnsupportedEncoding(format!(
                    "Arrow schema export for {:?} requires explicit fidelity reporting",
                    column.logical
                )));
            }
            Ok(arrow_schema::Field::new(
                column.name.clone(),
                result.value,
                column.nullable,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Schema::new(fields))
}

fn parse_segment_index(
    bytes: &[u8],
    mounted: &MountedCoveFile,
) -> Result<TableSegmentIndex, CoveError> {
    let mut indexes = mounted
        .footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::TableSegmentIndex as u16);
    let Some(entry) = indexes.next() else {
        return Ok(TableSegmentIndex::default());
    };
    if indexes.next().is_some() {
        return Err(CoveError::SegmentCorrupt);
    }
    let payload = compression::section_payload(bytes, entry)?;
    TableSegmentIndex::parse(&payload)
}

fn validate_table_segments(
    table: &TableEntry,
    segments: &[TableSegmentIndexEntryV1],
) -> Result<(), CoveError> {
    let rows = segments.iter().try_fold(0u64, |acc, segment| {
        if segment.column_count != table.columns.len() as u32 {
            return Err(CoveError::SegmentCorrupt);
        }
        acc.checked_add(segment.row_count as u64)
            .ok_or(CoveError::ArithOverflow)
    })?;
    if rows != table.row_count {
        return Err(CoveError::SegmentCorrupt);
    }
    Ok(())
}
