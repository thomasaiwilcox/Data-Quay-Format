use std::sync::Arc;

use cove_arrow::arrow::ArrowStringValidationPolicy;
use cove_core::{
    mount::{mount_cove_file, EngineMetadata, MountOptions, OutputRepresentation},
    postscript::CovePostscriptV1,
};

use super::{
    pruning::PlanningCache,
    schema::{schema_for_table, validate_schema_compatible_files},
    segment::{mounted_from_metadata, parse_segment_index, validate_table_segments},
    *,
};

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
        let pruning = pruning_from_bytes(&bytes, &mounted);
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
            bootstrap_stats: single_file_bootstrap_stats(),
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
            bootstrap_stats: single_file_bootstrap_stats(),
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
        column_domains: Arc::new(mounted.column_domains.clone()),
        zone_stats: Arc::new(mounted.zone_stats.clone()),
        exact_sets: Arc::new(parse_exact_sets_from_sections(bytes, &mounted.footer)),
        blooms: Arc::new(parse_blooms_from_sections(bytes, &mounted.footer)),
        lookups: Arc::new(parse_lookups_from_sections(bytes, &mounted.footer)),
        inverted: Arc::new(parse_inverted_from_sections(bytes, &mounted.footer)),
        aggregates: Arc::new(parse_aggregates_from_sections(bytes, &mounted.footer)),
        composites: Arc::new(parse_composites_from_sections(bytes, &mounted.footer)),
        topn: Arc::new(parse_topn_from_sections(bytes, &mounted.footer)),
    }
}

fn single_file_bootstrap_stats() -> DatasetBootstrapStats {
    DatasetBootstrapStats {
        files_considered: 1,
        files_validated: 1,
        ..DatasetBootstrapStats::default()
    }
}
