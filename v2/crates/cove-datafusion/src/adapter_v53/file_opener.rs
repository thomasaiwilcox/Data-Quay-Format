//! DataFusion 53.x file opener glue.

use std::{
    future::Future,
    ops::Range,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::{
    arrow::compute::cast,
    common::{DataFusionError, Result, ScalarValue},
    object_store::{path::Path, ObjectStore},
};
use datafusion_datasource::{
    file_stream::{FileOpenFuture, FileOpener},
    PartitionedFile, TableSchema,
};
use futures::{stream::BoxStream, Stream};
use tokio::sync::mpsc;

use crate::{
    adapter_v53::{
        cove_to_datafusion, metrics::CoveFileMetrics, physical_filter::lower_physical_filters,
        stream::DecodeStreamEvent,
    },
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
    output_projection: Option<Vec<usize>>,
    physical_filters: Vec<Arc<dyn datafusion::physical_expr_common::physical_expr::PhysicalExpr>>,
    metrics: CoveFileMetrics,
}

impl CoveFileOpener {
    pub(crate) fn new(
        object_store: Arc<dyn ObjectStore>,
        table_schema: TableSchema,
        options: CoveTableOptions,
        cache: Arc<CoveMetadataCache>,
        projection: Option<Vec<usize>>,
        output_projection: Option<Vec<usize>>,
        physical_filters: Vec<
            Arc<dyn datafusion::physical_expr_common::physical_expr::PhysicalExpr>,
        >,
        metrics: CoveFileMetrics,
    ) -> Self {
        Self {
            object_store,
            table_schema,
            options,
            cache,
            projection,
            output_projection,
            physical_filters,
            metrics,
        }
    }
}

impl FileOpener for CoveFileOpener {
    fn open(&self, partitioned_file: PartitionedFile) -> Result<FileOpenFuture> {
        if partitioned_file.range.is_some() {
            return Err(DataFusionError::Plan(
                "COVE DataFusion v2 adapter does not support DataFusion byte-range repartitioning"
                    .into(),
            ));
        }
        let object_store = Arc::clone(&self.object_store);
        let table_schema = self.table_schema.clone();
        let options = self.options;
        let cache = Arc::clone(&self.cache);
        let projection = self.projection.clone();
        let output_projection = self.output_projection.clone();
        let physical_filters = self.physical_filters.clone();
        let metrics = self.metrics.clone();
        Ok(Box::pin(async move {
            metrics.files_opened.add(1);
            let output_adapter = BatchOutputAdapter::new(
                table_schema.clone(),
                projection.clone(),
                output_projection,
                partitioned_file.partition_values.clone(),
            )?;
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
            if !schemas_compatible(state.schema().as_ref(), table_schema.file_schema().as_ref()) {
                return Err(DataFusionError::Plan(format!(
                    "COVE DataFusion v2 adapter schema mismatch for {}",
                    state.source()
                )));
            }
            let lowered_filters = lower_physical_filters(&state, &physical_filters);
            let plan = plan_scan(&state, projection.as_ref(), lowered_filters.filters)
                .map_err(cove_to_datafusion)?;
            let (sender, receiver) = mpsc::unbounded_channel();
            let handle = tokio::runtime::Handle::current();
            let decode_metrics = metrics.clone();
            let decode_task = tokio::task::spawn_blocking(move || {
                handle.block_on(async move {
                    let mut sink = UnboundedDecodeSink::new(sender.clone(), output_adapter);
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
    output_adapter: BatchOutputAdapter,
    stopped: bool,
}

impl UnboundedDecodeSink {
    fn new(
        sender: mpsc::UnboundedSender<DecodeStreamEvent>,
        output_adapter: BatchOutputAdapter,
    ) -> Self {
        Self {
            sender,
            output_adapter,
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
        let batch = self.output_adapter.adapt(batch)?;
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

#[derive(Debug, Clone)]
struct BatchOutputAdapter {
    table_schema: TableSchema,
    scan_projection: Option<Vec<usize>>,
    output_projection: Option<Vec<usize>>,
    partition_values: Vec<ScalarValue>,
}

impl BatchOutputAdapter {
    fn new(
        table_schema: TableSchema,
        scan_projection: Option<Vec<usize>>,
        output_projection: Option<Vec<usize>>,
        partition_values: Vec<ScalarValue>,
    ) -> Result<Self> {
        if partition_values.len() != table_schema.table_partition_cols().len() {
            return Err(DataFusionError::Plan(format!(
                "COVE DataFusion v2 adapter expected {} partition values, got {}",
                table_schema.table_partition_cols().len(),
                partition_values.len()
            )));
        }
        Ok(Self {
            table_schema,
            scan_projection,
            output_projection,
            partition_values,
        })
    }

    fn adapt(&self, batch: RecordBatch) -> std::result::Result<RecordBatch, cove_core::CoveError> {
        if self.output_projection.is_none() && self.partition_values.is_empty() {
            return Ok(batch);
        }
        let file_field_count = self.table_schema.file_schema().fields().len();
        let table_field_count = self.table_schema.table_schema().fields().len();
        let output_indices = self
            .output_projection
            .clone()
            .unwrap_or_else(|| (0..table_field_count).collect());
        let scan_indices = self
            .scan_projection
            .clone()
            .unwrap_or_else(|| (0..file_field_count).collect());
        let mut arrays = Vec::with_capacity(output_indices.len());
        let mut fields = Vec::with_capacity(output_indices.len());
        for output_index in output_indices {
            let field = Arc::new(self.table_schema.table_schema().field(output_index).clone())
                as Arc<Field>;
            if output_index < file_field_count {
                let Some(batch_index) =
                    scan_indices.iter().position(|index| *index == output_index)
                else {
                    return Err(cove_core::CoveError::BadSchema(format!(
                        "projected COVE file column {output_index} was not decoded"
                    )));
                };
                arrays.push(cast_array_for_field(
                    batch.column(batch_index).clone(),
                    field.data_type(),
                )?);
            } else {
                let partition_index = output_index
                    .checked_sub(file_field_count)
                    .ok_or(cove_core::CoveError::ArithOverflow)?;
                let array = self.partition_values[partition_index]
                    .to_array_of_size(batch.num_rows())
                    .map_err(|err| {
                        cove_core::CoveError::BadSchema(format!(
                            "cannot materialize partition column {partition_index}: {err}"
                        ))
                    })?;
                arrays.push(cast_array_for_field(array as ArrayRef, field.data_type())?);
            }
            fields.push(field);
        }
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|err| cove_core::CoveError::BadSection(format!("Arrow RecordBatch: {err}")))
    }
}

fn schemas_compatible(actual: &Schema, expected: &Schema) -> bool {
    actual.fields().len() == expected.fields().len()
        && actual
            .fields()
            .iter()
            .zip(expected.fields())
            .all(|(actual, expected)| {
                actual.name() == expected.name()
                    && data_types_compatible(actual.data_type(), expected.data_type())
            })
}

fn data_types_compatible(actual: &DataType, expected: &DataType) -> bool {
    actual == expected
        || matches!(
            (actual, expected),
            (DataType::Utf8, DataType::Utf8View)
                | (DataType::Utf8View, DataType::Utf8)
                | (DataType::Binary, DataType::BinaryView)
                | (DataType::BinaryView, DataType::Binary)
        )
}

fn cast_array_for_field(
    array: ArrayRef,
    data_type: &DataType,
) -> std::result::Result<ArrayRef, cove_core::CoveError> {
    if array.data_type() == data_type {
        return Ok(array);
    }
    cast(&array, data_type).map_err(|err| {
        cove_core::CoveError::BadSection(format!(
            "cannot adapt Arrow array from {:?} to {data_type:?}: {err}",
            array.data_type()
        ))
    })
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
