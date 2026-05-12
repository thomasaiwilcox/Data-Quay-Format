//! Logical optimizer hooks for COVE metadata-aware planning.

use std::{fmt::Debug, sync::Arc};

use arrow_array::{Int32Array, Int64Array, RecordBatch, StringArray, UInt32Array, UInt64Array};
use arrow_schema::{DataType, SchemaRef};
use datafusion::{
    common::{tree_node::Transformed, DataFusionError, Result, ScalarValue},
    datasource::{provider_as_source, source_as_provider},
    execution::context::SessionContext,
    logical_expr::{Expr, LogicalPlan, TableScan},
    optimizer::{ApplyOrder, OptimizerConfig, OptimizerRule},
};

use crate::{
    adapter_v53::{
        filter::classify_filter, metadata::CoveMetadataTableProvider,
        table_provider::CoveTableProvider,
    },
    metadata_aggregate::{
        canonical_utf8, exact_filecode_filtered_count, exact_filecode_group_counts,
        exact_unfiltered_counts, MetadataAggregatePlan,
    },
    planner::{CovePredicate, TopNScanHint},
};
use cove_core::constants::CovePhysicalKind;

pub(crate) const COVE_METADATA_OPTIMIZER: &str = "cove_metadata_optimizer";

#[derive(Debug, Default)]
pub(crate) struct CoveMetadataOptimizerRule;

pub(crate) fn install_cove_optimizer(ctx: &SessionContext) {
    let already_installed = ctx
        .state()
        .optimizers()
        .iter()
        .any(|rule| rule.name() == COVE_METADATA_OPTIMIZER);
    if !already_installed {
        ctx.add_optimizer_rule(Arc::new(CoveMetadataOptimizerRule));
    }
}

impl OptimizerRule for CoveMetadataOptimizerRule {
    fn name(&self) -> &str {
        COVE_METADATA_OPTIMIZER
    }

    fn apply_order(&self) -> Option<ApplyOrder> {
        Some(ApplyOrder::BottomUp)
    }

    fn rewrite(
        &self,
        plan: LogicalPlan,
        _config: &dyn OptimizerConfig,
    ) -> Result<Transformed<LogicalPlan>> {
        match plan {
            LogicalPlan::Aggregate(aggregate) => {
                if let Some(rewritten) = rewrite_exact_count_aggregate(&aggregate)? {
                    Ok(Transformed::yes(rewritten))
                } else {
                    Ok(Transformed::no(LogicalPlan::Aggregate(aggregate)))
                }
            }
            LogicalPlan::Sort(sort) => {
                if let Some(rewritten) = rewrite_topn_sort(&sort)? {
                    Ok(Transformed::yes(rewritten))
                } else {
                    Ok(Transformed::no(LogicalPlan::Sort(sort)))
                }
            }
            other => Ok(Transformed::no(other)),
        }
    }
}

fn rewrite_exact_count_aggregate(
    aggregate: &datafusion::logical_expr::Aggregate,
) -> Result<Option<LogicalPlan>> {
    let Some((scan, filters)) = aggregate_scan_and_filters(aggregate.input.as_ref()) else {
        return Ok(None);
    };
    let Some(provider) = cove_provider_from_scan(scan)? else {
        return Ok(None);
    };
    let Some(plan) = metadata_aggregate_plan(aggregate, scan, &filters, provider.as_ref())? else {
        return Ok(None);
    };
    let schema: SchemaRef = Arc::new(aggregate.schema.as_arrow().clone());
    let Some(batch) = record_batch_for_metadata_plan(&plan, Arc::clone(&schema))? else {
        return Ok(None);
    };
    let proof = plan.proof().clone();
    let table = CoveMetadataTableProvider::new(Arc::clone(&schema), batch, proof);
    let scan = TableScan::try_new(
        scan.table_name.clone(),
        provider_as_source(Arc::new(table)),
        None,
        Vec::new(),
        None,
    )?;
    Ok(Some(LogicalPlan::TableScan(scan)))
}

fn metadata_aggregate_plan(
    aggregate: &datafusion::logical_expr::Aggregate,
    scan: &TableScan,
    filters: &[Expr],
    provider: &CoveTableProvider,
) -> Result<Option<MetadataAggregatePlan>> {
    if aggregate.group_expr.is_empty() {
        let mut count_columns = Vec::with_capacity(aggregate.aggr_expr.len());
        for expr in &aggregate.aggr_expr {
            let Some(column_index) = count_column_index(expr, provider) else {
                return Ok(None);
            };
            count_columns.push(column_index);
        }
        if filters.is_empty() {
            if count_columns.iter().all(Option::is_none)
                && scan
                    .projection
                    .as_ref()
                    .map(|projection| !projection.is_empty())
                    .unwrap_or(false)
            {
                return Ok(None);
            }
            if count_columns.iter().all(Option::is_none)
                && provider
                    .state()
                    .table()
                    .columns
                    .iter()
                    .any(|column| column.physical == CovePhysicalKind::FileCode)
            {
                return Ok(None);
            }
            return exact_unfiltered_counts(provider.state(), &count_columns)
                .map_err(crate::adapter_v53::cove_to_datafusion);
        }
        if count_columns.len() == 1 && count_columns[0].is_none() && filters.len() == 1 {
            let Some((column_index, canonical_values)) = filecode_filter(provider, &filters[0])
            else {
                return Ok(None);
            };
            return exact_filecode_filtered_count(
                provider.state(),
                column_index,
                &canonical_values,
            )
            .map_err(crate::adapter_v53::cove_to_datafusion);
        }
        return Ok(None);
    }

    if aggregate.group_expr.len() == 1
        && aggregate.aggr_expr.len() == 1
        && filters.is_empty()
        && matches!(
            count_column_index(&aggregate.aggr_expr[0], provider),
            Some(None)
        )
    {
        let Expr::Column(column) = &aggregate.group_expr[0] else {
            return Ok(None);
        };
        let Some(column_index) = provider
            .state()
            .table()
            .columns
            .iter()
            .position(|candidate| candidate.name == column.name)
        else {
            return Ok(None);
        };
        return exact_filecode_group_counts(provider.state(), column_index)
            .map_err(crate::adapter_v53::cove_to_datafusion);
    }
    Ok(None)
}

fn aggregate_scan_and_filters(input: &LogicalPlan) -> Option<(&TableScan, Vec<Expr>)> {
    match input {
        LogicalPlan::TableScan(scan) => Some((scan, dedup_filters(scan.filters.clone()))),
        LogicalPlan::Projection(projection) if projection.expr.is_empty() => {
            aggregate_scan_and_filters(projection.input.as_ref())
        }
        LogicalPlan::Filter(filter) => {
            let (scan, mut filters) = aggregate_scan_and_filters(filter.input.as_ref())?;
            filters.push(filter.predicate.clone());
            Some((scan, dedup_filters(filters)))
        }
        _ => None,
    }
}

fn dedup_filters(filters: Vec<Expr>) -> Vec<Expr> {
    let mut out = Vec::new();
    for filter in filters {
        if !out
            .iter()
            .any(|existing: &Expr| existing.to_string() == filter.to_string())
        {
            out.push(filter);
        }
    }
    out
}

fn filecode_filter(provider: &CoveTableProvider, expr: &Expr) -> Option<(usize, Vec<Vec<u8>>)> {
    let filter = classify_filter(provider.state(), expr);
    match filter.predicate {
        Some(CovePredicate::FileCodeIn {
            column_index,
            canonical_values,
            ..
        }) if !canonical_values.is_empty() => Some((column_index, canonical_values)),
        _ => None,
    }
}

fn record_batch_for_metadata_plan(
    plan: &MetadataAggregatePlan,
    schema: SchemaRef,
) -> Result<Option<RecordBatch>> {
    let arrays = match plan {
        MetadataAggregatePlan::ScalarCounts { counts, .. } => {
            let mut arrays = Vec::with_capacity(counts.len());
            for (index, count) in counts.iter().enumerate() {
                let field = schema.field(index);
                arrays.push(count_array_for_type(*count, field.data_type())?);
            }
            arrays
        }
        MetadataAggregatePlan::FileCodeGroupCounts { groups, .. } => {
            if schema.fields().len() != 2 || schema.field(0).data_type() != &DataType::Utf8 {
                return Ok(None);
            }
            let labels = groups
                .iter()
                .map(|group| canonical_utf8(&group.canonical_value))
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(crate::adapter_v53::cove_to_datafusion)?;
            vec![
                Arc::new(StringArray::from(labels)) as arrow_array::ArrayRef,
                count_array_for_values(
                    &groups.iter().map(|group| group.count).collect::<Vec<_>>(),
                    schema.field(1).data_type(),
                )?,
            ]
        }
    };
    let batch = RecordBatch::try_new(schema, arrays)
        .map_err(|err| DataFusionError::ArrowError(Box::new(err), None))?;
    Ok(Some(batch))
}

fn rewrite_topn_sort(sort: &datafusion::logical_expr::Sort) -> Result<Option<LogicalPlan>> {
    let Some(fetch) = sort.fetch else {
        return Ok(None);
    };
    if sort.expr.len() != 1 {
        return Ok(None);
    }
    let Expr::Column(column) = &sort.expr[0].expr else {
        return Ok(None);
    };
    let LogicalPlan::TableScan(scan) = sort.input.as_ref() else {
        return Ok(None);
    };
    let Some(provider) = cove_provider_from_scan(scan)? else {
        return Ok(None);
    };
    let Some(column_index) = provider
        .state()
        .table()
        .columns
        .iter()
        .position(|candidate| candidate.name == column.name)
    else {
        return Ok(None);
    };
    let hint = TopNScanHint {
        column_index,
        descending: !sort.expr[0].asc,
        fetch,
    };
    if provider.topn_hint() == Some(hint) {
        return Ok(None);
    }
    let hinted_provider = Arc::new(provider.with_topn_hint(hint));
    let rewritten_scan = TableScan::try_new(
        scan.table_name.clone(),
        provider_as_source(hinted_provider),
        scan.projection.clone(),
        scan.filters.clone(),
        scan.fetch,
    )?;
    Ok(Some(LogicalPlan::Sort(datafusion::logical_expr::Sort {
        expr: sort.expr.clone(),
        input: Arc::new(LogicalPlan::TableScan(rewritten_scan)),
        fetch: sort.fetch,
    })))
}

fn cove_provider_from_scan(scan: &TableScan) -> Result<Option<Arc<CoveTableProvider>>> {
    let provider = source_as_provider(&scan.source)?;
    let Some(cove) = provider.as_any().downcast_ref::<CoveTableProvider>() else {
        return Ok(None);
    };
    Ok(Some(Arc::new(cove.clone())))
}

#[allow(deprecated)]
fn count_column_index(expr: &Expr, provider: &CoveTableProvider) -> Option<Option<usize>> {
    let Expr::AggregateFunction(func) = expr else {
        return None;
    };
    if !func.func.name().eq_ignore_ascii_case("count")
        || func.params.distinct
        || func.params.filter.is_some()
        || !func.params.order_by.is_empty()
    {
        return None;
    }
    match func.params.args.as_slice() {
        [] => Some(None),
        [Expr::Wildcard { .. }] => Some(None),
        [Expr::Literal(value, _)] if !value.is_null() => Some(None),
        [Expr::Column(column)] => provider
            .state()
            .table()
            .columns
            .iter()
            .position(|candidate| candidate.name == column.name)
            .map(Some),
        _ => None,
    }
}

fn count_array_for_type(count: u64, data_type: &DataType) -> Result<arrow_array::ArrayRef> {
    let scalar =
        match data_type {
            DataType::Int64 => ScalarValue::Int64(Some(i64::try_from(count).map_err(|_| {
                DataFusionError::Plan("metadata COUNT result exceeds Int64".into())
            })?)),
            DataType::UInt64 => ScalarValue::UInt64(Some(count)),
            DataType::Int32 => ScalarValue::Int32(Some(i32::try_from(count).map_err(|_| {
                DataFusionError::Plan("metadata COUNT result exceeds Int32".into())
            })?)),
            DataType::UInt32 => ScalarValue::UInt32(Some(u32::try_from(count).map_err(|_| {
                DataFusionError::Plan("metadata COUNT result exceeds UInt32".into())
            })?)),
            other => {
                return Err(DataFusionError::Plan(format!(
                    "unsupported metadata COUNT output type {other:?}"
                )));
            }
        };
    scalar.to_array()
}

fn count_array_for_values(counts: &[u64], data_type: &DataType) -> Result<arrow_array::ArrayRef> {
    match data_type {
        DataType::Int64 => counts
            .iter()
            .map(|count| {
                i64::try_from(*count).map_err(|_| {
                    DataFusionError::Plan("metadata COUNT result exceeds Int64".into())
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(|values| Arc::new(Int64Array::from(values)) as arrow_array::ArrayRef),
        DataType::UInt64 => {
            Ok(Arc::new(UInt64Array::from(counts.to_vec())) as arrow_array::ArrayRef)
        }
        DataType::Int32 => counts
            .iter()
            .map(|count| {
                i32::try_from(*count).map_err(|_| {
                    DataFusionError::Plan("metadata COUNT result exceeds Int32".into())
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(|values| Arc::new(Int32Array::from(values)) as arrow_array::ArrayRef),
        DataType::UInt32 => counts
            .iter()
            .map(|count| {
                u32::try_from(*count).map_err(|_| {
                    DataFusionError::Plan("metadata COUNT result exceeds UInt32".into())
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(|values| Arc::new(UInt32Array::from(values)) as arrow_array::ArrayRef),
        other => Err(DataFusionError::Plan(format!(
            "unsupported metadata COUNT output type {other:?}"
        ))),
    }
}
