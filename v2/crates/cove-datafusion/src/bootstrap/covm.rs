use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use cove_core::{
    artifact::{
        covm::{CovmFile, CovmFileEntryV1},
        covx::{CovxFile, CovxReferencedFileV1},
    },
    constants::DigestAlgorithm,
    digest::compute_digest,
    CoveError,
};

#[cfg(feature = "covi")]
use crate::bootstrap::covi::validate_covi_for_file;
use crate::{
    bootstrap::local::bootstrap_local_path_with_options,
    dataset_state::{DatasetBootstrapStats, DatasetState, FileMetadata},
    options::{CoveTableOptions, CovxDiscovery, SidecarDigestPolicy},
};

/// Build dataset state for a local `.covm` manifest.
///
/// This synchronous convenience wrapper blocks the current thread.
pub fn bootstrap_covm_local_file_with_options(
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
    futures::executor::block_on(bootstrap_covm_local_file_with_options_async(path, options))
}

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
        let state = bootstrap_local_path_with_options(&cove_path, options).await?;
        stats.files_validated += 1;
        match validate_covm_entry(entry, state.as_ref(), &cove_path, options) {
            CovmEntryFreshness::Fresh => {}
            CovmEntryFreshness::Stale => stats.covm_entries_stale += 1,
            CovmEntryFreshness::DigestFallback => stats.manifest_fallbacks += 1,
        }

        let mut file = state.files()[0].clone();
        validate_covx_for_file(&cove_path, &file, options, &mut stats)?;
        #[cfg(feature = "covi")]
        let covi = validate_covi_for_file(&cove_path, &file, options, &mut stats);
        file = FileMetadata::new(
            file.identity().clone(),
            None,
            Arc::new(file.mounted().clone()),
            Arc::new(file.table().clone()),
            Arc::new(file.segments().to_vec()),
            file.pruning().clone(),
            entry.flags,
        )
        .with_layout(file.layout().clone())
        .with_coverage_cache(file.coverage_cache().clone());
        #[cfg(feature = "covi")]
        {
            file = file.with_covi(covi);
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CovmEntryFreshness {
    Fresh,
    Stale,
    DigestFallback,
}

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
