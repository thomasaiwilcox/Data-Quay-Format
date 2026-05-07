//! DataFusion 53.x execution-plan glue.

use std::{any::Any, sync::Arc};

use arrow_schema::SchemaRef;
use datafusion::{
    common::{stats::Precision, DataFusionError, Result, Statistics},
    execution::{SendableRecordBatchStream, TaskContext},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        execution_plan::{Boundedness, EmissionType},
        metrics::{ExecutionPlanMetricsSet, MetricsSet},
        DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
    },
};

#[cfg(feature = "dynamic-filters")]
use datafusion::{
    common::config::ConfigOptions,
    physical_expr::PhysicalExpr,
    physical_plan::filter_pushdown::{
        ChildPushdownResult, FilterPushdownPhase, FilterPushdownPropagation, PushedDown,
    },
};

use crate::{
    adapter_v53::{
        explain::format_cove_exec,
        stream::{CoveRecordBatchStream, CoveStreamMetrics},
    },
    dataset_state::DatasetState,
    decode::ScanExecutionCache,
    planner::ScanPlan,
    task_graph::{build_task_graph, TaskGraph},
};

#[derive(Debug)]
pub struct CoveExec {
    state: Arc<DatasetState>,
    plan: ScanPlan,
    task_graph: Arc<TaskGraph>,
    schema: SchemaRef,
    properties: Arc<PlanProperties>,
    metrics: ExecutionPlanMetricsSet,
    scan_cache: Arc<ScanExecutionCache>,
    #[cfg(feature = "dynamic-filters")]
    dynamic_filters: Vec<Arc<dyn PhysicalExpr>>,
}

impl CoveExec {
    pub fn try_new(state: Arc<DatasetState>, plan: ScanPlan) -> Result<Self> {
        let schema = Arc::clone(&plan.output_schema);
        let task_graph = Arc::new(
            build_task_graph(&state, &plan).map_err(crate::adapter_v53::cove_to_datafusion)?,
        );
        let partition_count = task_graph.partitions.len();
        let properties = Arc::new(PlanProperties::new(
            EquivalenceProperties::new(Arc::clone(&schema)),
            Partitioning::UnknownPartitioning(partition_count),
            EmissionType::Incremental,
            Boundedness::Bounded,
        ));
        Ok(Self {
            state,
            plan,
            task_graph,
            schema,
            properties,
            metrics: ExecutionPlanMetricsSet::new(),
            scan_cache: Arc::new(ScanExecutionCache::default()),
            #[cfg(feature = "dynamic-filters")]
            dynamic_filters: Vec::new(),
        })
    }

    fn statistics_for_schema(&self, partition: Option<usize>) -> Statistics {
        let mut statistics = Statistics::new_unknown(self.schema.as_ref());
        if partition.is_none() {
            if let Ok(row_count) = self.state.exact_visible_row_count() {
                if let Ok(row_count) = usize::try_from(row_count) {
                    statistics.num_rows = Precision::Exact(row_count);
                }
            }
        }
        statistics.calculate_total_byte_size(self.schema.as_ref());
        statistics
    }
}

impl DisplayAs for CoveExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => {
                write!(f, "{}", format_cove_exec(&self.state, &self.plan))
            }
            DisplayFormatType::TreeRender => write!(f, "CoveExec"),
        }
    }
}

impl ExecutionPlan for CoveExec {
    fn name(&self) -> &str {
        "CoveExec"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        Vec::new()
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if !children.is_empty() {
            return Err(DataFusionError::Internal(
                "CoveExec is a leaf execution plan".into(),
            ));
        }
        Ok(self)
    }

    fn execute(
        &self,
        partition: usize,
        _context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        if partition >= self.task_graph.partitions.len() {
            return Err(DataFusionError::Internal(format!(
                "CoveExec has {} partitions, got partition {partition}",
                self.task_graph.partitions.len()
            )));
        }
        let metrics = CoveStreamMetrics::new(&self.metrics, partition);
        let tasks = self.task_graph.partitions[partition].tasks.clone();
        Ok(Box::pin(CoveRecordBatchStream::new(
            Arc::clone(&self.schema),
            Arc::clone(&self.state),
            Arc::clone(&self.scan_cache),
            self.plan.clone(),
            tasks,
            partition,
            self.task_graph.partitions.len(),
            #[cfg(feature = "dynamic-filters")]
            self.dynamic_filters.clone(),
            metrics,
        )))
    }

    fn metrics(&self) -> Option<MetricsSet> {
        Some(self.metrics.clone_inner())
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        if let Some(partition) = partition {
            if partition >= self.task_graph.partitions.len() {
                return Err(DataFusionError::Internal(format!(
                    "CoveExec has {} partitions, got partition {partition}",
                    self.task_graph.partitions.len()
                )));
            }
        }
        Ok(self.statistics_for_schema(partition))
    }

    #[cfg(feature = "dynamic-filters")]
    fn handle_child_pushdown_result(
        &self,
        _phase: FilterPushdownPhase,
        child_pushdown_result: ChildPushdownResult,
        _config: &ConfigOptions,
    ) -> Result<FilterPushdownPropagation<Arc<dyn ExecutionPlan>>> {
        let parent_filters = child_pushdown_result
            .parent_filters
            .into_iter()
            .map(|filter| filter.filter)
            .collect::<Vec<_>>();
        let pushdown_result = vec![PushedDown::No; parent_filters.len()];
        if parent_filters.is_empty() || !self.state.dynamic_filters_enabled() {
            return Ok(FilterPushdownPropagation::with_parent_pushdown_result(
                pushdown_result,
            ));
        }

        let mut dynamic_filters = self.dynamic_filters.clone();
        dynamic_filters.extend(parent_filters);
        let updated = Arc::new(Self {
            state: Arc::clone(&self.state),
            plan: self.plan.clone(),
            task_graph: Arc::clone(&self.task_graph),
            schema: Arc::clone(&self.schema),
            properties: Arc::clone(&self.properties),
            metrics: ExecutionPlanMetricsSet::new(),
            dynamic_filters,
        }) as Arc<dyn ExecutionPlan>;
        Ok(
            FilterPushdownPropagation::with_parent_pushdown_result(pushdown_result)
                .with_updated_node(updated),
        )
    }
}
