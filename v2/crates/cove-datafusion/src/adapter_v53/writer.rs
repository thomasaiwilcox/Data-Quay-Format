//! DataFusion 53.x bounded file sink for direct COVE-T writes.

use std::{any::Any, collections::BTreeMap, fmt, sync::Arc};

use arrow_array::{Array, ArrayRef, BooleanArray, RecordBatch};
use arrow_cast::display::array_value_to_string;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use arrow_select::filter::filter_record_batch;
use async_trait::async_trait;
use cove_arrow::parquet::{convert_arrow_record_batches, ParquetConversionOptions};
use cove_core::checksum;
use datafusion::{
    arrow::compute::cast,
    common::{not_impl_err, DataFusionError, Result},
    execution::{SendableRecordBatchStream, TaskContext},
    logical_expr::dml::InsertOp,
    object_store::{path::Path, ObjectStore, ObjectStoreExt, PutPayload},
    physical_plan::{metrics::MetricsSet, DisplayAs, DisplayFormatType},
};
use datafusion_datasource::{file_sink_config::FileSinkConfig, sink::DataSink};
use futures::StreamExt;

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
                "COVE DataFusion v2 writer supports append/overwrite output files, not {}",
                self.config.insert_op
            );
        }
        let writer_schema = normalized_writer_schema(self.config.output_schema());
        let partition_indexes = partition_indexes(&writer_schema, &self.config)?;
        let object_store = context
            .runtime_env()
            .object_store(&self.config.object_store_url)?;
        let mut row_count = 0u64;
        let mut batches = Vec::new();
        while let Some(batch) = data.next().await.transpose()? {
            row_count = row_count
                .checked_add(batch.num_rows() as u64)
                .ok_or_else(|| {
                    DataFusionError::Execution("COVE write row count overflow".into())
                })?;
            let batch = normalize_batch(batch, Arc::clone(&writer_schema))?;
            batches.push(batch);
        }
        if partition_indexes.is_empty() {
            let path = output_path(&self.config)?;
            write_cove_batches(
                &object_store,
                path,
                Arc::clone(&writer_schema),
                batches,
                &self.config.original_url,
            )
            .await?;
        } else {
            let partitioned = partition_batches(&batches, &partition_indexes)?;
            let base = output_directory(&self.config)?;
            for (ordinal, (partition_values, partition_batches)) in
                partitioned.into_iter().enumerate()
            {
                let mut path = base.clone();
                for ((name, _), value) in self
                    .config
                    .table_partition_cols
                    .iter()
                    .zip(partition_values.iter())
                {
                    path = path.join(format!("{}={}", escape_hive_segment(name), value));
                }
                path = path.join(format!("part-{ordinal:05}.{}", self.config.file_extension));
                write_cove_batches(
                    &object_store,
                    path,
                    Arc::clone(&writer_schema),
                    partition_batches,
                    &self.config.original_url,
                )
                .await?;
            }
        }
        Ok(row_count)
    }
}

async fn write_cove_batches(
    object_store: &Arc<dyn ObjectStore>,
    path: Path,
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
    source_identifier: &str,
) -> Result<()> {
    let mut conversion = ParquetConversionOptions::default();
    conversion.table_name = "datafusion_write".into();
    conversion.namespace = "datafusion".into();
    conversion.source_identifier = Some(source_identifier.to_string());
    conversion.source_digest = None;
    let fingerprint = format!(
        "arrow-schema-crc32c:{:08x}",
        checksum::crc32c(format!("{schema:?}").as_bytes())
    );
    let cove = convert_arrow_record_batches(
        "arrow:datafusion",
        fingerprint,
        schema,
        batches,
        &conversion,
    )
    .map_err(crate::adapter_v53::cove_to_datafusion)?;
    object_store
        .put(&path, PutPayload::from(cove.cove_bytes))
        .await
        .map_err(|err| DataFusionError::External(Box::new(err)))?;
    Ok(())
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

fn output_directory(config: &FileSinkConfig) -> Result<Path> {
    let Some(base) = config.table_paths.first() else {
        return Err(DataFusionError::Plan(
            "COVE DataFusion v2 writer requires an output table path".into(),
        ));
    };
    Ok(base.prefix().clone())
}

fn partition_indexes(schema: &SchemaRef, config: &FileSinkConfig) -> Result<Vec<usize>> {
    config
        .table_partition_cols
        .iter()
        .map(|(name, _)| {
            schema.index_of(name).map_err(|_| {
                DataFusionError::Plan(format!(
                    "COVE DataFusion v2 writer partition column {name} is not present in output schema"
                ))
            })
        })
        .collect()
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

fn partition_batches(
    batches: &[RecordBatch],
    partition_indexes: &[usize],
) -> Result<BTreeMap<Vec<String>, Vec<RecordBatch>>> {
    let mut out: BTreeMap<Vec<String>, Vec<RecordBatch>> = BTreeMap::new();
    for batch in batches {
        let mut masks: BTreeMap<Vec<String>, Vec<bool>> = BTreeMap::new();
        for row in 0..batch.num_rows() {
            let key = partition_indexes
                .iter()
                .map(|index| {
                    let array = batch.column(*index);
                    if array.is_null(row) {
                        Ok("__HIVE_DEFAULT_PARTITION__".to_string())
                    } else {
                        let value = array_value_to_string(array.as_ref(), row)
                            .map_err(DataFusionError::from)?;
                        Ok(escape_hive_segment(&value))
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            let mask = masks
                .entry(key)
                .or_insert_with(|| vec![false; batch.num_rows()]);
            mask[row] = true;
        }
        for (key, mask) in masks {
            let predicate = BooleanArray::from(mask);
            let filtered = filter_record_batch(batch, &predicate).map_err(DataFusionError::from)?;
            out.entry(key).or_default().push(filtered);
        }
    }
    Ok(out)
}

fn escape_hive_segment(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'%' => escaped.push_str("%25"),
            b'/' => escaped.push_str("%2F"),
            b'=' => escaped.push_str("%3D"),
            b'\0' => escaped.push_str("%00"),
            other => escaped.push(other as char),
        }
    }
    escaped
}
