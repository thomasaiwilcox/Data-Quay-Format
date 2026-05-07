//! DataFusion 53.x file opener glue.

use std::{ops::Range, sync::Arc};

use async_trait::async_trait;
use datafusion::{
    common::{DataFusionError, Result},
    object_store::{path::Path, ObjectStore},
};
use datafusion_datasource::{
    file_stream::{FileOpenFuture, FileOpener},
    PartitionedFile, TableSchema,
};
use futures::{stream, StreamExt};

use crate::{
    adapter_v53::{cove_to_datafusion, metrics::CoveFileMetrics},
    bootstrap::{bootstrap_range_reader_with_options, CoveMetadataCache},
    decode::decode_scan_with_reader,
    options::CoveTableOptions,
    planner::plan_scan,
    range_reader::{CoveRangeReader, RangeReadKind},
};

#[derive(Debug)]
pub(crate) struct CoveFileOpener {
    object_store: Arc<dyn ObjectStore>,
    table_schema: TableSchema,
    options: CoveTableOptions,
    cache: Arc<CoveMetadataCache>,
    projection: Option<Vec<usize>>,
    metrics: CoveFileMetrics,
}

impl CoveFileOpener {
    pub(crate) fn new(
        object_store: Arc<dyn ObjectStore>,
        table_schema: TableSchema,
        options: CoveTableOptions,
        cache: Arc<CoveMetadataCache>,
        projection: Option<Vec<usize>>,
        metrics: CoveFileMetrics,
    ) -> Self {
        Self {
            object_store,
            table_schema,
            options,
            cache,
            projection,
            metrics,
        }
    }
}

impl FileOpener for CoveFileOpener {
    fn open(&self, partitioned_file: PartitionedFile) -> Result<FileOpenFuture> {
        if partitioned_file.range.is_some() {
            return Err(DataFusionError::Plan(
                "COVE DataFusion M2 does not support DataFusion byte-range repartitioning".into(),
            ));
        }
        if !partitioned_file.partition_values.is_empty() {
            return Err(DataFusionError::Plan(
                "COVE DataFusion M2 compatibility does not support partition columns".into(),
            ));
        }
        let object_store = Arc::clone(&self.object_store);
        let table_schema = self.table_schema.clone();
        let options = self.options;
        let cache = Arc::clone(&self.cache);
        let projection = self.projection.clone();
        let metrics = self.metrics.clone();
        Ok(Box::pin(async move {
            metrics.files_opened.add(1);
            let location = partitioned_file.object_meta.location.clone();
            let source = location.to_string();
            let reader = ObjectStoreRangeReader::new(object_store, location, metrics.clone());
            let state = bootstrap_range_reader_with_options(
                source,
                partitioned_file.object_meta.size,
                &reader,
                options,
                Some(cache.as_ref()),
            )
            .await
            .map_err(cove_to_datafusion)?;
            if state.schema().as_ref() != table_schema.file_schema().as_ref() {
                return Err(DataFusionError::Plan(format!(
                    "COVE DataFusion M2 schema mismatch for {}",
                    state.source()
                )));
            }
            let plan =
                plan_scan(&state, projection.as_ref(), Vec::new()).map_err(cove_to_datafusion)?;
            let decoded = decode_scan_with_reader(&state, &plan, &reader)
                .await
                .map_err(cove_to_datafusion)?;
            metrics.record_decode(decoded.stats);
            let stream = stream::iter(decoded.batches.into_iter().map(Ok)).boxed();
            Ok(stream)
        }))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ObjectStoreRangeReader {
    object_store: Arc<dyn ObjectStore>,
    location: Path,
    metrics: CoveFileMetrics,
}

impl ObjectStoreRangeReader {
    pub(crate) fn new(
        object_store: Arc<dyn ObjectStore>,
        location: Path,
        metrics: CoveFileMetrics,
    ) -> Self {
        Self {
            object_store,
            location,
            metrics,
        }
    }
}

#[async_trait]
impl CoveRangeReader for ObjectStoreRangeReader {
    async fn read_ranges(
        &self,
        ranges: &[Range<u64>],
        kind: RangeReadKind,
    ) -> std::result::Result<Vec<Vec<u8>>, cove_core::CoveError> {
        self.metrics.range_requests.add(ranges.len());
        let bytes = self
            .object_store
            .get_ranges(&self.location, ranges)
            .await
            .map_err(|err| cove_core::CoveError::Io(std::io::Error::other(err.to_string())))?;
        let mut out = Vec::with_capacity(bytes.len());
        let mut bytes_read = 0usize;
        for chunk in bytes {
            bytes_read = bytes_read
                .checked_add(chunk.len())
                .ok_or(cove_core::CoveError::ArithOverflow)?;
            out.push(chunk.to_vec());
        }
        match kind {
            RangeReadKind::Metadata => self.metrics.metadata_bytes_read.add(bytes_read),
            RangeReadKind::Data => self.metrics.data_bytes_read.add(bytes_read),
        }
        Ok(out)
    }

    fn record_coalescing(&self, original_ranges: usize, coalesced_ranges: usize) {
        if original_ranges > coalesced_ranges {
            self.metrics.coalesced_range_requests.add(coalesced_ranges);
        }
    }
}
