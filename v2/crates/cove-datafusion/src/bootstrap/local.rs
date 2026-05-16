use std::{fs, path::Path, path::PathBuf, sync::Arc};

use cove_cache::CoverageCacheV2;
use cove_core::{constants::DigestAlgorithm, digest::compute_digest, CoveError};

#[cfg(feature = "covi")]
use crate::bootstrap::covi::validate_covi_for_file;
use crate::{
    bootstrap::{
        parse::{
            bootstrap_header_footer, parse_dictionary, parse_engine_metadata,
            parse_feature_scope_table, parse_layout_metadata, parse_pruning_metadata,
            parse_segment_index, parse_table_catalog,
        },
        CoveMetadataCache, CoveMetadataCacheKey,
    },
    dataset_state::{
        ordinary_table_scan_feature_use_request, CoverageCacheMetadata, DatasetBootstrapStats,
        DatasetState, FileIdentity,
    },
    options::{select_table, CoveTableOptions, CoverageCacheDiscovery},
    range_reader::{CoveRangeReader, LocalFileRangeReader},
};

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
    bootstrap_local_path_with_options(path.as_ref(), options).await
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
    DatasetState::from_bytes_with_table_options(source, bytes, options).map(Arc::new)
}

#[cfg(feature = "covi")]
pub fn bootstrap_bytes_with_covi_artifacts(
    source: impl Into<Arc<str>>,
    bytes: Vec<u8>,
    covi_artifacts: Vec<Vec<u8>>,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
    use cove_core::{constants::DigestAlgorithm, digest::compute_digest};
    use cove_index::execution::CoviValidationContextV2;

    let file_digest = compute_digest(DigestAlgorithm::Sha256, &bytes).ok();
    let state = bootstrap_bytes_with_options(source, bytes, options)?;
    let mut stats = crate::dataset_state::DatasetBootstrapStats::default();
    let mut covi = None;
    if let Some(file) = state.files().first() {
        for artifact_bytes in covi_artifacts {
            let mut context = CoviValidationContextV2::for_file(
                file.identity().file_id,
                file.identity().file_len,
                file.identity().footer_crc32c,
            )
            .with_dataset_id(file.identity().file_id)
            .with_file_code_keys(true);
            if let Some(digest) = file_digest.clone() {
                context = context.with_file_digest(DigestAlgorithm::Sha256, digest);
            }
            match cove_index::execution::ValidatedCoviArtifactV2::parse_and_validate(
                &artifact_bytes,
                context,
            ) {
                Ok(validated) => {
                    stats.covi_sidecars_loaded += 1;
                    covi = Some(Arc::new(validated));
                    break;
                }
                Err(_) => stats.covi_sidecars_stale += 1,
            }
        }
    }
    Ok(Arc::new(state.with_file_covi(0, covi, stats)?))
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
    let feature_scope_table = parse_feature_scope_table(reader, &header, &footer).await?;
    feature_scope_table
        .reject_unknowns_for_request(&ordinary_table_scan_feature_use_request(&footer))?;

    let provisional_key = CoveMetadataCacheKey {
        source: Arc::clone(&source),
        file_id: header.file_id,
        file_len,
        footer_crc32c: postscript.footer.crc32c,
        table_selection: options.table_selection().cloned(),
    };
    if let Some(cache) = cache {
        if let Some(cached) = cache.get(&provisional_key) {
            return Ok(cached);
        }
    }

    let table_catalog = parse_table_catalog(reader, &footer).await?;
    let table = select_table(&table_catalog, options.table_selection())?;
    let dictionary = parse_dictionary(reader, &footer).await?;
    let engine_metadata = parse_engine_metadata(reader, &footer).await?;
    let segment_index = parse_segment_index(reader, &footer).await?;
    let segments = segment_index
        .entries
        .into_iter()
        .filter(|segment| segment.table_id == table.table_id)
        .collect::<Vec<_>>();
    let pruning = parse_pruning_metadata(reader, &footer).await?;
    let layout = parse_layout_metadata(reader, &header, &footer, &table, &segments).await;

    let state = Arc::new(DatasetState::from_metadata_with_options_and_feature_scope(
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
        layout,
        Some(feature_scope_table),
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

pub(super) async fn bootstrap_local_path_with_options(
    path: &Path,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
    let file_len = fs::metadata(path)?.len();
    let reader = LocalFileRangeReader::new(path);
    let state = bootstrap_range_reader_with_options(
        path.display().to_string(),
        file_len,
        &reader,
        options.clone(),
        None,
    )
    .await?;
    let state = if options.coverage_cache_discovery() == CoverageCacheDiscovery::SiblingDiagnostic {
        let (cache, stats) = load_sibling_coverage_cache(path, state.as_ref())?;
        Arc::new(state.with_coverage_cache(cache, stats)?)
    } else {
        state
    };
    #[cfg(feature = "covi")]
    {
        let mut state = state;
        let mut stats = crate::dataset_state::DatasetBootstrapStats::default();
        let covi = state
            .files()
            .first()
            .and_then(|file| validate_covi_for_file(path, file, options, &mut stats));
        if covi.is_some() || stats.covi_sidecars_ignored != 0 || stats.covi_sidecars_stale != 0 {
            state = Arc::new(state.with_file_covi(0, covi, stats)?);
        }
        return Ok(state);
    }
    #[cfg(not(feature = "covi"))]
    Ok(state)
}

fn load_sibling_coverage_cache(
    path: &Path,
    state: &DatasetState,
) -> Result<(CoverageCacheMetadata, DatasetBootstrapStats), CoveError> {
    let mut stats = DatasetBootstrapStats::default();
    let cache_path = sibling_coverage_cache_path(path);
    let bytes = match fs::read(&cache_path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok((CoverageCacheMetadata::enabled_empty(), stats));
        }
        Err(_) => {
            stats.coverage_cache_entries_ignored += 1;
            stats.coverage_cache_invalidations += 1;
            return Ok((CoverageCacheMetadata::enabled_empty(), stats));
        }
    };
    let cache = match CoverageCacheV2::parse(&bytes) {
        Ok(cache) => cache,
        Err(_) => {
            stats.coverage_cache_entries_ignored += 1;
            stats.coverage_cache_invalidations += 1;
            return Ok((CoverageCacheMetadata::enabled_empty(), stats));
        }
    };
    let (dataset_id, snapshot_id) = coverage_cache_ids(state.identity())?;
    if cache.header.dataset_id != dataset_id || cache.header.snapshot_id != snapshot_id {
        stats.coverage_cache_entries_stale += cache.entries.len().max(1);
        stats.coverage_cache_invalidations += cache.entries.len().max(1);
        return Ok((CoverageCacheMetadata::enabled_empty(), stats));
    }

    let mut valid_entries = Vec::new();
    for entry in cache.entries {
        if coverage_cache_entry_usable(state, &entry) {
            valid_entries.push(entry);
        } else {
            stats.coverage_cache_entries_ignored += 1;
            stats.coverage_cache_invalidations += 1;
        }
    }
    stats.coverage_cache_entries_loaded = valid_entries.len();
    Ok((
        CoverageCacheMetadata::enabled_with_entries(valid_entries),
        stats,
    ))
}

fn sibling_coverage_cache_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.cache", path.display()))
}

fn coverage_cache_ids(identity: &FileIdentity) -> Result<([u8; 16], [u8; 16]), CoveError> {
    let mut seed = Vec::with_capacity(28);
    seed.extend_from_slice(&identity.file_id);
    seed.extend_from_slice(&identity.file_len.to_le_bytes());
    seed.extend_from_slice(&identity.footer_crc32c.to_le_bytes());
    let digest = compute_digest(DigestAlgorithm::Sha256, &seed)?;
    let mut snapshot_id = [0u8; 16];
    snapshot_id.copy_from_slice(&digest[..16]);
    Ok((identity.file_id, snapshot_id))
}

fn coverage_cache_entry_usable(
    state: &DatasetState,
    entry: &cove_cache::CoverageCacheEntryV2,
) -> bool {
    let predicate_exists = state
        .pruning()
        .predicate_forms
        .iter()
        .any(|form| form.predicate_form_id == entry.predicate_normal_form_ref)
        || state
            .pruning()
            .predicate_forms_with_payloads
            .iter()
            .any(|form| form.form.predicate_form_id == entry.predicate_normal_form_ref);
    if !predicate_exists {
        return false;
    }
    state.pruning().coverage_sets.iter().any(|set| {
        set.header.coverage_set_id == entry.coverage_set_ref
            && set.header.predicate_form_ref == entry.predicate_normal_form_ref
            && set.header.granularity == entry.coverage_granularity
            && set.header.proof_strength == entry.proof_strength
            && set.header.exactness == entry.exactness
            && !set.header.exactness.may_under_include()
            && set.header.proof_strength.allows_pruning()
    })
}
