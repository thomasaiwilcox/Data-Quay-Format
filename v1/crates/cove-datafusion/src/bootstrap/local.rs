use std::{fs, path::Path, sync::Arc};

use cove_core::CoveError;

use crate::{
    bootstrap::{
        parse::{
            bootstrap_header_footer, parse_dictionary, parse_engine_metadata,
            parse_pruning_metadata, parse_segment_index, parse_table_catalog,
        },
        CoveMetadataCache, CoveMetadataCacheKey,
    },
    dataset_state::DatasetState,
    options::CoveTableOptions,
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

pub(super) async fn bootstrap_local_path_with_options(
    path: &Path,
    options: CoveTableOptions,
) -> Result<Arc<DatasetState>, CoveError> {
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
