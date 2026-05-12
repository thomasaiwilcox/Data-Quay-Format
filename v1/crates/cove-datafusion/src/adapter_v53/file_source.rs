//! DataFusion 53.x file-source glue.

use std::{any::Any, fmt, sync::Arc};

use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::{
    common::{config::ConfigOptions, DataFusionError, Result},
    object_store::ObjectStore,
    physical_expr::expressions::Column,
    physical_expr_common::physical_expr::PhysicalExpr,
    physical_plan::{
        filter_pushdown::{FilterPushdownPropagation, PushedDown},
        metrics::ExecutionPlanMetricsSet,
        DisplayFormatType,
    },
};
use datafusion_datasource::{
    file::FileSource, file_scan_config::FileScanConfig, file_stream::FileOpener, TableSchema,
};

use crate::{
    adapter_v53::{file_opener::CoveFileOpener, metrics::CoveFileMetrics},
    bootstrap::CoveMetadataCache,
    options::CoveTableOptions,
};

#[derive(Debug, Clone)]
pub struct CoveFileSource {
    table_schema: TableSchema,
    options: CoveTableOptions,
    cache: Arc<CoveMetadataCache>,
    projection: Option<ProjectionExprs>,
    scan_projection: Option<Vec<usize>>,
    batch_size: Option<usize>,
    metrics: ExecutionPlanMetricsSet,
}

impl CoveFileSource {
    pub fn new(
        table_schema: TableSchema,
        options: CoveTableOptions,
        cache: Arc<CoveMetadataCache>,
    ) -> Self {
        Self {
            table_schema,
            options,
            cache,
            projection: None,
            scan_projection: None,
            batch_size: None,
            metrics: ExecutionPlanMetricsSet::new(),
        }
    }
}

impl FileSource for CoveFileSource {
    fn create_file_opener(
        &self,
        object_store: Arc<dyn ObjectStore>,
        base_config: &FileScanConfig,
        partition: usize,
    ) -> Result<Arc<dyn FileOpener>> {
        if !self.table_schema.table_partition_cols().is_empty() {
            return Err(DataFusionError::Plan(
                "COVE DataFusion M2 compatibility does not support partition columns".into(),
            ));
        }
        let _ = base_config;
        Ok(Arc::new(CoveFileOpener::new(
            object_store,
            self.table_schema.clone(),
            self.options,
            Arc::clone(&self.cache),
            self.scan_projection.clone(),
            CoveFileMetrics::new(&self.metrics, partition),
        )))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn table_schema(&self) -> &TableSchema {
        &self.table_schema
    }

    fn with_batch_size(&self, batch_size: usize) -> Arc<dyn FileSource> {
        let mut source = self.clone();
        source.batch_size = Some(batch_size);
        Arc::new(source)
    }

    fn projection(&self) -> Option<&ProjectionExprs> {
        self.projection.as_ref()
    }

    fn metrics(&self) -> &ExecutionPlanMetricsSet {
        &self.metrics
    }

    fn file_type(&self) -> &str {
        "cove"
    }

    fn fmt_extra(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(projection) = &self.scan_projection {
            write!(f, ", cove_projection={projection:?}")?;
        }
        Ok(())
    }

    fn supports_repartitioning(&self) -> bool {
        false
    }

    fn try_pushdown_filters(
        &self,
        filters: Vec<Arc<dyn PhysicalExpr>>,
        _config: &ConfigOptions,
    ) -> Result<FilterPushdownPropagation<Arc<dyn FileSource>>> {
        Ok(FilterPushdownPropagation::with_parent_pushdown_result(
            vec![PushedDown::No; filters.len()],
        ))
    }

    fn try_pushdown_projection(
        &self,
        projection: &ProjectionExprs,
    ) -> Result<Option<Arc<dyn FileSource>>> {
        let projection = match &self.projection {
            Some(existing) => existing.try_merge(projection)?,
            None => projection.clone(),
        };
        let Some(indices) = direct_projection_indices(&projection, &self.table_schema)? else {
            return Ok(None);
        };
        let mut source = self.clone();
        source.projection = Some(projection);
        source.scan_projection = Some(indices);
        Ok(Some(Arc::new(source)))
    }
}

fn direct_projection_indices(
    projection: &ProjectionExprs,
    table_schema: &TableSchema,
) -> Result<Option<Vec<usize>>> {
    let file_field_count = table_schema.file_schema().fields().len();
    let mut indices = Vec::with_capacity(projection.as_ref().len());
    for expr in projection.iter() {
        let Some(column) = expr.expr.as_any().downcast_ref::<Column>() else {
            return Ok(None);
        };
        if column.index() >= file_field_count {
            return Ok(None);
        }
        let field = table_schema.file_schema().field(column.index());
        if expr.alias != *field.name() || column.name() != field.name() {
            return Ok(None);
        }
        indices.push(column.index());
    }
    Ok(Some(indices))
}
