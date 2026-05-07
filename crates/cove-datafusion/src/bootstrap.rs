//! Footer and dataset bootstrap helpers for COVE-backed DataFusion datasets.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

#[cfg(feature = "covm")]
use crate::options::{CovxDiscovery, SidecarDigestPolicy};
use crate::{
    dataset_state::{DatasetBootstrapStats, DatasetState, FileMetadata, PruningMetadata},
    options::CoveTableOptions,
    overlay::{CoveOverlaySnapshot, OverlayFileDigest, OverlayFileIdentity},
};
#[cfg(feature = "covm")]
use cove_core::artifact::{
    covm::{CovmFile, CovmFileEntryV1},
    covx::{CovxFile, CovxReferencedFileV1},
};
use cove_core::{
    checksum,
    compression::section_payload_from_raw,
    constants::{DigestAlgorithm, SectionKind},
    dictionary::FileDictionary,
    digest::compute_digest,
    domain::ColumnDomain,
    footer::{CoveFooter, CoveSectionEntryV1},
    header::{CoveHeaderV1, HEADER_SIZE},
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    mount::EngineMetadata,
    postscript::{CovePostscriptV1, POSTSCRIPT_TOTAL_SIZE},
    profile::cove_e::{
        CodeSpaceDescriptorV1, EngineMountPolicyV1, EngineProfileRegistry,
        ExecutionCodeDescriptorV1, ExecutionScopeDescriptorV1,
    },
    segment::TableSegmentIndex,
    table::TableCatalog,
    zone_stats::ZoneStatsSection,
    CoveError,
};

use crate::range_reader::{CoveRangeReader, LocalFileRangeReader, RangeReadKind};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CoveMetadataCacheKey {
    pub source: Arc<str>,
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: u32,
}

#[derive(Debug, Default)]
pub struct CoveMetadataCache {
    entries: Mutex<HashMap<CoveMetadataCacheKey, Arc<DatasetState>>>,
}

impl CoveMetadataCache {
    fn entries(&self) -> MutexGuard<'_, HashMap<CoveMetadataCacheKey, Arc<DatasetState>>> {
        match self.entries.lock() {
            Ok(entries) => entries,
            // INVARIANT: cache poisoning must not silently disable metadata reuse.
            // The cache only stores immutable DatasetState values, so recovering
            // the guard is deterministic and keeps fallback behavior visible in
            // tests instead of degrading to repeated reparsing.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn get(&self, key: &CoveMetadataCacheKey) -> Option<Arc<DatasetState>> {
        self.entries().get(key).cloned()
    }

    pub fn insert(&self, key: CoveMetadataCacheKey, state: Arc<DatasetState>) {
        self.entries().insert(key, state);
    }
}

/// Load a local COVE file into immutable single-file dataset state.
///
/// This synchronous convenience wrapper blocks the current thread.
pub fn bootstrap_local_file(path: impl AsRef<Path>) -> Result<Arc<DatasetState>, CoveError> {
    bootstrap_local_file_with_options(path, CoveTableOptions::default())
}

/// Load a local COVE file into immutable single-file dataset state with table
/// registration options.
///
/// This synchronous convenience wrapper blocks the current thread.
pub fn bootstrap_local_file_with_options(
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
    futures::executor::block_on(bootstrap_local_file_with_options_async(path, options))
}

/// Load a local COVE file into immutable single-file dataset state.
pub async fn bootstrap_local_file_async(
    path: impl AsRef<Path>,
) -> Result<Arc<DatasetState>, CoveError> {
    bootstrap_local_file_with_options_async(path, CoveTableOptions::default()).await
}

/// Load a local COVE file into immutable single-file dataset state with table
/// registration options.
pub async fn bootstrap_local_file_with_options_async(
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
    let path = path.as_ref();
    let file_len = fs::metadata(path)?.len();
    let reader = LocalFileRangeReader::new(path);
    bootstrap_range_reader_with_options(
        path.display().to_string(),
        file_len,
        &reader,
        options,
        None,
    )
    .await
}

/// Build dataset state for an overlay snapshot.
///
/// This synchronous convenience wrapper blocks the current thread.
pub fn bootstrap_overlay_snapshot_with_options(
    snapshot: CoveOverlaySnapshot,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
    futures::executor::block_on(bootstrap_overlay_snapshot_with_options_async(
        snapshot, options,
    ))
}

/// Build dataset state for an overlay snapshot.
pub async fn bootstrap_overlay_snapshot_with_options_async(
    snapshot: CoveOverlaySnapshot,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
    let mut files = Vec::with_capacity(snapshot.files.len());
    let mut stats = DatasetBootstrapStats {
        files_considered: snapshot.files.len(),
        ..DatasetBootstrapStats::default()
    };

    for overlay_file in snapshot.files {
        if overlay_file.visibility.is_explicitly_hidden() {
            stats.files_pruned += 1;
            stats.overlay_files_hidden += 1;
            continue;
        }
        let path = overlay_uri_to_path(overlay_file.uri.as_ref());
        let file_len = fs::metadata(&path)?.len();
        let reader = LocalFileRangeReader::new(&path);
        let state = bootstrap_range_reader_with_options(
            path.display().to_string(),
            file_len,
            &reader,
            options,
            None,
        )
        .await?;
        validate_overlay_identity(
            overlay_file.expected_identity.as_ref(),
            state.as_ref(),
            &path,
        )?;
        stats.files_validated += 1;

        let base = state.files()[0].clone();
        let visible_rows = overlay_file
            .visibility
            .visible_count(base.table().row_count)?;
        let hidden_rows = base
            .table()
            .row_count
            .checked_sub(visible_rows)
            .ok_or(CoveError::ArithOverflow)?;
        stats.overlay_rows_hidden = stats
            .overlay_rows_hidden
            .checked_add(usize::try_from(hidden_rows).map_err(|_| CoveError::ArithOverflow)?)
            .ok_or(CoveError::ArithOverflow)?;
        files.push(FileMetadata::new_with_visibility(
            base.identity().clone(),
            None,
            Arc::new(base.mounted().clone()),
            Arc::new(base.table().clone()),
            Arc::new(base.segments().to_vec()),
            base.pruning().clone(),
            overlay_file.visibility,
            base.flags(),
        ));
    }

    DatasetState::from_file_metadata_with_options(
        snapshot.snapshot_id,
        files,
        stats,
        options.arrow_export_options(),
        options.execution_code_policy(),
        options.page_payload_validation_policy(),
        options.local_file_read_policy(),
        options.target_morsels_per_partition(),
        options.range_coalescing(),
        options.dynamic_filters_enabled(),
    )
    .map(Arc::new)
}

#[cfg(feature = "covm")]
/// Build dataset state for a local `.covm` manifest.
///
/// This synchronous convenience wrapper blocks the current thread.
pub fn bootstrap_covm_local_file_with_options(
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
    futures::executor::block_on(bootstrap_covm_local_file_with_options_async(path, options))
}

#[cfg(feature = "covm")]
/// Build dataset state for a local `.covm` manifest.
pub async fn bootstrap_covm_local_file_with_options_async(
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
    let manifest_path = path.as_ref();
    let manifest_bytes = fs::read(manifest_path)?;
    let covm = CovmFile::parse(&manifest_bytes)?;
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut files = Vec::with_capacity(covm.files.len());
    let mut stats = DatasetBootstrapStats {
        files_considered: covm.files.len(),
        ..DatasetBootstrapStats::default()
    };

    for entry in &covm.files {
        let cove_path = resolve_covm_uri(manifest_dir, &entry.uri)?;
        let file_len = fs::metadata(&cove_path)?.len();
        let reader = LocalFileRangeReader::new(&cove_path);
        let state = bootstrap_range_reader_with_options(
            cove_path.display().to_string(),
            file_len,
            &reader,
            options,
            None,
        )
        .await?;
        stats.files_validated += 1;
        match validate_covm_entry(entry, state.as_ref(), &cove_path, options) {
            CovmEntryFreshness::Fresh => {}
            CovmEntryFreshness::Stale => stats.covm_entries_stale += 1,
            CovmEntryFreshness::DigestFallback => stats.manifest_fallbacks += 1,
        }

        let mut file = state.files()[0].clone();
        validate_covx_for_file(&cove_path, &file, options, &mut stats)?;
        file = FileMetadata::new(
            file.identity().clone(),
            None,
            Arc::new(file.mounted().clone()),
            Arc::new(file.table().clone()),
            Arc::new(file.segments().to_vec()),
            file.pruning().clone(),
            entry.flags,
        );
        files.push(file);
    }

    DatasetState::from_file_metadata_with_options(
        manifest_path.display().to_string(),
        files,
        stats,
        options.arrow_export_options(),
        options.execution_code_policy(),
        options.page_payload_validation_policy(),
        options.local_file_read_policy(),
        options.target_morsels_per_partition(),
        options.range_coalescing(),
        options.dynamic_filters_enabled(),
    )
    .map(Arc::new)
}

/// Build immutable single-file dataset state from caller-provided bytes.
pub fn bootstrap_bytes(
    source: impl Into<Arc<str>>,
    bytes: Vec<u8>,
) -> Result<Arc<DatasetState>, CoveError> {
    bootstrap_bytes_with_options(source, bytes, CoveTableOptions::default())
}

/// Build immutable single-file dataset state from caller-provided bytes and
/// explicit table registration options.
pub fn bootstrap_bytes_with_options(
    source: impl Into<Arc<str>>,
    bytes: Vec<u8>,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
    DatasetState::from_bytes_with_options(
        source,
        bytes,
        options.arrow_export_options(),
        options.execution_code_policy(),
        options.page_payload_validation_policy(),
        options.local_file_read_policy(),
        options.target_morsels_per_partition(),
        options.range_coalescing(),
        options.dynamic_filters_enabled(),
    )
    .map(Arc::new)
}

pub async fn bootstrap_range_reader_with_options<R: CoveRangeReader + ?Sized>(
    source: impl Into<Arc<str>>,
    file_len: u64,
    reader: &R,
    options: CoveTableOptions,
    cache: Option<&CoveMetadataCache>,
) -> Result<Arc<DatasetState>, CoveError> {
    let source = source.into();
    let (header, postscript, footer) = bootstrap_header_footer(file_len, reader).await?;

    let provisional_key = CoveMetadataCacheKey {
        source: Arc::clone(&source),
        file_id: header.file_id,
        file_len,
        footer_crc32c: postscript.footer.crc32c,
    };
    if let Some(cache) = cache {
        if let Some(cached) = cache.get(&provisional_key) {
            return Ok(cached);
        }
    }

    let table_catalog = parse_table_catalog(reader, &footer).await?;
    if table_catalog.tables.len() != 1 {
        return Err(CoveError::BadSchema(format!(
            "COVE DataFusion M2 compatibility supports exactly one table per file, found {}",
            table_catalog.tables.len()
        )));
    }
    let table = table_catalog.tables[0].clone();
    let dictionary = parse_dictionary(reader, &footer).await?;
    let engine_metadata = parse_engine_metadata(reader, &footer).await?;
    let segment_index = parse_segment_index(reader, &footer).await?;
    let segments = segment_index
        .entries
        .into_iter()
        .filter(|segment| segment.table_id == table.table_id)
        .collect::<Vec<_>>();
    let pruning = parse_pruning_metadata(reader, &footer).await?;

    let state = Arc::new(DatasetState::from_metadata_with_options(
        Arc::clone(&source),
        file_len,
        postscript.footer.crc32c,
        header,
        footer,
        table,
        dictionary,
        engine_metadata,
        segments,
        pruning,
        options.arrow_export_options(),
        options.execution_code_policy(),
        options.page_payload_validation_policy(),
        options.local_file_read_policy(),
        options.target_morsels_per_partition(),
        options.range_coalescing(),
        options.dynamic_filters_enabled(),
    )?);
    if let Some(cache) = cache {
        cache.insert(provisional_key, Arc::clone(&state));
    }
    Ok(state)
}

async fn bootstrap_header_footer<R: CoveRangeReader + ?Sized>(
    file_len: u64,
    reader: &R,
) -> Result<(CoveHeaderV1, CovePostscriptV1, CoveFooter), CoveError> {
    if file_len < (HEADER_SIZE + POSTSCRIPT_TOTAL_SIZE) as u64 {
        return Err(CoveError::BufferTooShort);
    }
    let tail_start = file_len
        .checked_sub(POSTSCRIPT_TOTAL_SIZE as u64)
        .ok_or(CoveError::BufferTooShort)?;
    let ranges = reader
        .read_ranges(
            &[0..HEADER_SIZE as u64, tail_start..file_len],
            RangeReadKind::Metadata,
        )
        .await?;
    if ranges.len() != 2 {
        return Err(CoveError::BufferTooShort);
    }
    let header = CoveHeaderV1::parse(&ranges[0])?;
    let postscript = CovePostscriptV1::parse_from_tail(&ranges[1])?;
    if postscript.file_len != file_len {
        return Err(CoveError::OffsetRange);
    }
    if header.required_features != postscript.required_features
        || header.optional_features != postscript.optional_features
    {
        return Err(CoveError::BadSection(
            "header and postscript feature bits differ".into(),
        ));
    }
    let footer_end = postscript.footer.end_offset()?;
    if postscript.footer.offset < HEADER_SIZE as u64 || footer_end > tail_start {
        return Err(CoveError::OffsetRange);
    }
    let footer_raw = reader
        .read_range(
            postscript.footer.offset..footer_end,
            RangeReadKind::Metadata,
        )
        .await?;
    let footer_payload = section_payload_from_raw(
        &footer_raw,
        postscript.footer.length,
        postscript.footer.uncompressed_length,
        postscript.footer.compression,
        postscript.footer.crc32c,
    )?;
    let footer = CoveFooter::parse(&footer_payload)?;
    if footer.header.total_len()? != postscript.footer.uncompressed_length {
        return Err(CoveError::BadSection(
            "footer header length does not match postscript footer uncompressed_length".into(),
        ));
    }
    validate_section_ranges(&footer, postscript.footer.offset)?;
    Ok((header, postscript, footer))
}

fn validate_section_ranges(footer: &CoveFooter, footer_start: u64) -> Result<(), CoveError> {
    let mut ranges: Vec<(u64, u64, u32)> = Vec::new();
    let mut last_section_id = None;
    for entry in &footer.sections {
        if let Some(last) = last_section_id {
            if entry.section_id <= last {
                return Err(CoveError::BadSection(format!(
                    "section_id {} is not greater than previous id {}",
                    entry.section_id, last
                )));
            }
        }
        last_section_id = Some(entry.section_id);
        let section_end = entry.end_offset()?;
        if entry.offset < HEADER_SIZE as u64 || section_end > footer_start {
            return Err(CoveError::OffsetRange);
        }
        for (start, end, id) in &ranges {
            if entry.length != 0 && entry.offset < *end && section_end > *start {
                return Err(CoveError::BadSection(format!(
                    "section {} overlaps section {id}",
                    entry.section_id
                )));
            }
        }
        ranges.push((entry.offset, section_end, entry.section_id));
    }
    Ok(())
}

async fn parse_table_catalog<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<TableCatalog, CoveError> {
    let entries = find_sections(footer, SectionKind::TableCatalog);
    if entries.len() != 1 {
        return Err(CoveError::BadSchema(format!(
            "COVE DataFusion M2 requires exactly one table catalog section, found {}",
            entries.len()
        )));
    }
    let payload = read_section_payload(reader, entries[0]).await?;
    TableCatalog::parse(&payload)
}

async fn parse_dictionary<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<Option<FileDictionary>, CoveError> {
    let Some(index_entry) = find_sections(footer, SectionKind::FileDictionaryIndex)
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let index_payload = read_section_payload(reader, index_entry).await?;
    let payload = match find_sections(footer, SectionKind::FileDictionaryPayload)
        .into_iter()
        .next()
    {
        Some(entry) => read_section_payload(reader, entry).await?,
        None => Vec::new(),
    };
    FileDictionary::parse(&index_payload, &payload).map(Some)
}

async fn parse_engine_metadata<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<EngineMetadata, CoveError> {
    Ok(EngineMetadata {
        engine_profile_registries: parse_engine_profile_registries(reader, footer).await?,
        execution_descriptors: parse_execution_descriptors(reader, footer).await?,
        execution_scopes: parse_execution_scopes(reader, footer).await?,
        code_spaces: parse_code_spaces(reader, footer).await?,
        engine_mount_policies: parse_engine_mount_policies(reader, footer).await?,
    })
}

async fn parse_engine_profile_registries<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<Vec<EngineProfileRegistry>, CoveError> {
    let mut out = Vec::new();
    for entry in find_sections(footer, SectionKind::EngineProfileRegistry) {
        let payload = read_section_payload(reader, entry).await?;
        out.push(EngineProfileRegistry::parse(&payload)?);
    }
    Ok(out)
}

async fn parse_execution_descriptors<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<Vec<ExecutionCodeDescriptorV1>, CoveError> {
    let mut out = Vec::new();
    for entry in find_sections(footer, SectionKind::ExecutionCodeDescriptor) {
        let payload = read_section_payload(reader, entry).await?;
        out.push(ExecutionCodeDescriptorV1::parse(&payload)?);
    }
    Ok(out)
}

async fn parse_execution_scopes<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<Vec<ExecutionScopeDescriptorV1>, CoveError> {
    let mut out = Vec::new();
    for entry in find_sections(footer, SectionKind::ExecutionScopeDescriptor) {
        let payload = read_section_payload(reader, entry).await?;
        out.push(ExecutionScopeDescriptorV1::parse(&payload)?);
    }
    Ok(out)
}

async fn parse_code_spaces<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<Vec<CodeSpaceDescriptorV1>, CoveError> {
    let mut out = Vec::new();
    for entry in find_sections(footer, SectionKind::CodeSpaceDescriptor) {
        let payload = read_section_payload(reader, entry).await?;
        out.push(CodeSpaceDescriptorV1::parse(&payload)?);
    }
    Ok(out)
}

async fn parse_engine_mount_policies<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<Vec<EngineMountPolicyV1>, CoveError> {
    let mut out = Vec::new();
    for entry in find_sections(footer, SectionKind::EngineMountPolicy) {
        let payload = read_section_payload(reader, entry).await?;
        out.push(EngineMountPolicyV1::parse(&payload)?);
    }
    Ok(out)
}

async fn parse_segment_index<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<TableSegmentIndex, CoveError> {
    let entries = find_sections(footer, SectionKind::TableSegmentIndex);
    if entries.is_empty() {
        return Ok(TableSegmentIndex::default());
    }
    if entries.len() != 1 {
        return Err(CoveError::SegmentCorrupt);
    }
    let payload = read_section_payload(reader, entries[0]).await?;
    TableSegmentIndex::parse(&payload)
}

async fn parse_pruning_metadata<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<PruningMetadata, CoveError> {
    let mut column_domains = Vec::new();
    for entry in find_sections(footer, SectionKind::ColumnDomain) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(domain) = ColumnDomain::parse(&payload) {
                column_domains.push(domain);
            }
        }
    }

    let mut zone_stats = Vec::new();
    for entry in find_sections(footer, SectionKind::ZoneStats) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(section) = ZoneStatsSection::parse(&payload) {
                zone_stats.push(section);
            }
        }
    }

    let mut exact_sets = Vec::new();
    for entry in find_sections(footer, SectionKind::ExactSetIndex) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(index) = ExactSetIndex::parse(&payload) {
                exact_sets.push(index);
            }
        }
    }

    let mut blooms = Vec::new();
    for entry in find_sections(footer, SectionKind::BloomIndex) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(index) = BloomFilterIndex::parse(&payload) {
                blooms.push(index);
            }
        }
    }

    let mut lookups = Vec::new();
    for entry in find_sections(footer, SectionKind::LookupIndex) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(index) = LookupIndex::parse(&payload) {
                lookups.push(index);
            }
        }
    }

    let mut inverted = Vec::new();
    for entry in find_sections(footer, SectionKind::InvertedMorselIndex) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(index) = InvertedMorselIndex::parse(&payload) {
                inverted.push(index);
            }
        }
    }

    let mut aggregates = Vec::new();
    for entry in find_sections(footer, SectionKind::AggregateSynopsis) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(index) = AggregateSynopsis::parse(&payload) {
                aggregates.push(index);
            }
        }
    }

    let mut composites = Vec::new();
    for entry in find_sections(footer, SectionKind::CompositeZoneIndex) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(index) = CompositeIndex::parse(&payload) {
                composites.push(index);
            }
        }
    }

    let mut topn = Vec::new();
    for entry in find_sections(footer, SectionKind::TopNZoneSummary) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(index) = TopNSummary::parse(&payload) {
                topn.push(index);
            }
        }
    }

    Ok(PruningMetadata {
        column_domains: Arc::new(column_domains),
        zone_stats: Arc::new(zone_stats),
        exact_sets: Arc::new(exact_sets),
        blooms: Arc::new(blooms),
        lookups: Arc::new(lookups),
        inverted: Arc::new(inverted),
        aggregates: Arc::new(aggregates),
        composites: Arc::new(composites),
        topn: Arc::new(topn),
    })
}

async fn read_section_payload<R: CoveRangeReader + ?Sized>(
    reader: &R,
    entry: &CoveSectionEntryV1,
) -> Result<Vec<u8>, CoveError> {
    let end = entry.end_offset()?;
    let raw = reader
        .read_range(entry.offset..end, RangeReadKind::Metadata)
        .await?;
    if checksum::crc32c(&raw) != entry.crc32c {
        return Err(CoveError::ChecksumMismatch);
    }
    section_payload_from_raw(
        &raw,
        entry.length,
        entry.uncompressed_length,
        entry.compression,
        entry.crc32c,
    )
    .map(|payload| payload.into_owned())
}

fn find_sections(footer: &CoveFooter, kind: SectionKind) -> Vec<&CoveSectionEntryV1> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == kind as u16)
        .collect()
}

fn overlay_uri_to_path(uri: &str) -> PathBuf {
    if let Some(rest) = uri.strip_prefix("file://") {
        return PathBuf::from(rest);
    }
    PathBuf::from(uri)
}

fn validate_overlay_identity(
    expected: Option<&OverlayFileIdentity>,
    state: &DatasetState,
    path: &Path,
) -> Result<(), CoveError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if expected.file_id != *state.file_id()
        || expected.file_len != state.file_len()
        || expected.footer_crc32c != state.footer_crc32c()
    {
        return Err(CoveError::BadSection(format!(
            "overlay identity mismatch for {}",
            path.display()
        )));
    }
    if let Some(digest) = &expected.digest {
        validate_overlay_digest(digest, path)?;
    }
    Ok(())
}

fn validate_overlay_digest(digest: &OverlayFileDigest, path: &Path) -> Result<(), CoveError> {
    let Some(algorithm) = DigestAlgorithm::from_u16(digest.algorithm) else {
        return Err(CoveError::BadSection(format!(
            "overlay digest uses unknown algorithm {}",
            digest.algorithm
        )));
    };
    if algorithm == DigestAlgorithm::None {
        if digest.bytes.is_empty() {
            return Ok(());
        }
        return Err(CoveError::BadSection(
            "overlay digest bytes supplied for DigestAlgorithm::None".into(),
        ));
    }
    let bytes = fs::read(path)?;
    let actual = compute_digest(algorithm, &bytes)?;
    if actual == digest.bytes {
        Ok(())
    } else {
        Err(CoveError::BadSection(format!(
            "overlay digest mismatch for {}",
            path.display()
        )))
    }
}

#[cfg(feature = "covm")]
fn resolve_covm_uri(manifest_dir: &Path, uri: &str) -> Result<PathBuf, CoveError> {
    if let Some(rest) = uri.strip_prefix("file://") {
        return Ok(PathBuf::from(rest));
    }
    let path = Path::new(uri);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(manifest_dir.join(path))
    }
}

#[cfg(feature = "covm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CovmEntryFreshness {
    Fresh,
    Stale,
    DigestFallback,
}

#[cfg(feature = "covm")]
fn validate_covm_entry(
    entry: &CovmFileEntryV1,
    state: &DatasetState,
    path: &Path,
    options: CoveTableOptions,
) -> CovmEntryFreshness {
    if entry.file_id != *state.file_id()
        || entry.file_len != state.file_len()
        || entry.footer_crc32c != state.footer_crc32c()
    {
        return CovmEntryFreshness::Stale;
    }
    validate_optional_digest(
        entry.digest_algorithm,
        &entry.digest,
        path,
        options.sidecar_digest_policy(),
    )
}

#[cfg(feature = "covm")]
fn validate_covx_for_file(
    path: &Path,
    file: &FileMetadata,
    options: CoveTableOptions,
    stats: &mut DatasetBootstrapStats,
) -> Result<(), CoveError> {
    if !cfg!(feature = "covx") || options.covx_discovery() == CovxDiscovery::Disabled {
        return Ok(());
    }
    let Some(path) = discover_covx_path(path, options.covx_discovery()) else {
        return Ok(());
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            stats.covx_sidecars_ignored += 1;
            return Ok(());
        }
    };
    let covx = match CovxFile::parse(&bytes) {
        Ok(covx) => covx,
        Err(_) => {
            stats.covx_sidecars_ignored += 1;
            return Ok(());
        }
    };
    let Some(entry) = covx
        .referenced_files
        .iter()
        .find(|entry| entry.file_id == file.identity().file_id)
    else {
        stats.covx_sidecars_stale += 1;
        return Ok(());
    };
    match validate_covx_entry(entry, file, options, file.source()) {
        CovmEntryFreshness::Fresh => stats.covx_sidecars_loaded += 1,
        CovmEntryFreshness::Stale => stats.covx_sidecars_stale += 1,
        CovmEntryFreshness::DigestFallback => stats.covx_sidecars_ignored += 1,
    }
    Ok(())
}

#[cfg(feature = "covm")]
fn discover_covx_path(path: &Path, discovery: CovxDiscovery) -> Option<PathBuf> {
    if discovery != CovxDiscovery::SiblingExtension {
        return None;
    }
    let appended = PathBuf::from(format!("{}.covx", path.display()));
    if appended.is_file() {
        return Some(appended);
    }
    let replaced = path.with_extension("covx");
    replaced.is_file().then_some(replaced)
}

#[cfg(feature = "covm")]
fn validate_covx_entry(
    entry: &CovxReferencedFileV1,
    file: &FileMetadata,
    options: CoveTableOptions,
    file_path: &str,
) -> CovmEntryFreshness {
    if entry.file_id != file.identity().file_id
        || entry.file_len != file.identity().file_len
        || entry.footer_crc32c != file.identity().footer_crc32c
    {
        return CovmEntryFreshness::Stale;
    }
    validate_optional_digest(
        entry.digest_algorithm,
        &entry.digest,
        Path::new(file_path),
        options.sidecar_digest_policy(),
    )
}

#[cfg(feature = "covm")]
fn validate_optional_digest(
    digest_algorithm: u16,
    expected_digest: &[u8],
    path: &Path,
    policy: SidecarDigestPolicy,
) -> CovmEntryFreshness {
    if expected_digest.is_empty() && digest_algorithm == DigestAlgorithm::None as u16 {
        return CovmEntryFreshness::Fresh;
    }
    let Some(algorithm) = DigestAlgorithm::from_u16(digest_algorithm) else {
        return CovmEntryFreshness::Stale;
    };
    if policy != SidecarDigestPolicy::FullFileDigestOnDemand {
        return CovmEntryFreshness::DigestFallback;
    }
    match fs::read(path)
        .ok()
        .and_then(|bytes| compute_digest(algorithm, &bytes).ok())
    {
        Some(actual) if actual.as_slice() == expected_digest => CovmEntryFreshness::Fresh,
        Some(_) => CovmEntryFreshness::Stale,
        None => CovmEntryFreshness::DigestFallback,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        panic::{catch_unwind, AssertUnwindSafe},
        sync::Arc,
    };

    use super::{bootstrap_range_reader_with_options, CoveMetadataCache, CoveMetadataCacheKey};
    use crate::{options::CoveTableOptions, range_reader::MemoryRangeReader};
    use cove_core::{
        constants::{CoveLogicalType, CovePhysicalKind},
        table::{ColumnEntry, TableCatalog, TableEntry},
        writer::ScanProfileCoveWriter,
    };

    #[test]
    fn metadata_cache_reuses_bootstrapped_state() {
        let bytes = cache_test_bytes();
        let reader = MemoryRangeReader::new(bytes.clone());
        let cache = CoveMetadataCache::default();

        let first = futures::executor::block_on(bootstrap_range_reader_with_options(
            "memory://cache-hit",
            bytes.len() as u64,
            &reader,
            CoveTableOptions::default(),
            Some(&cache),
        ))
        .unwrap();
        let second = futures::executor::block_on(bootstrap_range_reader_with_options(
            "memory://cache-hit",
            bytes.len() as u64,
            &reader,
            CoveTableOptions::default(),
            Some(&cache),
        ))
        .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn metadata_cache_recovers_after_poison() {
        let bytes = cache_test_bytes();
        let reader = MemoryRangeReader::new(bytes.clone());
        let cache = CoveMetadataCache::default();
        let state = futures::executor::block_on(bootstrap_range_reader_with_options(
            "memory://cache-poison",
            bytes.len() as u64,
            &reader,
            CoveTableOptions::default(),
            None,
        ))
        .unwrap();
        let key = CoveMetadataCacheKey {
            source: Arc::from("memory://cache-poison"),
            file_id: *state.file_id(),
            file_len: state.file_len(),
            footer_crc32c: state.footer_crc32c(),
        };

        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = cache.entries.lock().unwrap();
            panic!("poison cache lock for recovery test");
        }));
        assert!(cache.entries.is_poisoned());

        cache.insert(key.clone(), Arc::clone(&state));
        let cached = cache.get(&key).unwrap();
        assert!(Arc::ptr_eq(&cached, &state));
    }

    fn cache_test_bytes() -> Vec<u8> {
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "events".into(),
                row_count: 0,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![ColumnEntry {
                    column_id: 1,
                    name: "id".into(),
                    logical: CoveLogicalType::Int64,
                    physical: CovePhysicalKind::NumCode,
                    nullable: false,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        };
        ScanProfileCoveWriter::new(catalog).write().unwrap()
    }
}
