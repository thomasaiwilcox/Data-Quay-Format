//! DataFusion 53.x file-format integration path.

use std::{any::Any, collections::HashMap, sync::Arc};

use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::{
    catalog::{Session, TableProvider, TableProviderFactory},
    common::{not_impl_err, stats::Precision, DataFusionError, GetExt, Result, Statistics},
    datasource::{
        file_format::{file_compression_type::FileCompressionType, FileFormat, FileFormatFactory},
        listing_table_factory::ListingTableFactory,
    },
    logical_expr::CreateExternalTable,
    object_store::{ObjectMeta, ObjectStore},
    physical_plan::{metrics::ExecutionPlanMetricsSet, ExecutionPlan},
};
use datafusion_datasource::{
    file::FileSource, file_scan_config::FileScanConfig, source::DataSourceExec, TableSchema,
};

use crate::{
    adapter_v53::{
        cove_to_datafusion, file_opener::ObjectStoreRangeReader, file_source::CoveFileSource,
        metrics::CoveFileMetrics,
    },
    bootstrap::{bootstrap_range_reader_with_options, CoveMetadataCache},
    options::CoveTableOptions,
};

#[derive(Debug, Clone)]
pub struct CoveFileFormat {
    options: CoveTableOptions,
    cache: Arc<CoveMetadataCache>,
}

impl CoveFileFormat {
    pub fn new(options: CoveTableOptions) -> Self {
        Self {
            options,
            cache: Arc::new(CoveMetadataCache::default()),
        }
    }

    pub fn options(&self) -> CoveTableOptions {
        self.options
    }

    pub fn cache(&self) -> Arc<CoveMetadataCache> {
        Arc::clone(&self.cache)
    }

    async fn state_for_object(
        &self,
        store: &Arc<dyn ObjectStore>,
        object: &ObjectMeta,
    ) -> Result<Arc<crate::dataset_state::DatasetState>> {
        let metrics_set = ExecutionPlanMetricsSet::new();
        let metrics = CoveFileMetrics::new(&metrics_set, 0);
        let reader =
            ObjectStoreRangeReader::new(Arc::clone(store), object.location.clone(), metrics);
        bootstrap_range_reader_with_options(
            object.location.to_string(),
            object.size,
            &reader,
            self.options,
            Some(self.cache.as_ref()),
        )
        .await
        .map_err(cove_to_datafusion)
    }
}

impl Default for CoveFileFormat {
    fn default() -> Self {
        Self::new(CoveTableOptions::default())
    }
}

#[async_trait]
impl FileFormat for CoveFileFormat {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn get_ext(&self) -> String {
        "cove".into()
    }

    fn get_ext_with_compression(
        &self,
        file_compression_type: &FileCompressionType,
    ) -> Result<String> {
        Ok(format!(
            "{}{}",
            self.get_ext(),
            file_compression_type.get_ext()
        ))
    }

    fn compression_type(&self) -> Option<FileCompressionType> {
        None
    }

    async fn infer_schema(
        &self,
        _state: &dyn Session,
        store: &Arc<dyn ObjectStore>,
        objects: &[ObjectMeta],
    ) -> Result<SchemaRef> {
        let Some(first) = objects.first() else {
            return Err(DataFusionError::Plan(
                "COVE DataFusion M2 cannot infer schema from an empty listing".into(),
            ));
        };
        let first_state = self.state_for_object(store, first).await?;
        let schema = first_state.schema();
        for object in &objects[1..] {
            let state = self.state_for_object(store, object).await?;
            if state.schema().as_ref() != schema.as_ref() {
                return Err(DataFusionError::Plan(format!(
                    "COVE DataFusion M2 schema mismatch between {} and {}",
                    first_state.source(),
                    state.source()
                )));
            }
        }
        Ok(schema)
    }

    async fn infer_stats(
        &self,
        _state: &dyn Session,
        store: &Arc<dyn ObjectStore>,
        table_schema: SchemaRef,
        object: &ObjectMeta,
    ) -> Result<Statistics> {
        let state = self.state_for_object(store, object).await?;
        let mut statistics = Statistics::new_unknown(table_schema.as_ref());
        statistics.num_rows = Precision::Exact(state.table().row_count as usize);
        statistics.calculate_total_byte_size(table_schema.as_ref());
        Ok(statistics)
    }

    async fn create_physical_plan(
        &self,
        _state: &dyn Session,
        conf: FileScanConfig,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        Ok(DataSourceExec::from_data_source(conf) as Arc<dyn ExecutionPlan>)
    }

    async fn create_writer_physical_plan(
        &self,
        _input: Arc<dyn ExecutionPlan>,
        _state: &dyn Session,
        _conf: datafusion_datasource::file_sink_config::FileSinkConfig,
        _order_requirements: Option<datafusion::physical_expr_common::sort_expr::LexRequirement>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        not_impl_err!("COVE DataFusion M2 compatibility is read-only")
    }

    fn file_source(&self, table_schema: TableSchema) -> Arc<dyn FileSource> {
        Arc::new(CoveFileSource::new(
            table_schema,
            self.options,
            Arc::clone(&self.cache),
        ))
    }
}

#[derive(Debug, Default)]
pub struct CoveFormatFactory;

impl GetExt for CoveFormatFactory {
    fn get_ext(&self) -> String {
        "cove".into()
    }
}

impl FileFormatFactory for CoveFormatFactory {
    fn create(
        &self,
        _state: &dyn Session,
        format_options: &HashMap<String, String>,
    ) -> Result<Arc<dyn FileFormat>> {
        if !format_options.is_empty() {
            let mut keys = format_options.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            return Err(DataFusionError::Plan(format!(
                "COVE DataFusion M2 does not support SQL format options: {}",
                keys.join(", ")
            )));
        }
        Ok(Arc::new(CoveFileFormat::default()))
    }

    fn default(&self) -> Arc<dyn FileFormat> {
        Arc::new(CoveFileFormat::default())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Default)]
pub struct CoveTableFactory {
    listing: ListingTableFactory,
}

impl CoveTableFactory {
    pub fn new() -> Self {
        Self {
            listing: ListingTableFactory::new(),
        }
    }
}

#[async_trait]
impl TableProviderFactory for CoveTableFactory {
    async fn create(
        &self,
        state: &dyn Session,
        cmd: &CreateExternalTable,
    ) -> Result<Arc<dyn TableProvider>> {
        if cmd.unbounded {
            return Err(DataFusionError::Plan(
                "COVE DataFusion M2 SQL external tables are bounded only".into(),
            ));
        }
        self.listing.create(state, cmd).await
    }
}
