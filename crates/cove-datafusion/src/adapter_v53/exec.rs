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

const FILTERED_STREAMING_MIN_TASK_ROWS: u64 = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoveMaterializationMode {
    Streaming,
    Buffered,
}

impl CoveMaterializationMode {
    fn choose(
        plan: &ScanPlan,
        task_graph: &TaskGraph,
        fetch: Option<usize>,
        has_dynamic_filters: bool,
    ) -> Self {
        if fetch.is_some()
            || plan.topn_hint.is_some()
            || has_dynamic_filters
            || filtered_scan_should_stream(plan, task_graph)
        {
            Self::Streaming
        } else {
            Self::Buffered
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Streaming => "streaming",
            Self::Buffered => "buffered",
        }
    }

    fn emission_type(self) -> EmissionType {
        match self {
            Self::Streaming => EmissionType::Incremental,
            Self::Buffered => EmissionType::Final,
        }
    }
}

fn filtered_scan_should_stream(plan: &ScanPlan, task_graph: &TaskGraph) -> bool {
    if plan.filters.is_empty() {
        return false;
    }
    let task_rows = task_graph
        .tasks
        .iter()
        .map(|task| u64::from(task.row_count))
        .sum::<u64>();
    task_rows >= FILTERED_STREAMING_MIN_TASK_ROWS
}

#[derive(Debug)]
pub struct CoveExec {
    state: Arc<DatasetState>,
    plan: ScanPlan,
    task_graph: Arc<TaskGraph>,
    schema: SchemaRef,
    properties: Arc<PlanProperties>,
    metrics: ExecutionPlanMetricsSet,
    scan_cache: Arc<ScanExecutionCache>,
    fetch: Option<usize>,
    materialization_mode: CoveMaterializationMode,
    #[cfg(feature = "dynamic-filters")]
    dynamic_filters: Vec<Arc<dyn PhysicalExpr>>,
}

impl CoveExec {
    pub fn try_new(state: Arc<DatasetState>, plan: ScanPlan) -> Result<Self> {
        Self::try_new_with_fetch(state, plan, None)
    }

    pub(crate) fn try_new_with_fetch(
        state: Arc<DatasetState>,
        plan: ScanPlan,
        fetch: Option<usize>,
    ) -> Result<Self> {
        let schema = Arc::clone(&plan.output_schema);
        let task_graph = Arc::new(
            build_task_graph(&state, &plan).map_err(crate::adapter_v53::cove_to_datafusion)?,
        );
        let materialization_mode =
            CoveMaterializationMode::choose(&plan, &task_graph, fetch, false);
        let properties =
            plan_properties(&schema, task_graph.partitions.len(), materialization_mode);
        Ok(Self {
            state,
            plan,
            task_graph,
            schema,
            properties,
            metrics: ExecutionPlanMetricsSet::new(),
            scan_cache: Arc::new(ScanExecutionCache::default()),
            fetch,
            materialization_mode,
            #[cfg(feature = "dynamic-filters")]
            dynamic_filters: Vec::new(),
        })
    }

    fn selected_materialization_mode(
        &self,
        fetch: Option<usize>,
        has_dynamic_filters: bool,
    ) -> CoveMaterializationMode {
        CoveMaterializationMode::choose(&self.plan, &self.task_graph, fetch, has_dynamic_filters)
    }

    fn has_dynamic_filters(&self) -> bool {
        #[cfg(feature = "dynamic-filters")]
        {
            !self.dynamic_filters.is_empty()
        }
        #[cfg(not(feature = "dynamic-filters"))]
        {
            false
        }
    }

    fn properties_for_mode(&self, mode: CoveMaterializationMode) -> Arc<PlanProperties> {
        plan_properties(&self.schema, self.task_graph.partitions.len(), mode)
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

fn plan_properties(
    schema: &SchemaRef,
    partition_count: usize,
    materialization_mode: CoveMaterializationMode,
) -> Arc<PlanProperties> {
    Arc::new(PlanProperties::new(
        EquivalenceProperties::new(Arc::clone(schema)),
        Partitioning::UnknownPartitioning(partition_count),
        materialization_mode.emission_type(),
        Boundedness::Bounded,
    ))
}

impl DisplayAs for CoveExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => {
                write!(
                    f,
                    "{}",
                    format_cove_exec(&self.state, &self.plan, self.materialization_mode.as_str())
                )
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
        match self.materialization_mode {
            CoveMaterializationMode::Streaming => {
                Ok(Box::pin(CoveRecordBatchStream::new_streaming(
                    Arc::clone(&self.schema),
                    Arc::clone(&self.state),
                    Arc::clone(&self.scan_cache),
                    self.plan.clone(),
                    tasks,
                    partition,
                    self.task_graph.partitions.len(),
                    self.fetch,
                    #[cfg(feature = "dynamic-filters")]
                    self.dynamic_filters.clone(),
                    metrics,
                )))
            }
            CoveMaterializationMode::Buffered => Ok(Box::pin(CoveRecordBatchStream::new_buffered(
                Arc::clone(&self.schema),
                Arc::clone(&self.state),
                Arc::clone(&self.scan_cache),
                self.plan.clone(),
                tasks,
                partition,
                self.task_graph.partitions.len(),
                metrics,
            ))),
        }
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

    fn supports_limit_pushdown(&self) -> bool {
        true
    }

    fn with_fetch(&self, limit: Option<usize>) -> Option<Arc<dyn ExecutionPlan>> {
        let materialization_mode =
            self.selected_materialization_mode(limit, self.has_dynamic_filters());
        Some(Arc::new(Self {
            state: Arc::clone(&self.state),
            plan: self.plan.clone(),
            task_graph: Arc::clone(&self.task_graph),
            schema: Arc::clone(&self.schema),
            properties: self.properties_for_mode(materialization_mode),
            metrics: ExecutionPlanMetricsSet::new(),
            scan_cache: Arc::clone(&self.scan_cache),
            fetch: limit,
            materialization_mode,
            #[cfg(feature = "dynamic-filters")]
            dynamic_filters: self.dynamic_filters.clone(),
        }))
    }

    fn fetch(&self) -> Option<usize> {
        self.fetch
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
        let materialization_mode =
            self.selected_materialization_mode(self.fetch, !dynamic_filters.is_empty());
        let updated = Arc::new(Self {
            state: Arc::clone(&self.state),
            plan: self.plan.clone(),
            task_graph: Arc::clone(&self.task_graph),
            schema: Arc::clone(&self.schema),
            properties: self.properties_for_mode(materialization_mode),
            metrics: ExecutionPlanMetricsSet::new(),
            scan_cache: Arc::clone(&self.scan_cache),
            fetch: self.fetch,
            materialization_mode,
            dynamic_filters,
        }) as Arc<dyn ExecutionPlan>;
        Ok(
            FilterPushdownPropagation::with_parent_pushdown_result(pushdown_result)
                .with_updated_node(updated),
        )
    }
}
