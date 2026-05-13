//! DataFusion 53.x file-source glue.

use std::{any::Any, fmt, sync::Arc};

use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::{
    common::{config::ConfigOptions, Result},
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
    adapter_v53::{
        file_opener::CoveFileOpener, metrics::CoveFileMetrics,
        physical_filter::lower_physical_filter,
    },
    bootstrap::CoveMetadataCache,
    dataset_state::DatasetState,
    options::{CoveTableOptions, FilterResidualPolicy},
    planner::CoveFilterUse,
};

#[derive(Debug, Clone)]
pub struct CoveFileSource {
    table_schema: TableSchema,
    options: CoveTableOptions,
    cache: Arc<CoveMetadataCache>,
    projection: Option<ProjectionExprs>,
    output_projection: Option<Vec<usize>>,
    scan_projection: Option<Vec<usize>>,
    physical_filters: Vec<Arc<dyn PhysicalExpr>>,
    exactness_states: Option<Arc<Vec<Arc<DatasetState>>>>,
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
            output_projection: None,
            scan_projection: None,
            physical_filters: Vec::new(),
            exactness_states: None,
            batch_size: None,
            metrics: ExecutionPlanMetricsSet::new(),
        }
    }

    pub(crate) fn with_exactness_states(mut self, states: Arc<Vec<Arc<DatasetState>>>) -> Self {
        self.exactness_states = Some(states);
        self
    }
}

impl FileSource for CoveFileSource {
    fn create_file_opener(
        &self,
        object_store: Arc<dyn ObjectStore>,
        base_config: &FileScanConfig,
        partition: usize,
    ) -> Result<Arc<dyn FileOpener>> {
        let _ = base_config;
        Ok(Arc::new(CoveFileOpener::new(
            object_store,
            self.table_schema.clone(),
            self.options,
            Arc::clone(&self.cache),
            self.scan_projection.clone(),
            self.output_projection.clone(),
            self.physical_filters.clone(),
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
        if !self.physical_filters.is_empty() {
            write!(f, ", cove_advisory_filters={}", self.physical_filters.len())?;
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
        let pushed = filters
            .iter()
            .map(|filter| {
                if self.options.filter_residual_policy()
                    == FilterResidualPolicy::ElideExactWhenProven
                    && self.filter_is_exact_for_every_file(filter)
                {
                    PushedDown::Yes
                } else {
                    PushedDown::No
                }
            })
            .collect::<Vec<_>>();
        let mut source = self.clone();
        source.physical_filters.extend(filters);
        Ok(
            FilterPushdownPropagation::with_parent_pushdown_result(pushed)
                .with_updated_node(Arc::new(source)),
        )
    }

    fn try_pushdown_projection(
        &self,
        projection: &ProjectionExprs,
    ) -> Result<Option<Arc<dyn FileSource>>> {
        let projection = match &self.projection {
            Some(existing) => existing.try_merge(projection)?,
            None => projection.clone(),
        };
        let Some(direct_projection) = direct_projection_indices(&projection, &self.table_schema)?
        else {
            return Ok(None);
        };
        let mut source = self.clone();
        source.projection = Some(projection);
        source.output_projection = Some(direct_projection.output_indices);
        source.scan_projection = Some(direct_projection.file_indices);
        Ok(Some(Arc::new(source)))
    }
}

impl CoveFileSource {
    fn filter_is_exact_for_every_file(&self, filter: &Arc<dyn PhysicalExpr>) -> bool {
        let Some(states) = &self.exactness_states else {
            return false;
        };
        if states.is_empty() {
            return false;
        }
        states.iter().all(|state| {
            let lowered = lower_physical_filter(state, filter.as_ref());
            lowered.all_supported()
                && lowered
                    .filters
                    .iter()
                    .all(|filter| filter.use_kind == CoveFilterUse::FullRowPredicateExact)
        })
    }
}

struct DirectProjection {
    output_indices: Vec<usize>,
    file_indices: Vec<usize>,
}

fn direct_projection_indices(
    projection: &ProjectionExprs,
    table_schema: &TableSchema,
) -> Result<Option<DirectProjection>> {
    let file_field_count = table_schema.file_schema().fields().len();
    let table_field_count = table_schema.table_schema().fields().len();
    let mut output_indices = Vec::with_capacity(projection.as_ref().len());
    let mut file_indices = Vec::new();
    for expr in projection.iter() {
        let Some(column) = expr.expr.as_any().downcast_ref::<Column>() else {
            return Ok(None);
        };
        if column.index() >= table_field_count {
            return Ok(None);
        }
        let field = table_schema.table_schema().field(column.index());
        if expr.alias != *field.name() || column.name() != field.name() {
            return Ok(None);
        }
        output_indices.push(column.index());
        if column.index() < file_field_count && !file_indices.contains(&column.index()) {
            file_indices.push(column.index());
        }
    }
    Ok(Some(DirectProjection {
        output_indices,
        file_indices,
    }))
}
