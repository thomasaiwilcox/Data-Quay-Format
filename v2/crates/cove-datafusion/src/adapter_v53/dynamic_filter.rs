//! Native-mode dynamic-filter snapshots for DataFusion 53.x.

use std::sync::Arc;

use datafusion::{common::Result, physical_expr::PhysicalExpr};

use crate::{
    adapter_v53::physical_filter::lower_physical_filter, dataset_state::DatasetState,
    planner::FilterPlan,
};

#[derive(Debug, Default)]
pub(crate) struct DynamicFilterSnapshot {
    pub(crate) filters: Vec<FilterPlan>,
    pub(crate) snapshots: usize,
    pub(crate) fallbacks: usize,
}

pub(crate) fn snapshot_dynamic_filters(
    state: &DatasetState,
    filters: &[Arc<dyn PhysicalExpr>],
) -> Result<DynamicFilterSnapshot> {
    let mut out = DynamicFilterSnapshot::default();
    for filter in filters {
        if filter.snapshot_generation() == 0 {
            continue;
        }
        out.snapshots += 1;
        let snapshot = filter.snapshot()?.unwrap_or_else(|| Arc::clone(filter));
        let lowered = lower_physical_filter(state, snapshot.as_ref());
        out.filters.extend(lowered.filters);
        out.fallbacks += lowered.fallbacks;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cove_core::constants::CovePhysicalKind;
    use datafusion::{
        logical_expr::Operator,
        physical_expr::expressions::{lit, BinaryExpr, Column, DynamicFilterPhysicalExpr},
    };

    const SCAN_TABLE: &[u8] =
        include_bytes!("../../../../conformance/accept/cove_t_bool_numcode_declared.cove");

    #[test]
    fn dynamic_snapshot_lowers_supported_numeric_filter() {
        let state = crate::bootstrap::bootstrap_bytes("dynamic.cove", SCAN_TABLE.to_vec())
            .expect("fixture should bootstrap");
        let column_index = state
            .table()
            .columns
            .iter()
            .position(|column| column.physical == CovePhysicalKind::NumCode)
            .expect("fixture should have a NumCode column");
        let name = state.table().columns[column_index].name.as_str();
        let column = Arc::new(Column::new(name, column_index)) as Arc<dyn PhysicalExpr>;
        let dynamic = Arc::new(DynamicFilterPhysicalExpr::new(
            vec![Arc::clone(&column)],
            lit(true),
        ));
        dynamic
            .update(Arc::new(BinaryExpr::new(column, Operator::GtEq, lit(0i64))))
            .expect("dynamic update");

        let snapshot = snapshot_dynamic_filters(&state, &[dynamic]);

        let snapshot = snapshot.expect("snapshot should not fail");
        assert_eq!(snapshot.snapshots, 1);
        assert_eq!(snapshot.fallbacks, 0);
        assert_eq!(snapshot.filters.len(), 1);
        assert_eq!(snapshot.filters[0].use_kind, CoveFilterUse::PruningOnly);
    }
}
