use std::sync::Arc;

use cove_arrow::arrow::ArrowStringValidationPolicy;
use cove_core::{
    compression::section_payload_from_raw,
    constants::SectionKind,
    feature_scope::FeatureScopeTable,
    mount::{mount_cove_file, EngineMetadata, MountOptions, OutputRepresentation},
    postscript::CovePostscriptV1,
    reader,
};
use cove_layout::{
    validate_fast_metadata_authority, validate_page_cluster_authority, FastMetadataIndexV2,
    LayoutPlanV2, PageClusterDirectoryV2, ScanSplitIndexV2, ValidatedLayoutPlanV2,
    ValidatedScanSplitIndexV2, ValidatedZeroCopyBufferMapV2, ZeroCopyBufferMapV2,
};

use super::{
    pruning::PlanningCache,
    schema::{schema_for_table, validate_schema_compatible_files},
    segment::{mounted_from_metadata, parse_segment_index, validate_table_segments},
    *,
};
use crate::options::{select_table, CoveTableOptions, CoveTableSelection};

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

    /// Build single-file state from already-loaded bytes and full table
    /// registration options, including optional multi-table selection.
    pub fn from_bytes_with_table_options(
        source: impl Into<Arc<str>>,
        bytes: Vec<u8>,
        options: CoveTableOptions,
    ) -> Result<Self, CoveError> {
        Self::from_bytes_with_selected_table(
            source,
            bytes,
            options.arrow_export_options(),
            options.execution_code_policy(),
            options.page_payload_validation_policy(),
            options.local_file_read_policy(),
            options.target_morsels_per_partition(),
            options.range_coalescing(),
            options.dynamic_filters_enabled(),
            options.table_selection().cloned(),
        )
    }

    /// Build single-file state from already-loaded bytes and explicit Arrow
    /// export options. This compatibility entrypoint keeps the original
    /// single-table behavior; use [`Self::from_bytes_with_table_options`] for
    /// selected-table reads from multi-table files.
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
        Self::from_bytes_with_selected_table(
            source,
            bytes,
            arrow_export_options,
            execution_code_policy,
            page_payload_validation_policy,
            local_file_read_policy,
            target_morsels_per_partition,
            range_coalescing,
            dynamic_filters_enabled,
            None,
        )
    }

    fn from_bytes_with_selected_table(
        source: impl Into<Arc<str>>,
        bytes: Vec<u8>,
        arrow_export_options: ArrowExportOptions,
        execution_code_policy: ExecutionCodePolicy,
        page_payload_validation_policy: PagePayloadValidationPolicy,
        local_file_read_policy: LocalFileReadPolicy,
        target_morsels_per_partition: usize,
        range_coalescing: RangeCoalescingOptions,
        dynamic_filters_enabled: bool,
        table_selection: Option<CoveTableSelection>,
    ) -> Result<Self, CoveError> {
        let source = source.into();
        let postscript = CovePostscriptV1::parse_from_tail(&bytes)?;
        let validated = reader::validate_bytes(&bytes)?;
        let feature_scope_table = reader::feature_scope_table_for(&bytes, &validated)?;
        feature_scope_table.reject_unknowns_for_request(
            &ordinary_table_scan_feature_use_request(&validated.footer),
        )?;
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
        let table = select_table(table_catalog, table_selection.as_ref())?;
        let schema = Arc::new(schema_for_table(
            &table,
            mounted.dictionary.is_some(),
            mounted.nested_schemas.as_slice(),
            arrow_export_options,
        )?);
        let segment_index = parse_segment_index(&bytes, &mounted)?;
        let segments = segment_index
            .entries
            .into_iter()
            .filter(|segment| segment.table_id == table.table_id)
            .collect::<Vec<_>>();
        validate_table_segments(&table, &segments)?;
        let pruning = pruning_from_bytes(&bytes, &mounted);
        let layout = layout_from_bytes(&bytes, &mounted, &table, &segments);
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
            layout: layout.clone(),
            coverage_cache: CoverageCacheMetadata::disabled(),
            feature_scope_table: Some(feature_scope_table.clone()),
            #[cfg(feature = "covi")]
            covi: None,
            visibility: RowVisibility::All,
            flags: 0,
        };
        let file_table = FileTable::from_files(std::slice::from_ref(&file))?;
        let planning_cache = PlanningCache::build(table.as_ref(), &pruning);

        let mut bootstrap_stats = single_file_bootstrap_stats();
        bootstrap_stats.add_layout_metadata(&layout);

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
            layout,
            planning_cache,
            coverage_cache: CoverageCacheMetadata::disabled(),
            file_table,
            files: Arc::new(vec![file]),
            utf8_proof_cache: Arc::new(Utf8ProofCache::default()),
            bootstrap_stats,
        })
    }

    pub fn from_metadata_with_options(
        source: impl Into<Arc<str>>,
        file_len: u64,
        footer_crc32c: u32,
        header: cove_core::header::CoveHeaderV1,
        footer: cove_core::footer::CoveFooter,
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
        Self::from_metadata_with_options_and_feature_scope(
            source,
            file_len,
            footer_crc32c,
            header,
            footer,
            table,
            dictionary,
            engine_metadata,
            segments,
            pruning,
            LayoutPlanningMetadataV2::default(),
            None,
            arrow_export_options,
            execution_code_policy,
            page_payload_validation_policy,
            local_file_read_policy,
            target_morsels_per_partition,
            range_coalescing,
            dynamic_filters_enabled,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_metadata_with_options_and_feature_scope(
        source: impl Into<Arc<str>>,
        file_len: u64,
        footer_crc32c: u32,
        header: cove_core::header::CoveHeaderV1,
        footer: cove_core::footer::CoveFooter,
        table: TableEntry,
        dictionary: Option<cove_core::dictionary::FileDictionary>,
        engine_metadata: EngineMetadata,
        segments: Vec<TableSegmentIndexEntryV1>,
        pruning: PruningMetadata,
        layout: LayoutPlanningMetadataV2,
        feature_scope_table: Option<FeatureScopeTable>,
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
            pruning.nested_schemas.as_slice(),
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
            layout: layout.clone(),
            coverage_cache: CoverageCacheMetadata::disabled(),
            feature_scope_table,
            #[cfg(feature = "covi")]
            covi: None,
            visibility: RowVisibility::All,
            flags: 0,
        };
        let file_table = FileTable::from_files(std::slice::from_ref(&file))?;
        let planning_cache = PlanningCache::build(table.as_ref(), &pruning);

        let mut bootstrap_stats = single_file_bootstrap_stats();
        bootstrap_stats.add_layout_metadata(&layout);

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
            layout,
            planning_cache,
            coverage_cache: CoverageCacheMetadata::disabled(),
            file_table,
            files: Arc::new(vec![file]),
            utf8_proof_cache: Arc::new(Utf8ProofCache::default()),
            bootstrap_stats,
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
        let primary = &files[0];
        let schema = Arc::new(schema_for_table(
            &logical_table,
            files.iter().any(|file| file.mounted.dictionary.is_some()),
            primary.pruning.nested_schemas.as_slice(),
            arrow_export_options,
        )?);
        let file_table = FileTable::from_files(&files)?;
        bootstrap_stats.files_considered = bootstrap_stats.files_considered.max(files.len());
        let pruning = primary.pruning.clone();
        let layout = primary.layout.clone();
        let coverage_cache = primary.coverage_cache.clone();
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
            layout,
            planning_cache,
            coverage_cache,
            file_table,
            files: Arc::new(files),
            utf8_proof_cache: Arc::new(Utf8ProofCache::default()),
            bootstrap_stats,
        })
    }

    pub fn single_file_view(&self, file_ordinal: usize) -> Result<Self, CoveError> {
        let file = self.file(file_ordinal)?.clone();
        let file_table = FileTable::from_files(std::slice::from_ref(&file))?;
        let planning_cache = PlanningCache::build(file.table.as_ref(), &file.pruning);
        let schema = Arc::new(schema_for_table(
            file.table.as_ref(),
            file.mounted.dictionary.is_some(),
            file.pruning.nested_schemas.as_slice(),
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
            layout: file.layout.clone(),
            planning_cache,
            coverage_cache: file.coverage_cache.clone(),
            file_table,
            files: Arc::new(vec![file]),
            utf8_proof_cache: Arc::clone(&self.utf8_proof_cache),
            bootstrap_stats: single_file_bootstrap_stats(),
        })
    }
}

impl FileTable {
    pub(super) fn from_files(files: &[FileMetadata]) -> Result<Self, CoveError> {
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

fn pruning_from_bytes(bytes: &[u8], mounted: &MountedCoveFile) -> PruningMetadata {
    PruningMetadata {
        nested_schemas: Arc::new(mounted.nested_schemas.clone()),
        codec_descriptors: Arc::new(parse_codec_descriptors_from_sections(
            bytes,
            &mounted.footer,
        )),
        column_domains: Arc::new(mounted.column_domains.clone()),
        zone_stats: Arc::new(mounted.zone_stats.clone()),
        exact_sets: Arc::new(parse_exact_sets_from_sections(bytes, &mounted.footer)),
        blooms: Arc::new(parse_blooms_from_sections(bytes, &mounted.footer)),
        lookups: Arc::new(parse_lookups_from_sections(bytes, &mounted.footer)),
        inverted: Arc::new(parse_inverted_from_sections(bytes, &mounted.footer)),
        aggregates: Arc::new(parse_aggregates_from_sections(bytes, &mounted.footer)),
        composites: Arc::new(parse_composites_from_sections(bytes, &mounted.footer)),
        topn: Arc::new(parse_topn_from_sections(bytes, &mounted.footer)),
        coverage_providers: Arc::new(parse_coverage_providers_from_sections(
            bytes,
            &mounted.footer,
        )),
        coverage_sets: Arc::new(parse_coverage_sets_from_sections(bytes, &mounted.footer)),
        coverage_proofs: Arc::new(parse_coverage_proofs_from_sections(bytes, &mounted.footer)),
        coverage_plan_candidates: Arc::new(parse_coverage_plan_candidates_from_sections(
            bytes,
            &mounted.footer,
        )),
        predicate_forms: Arc::new(parse_predicate_forms_from_sections(bytes, &mounted.footer)),
        predicate_forms_with_payloads: Arc::new(parse_predicate_forms_with_payloads_from_sections(
            bytes,
            &mounted.footer,
        )),
    }
}

fn layout_from_bytes(
    bytes: &[u8],
    mounted: &MountedCoveFile,
    table: &TableEntry,
    segments: &[TableSegmentIndexEntryV1],
) -> LayoutPlanningMetadataV2 {
    let mut layout = LayoutPlanningMetadataV2::default();

    layout.fast_metadata = mounted
        .footer
        .sections
        .iter()
        .find(|entry| entry.section_id == mounted.header.fast_metadata_section_id)
        .filter(|entry| entry.section_kind == SectionKind::FastMetadataIndex as u16)
        .and_then(|entry| section_payload_from_entry(bytes, entry).ok())
        .and_then(|payload| FastMetadataIndexV2::parse(&payload).ok())
        .and_then(|index| {
            validate_fast_metadata_authority(&index, &mounted.footer)
                .ok()
                .map(|_| Arc::new(index))
        });
    if layout.fast_metadata.is_some() {
        layout.record_loaded();
    } else if mounted.header.fast_metadata_section_id != 0 {
        layout.record_ignored();
    }

    for entry in mounted
        .footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::PageClusterDirectory as u16)
    {
        let Some(directory) = section_payload_from_entry(bytes, entry)
            .ok()
            .and_then(|payload| PageClusterDirectoryV2::parse(&payload).ok())
            .filter(|directory| {
                validate_page_cluster_authority(directory, &mounted.footer, table, segments).is_ok()
            })
        else {
            layout.record_ignored();
            continue;
        };
        if layout.page_clusters.is_none() {
            layout.page_clusters = Some(Arc::new(directory));
            layout.record_loaded();
        } else {
            layout.record_ignored();
        }
    }

    for entry in mounted
        .footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::ScanSplitIndex as u16)
    {
        let Some(index) = section_payload_from_entry(bytes, entry)
            .ok()
            .and_then(|payload| ScanSplitIndexV2::parse(&payload).ok())
            .and_then(|index| {
                ValidatedScanSplitIndexV2::validate(
                    index,
                    table,
                    segments,
                    layout.page_clusters.as_deref(),
                )
                .ok()
                .map(|validated| validated.index)
            })
        else {
            layout.record_ignored();
            continue;
        };
        if layout.scan_splits.is_none() {
            layout.scan_splits = Some(Arc::new(index));
            layout.record_loaded();
        } else {
            layout.record_ignored();
        }
    }

    let mut layout_plans = Vec::new();
    for entry in mounted
        .footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::LayoutPlan as u16)
    {
        match section_payload_from_entry(bytes, entry)
            .ok()
            .and_then(|payload| LayoutPlanV2::parse(&payload).ok())
            .and_then(|plan| {
                ValidatedLayoutPlanV2::validate(
                    plan,
                    &mounted.footer,
                    table,
                    segments,
                    layout.page_clusters.as_deref(),
                    layout.scan_splits.as_deref(),
                )
                .ok()
                .map(|validated| validated.plan)
            }) {
            Some(plan) => {
                layout_plans.push(plan);
                layout.record_loaded();
            }
            None => layout.record_ignored(),
        }
    }
    layout.layout_plans = Arc::new(layout_plans);

    let mut zero_copy_maps = Vec::new();
    for entry in mounted
        .footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::ZeroCopyBufferMap as u16)
    {
        match section_payload_from_entry(bytes, entry)
            .ok()
            .and_then(|payload| ZeroCopyBufferMapV2::parse(&payload).ok())
            .and_then(|map| {
                ValidatedZeroCopyBufferMapV2::validate(map, table, segments)
                    .ok()
                    .map(|validated| validated.map)
            }) {
            Some(map) => {
                zero_copy_maps.push(map);
                layout.record_loaded();
            }
            None => layout.record_ignored(),
        }
    }
    layout.zero_copy_maps = Arc::new(zero_copy_maps);

    layout
}

fn section_payload_from_entry(
    bytes: &[u8],
    entry: &cove_core::footer::CoveSectionEntryV1,
) -> Result<Vec<u8>, CoveError> {
    let start = usize::try_from(entry.offset).map_err(|_| CoveError::OffsetRange)?;
    let end = usize::try_from(entry.end_offset()?).map_err(|_| CoveError::OffsetRange)?;
    if end > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    Ok(section_payload_from_raw(
        &bytes[start..end],
        entry.length,
        entry.uncompressed_length,
        entry.compression,
        entry.crc32c,
    )?
    .into_owned())
}

fn single_file_bootstrap_stats() -> DatasetBootstrapStats {
    DatasetBootstrapStats {
        files_considered: 1,
        files_validated: 1,
        ..DatasetBootstrapStats::default()
    }
}
