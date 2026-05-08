//! DataFusion 53.x table-provider glue.

use std::{any::Any, sync::Arc};

use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::{
    catalog::{Session, TableProvider},
    common::{stats::Precision, DataFusionError, Result, Statistics},
    logical_expr::{Expr, TableProviderFilterPushDown, TableType},
    physical_plan::ExecutionPlan,
};

use crate::{
    adapter_v53::{
        exec::CoveExec,
        filter::{classify_filter, filter_use_is_pushed},
    },
    dataset_state::DatasetState,
    planner::{plan_scan, CoveFilterUse, TopNScanHint},
};

#[derive(Debug, Clone)]
pub struct CoveTableProvider {
    state: Arc<DatasetState>,
    schema: SchemaRef,
    topn_hint: Option<TopNScanHint>,
}

impl CoveTableProvider {
    pub fn new(state: Arc<DatasetState>) -> Self {
        let schema = state.schema();
        Self {
            state,
            schema,
            topn_hint: None,
        }
    }

    pub fn state(&self) -> &Arc<DatasetState> {
        &self.state
    }

    pub fn with_topn_hint(&self, topn_hint: TopNScanHint) -> Self {
        Self {
            state: Arc::clone(&self.state),
            schema: Arc::clone(&self.schema),
            topn_hint: Some(topn_hint),
        }
    }

    pub fn topn_hint(&self) -> Option<TopNScanHint> {
        self.topn_hint
    }

    fn table_statistics(&self) -> Statistics {
        let mut statistics = Statistics::new_unknown(self.schema.as_ref());
        if let Ok(row_count) = self.state.exact_visible_row_count() {
            if let Ok(row_count) = usize::try_from(row_count) {
                statistics.num_rows = Precision::Exact(row_count);
            }
        }
        statistics.calculate_total_byte_size(self.schema.as_ref());
        statistics
    }
}

#[async_trait]
impl TableProvider for CoveTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let filter_plans = filters
            .iter()
            .map(|filter| classify_filter(&self.state, filter))
            .collect::<Vec<_>>();
        if let Some(filter) = filter_plans
            .iter()
            .find(|filter| !filter_use_is_pushed(filter.use_kind))
        {
            return Err(DataFusionError::Plan(format!(
                "COVE DataFusion native provider received unsupported pushed filter: {}",
                filter.display
            )));
        }
        let mut plan = plan_scan(&self.state, projection, filter_plans)
            .map_err(crate::adapter_v53::cove_to_datafusion)?;
        plan.topn_hint = self.topn_hint;
        CoveExec::try_new_with_fetch(Arc::clone(&self.state), plan, limit)
            .map(|exec| Arc::new(exec) as Arc<dyn ExecutionPlan>)
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> Result<Vec<TableProviderFilterPushDown>> {
        Ok(filters
            .iter()
            .map(
                |filter| match classify_filter(&self.state, filter).use_kind {
                    CoveFilterUse::Unsupported => TableProviderFilterPushDown::Unsupported,
                    CoveFilterUse::PruningOnly => TableProviderFilterPushDown::Inexact,
                    CoveFilterUse::FullRowPredicateExact => TableProviderFilterPushDown::Exact,
                },
            )
            .collect())
    }

    fn statistics(&self) -> Option<Statistics> {
        Some(self.table_statistics())
    }
}
