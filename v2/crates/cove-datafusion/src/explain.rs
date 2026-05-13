//! Public planning and reporting helpers for v2 utility CLIs.

use std::{collections::BTreeSet, ops::Range, path::Path, sync::Arc};

use cove_core::{
    canonical::CanonicalValue,
    constants::{CoveLogicalType, CovePhysicalKind},
    CoveError,
};
use serde_json::{json, Value};

use crate::{
    bootstrap::bootstrap_local_file_with_options,
    dataset_state::DatasetState,
    decode::{decode_local_dataset_scan, DecodeStats, DecodedScan},
    options::CoveTableOptions,
    planner::{
        plan_scan, FilterPlan, NullPredicateKind, NumericPredicateOp, PredicateLiteral, ScanPlan,
        TopNScanHint,
    },
    range_reader::{coalesced_range_stats, CoalescedRangeStats},
    scan_program::PredicateExactness,
    task_graph::{build_task_graph, TaskGraph},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterDsl {
    pub column: String,
    pub op: FilterOp,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    Eq,
    Lt,
    Lte,
    Gt,
    Gte,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExplainOptions {
    pub projection: Option<Vec<String>>,
    pub filters: Vec<FilterDsl>,
    pub top_n: Option<TopNDsl>,
    pub table_options: CoveTableOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopNDsl {
    pub column: String,
    pub fetch: usize,
    pub descending: bool,
}

#[derive(Debug, Clone)]
pub struct PlannedScan {
    pub state: Arc<DatasetState>,
    pub plan: ScanPlan,
    pub graph: TaskGraph,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PruningExplainReport {
    pub version: u32,
    pub source: String,
    pub table: Value,
    pub projection: Value,
    pub filters: Vec<Value>,
    pub summary: Value,
    pub decisions: Vec<Value>,
    pub coverage: Value,
    pub sidecars: Value,
    pub residuals: Value,
}

impl PruningExplainReport {
    pub fn to_json_value(&self) -> Value {
        json!({
            "version": self.version,
            "source": self.source,
            "table": self.table,
            "projection": self.projection,
            "filters": self.filters,
            "summary": self.summary,
            "decisions": self.decisions,
            "coverage": self.coverage,
            "sidecars": self.sidecars,
            "residuals": self.residuals,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanCostReport {
    pub version: u32,
    pub source: String,
    pub estimated: Value,
    pub observed: Option<Value>,
    pub range_plan: Value,
    pub coverage_metrics: Value,
    pub fallbacks: Value,
}

impl PlanCostReport {
    pub fn to_json_value(&self) -> Value {
        json!({
            "version": self.version,
            "source": self.source,
            "estimated": self.estimated,
            "observed": self.observed,
            "range_plan": self.range_plan,
            "coverage_metrics": self.coverage_metrics,
            "fallbacks": self.fallbacks,
        })
    }
}

pub fn parse_filter_dsl(raw: &str) -> Result<FilterDsl, CoveError> {
    let mut column = None;
    let mut op = None;
    let mut value = None;
    for part in raw.split(',') {
        let (key, raw_value) = part.split_once('=').ok_or_else(|| {
            CoveError::BadSchema(format!("filter component {part:?} must be key=value"))
        })?;
        match key {
            "column" => column = Some(raw_value.to_string()),
            "op" => op = Some(parse_filter_op(raw_value)?),
            "value" => value = Some(raw_value.to_string()),
            _ => {
                return Err(CoveError::BadSchema(format!(
                    "unknown filter component {key:?}"
                )))
            }
        }
    }
    let op = op.ok_or_else(|| CoveError::BadSchema("filter requires op=<op>".into()))?;
    if !matches!(op, FilterOp::IsNull | FilterOp::IsNotNull) && value.is_none() {
        return Err(CoveError::BadSchema(
            "filter requires value=<literal> for this operator".into(),
        ));
    }
    Ok(FilterDsl {
        column: column
            .ok_or_else(|| CoveError::BadSchema("filter requires column=<name|index>".into()))?,
        op,
        value,
    })
}

pub fn parse_projection_dsl(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn parse_topn_dsl(raw: &str) -> Result<TopNDsl, CoveError> {
    let mut column = None;
    let mut fetch = None;
    let mut descending = false;
    for part in raw.split(',') {
        let (key, value) = part.split_once('=').ok_or_else(|| {
            CoveError::BadSchema(format!("top-N component {part:?} must be key=value"))
        })?;
        match key {
            "column" => column = Some(value.to_string()),
            "fetch" => {
                fetch = Some(value.parse::<usize>().map_err(|_| {
                    CoveError::BadSchema("top-N fetch must be a positive integer".into())
                })?)
            }
            "desc" | "descending" => {
                descending = matches!(value, "true" | "1" | "yes" | "desc");
            }
            _ => {
                return Err(CoveError::BadSchema(format!(
                    "unknown top-N component {key:?}"
                )))
            }
        }
    }
    let fetch = fetch.ok_or_else(|| CoveError::BadSchema("top-N requires fetch=<n>".into()))?;
    if fetch == 0 {
        return Err(CoveError::BadSchema(
            "top-N fetch must be greater than zero".into(),
        ));
    }
    Ok(TopNDsl {
        column: column
            .ok_or_else(|| CoveError::BadSchema("top-N requires column=<name|index>".into()))?,
        fetch,
        descending,
    })
}

pub fn plan_local_file(
    path: impl AsRef<Path>,
    options: ExplainOptions,
) -> Result<PlannedScan, CoveError> {
    let state = bootstrap_local_file_with_options(path.as_ref(), options.table_options)?;
    let projection = options
        .projection
        .as_ref()
        .map(|columns| resolve_projection(&state, columns))
        .transpose()?;
    let filters = build_filter_plans(&state, &options.filters)?;
    let mut plan = plan_scan(&state, projection.as_ref(), filters)?;
    if let Some(top_n) = options.top_n {
        let column_index = resolve_column(&state, &top_n.column)?;
        plan.topn_hint = Some(TopNScanHint {
            column_index,
            descending: top_n.descending,
            fetch: top_n.fetch,
        });
    }
    let graph = build_task_graph(&state, &plan)?;
    Ok(PlannedScan { state, plan, graph })
}

pub fn explain_pruning(
    path: impl AsRef<Path>,
    options: ExplainOptions,
) -> Result<PruningExplainReport, CoveError> {
    let planned = plan_local_file(path, options)?;
    Ok(pruning_report(&planned))
}

pub fn plan_cost(
    path: impl AsRef<Path>,
    options: ExplainOptions,
    execute: bool,
) -> Result<PlanCostReport, CoveError> {
    let planned = plan_local_file(path, options)?;
    let observed = if execute {
        Some(decode_local_dataset_scan(&planned.state, &planned.plan)?.stats)
    } else {
        None
    };
    Ok(cost_report(&planned, observed))
}

pub fn execute_planned_scan(planned: &PlannedScan) -> Result<DecodedScan, CoveError> {
    decode_local_dataset_scan(&planned.state, &planned.plan)
}

pub fn pruning_report(planned: &PlannedScan) -> PruningExplainReport {
    let state = &planned.state;
    let plan = &planned.plan;
    let graph = &planned.graph;
    let kept = task_keys(graph);
    let mut decisions = Vec::new();
    for file_ordinal in 0..state.file_count() {
        let Ok(file) = state.file(file_ordinal) else {
            continue;
        };
        for (segment_index, segment) in file.segments().iter().enumerate() {
            for morsel_id in 0..segment.morsel_count {
                let row_start = u64::from(segment.row_start).saturating_add(
                    u64::from(morsel_id).saturating_mul(u64::from(segment.morsel_row_count)),
                );
                let row_count = morsel_row_count_for(segment, morsel_id).unwrap_or(0);
                let key = (file_ordinal, segment.segment_id, morsel_id);
                let kept_task = kept.contains(&key);
                decisions.push(json!({
                    "file_ordinal": file_ordinal,
                    "source": file.source(),
                    "segment_index": segment_index,
                    "segment_id": segment.segment_id,
                    "morsel_id": morsel_id,
                    "row_start": row_start,
                    "row_count": row_count,
                    "decision": if kept_task { "kept" } else { "pruned" },
                    "evidence_kind": if kept_task { "metadata_task_graph" } else { "metadata_pruning_or_visibility_overlay" },
                    "fallback_reason": fallback_reason(planned),
                    "split_id": graph
                        .tasks
                        .iter()
                        .find(|task| {
                            task.file_ordinal == file_ordinal
                                && task.segment_id == segment.segment_id
                                && task.morsel_id == morsel_id
                        })
                        .and_then(|task| task.split_id),
                    "cluster_id": graph
                        .tasks
                        .iter()
                        .find(|task| {
                            task.file_ordinal == file_ordinal
                                && task.segment_id == segment.segment_id
                                && task.morsel_id == morsel_id
                        })
                        .and_then(|task| task.cluster_id),
                }));
            }
        }
    }

    let stats = state.bootstrap_stats();
    PruningExplainReport {
        version: 1,
        source: state.source().to_string(),
        table: json!({
            "table_id": state.table().table_id,
            "namespace": &state.table().namespace,
            "name": &state.table().name,
            "row_count": state.table().row_count,
            "column_count": state.table().columns.len(),
            "file_count": state.file_count(),
        }),
        projection: json!({
            "columns": projection_report(state, &plan.scan_projection),
            "scan_column_indexes": &plan.scan_projection,
            "predicate_column_indexes": &plan.predicate_columns,
            "top_n": plan.topn_hint.map(|hint| {
                json!({
                    "column_index": hint.column_index,
                    "column": state.table().columns.get(hint.column_index).map(|column| column.name.as_str()),
                    "descending": hint.descending,
                    "fetch": hint.fetch,
                })
            }),
        }),
        filters: plan
            .filters
            .iter()
            .map(|filter| {
                json!({
                    "display": &filter.display,
                    "use_kind": format!("{:?}", filter.use_kind),
                    "predicate_columns": &filter.predicate_columns,
                    "coverage_predicate_form_ref": filter.coverage_predicate_form_ref,
                    "residual_required": filter.use_kind != crate::planner::CoveFilterUse::FullRowPredicateExact,
                })
            })
            .collect(),
        summary: json!({
            "files_considered": stats.files_considered.max(state.file_count()),
            "files_pruned": stats.files_pruned,
            "files_validated": stats.files_validated,
            "segments_considered": state.files().iter().map(|file| file.segments().len()).sum::<usize>(),
            "morsels_considered": graph.morsels_considered,
            "morsels_kept": graph.tasks.len(),
            "morsels_pruned": graph.morsels_pruned,
            "tasks": graph.tasks.len(),
            "partitions": graph.partitions.len(),
            "scan_splits_used": graph.scan_splits_used,
            "rows_considered": state.table().row_count,
            "rows_in_kept_tasks": graph.tasks.iter().map(|task| u64::from(task.row_count)).sum::<u64>(),
        }),
        decisions,
        coverage: coverage_report(state, plan),
        sidecars: sidecar_report(state),
        residuals: residual_report(plan),
    }
}

pub fn cost_report(planned: &PlannedScan, observed: Option<DecodeStats>) -> PlanCostReport {
    let state = &planned.state;
    let graph = &planned.graph;
    let ranges = estimate_task_ranges(planned);
    let range_stats = coalesced_range_stats(&ranges, state.range_coalescing()).unwrap_or_default();
    let segment_payload_bytes = state
        .files()
        .iter()
        .flat_map(|file| file.segments())
        .map(|segment| segment.length)
        .sum::<u64>();
    let metadata_bytes = state.file_len().saturating_sub(segment_payload_bytes);
    let estimated_payload_bytes = ranges
        .iter()
        .map(|range| range.end.saturating_sub(range.start))
        .sum::<u64>();
    let observed_json = observed.map(|stats| decode_stats_json(stats));
    PlanCostReport {
        version: 1,
        source: state.source().to_string(),
        estimated: json!({
            "metadata_bytes": metadata_bytes,
            "payload_bytes": estimated_payload_bytes,
            "tasks": graph.tasks.len(),
            "partitions": graph.partitions.len(),
            "rows_considered": state.table().row_count,
            "rows_materialized": graph.tasks.iter().map(|task| u64::from(task.row_count)).sum::<u64>(),
            "full_scan_fallback_frequency": usize::from(graph.morsels_pruned == 0 && !planned.plan.filters.is_empty()),
        }),
        observed: observed_json,
        range_plan: json!({
            "original_range_requests": range_stats.original_ranges,
            "coalesced_range_requests": range_stats.coalesced_ranges,
            "original_bytes": range_stats.original_bytes,
            "coalesced_bytes": range_stats.coalesced_bytes,
            "coalescing_max_gap": state.range_coalescing().max_gap,
            "coalescing_max_span": state.range_coalescing().max_span,
        }),
        coverage_metrics: coverage_report(state, &planned.plan),
        fallbacks: json!({
            "manifest_fallbacks": state.bootstrap_stats().manifest_fallbacks,
            "sidecar_index_fallbacks": state.bootstrap_stats().sidecar_index_fallbacks,
            "covi_candidates_used": planned.plan.covi_candidates.as_ref().map(|candidates| candidates.len()).unwrap_or(0),
            "residual_filters": planned.plan.scan_program.inexact_filters,
            "unsupported_filters": planned
                .plan
                .filters
                .iter()
                .filter(|filter| filter.use_kind == crate::planner::CoveFilterUse::Unsupported)
                .count(),
        }),
    }
}

fn build_filter_plans(
    state: &DatasetState,
    filters: &[FilterDsl],
) -> Result<Vec<FilterPlan>, CoveError> {
    filters
        .iter()
        .map(|filter| build_filter_plan(state, filter))
        .collect()
}

fn build_filter_plan(state: &DatasetState, filter: &FilterDsl) -> Result<FilterPlan, CoveError> {
    let column_index = resolve_column(state, &filter.column)?;
    let column = &state.table().columns[column_index];
    let display = filter_display(column.name.as_str(), filter);
    match filter.op {
        FilterOp::IsNull => Ok(FilterPlan::pruning_null(
            column_index,
            NullPredicateKind::IsNull,
            display,
        )),
        FilterOp::IsNotNull => Ok(FilterPlan::pruning_null(
            column_index,
            NullPredicateKind::IsNotNull,
            display,
        )),
        FilterOp::Eq if column.physical == CovePhysicalKind::FileCode => {
            let value = filter.value.as_deref().unwrap_or_default();
            let canonical = canonical_literal(column.logical, value)?;
            let mut file_codes = Vec::new();
            for file_ordinal in 0..state.file_count() {
                if let Some(file_code) = state.file_code_for_canonical(file_ordinal, &canonical)? {
                    file_codes.push(file_code);
                }
            }
            Ok(FilterPlan::pruning_file_code_in_with_canonical(
                column_index,
                file_codes,
                vec![canonical],
                display,
            ))
        }
        FilterOp::Eq if column.physical == CovePhysicalKind::VarBytes => {
            let value = filter.value.as_deref().unwrap_or_default();
            Ok(FilterPlan::pruning_varbytes_eq(
                column_index,
                literal_bytes(column.logical, value)?,
                display,
            ))
        }
        FilterOp::Eq | FilterOp::Lt | FilterOp::Lte | FilterOp::Gt | FilterOp::Gte
            if column.physical == CovePhysicalKind::NumCode =>
        {
            let value = filter.value.as_deref().unwrap_or_default();
            Ok(FilterPlan::pruning_numeric(
                column_index,
                numeric_op(filter.op)?,
                numeric_literal(column.logical, value)?,
                display,
            ))
        }
        _ => Ok(FilterPlan::unsupported(display)),
    }
}

fn parse_filter_op(raw: &str) -> Result<FilterOp, CoveError> {
    match raw {
        "eq" => Ok(FilterOp::Eq),
        "lt" => Ok(FilterOp::Lt),
        "lte" => Ok(FilterOp::Lte),
        "gt" => Ok(FilterOp::Gt),
        "gte" => Ok(FilterOp::Gte),
        "is-null" => Ok(FilterOp::IsNull),
        "is-not-null" => Ok(FilterOp::IsNotNull),
        _ => Err(CoveError::BadSchema(format!(
            "unsupported filter op {raw:?}; expected eq|lt|lte|gt|gte|is-null|is-not-null"
        ))),
    }
}

fn numeric_op(op: FilterOp) -> Result<NumericPredicateOp, CoveError> {
    match op {
        FilterOp::Eq => Ok(NumericPredicateOp::Eq),
        FilterOp::Lt => Ok(NumericPredicateOp::Lt),
        FilterOp::Lte => Ok(NumericPredicateOp::LtEq),
        FilterOp::Gt => Ok(NumericPredicateOp::Gt),
        FilterOp::Gte => Ok(NumericPredicateOp::GtEq),
        _ => Err(CoveError::BadSchema("operator is not numeric".into())),
    }
}

fn resolve_projection(state: &DatasetState, columns: &[String]) -> Result<Vec<usize>, CoveError> {
    columns
        .iter()
        .map(|column| resolve_column(state, column))
        .collect()
}

fn resolve_column(state: &DatasetState, raw: &str) -> Result<usize, CoveError> {
    if let Ok(index) = raw.parse::<usize>() {
        if index < state.table().columns.len() {
            return Ok(index);
        }
        return Err(CoveError::BadSchema(format!(
            "column index {index} is out of bounds for {} columns",
            state.table().columns.len()
        )));
    }
    state
        .table()
        .columns
        .iter()
        .position(|column| column.name == raw)
        .ok_or_else(|| CoveError::BadSchema(format!("unknown column {raw:?}")))
}

fn filter_display(column_name: &str, filter: &FilterDsl) -> String {
    match filter.op {
        FilterOp::IsNull => format!("{column_name} IS NULL"),
        FilterOp::IsNotNull => format!("{column_name} IS NOT NULL"),
        FilterOp::Eq => format!("{column_name} = {}", filter.value.as_deref().unwrap_or("")),
        FilterOp::Lt => format!("{column_name} < {}", filter.value.as_deref().unwrap_or("")),
        FilterOp::Lte => format!("{column_name} <= {}", filter.value.as_deref().unwrap_or("")),
        FilterOp::Gt => format!("{column_name} > {}", filter.value.as_deref().unwrap_or("")),
        FilterOp::Gte => format!("{column_name} >= {}", filter.value.as_deref().unwrap_or("")),
    }
}

fn canonical_literal(logical: CoveLogicalType, value: &str) -> Result<Vec<u8>, CoveError> {
    match logical {
        CoveLogicalType::Utf8 => CanonicalValue::Utf8(value).encode(),
        CoveLogicalType::Binary => {
            let bytes = parse_bytes_literal(value)?;
            CanonicalValue::Bytes(&bytes).encode()
        }
        CoveLogicalType::Bool => CanonicalValue::Bool(parse_bool(value)?).encode(),
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64 => CanonicalValue::Int {
            width: integer_width(logical),
            value: i128::from(parse_i64(value)?),
        }
        .encode(),
        CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64 => CanonicalValue::Uint {
            width: integer_width(logical),
            value: u128::from(parse_u64(value)?),
        }
        .encode(),
        CoveLogicalType::Float32 => CanonicalValue::Float32(parse_f64(value)? as f32).encode(),
        CoveLogicalType::Float64 => CanonicalValue::Float64(parse_f64(value)?).encode(),
        CoveLogicalType::DateDays => CanonicalValue::DateDays(parse_i64(value)? as i32).encode(),
        CoveLogicalType::TimestampMicros => {
            CanonicalValue::TimestampMicros(parse_i64(value)?).encode()
        }
        CoveLogicalType::TimestampNanos => {
            CanonicalValue::TimestampNanos(parse_i64(value)?).encode()
        }
        CoveLogicalType::Json => CanonicalValue::Json(value).encode(),
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "filter DSL cannot encode canonical literal for {logical:?}"
        ))),
    }
}

fn numeric_literal(logical: CoveLogicalType, value: &str) -> Result<PredicateLiteral, CoveError> {
    match logical {
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64
        | CoveLogicalType::DateDays
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos
        | CoveLogicalType::Decimal64 => Ok(PredicateLiteral::Int64(parse_i64(value)?)),
        CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64 => Ok(PredicateLiteral::UInt64(parse_u64(value)?)),
        CoveLogicalType::Float32 | CoveLogicalType::Float64 => {
            Ok(PredicateLiteral::Float64(parse_f64(value)?))
        }
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "filter DSL cannot encode numeric literal for {logical:?}"
        ))),
    }
}

fn literal_bytes(logical: CoveLogicalType, value: &str) -> Result<Vec<u8>, CoveError> {
    match logical {
        CoveLogicalType::Utf8 | CoveLogicalType::Json => Ok(value.as_bytes().to_vec()),
        CoveLogicalType::Binary => parse_bytes_literal(value),
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "filter DSL cannot encode VarBytes literal for {logical:?}"
        ))),
    }
}

fn parse_i64(value: &str) -> Result<i64, CoveError> {
    value
        .parse::<i64>()
        .map_err(|_| CoveError::BadSchema(format!("literal {value:?} is not an i64")))
}

fn parse_u64(value: &str) -> Result<u64, CoveError> {
    value
        .parse::<u64>()
        .map_err(|_| CoveError::BadSchema(format!("literal {value:?} is not a u64")))
}

fn parse_f64(value: &str) -> Result<f64, CoveError> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| CoveError::BadSchema(format!("literal {value:?} is not an f64")))?;
    if parsed.is_nan() {
        return Err(CoveError::BadSchema(
            "NaN filter literals are not supported".into(),
        ));
    }
    Ok(parsed)
}

fn parse_bool(value: &str) -> Result<bool, CoveError> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(CoveError::BadSchema(format!(
            "literal {value:?} is not a bool"
        ))),
    }
}

fn parse_bytes_literal(value: &str) -> Result<Vec<u8>, CoveError> {
    let Some(hex) = value.strip_prefix("0x") else {
        return Ok(value.as_bytes().to_vec());
    };
    if hex.len() % 2 != 0 {
        return Err(CoveError::BadSchema(
            "hex byte literal has odd length".into(),
        ));
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let hi = hex_digit(pair[0])?;
        let lo = hex_digit(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_digit(byte: u8) -> Result<u8, CoveError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(CoveError::BadSchema("invalid hex digit".into())),
    }
}

fn integer_width(logical: CoveLogicalType) -> u8 {
    match logical {
        CoveLogicalType::Int8 | CoveLogicalType::UInt8 => 1,
        CoveLogicalType::Int16 | CoveLogicalType::UInt16 => 2,
        CoveLogicalType::Int32 | CoveLogicalType::UInt32 | CoveLogicalType::DateDays => 4,
        _ => 8,
    }
}

fn projection_report(state: &DatasetState, projection: &[usize]) -> Vec<Value> {
    projection
        .iter()
        .filter_map(|index| {
            state
                .table()
                .columns
                .get(*index)
                .map(|column| (*index, column))
        })
        .map(|(index, column)| {
            json!({
                "index": index,
                "column_id": column.column_id,
                "name": &column.name,
                "logical": format!("{:?}", column.logical),
                "physical": format!("{:?}", column.physical),
            })
        })
        .collect()
}

fn coverage_report(state: &DatasetState, plan: &ScanPlan) -> Value {
    let pruning = state.pruning();
    let cache = state.coverage_cache().runtime_stats();
    let bootstrap = state.bootstrap_stats();
    json!({
        "coverage_expr_present": plan.coverage_expr.is_some(),
        "coverage_providers": pruning.coverage_providers.len(),
        "coverage_sets": pruning.coverage_sets.len(),
        "coverage_proofs": pruning.coverage_proofs.len(),
        "coverage_plan_candidates": pruning.coverage_plan_candidates.len(),
        "predicate_forms": pruning.predicate_forms.len(),
        "predicate_forms_with_payloads": pruning.predicate_forms_with_payloads.len(),
        "covi_candidates": plan.covi_candidates.as_ref().map(|candidates| candidates.len()).unwrap_or(0),
        "covi_used": plan.covi_candidates.is_some(),
        "covx_indexes": {
            "column_domains": pruning.column_domains.len(),
            "zone_stats": pruning.zone_stats.len(),
            "exact_sets": pruning.exact_sets.len(),
            "blooms": pruning.blooms.len(),
            "lookups": pruning.lookups.len(),
            "inverted": pruning.inverted.len(),
            "aggregates": pruning.aggregates.len(),
            "composites": pruning.composites.len(),
            "topn": pruning.topn.len(),
        },
        "coverage_cache": {
            "enabled": cache.enabled,
            "entries": cache.entries,
            "hits": cache.hits,
            "misses": cache.misses,
            "entries_loaded": bootstrap.coverage_cache_entries_loaded,
            "entries_stale": bootstrap.coverage_cache_entries_stale,
            "entries_ignored": bootstrap.coverage_cache_entries_ignored,
            "invalidations": bootstrap.coverage_cache_invalidations,
        },
    })
}

fn sidecar_report(state: &DatasetState) -> Value {
    let stats = state.bootstrap_stats();
    json!({
        "covm": {
            "entries_stale": stats.covm_entries_stale,
            "manifest_fallbacks": stats.manifest_fallbacks,
        },
        "covx": {
            "loaded": stats.covx_sidecars_loaded,
            "stale": stats.covx_sidecars_stale,
            "ignored": stats.covx_sidecars_ignored,
        },
        "covi": {
            "loaded": stats.covi_sidecars_loaded,
            "stale": stats.covi_sidecars_stale,
            "ignored": stats.covi_sidecars_ignored,
            "candidate_pruned": stats.covi_candidate_pruned,
            "index_only_answers": stats.covi_index_only_answers,
        },
        "covel": {
            "sections_loaded": stats.covel_sections_loaded,
            "sections_ignored": stats.covel_sections_ignored,
            "scan_splits_loaded": stats.covel_scan_splits_loaded,
            "zero_copy_maps_loaded": stats.covel_zero_copy_maps_loaded,
        },
    })
}

fn residual_report(plan: &ScanPlan) -> Value {
    let exactness = plan
        .scan_program
        .ops
        .iter()
        .map(|op| match op {
            crate::scan_program::ScanOp::Null { exactness, .. }
            | crate::scan_program::ScanOp::Numeric { exactness, .. }
            | crate::scan_program::ScanOp::FileCodeIn { exactness, .. }
            | crate::scan_program::ScanOp::VarBytesEq { exactness, .. } => *exactness,
        })
        .collect::<Vec<_>>();
    json!({
        "scan_program": plan.scan_program.display_summary(),
        "exact_filters": plan.scan_program.exact_filters,
        "residual_filters": plan.scan_program.inexact_filters,
        "residual_filter_status": if plan.scan_program.inexact_filters == 0 { "none" } else { "required" },
        "exactness": exactness
            .iter()
            .map(|value| match value {
                PredicateExactness::PruningOnly => "PruningOnly",
                PredicateExactness::FullRowPredicateExact => "FullRowPredicateExact",
            })
            .collect::<Vec<_>>(),
    })
}

fn fallback_reason(planned: &PlannedScan) -> Option<&'static str> {
    let stats = planned.state.bootstrap_stats();
    if stats.manifest_fallbacks != 0 {
        Some("manifest_fallback")
    } else if stats.sidecar_index_fallbacks != 0 {
        Some("sidecar_index_fallback")
    } else if planned.plan.scan_program.inexact_filters != 0 {
        Some("residual_filter_required")
    } else {
        None
    }
}

fn task_keys(graph: &TaskGraph) -> BTreeSet<(usize, u32, u32)> {
    graph
        .tasks
        .iter()
        .map(|task| (task.file_ordinal, task.segment_id, task.morsel_id))
        .collect()
}

fn estimate_task_ranges(planned: &PlannedScan) -> Vec<Range<u64>> {
    let mut ranges = Vec::new();
    for task in &planned.graph.tasks {
        let Ok(file) = planned.state.file(task.file_ordinal) else {
            continue;
        };
        let Some(segment) = file.segments().get(task.segment_index) else {
            continue;
        };
        ranges.push(estimate_task_range(segment, task.row_start, task.row_count));
    }
    ranges
}

fn estimate_task_range(
    segment: &cove_core::segment::TableSegmentIndexEntryV1,
    row_start: u64,
    row_count: u32,
) -> Range<u64> {
    if segment.row_count == 0 || segment.length == 0 {
        return segment.offset..segment.offset;
    }
    let relative_row = row_start.saturating_sub(u64::from(segment.row_start));
    let start_delta = ((u128::from(segment.length) * u128::from(relative_row))
        / u128::from(segment.row_count)) as u64;
    let len = ((u128::from(segment.length) * u128::from(row_count)) / u128::from(segment.row_count))
        .max(1) as u64;
    let start = segment.offset.saturating_add(start_delta);
    let end = start
        .saturating_add(len)
        .min(segment.offset.saturating_add(segment.length));
    start..end
}

fn morsel_row_count_for(
    segment: &cove_core::segment::TableSegmentIndexEntryV1,
    morsel_id: u32,
) -> Result<u32, CoveError> {
    let start = morsel_id
        .checked_mul(segment.morsel_row_count)
        .ok_or(CoveError::ArithOverflow)?;
    if start >= segment.row_count {
        return Err(CoveError::OffsetRange);
    }
    let remaining = segment.row_count - start;
    Ok(remaining.min(segment.morsel_row_count))
}

fn decode_stats_json(stats: DecodeStats) -> Value {
    json!({
        "metadata_bytes_read": stats.metadata_bytes_read,
        "data_bytes_read": stats.data_bytes_read,
        "range_requests": stats.range_requests,
        "original_range_requests": stats.original_range_requests,
        "coalesced_range_requests": stats.coalesced_range_requests,
        "range_bytes_requested": stats.range_bytes_requested,
        "range_bytes_used": stats.range_bytes_used,
        "pages_decoded": stats.pages_decoded,
        "rows_selected": stats.rows_selected,
        "rows_materialized": stats.rows_materialized,
        "morsels_considered": stats.morsels_considered,
        "morsels_pruned": stats.morsels_pruned,
        "scan_tasks": stats.scan_tasks,
        "scan_partitions": stats.scan_partitions,
        "residual_rows": stats.residual_rows,
        "exact_predicates": stats.exact_predicates,
        "residual_predicates": stats.residual_predicates,
        "zero_copy_compatible_buffers": stats.zero_copy_compatible_buffers,
        "zero_copy_materialized_buffers": stats.zero_copy_materialized_buffers,
        "lookup_index_hits": stats.lookup_index_hits,
        "lookup_index_misses": stats.lookup_index_misses,
        "index_fallbacks": stats.index_fallbacks,
    })
}

impl Default for CoalescedRangeStats {
    fn default() -> Self {
        Self {
            original_ranges: 0,
            coalesced_ranges: 0,
            original_bytes: 0,
            coalesced_bytes: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_filter_dsl() {
        let filter = parse_filter_dsl("column=id,op=gte,value=42").unwrap();
        assert_eq!(filter.column, "id");
        assert_eq!(filter.op, FilterOp::Gte);
        assert_eq!(filter.value.as_deref(), Some("42"));
    }

    #[test]
    fn parses_hex_byte_literal() {
        assert_eq!(parse_bytes_literal("0x0aFF").unwrap(), vec![10, 255]);
    }
}
