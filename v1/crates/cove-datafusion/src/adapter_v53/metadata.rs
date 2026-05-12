//! DataFusion 53.x metadata-only result provider and execution plan.

use std::{any::Any, sync::Arc};

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::{
    catalog::{Session, TableProvider},
    common::{stats::Precision, DataFusionError, Result, Statistics},
    execution::{SendableRecordBatchStream, TaskContext},
    logical_expr::TableType,
    physical_expr::EquivalenceProperties,
    physical_plan::{
        execution_plan::{Boundedness, EmissionType},
        memory::MemoryStream,
        metrics::{Count, ExecutionPlanMetricsSet, MetricBuilder, MetricsSet},
        DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
    },
};

use crate::metadata_aggregate::MetadataAggregateProof;

#[derive(Debug)]
pub(crate) struct CoveMetadataTableProvider {
    schema: SchemaRef,
    batch: RecordBatch,
    proof: MetadataAggregateProof,
}

impl CoveMetadataTableProvider {
    pub(crate) fn new(
        schema: SchemaRef,
        batch: RecordBatch,
        proof: MetadataAggregateProof,
    ) -> Self {
        Self {
            schema,
            batch,
            proof,
        }
    }
}

#[async_trait]
impl TableProvider for CoveMetadataTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        _projection: Option<&Vec<usize>>,
        filters: &[datafusion::logical_expr::Expr],
        _limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if !filters.is_empty() {
            return Err(DataFusionError::Internal(
                "COVE metadata provider does not accept pushed filters".into(),
            ));
        }
        CoveMetadataExec::try_new(
            Arc::clone(&self.schema),
            self.batch.clone(),
            self.proof.clone(),
        )
        .map(|exec| Arc::new(exec) as Arc<dyn ExecutionPlan>)
    }

    fn statistics(&self) -> Option<Statistics> {
        let mut statistics = Statistics::new_unknown(self.schema.as_ref());
        statistics.num_rows = Precision::Exact(self.batch.num_rows());
        statistics.calculate_total_byte_size(self.schema.as_ref());
        Some(statistics)
    }
}

#[derive(Debug)]
pub(crate) struct CoveMetadataExec {
    schema: SchemaRef,
    batch: RecordBatch,
    proof: MetadataAggregateProof,
    properties: Arc<PlanProperties>,
    metrics: ExecutionPlanMetricsSet,
}

impl CoveMetadataExec {
    fn try_new(
        schema: SchemaRef,
        batch: RecordBatch,
        proof: MetadataAggregateProof,
    ) -> Result<Self> {
        let properties = Arc::new(PlanProperties::new(
            EquivalenceProperties::new(Arc::clone(&schema)),
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Bounded,
        ));
        Ok(Self {
            schema,
            batch,
            proof,
            properties,
            metrics: ExecutionPlanMetricsSet::new(),
        })
    }
}

impl DisplayAs for CoveMetadataExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => write!(
                f,
                "CoveMetadataExec: proof={:?}, rows={}, reason={}",
                self.proof.kind,
                self.batch.num_rows(),
                self.proof.reason
            ),
            DisplayFormatType::TreeRender => write!(f, "CoveMetadataExec"),
        }
    }
}

impl ExecutionPlan for CoveMetadataExec {
    fn name(&self) -> &str {
        "CoveMetadataExec"
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
                "CoveMetadataExec is a leaf execution plan".into(),
            ));
        }
        Ok(self)
    }

    fn execute(
        &self,
        partition: usize,
        _context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        if partition != 0 {
            return Err(DataFusionError::Internal(format!(
                "CoveMetadataExec has one partition, got partition {partition}"
            )));
        }
        let metrics = CoveMetadataMetrics::new(&self.metrics, partition);
        metrics.record(&self.proof, self.batch.num_rows());
        Ok(Box::pin(MemoryStream::try_new(
            vec![self.batch.clone()],
            Arc::clone(&self.schema),
            None,
        )?))
    }

    fn metrics(&self) -> Option<MetricsSet> {
        Some(self.metrics.clone_inner())
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        if let Some(partition) = partition {
            if partition != 0 {
                return Err(DataFusionError::Internal(format!(
                    "CoveMetadataExec has one partition, got partition {partition}"
                )));
            }
        }
        let mut statistics = Statistics::new_unknown(self.schema.as_ref());
        statistics.num_rows = Precision::Exact(self.batch.num_rows());
        statistics.calculate_total_byte_size(self.schema.as_ref());
        Ok(statistics)
    }
}

struct CoveMetadataMetrics {
    fast_path_answers: Count,
    rows_emitted: Count,
    dictionary_group_labels_decoded: Count,
}

impl CoveMetadataMetrics {
    fn new(metrics: &ExecutionPlanMetricsSet, partition: usize) -> Self {
        Self {
            fast_path_answers: MetricBuilder::new(metrics)
                .counter("cove_metadata_fast_path_answers", partition),
            rows_emitted: MetricBuilder::new(metrics)
                .counter("cove_metadata_rows_emitted", partition),
            dictionary_group_labels_decoded: MetricBuilder::new(metrics)
                .counter("cove_dictionary_group_labels_decoded", partition),
        }
    }

    fn record(&self, proof: &MetadataAggregateProof, rows: usize) {
        self.fast_path_answers.add(1);
        self.rows_emitted.add(rows);
        self.dictionary_group_labels_decoded
            .add(proof.dictionary_group_labels_decoded);
    }
}
