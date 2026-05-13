//! DataFusion 53.x bounded file sink for starter COVE-T writes.

use std::{any::Any, fmt, sync::Arc};

use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use async_trait::async_trait;
use cove_arrow::parquet::{convert_parquet_bytes, ParquetConversionOptions};
use datafusion::{
    arrow::compute::cast,
    common::{not_impl_err, DataFusionError, Result},
    execution::{SendableRecordBatchStream, TaskContext},
    logical_expr::dml::InsertOp,
    object_store::{path::Path, ObjectStoreExt, PutPayload},
    physical_plan::{metrics::MetricsSet, DisplayAs, DisplayFormatType},
};
use datafusion_datasource::{file_sink_config::FileSinkConfig, sink::DataSink};
use futures::StreamExt;
use parquet::arrow::ArrowWriter;

#[derive(Debug)]
pub(crate) struct CoveFileSink {
    config: FileSinkConfig,
}

impl CoveFileSink {
    pub(crate) fn new(config: FileSinkConfig) -> Self {
        Self { config }
    }
}

impl DisplayAs for CoveFileSink {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => {
                write!(f, "CoveFileSink(file={})", self.config.original_url)
            }
            DisplayFormatType::TreeRender => {
                writeln!(f, "format: cove")?;
                write!(f, "file={}", self.config.original_url)
            }
        }
    }
}

#[async_trait]
impl DataSink for CoveFileSink {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn metrics(&self) -> Option<MetricsSet> {
        None
    }

    fn schema(&self) -> &SchemaRef {
        self.config.output_schema()
    }

    async fn write_all(
        &self,
        mut data: SendableRecordBatchStream,
        context: &Arc<TaskContext>,
    ) -> Result<u64> {
        if !matches!(
            self.config.insert_op,
            InsertOp::Append | InsertOp::Overwrite
        ) {
            return not_impl_err!(
                "COVE DataFusion v2 starter writer supports append/overwrite output files, not {}",
                self.config.insert_op
            );
        }
        if !self.config.table_partition_cols.is_empty() {
            return not_impl_err!(
                "COVE DataFusion v2 starter writer does not support partitioned writes"
            );
        }
        validate_supported_schema(self.config.output_schema())?;
        let writer_schema = normalized_writer_schema(self.config.output_schema());

        let mut parquet_bytes = Vec::new();
        let mut writer = ArrowWriter::try_new(&mut parquet_bytes, Arc::clone(&writer_schema), None)
            .map_err(|err| DataFusionError::External(Box::new(err)))?;
        let mut row_count = 0u64;
        while let Some(batch) = data.next().await.transpose()? {
            validate_supported_schema(&batch.schema())?;
            row_count = row_count
                .checked_add(batch.num_rows() as u64)
                .ok_or_else(|| {
                    DataFusionError::Execution("COVE write row count overflow".into())
                })?;
            let batch = normalize_batch(batch, Arc::clone(&writer_schema))?;
            writer
                .write(&batch)
                .map_err(|err| DataFusionError::External(Box::new(err)))?;
        }
        writer
            .close()
            .map_err(|err| DataFusionError::External(Box::new(err)))?;

        let mut conversion = ParquetConversionOptions::default();
        conversion.table_name = "datafusion_write".into();
        conversion.namespace = "datafusion".into();
        let cove = convert_parquet_bytes(&parquet_bytes, &conversion)
            .map_err(crate::adapter_v53::cove_to_datafusion)?;
        let object_store = context
            .runtime_env()
            .object_store(&self.config.object_store_url)?;
        let path = output_path(&self.config)?;
        object_store
            .put(&path, PutPayload::from(cove.cove_bytes))
            .await
            .map_err(|err| DataFusionError::External(Box::new(err)))?;
        Ok(row_count)
    }
}

fn output_path(config: &FileSinkConfig) -> Result<Path> {
    let Some(base) = config.table_paths.first() else {
        return Err(DataFusionError::Plan(
            "COVE DataFusion v2 writer requires an output table path".into(),
        ));
    };
    if config.file_output_mode.single_file_output(base) {
        Ok(base.prefix().clone())
    } else {
        Ok(base
            .prefix()
            .clone()
            .join(format!("part-0.{}", config.file_extension)))
    }
}

fn validate_supported_schema(schema: &SchemaRef) -> Result<()> {
    for field in schema.fields() {
        validate_supported_type(field.name(), field.data_type())?;
    }
    Ok(())
}

fn normalized_writer_schema(schema: &SchemaRef) -> SchemaRef {
    Arc::new(Schema::new_with_metadata(
        schema
            .fields()
            .iter()
            .map(|field| {
                Field::new(
                    field.name(),
                    normalized_writer_type(field.data_type()),
                    field.is_nullable(),
                )
            })
            .collect::<Vec<_>>(),
        schema.metadata().clone(),
    ))
}

fn normalized_writer_type(data_type: &DataType) -> DataType {
    match data_type {
        DataType::Utf8View => DataType::Utf8,
        DataType::BinaryView => DataType::Binary,
        other => other.clone(),
    }
}

fn normalize_batch(batch: RecordBatch, schema: SchemaRef) -> Result<RecordBatch> {
    let arrays = batch
        .columns()
        .iter()
        .zip(schema.fields())
        .map(|(array, field)| {
            if array.data_type() == field.data_type() {
                Ok(Arc::clone(array))
            } else {
                cast(array, field.data_type()).map_err(DataFusionError::from)
            }
        })
        .collect::<Result<Vec<ArrayRef>>>()?;
    RecordBatch::try_new(schema, arrays).map_err(DataFusionError::from)
}

fn validate_supported_type(name: &str, data_type: &DataType) -> Result<()> {
    match data_type {
        DataType::Boolean
        | DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::Float32
        | DataType::Float64
        | DataType::Date32
        | DataType::Utf8
        | DataType::Utf8View
        | DataType::LargeUtf8
        | DataType::Binary
        | DataType::BinaryView
        | DataType::LargeBinary
        | DataType::Timestamp(
            TimeUnit::Second
            | TimeUnit::Millisecond
            | TimeUnit::Microsecond
            | TimeUnit::Nanosecond,
            None,
        ) => Ok(()),
        other => not_impl_err!(
            "COVE DataFusion v2 starter writer does not support column {name} with Arrow type {other:?}"
        ),
    }
}
