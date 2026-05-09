use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use cove_core::{constants::DigestAlgorithm, digest::compute_digest, CoveError};

use crate::{
    bootstrap::local::bootstrap_local_path_with_options,
    dataset_state::{DatasetBootstrapStats, DatasetState, FileMetadata},
    options::CoveTableOptions,
    overlay::{CoveOverlaySnapshot, OverlayFileDigest, OverlayFileIdentity},
};

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
        let state = bootstrap_local_path_with_options(&path, options).await?;
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
