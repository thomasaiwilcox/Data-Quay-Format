//! DataFusion 53.x file opener glue.

use std::{
    future::Future,
    ops::Range,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use arrow_array::RecordBatch;
use async_trait::async_trait;
use datafusion::{
    common::{DataFusionError, Result},
    object_store::{path::Path, ObjectStore},
};
use datafusion_datasource::{
    file_stream::{FileOpenFuture, FileOpener},
    PartitionedFile, TableSchema,
};
use futures::{stream::BoxStream, Stream};
use tokio::sync::mpsc;

use crate::{
    adapter_v53::{cove_to_datafusion, metrics::CoveFileMetrics, stream::DecodeStreamEvent},
    bootstrap::{bootstrap_range_reader_with_options, CoveMetadataCache},
    decode::{decode_scan_with_reader_to_sink, DecodeControl, DecodeSink, DecodeStats},
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
            let (sender, receiver) = mpsc::unbounded_channel();
            let handle = tokio::runtime::Handle::current();
            let decode_metrics = metrics.clone();
            let decode_task = tokio::task::spawn_blocking(move || {
                handle.block_on(async move {
                    let mut sink = UnboundedDecodeSink::new(sender.clone());
                    match decode_scan_with_reader_to_sink(&state, &plan, &reader, &mut sink).await {
                        Ok(stats) => {
                            decode_metrics.record_decode(stats);
                            let _ = sender.send(DecodeStreamEvent::Finished);
                        }
                        Err(error) => {
                            let _ = sender.send(DecodeStreamEvent::Failed(error));
                        }
                    }
                });
            });
            let stream: BoxStream<'static, Result<RecordBatch>> = Box::pin(CoveFileDecodeStream {
                receiver,
                decode_task: Some(decode_task),
                done: false,
            });
            Ok(stream)
        }))
    }
}

struct CoveFileDecodeStream {
    receiver: mpsc::UnboundedReceiver<DecodeStreamEvent>,
    decode_task: Option<tokio::task::JoinHandle<()>>,
    done: bool,
}

struct UnboundedDecodeSink {
    sender: mpsc::UnboundedSender<DecodeStreamEvent>,
    stopped: bool,
}

impl UnboundedDecodeSink {
    fn new(sender: mpsc::UnboundedSender<DecodeStreamEvent>) -> Self {
        Self {
            sender,
            stopped: false,
        }
    }
}

impl DecodeSink for UnboundedDecodeSink {
    fn emit_batch(
        &mut self,
        batch: RecordBatch,
        stats: &mut DecodeStats,
    ) -> std::result::Result<DecodeControl, cove_core::CoveError> {
        let rows = batch.num_rows();
        if self.sender.send(DecodeStreamEvent::Batch(batch)).is_err() {
            self.stopped = true;
            return Ok(DecodeControl::Stop);
        }
        stats.rows_materialized += rows;
        Ok(DecodeControl::Continue)
    }

    fn should_stop(&self) -> bool {
        self.stopped
    }
}

impl Stream for CoveFileDecodeStream {
    type Item = Result<RecordBatch>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        match Pin::new(&mut this.receiver).poll_recv(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(DecodeStreamEvent::Batch(batch))) => Poll::Ready(Some(Ok(batch))),
            Poll::Ready(Some(DecodeStreamEvent::Finished)) => {
                this.done = true;
                this.decode_task = None;
                Poll::Ready(None)
            }
            Poll::Ready(Some(DecodeStreamEvent::Failed(error))) => {
                this.done = true;
                this.decode_task = None;
                Poll::Ready(Some(Err(cove_to_datafusion(error))))
            }
            Poll::Ready(None) => {
                if let Some(task) = this.decode_task.as_mut() {
                    match Pin::new(task).poll(cx) {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(Ok(())) => {
                            this.decode_task = None;
                            this.done = true;
                            Poll::Ready(None)
                        }
                        Poll::Ready(Err(error)) => {
                            this.decode_task = None;
                            this.done = true;
                            Poll::Ready(Some(Err(DataFusionError::Execution(format!(
                                "CoveFileOpener decode task failed: {error}"
                            )))))
                        }
                    }
                } else {
                    this.done = true;
                    Poll::Ready(None)
                }
            }
        }
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
