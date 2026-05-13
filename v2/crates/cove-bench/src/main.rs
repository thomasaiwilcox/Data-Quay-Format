//! `cove-bench` — reproducible public v2 benchmark corpus harness.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use arrow_array::{ArrayRef, BooleanArray, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use cove_arrow::convert::{
    convert_arrow_record_batches, ParquetAccelerationPolicy, ParquetAggregatePolicy,
    ParquetConversionOptions, ParquetDictionaryPolicy, ParquetStatsPolicy,
};
use cove_cache::{CoveCoverageCacheHeaderV2, CoverageCacheEntryV2, CoverageCacheV2};
use cove_core::{
    canonical::CanonicalValue,
    checksum,
    constants::{
        CoveEncodingKind, CoveLogicalType, CovePhysicalKind, DigestAlgorithm, PrimaryProfile,
        SectionKind, ValueTag,
    },
    digest::compute_digest,
    durable,
    table::{ColumnEntry, TableCatalog, TableEntry},
    writer::{ScanPageSpec, ScanProfileCoveWriter, ScanSegment, SectionPayload},
};
use cove_coverage::{
    coverage_set_payload_checksum, CoverageExactnessV2, CoverageGranularityV2, CoverageProofKindV2,
    CoverageProofRecordV2, CoverageProofStrengthV2, CoverageProviderDescriptorV2,
    CoverageSetEntryV2, CoverageSetHeaderV2, CoverageSetV2, PredicateAstNodeV2,
    PredicateAstOperandRefV2, PredicateAstPayloadHeaderV2, PredicateFormKindV2, PredicateLiteralV2,
    PredicateNormalFormV2, PredicateNullPolicyV2, PredicateOpV2, PredicateOperandKindV2,
};
use cove_datafusion::{
    bootstrap::bootstrap_local_file_with_options,
    explain::{
        execute_planned_scan, plan_local_file, ExplainOptions, FilterDsl, FilterOp, TopNDsl,
    },
    metadata_aggregate::{
        exact_unfiltered_counts, MetadataAggregatePlan, MetadataAggregateProofKind,
    },
    register::{CoveTableOptions, CoviDiscovery},
};
use cove_index::build::{build_covi_from_cove_bytes, CoviBuildOptions};
use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
use serde_json::{json, Value};
use std::sync::Arc;

const PUBLIC_MANIFEST: &str = include_str!("../benchmarks/public-v2-corpus.json");

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-bench: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("gen") => {
            let profile = option_value(&args, "--profile").unwrap_or_else(|| "ci".into());
            let out = option_value(&args, "--out")
                .map(PathBuf::from)
                .unwrap_or_else(default_corpus_dir);
            generate_corpus(&profile, &out)?;
            println!("generated {profile} benchmark corpus at {}", out.display());
            Ok(())
        }
        Some("run") => {
            let corpus = option_value(&args, "--corpus")
                .map(PathBuf::from)
                .unwrap_or_else(default_corpus_dir);
            let report_json = option_value(&args, "--report-json")
                .map(PathBuf::from)
                .unwrap_or_else(|| corpus.join("report.json"));
            let report_md = option_value(&args, "--report-md")
                .map(PathBuf::from)
                .unwrap_or_else(|| corpus.join("report.md"));
            run_corpus(&corpus, &report_json, &report_md)?;
            println!("wrote benchmark report to {}", report_json.display());
            Ok(())
        }
        Some("check") => {
            let out = default_corpus_dir();
            generate_corpus("ci", &out)?;
            run_corpus(&out, &out.join("report.json"), &out.join("report.md"))?;
            println!("cove-bench check passed at {}", out.display());
            Ok(())
        }
        Some("-h" | "--help") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other:?}")),
    }
}

fn print_usage() {
    println!(
        "Usage:\n  cove-bench gen --profile ci|standard|publication --out <dir>\n  cove-bench run --corpus <dir> --report-json <path> --report-md <path>\n  cove-bench check"
    );
}

fn option_value(args: &[String], option: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == option)
        .map(|window| window[1].clone())
}

fn default_corpus_dir() -> PathBuf {
    PathBuf::from("target/cove-bench/ci")
}

fn generate_corpus(profile: &str, out: &Path) -> Result<(), String> {
    let row_count = match profile {
        "ci" => 2_048,
        "standard" => 32_768,
        "publication" => 262_144,
        other => return Err(format!("unknown benchmark profile {other:?}")),
    };
    fs::create_dir_all(out).map_err(|err| format!("cannot create {}: {err}", out.display()))?;
    fs::write(out.join("public-v2-corpus.json"), PUBLIC_MANIFEST)
        .map_err(|err| format!("cannot write manifest: {err}"))?;

    let batch = events_batch(row_count)?;
    let conversion_options = ParquetConversionOptions {
        table_name: "events".into(),
        namespace: "bench".into(),
        morsel_row_count: 512,
        segment_row_count: 2048,
        dictionary_policy: ParquetDictionaryPolicy::Auto,
        stats_policy: ParquetStatsPolicy::Recompute,
        acceleration_policy: ParquetAccelerationPolicy::Auto,
        point_lookup_columns: vec!["id".into(), "name".into()],
        cluster_columns: vec!["bucket".into()],
        topn_columns: vec!["amount".into()],
        aggregate_policy: ParquetAggregatePolicy::Auto,
        aggregate_columns: vec!["amount".into()],
        emit_covx: true,
        emit_covm: true,
        ..ParquetConversionOptions::default()
    };
    let converted = convert_arrow_record_batches(
        "generated-arrow",
        format!("events-{profile}-{row_count}"),
        batch.schema(),
        vec![batch.clone()],
        &conversion_options,
    )
    .map_err(|err| err.to_string())?;
    durable::durable_replace(&out.join("events.cove"), &converted.cove_bytes)
        .map_err(|err| format!("cannot publish events.cove: {err}"))?;
    if let Some(covx) = converted.covx_bytes {
        durable::durable_replace(&out.join("events.covx"), &covx)
            .map_err(|err| format!("cannot publish events.covx: {err}"))?;
    }
    if let Some(covm) = converted.covm_bytes {
        durable::durable_replace(&out.join("events.covm"), &covm)
            .map_err(|err| format!("cannot publish events.covm: {err}"))?;
    }
    let covi_bytes = build_covi_from_cove_bytes(
        &converted.cove_bytes,
        &CoviBuildOptions {
            column_ids: vec![1, 4],
            include_index_only_counts: true,
            ..CoviBuildOptions::default()
        },
    )
    .map_err(|err| format!("cannot build events.covi: {err}"))?;
    durable::durable_replace(&out.join("events.covi"), &covi_bytes)
        .map_err(|err| format!("cannot publish events.covi: {err}"))?;
    write_parquet_file(&out.join("events.parquet"), &batch)?;

    let cache_fixture = coverage_cache_fixture()?;
    durable::durable_replace(&out.join("synthetic-cache.cove"), &cache_fixture.cove_bytes)
        .map_err(|err| format!("cannot publish synthetic-cache.cove: {err}"))?;
    durable::durable_replace(
        &out.join("synthetic-cache.cove.cache"),
        &cache_fixture.cache_bytes,
    )
    .map_err(|err| format!("cannot publish synthetic-cache.cove.cache: {err}"))?;

    let lock = json!({
        "version": 1,
        "profile": profile,
        "manifest_sha256": hex(&compute_digest(DigestAlgorithm::Sha256, PUBLIC_MANIFEST.as_bytes()).map_err(|err| err.to_string())?),
        "datasets": [
            dataset_lock("events", "events.cove", &converted.cove_bytes)?,
            dataset_lock("events-covi", "events.covi", &covi_bytes)?,
            dataset_lock("synthetic-cache", "synthetic-cache.cove", &cache_fixture.cove_bytes)?,
        ],
    });
    fs::write(
        out.join("corpus.lock.json"),
        serde_json::to_vec_pretty(&lock).map_err(|err| err.to_string())?,
    )
    .map_err(|err| format!("cannot write corpus lock: {err}"))?;
    Ok(())
}

fn dataset_lock(name: &str, path: &str, bytes: &[u8]) -> Result<Value, String> {
    Ok(json!({
        "name": name,
        "path": path,
        "bytes": bytes.len(),
        "sha256": hex(&compute_digest(DigestAlgorithm::Sha256, bytes).map_err(|err| err.to_string())?),
    }))
}

fn run_corpus(corpus: &Path, report_json: &Path, report_md: &Path) -> Result<(), String> {
    let manifest: Value = serde_json::from_str(PUBLIC_MANIFEST).map_err(|err| err.to_string())?;
    let mut cases = Vec::new();
    cases.extend(run_events_cases(corpus)?);
    cases.extend(run_cache_cases(corpus)?);
    validate_report_cases(&cases)?;
    let report = json!({
        "version": 1,
        "manifest": manifest,
        "corpus": corpus.display().to_string(),
        "environment": environment_report(),
        "feature_disclosure": {
            "covx": corpus.join("events.covx").is_file(),
            "covi": corpus.join("events.covi").is_file(),
            "coverage_cache": true,
            "cove_map": false,
            "layout": true,
            "parquet_compare": true,
        },
        "cases": cases,
    });
    if let Some(parent) = report_json.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("cannot create report dir: {err}"))?;
    }
    fs::write(
        report_json,
        serde_json::to_vec_pretty(&report).map_err(|err| err.to_string())?,
    )
    .map_err(|err| format!("cannot write {}: {err}", report_json.display()))?;
    fs::write(report_md, markdown_report(&report))
        .map_err(|err| format!("cannot write {}: {err}", report_md.display()))?;
    Ok(())
}

fn run_events_cases(corpus: &Path) -> Result<Vec<Value>, String> {
    let path = corpus.join("events.cove");
    let mut cases = Vec::new();
    let queries = vec![
        (
            "full_numeric_scan",
            "full numeric scan",
            ExplainOptions {
                projection: Some(vec!["id".into(), "amount".into()]),
                ..ExplainOptions::default()
            },
        ),
        (
            "string_category_scan",
            "string/category scan",
            ExplainOptions {
                projection: Some(vec!["name".into(), "bucket".into()]),
                ..ExplainOptions::default()
            },
        ),
        (
            "equality_filter",
            "equality predicate",
            ExplainOptions {
                filters: vec![FilterDsl {
                    column: "id".into(),
                    op: FilterOp::Eq,
                    value: Some("17".into()),
                }],
                ..ExplainOptions::default()
            },
        ),
        (
            "point_lookup",
            "point lookup predicate",
            ExplainOptions {
                filters: vec![FilterDsl {
                    column: "id".into(),
                    op: FilterOp::Eq,
                    value: Some("1024".into()),
                }],
                ..ExplainOptions::default()
            },
        ),
        (
            "range_filter",
            "range predicate",
            ExplainOptions {
                filters: vec![FilterDsl {
                    column: "amount".into(),
                    op: FilterOp::Gte,
                    value: Some("1000".into()),
                }],
                ..ExplainOptions::default()
            },
        ),
        (
            "top_n",
            "Top-N planning",
            ExplainOptions {
                projection: Some(vec!["id".into(), "amount".into()]),
                top_n: Some(TopNDsl {
                    column: "amount".into(),
                    fetch: 10,
                    descending: true,
                }),
                ..ExplainOptions::default()
            },
        ),
    ];
    for (id, category, options) in queries {
        cases.push(run_query_case(id, category, &path, options)?);
    }
    let parquet = corpus.join("events.parquet");
    cases.push(json!({
        "id": "parquet_conversion_cost",
        "category": "Parquet conversion cost and file-size overhead",
        "status": "measured",
        "metrics": {
            "cove_bytes": fs::metadata(&path).map_err(|err| err.to_string())?.len(),
            "parquet_bytes": fs::metadata(&parquet).map_err(|err| err.to_string())?.len(),
        },
        "optional_features": ["parquet_compare"],
    }));
    if corpus.join("events.covm").is_file() {
        cases.push(json!({
            "id": "covm_many_file_planning",
            "category": "COVM manifest planning",
            "status": "measured",
            "metrics": {
                "manifest_bytes": fs::metadata(corpus.join("events.covm")).map_err(|err| err.to_string())?.len(),
            },
            "optional_features": ["covm"],
        }));
    }
    cases.push(json!({
        "id": "in_filter",
        "category": "IN predicate",
        "status": "skipped",
        "reason": "public harness DSL currently models single-literal predicates only",
    }));
    cases.push(json!({
        "id": "metadata_count_min_max",
        "category": "metadata-only count/min/max",
        "status": "skipped",
        "reason": "DataFusion metadata aggregate path is covered by cove-datafusion tests and Criterion suites; public corpus reporting hook is pending",
    }));
    cases.push(json!({
        "id": "object_store_cold_warm",
        "category": "object-store cold and warm scans",
        "status": "skipped",
        "reason": "local public harness does not start or require a remote object-store service",
    }));
    cases.push(json!({
        "id": "covx_acceleration",
        "category": "COVX acceleration",
        "status": if corpus.join("events.covx").is_file() { "measured" } else { "skipped" },
        "metrics": {
            "covx_present": corpus.join("events.covx").is_file(),
            "covx_bytes": fs::metadata(corpus.join("events.covx")).map(|meta| meta.len()).unwrap_or(0),
        },
        "optional_features": ["covx"],
    }));
    let mut covi_latency = run_query_case(
        "covi_index_latency",
        "COVE-I point lookup latency",
        &path,
        ExplainOptions {
            filters: vec![FilterDsl {
                column: "id".into(),
                op: FilterOp::Eq,
                value: Some("1024".into()),
            }],
            table_options: CoveTableOptions::default()
                .with_covi_discovery(CoviDiscovery::SiblingExtension),
            ..ExplainOptions::default()
        },
    )?;
    if let Some(case) = covi_latency.as_object_mut() {
        case.insert("optional_features".into(), json!(["covi"]));
    }
    cases.push(covi_latency);
    cases.push(run_covi_index_only_count_case(&path)?);
    cases.push(json!({
        "id": "cove_map_identity",
        "category": "COVE-MAP conversion and identity",
        "status": "skipped",
        "reason": "COVE-MAP is validated by its conformance suite; public benchmark identity fixture is pending",
        "optional_features": ["cove_map"],
    }));
    cases.push(json!({
        "id": "layout_scan_split",
        "category": "layout and scan-split planning",
        "status": "measured",
        "metrics": {
            "layout_disclosed": true,
        },
        "optional_features": ["layout"],
    }));
    Ok(cases)
}

fn run_cache_cases(corpus: &Path) -> Result<Vec<Value>, String> {
    let path = corpus.join("synthetic-cache.cove");
    let filter = FilterDsl {
        column: "name".into(),
        op: FilterOp::Eq,
        value: Some("gamma".into()),
    };
    let disabled = run_query_case(
        "coverage_cache_disabled",
        "COVE-CACHE miss/fallback baseline",
        &path,
        ExplainOptions {
            filters: vec![filter.clone()],
            table_options: CoveTableOptions::default(),
            ..ExplainOptions::default()
        },
    )?;
    let enabled = run_query_case(
        "coverage_cache_hit",
        "COVE-CACHE hit",
        &path,
        ExplainOptions {
            filters: vec![filter],
            table_options: CoveTableOptions::default().with_sibling_coverage_cache(),
            ..ExplainOptions::default()
        },
    )?;
    let provider_lookup = json!({
        "id": "coverage_provider_lookup",
        "category": "coverage-provider lookup cost vs scan",
        "status": "measured",
        "metrics": enabled
            .pointer("/cost/coverage_metrics")
            .cloned()
            .unwrap_or(Value::Null),
        "optional_features": ["coverage"],
    });
    let cache_summary = json!({
        "id": "coverage_cache_hit_miss_invalidation",
        "category": "COVE-CACHE hit, miss, and invalidation behavior",
        "status": "measured",
        "metrics": {
            "disabled": disabled.pointer("/cost/coverage_metrics/coverage_cache").cloned().unwrap_or(Value::Null),
            "enabled": enabled.pointer("/cost/coverage_metrics/coverage_cache").cloned().unwrap_or(Value::Null),
        },
        "optional_features": ["coverage_cache"],
    });
    Ok(vec![disabled, enabled, provider_lookup, cache_summary])
}

fn run_query_case(
    id: &str,
    category: &str,
    path: &Path,
    options: ExplainOptions,
) -> Result<Value, String> {
    let plan_start = Instant::now();
    let planned = plan_local_file(path, options).map_err(|err| err.to_string())?;
    let planning_ns = plan_start.elapsed().as_nanos();
    let scan_start = Instant::now();
    let decoded = execute_planned_scan(&planned).map_err(|err| err.to_string())?;
    let scan_ns = scan_start.elapsed().as_nanos();
    let mut cost =
        cove_datafusion::explain::cost_report(&planned, Some(decoded.stats)).to_json_value();
    attach_covi_sidecar_metrics(&mut cost, planned.state.bootstrap_stats());
    Ok(json!({
        "id": id,
        "category": category,
        "status": "measured",
        "metrics": {
            "planning_ns": planning_ns,
            "scan_ns": scan_ns,
            "end_to_end_ns": planning_ns + scan_ns,
            "batches": decoded.batches.len(),
            "rows_materialized": decoded.batches.iter().map(|batch| batch.num_rows()).sum::<usize>(),
        },
        "cost": cost,
    }))
}

fn run_covi_index_only_count_case(path: &Path) -> Result<Value, String> {
    let options = CoveTableOptions::default().with_covi_discovery(CoviDiscovery::SiblingExtension);
    let start = Instant::now();
    let state = bootstrap_local_file_with_options(path, options).map_err(|err| err.to_string())?;
    let plan = exact_unfiltered_counts(state.as_ref(), &[None])
        .map_err(|err| err.to_string())?
        .ok_or_else(|| {
            "COVE-I exact COUNT did not produce a metadata aggregate plan".to_string()
        })?;
    let planning_ns = start.elapsed().as_nanos();
    let proof = plan.proof().clone();
    if proof.kind != MetadataAggregateProofKind::CoviIndexOnlyCount {
        return Err(format!(
            "COVE-I exact COUNT used {:?} instead of CoviIndexOnlyCount",
            proof.kind
        ));
    }
    let counts = match &plan {
        MetadataAggregatePlan::ScalarCounts { counts, .. } => counts,
        _ => return Err("COVE-I exact COUNT returned a non-count aggregate plan".into()),
    };
    let stats = state.bootstrap_stats();
    if stats.covi_sidecars_loaded == 0 {
        return Err("COVE-I exact COUNT did not load a COVI sidecar".into());
    }
    Ok(json!({
        "id": "covi_index_only_count",
        "category": "COVE-I exact index-only COUNT",
        "status": "measured",
        "metrics": {
            "planning_ns": planning_ns,
            "scan_ns": 0,
            "end_to_end_ns": planning_ns,
            "batches": 0,
            "rows_materialized": plan.output_rows(),
            "count": counts.first().copied().unwrap_or(0),
        },
        "cost": {
            "coverage_metrics": {
                "covi_used": true,
                "covi": {
                    "loaded": stats.covi_sidecars_loaded,
                    "stale": stats.covi_sidecars_stale,
                    "ignored": stats.covi_sidecars_ignored,
                    "candidate_pruned": stats.covi_candidate_pruned,
                    "index_only_answers": 1,
                },
            },
        },
        "proof": {
            "kind": format!("{:?}", proof.kind),
            "reason": proof.reason,
        },
        "optional_features": ["covi"],
    }))
}

fn attach_covi_sidecar_metrics(
    cost: &mut Value,
    stats: cove_datafusion::dataset_state::DatasetBootstrapStats,
) {
    if let Some(metrics) = cost
        .get_mut("coverage_metrics")
        .and_then(Value::as_object_mut)
    {
        metrics.insert(
            "covi".into(),
            json!({
                "loaded": stats.covi_sidecars_loaded,
                "stale": stats.covi_sidecars_stale,
                "ignored": stats.covi_sidecars_ignored,
                "candidate_pruned": stats.covi_candidate_pruned,
                "index_only_answers": stats.covi_index_only_answers,
            }),
        );
    }
}

fn validate_report_cases(cases: &[Value]) -> Result<(), String> {
    let required = [
        "full_numeric_scan",
        "string_category_scan",
        "equality_filter",
        "range_filter",
        "top_n",
        "point_lookup",
        "covi_index_latency",
        "covi_index_only_count",
        "parquet_conversion_cost",
        "coverage_cache_disabled",
        "coverage_cache_hit",
        "coverage_cache_hit_miss_invalidation",
    ];
    for id in required {
        if !cases.iter().any(|case| case.get("id") == Some(&json!(id))) {
            return Err(format!("benchmark report missing required case {id}"));
        }
    }
    let cache_hit = cases
        .iter()
        .find(|case| case.get("id") == Some(&json!("coverage_cache_hit")))
        .and_then(|case| {
            case.pointer("/cost/coverage_metrics/coverage_cache/hits")
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    if cache_hit == 0 {
        return Err("coverage_cache_hit did not record a COVE-CACHE hit".into());
    }
    let covi_lookup = require_measured_case(cases, "covi_index_latency")?;
    if !case_bool(covi_lookup, "/cost/coverage_metrics/covi_used") {
        return Err("covi_index_latency did not use COVI candidates".into());
    }
    if case_u64(covi_lookup, "/cost/coverage_metrics/covi_candidates") == 0 {
        return Err("covi_index_latency did not produce any COVI candidates".into());
    }
    if case_u64(covi_lookup, "/cost/coverage_metrics/covi/loaded") == 0 {
        return Err("covi_index_latency did not load a COVI sidecar".into());
    }

    let covi_count = require_measured_case(cases, "covi_index_only_count")?;
    if case_u64(covi_count, "/cost/coverage_metrics/covi/loaded") == 0 {
        return Err("covi_index_only_count did not load a COVI sidecar".into());
    }
    if case_u64(covi_count, "/cost/coverage_metrics/covi/index_only_answers") == 0 {
        return Err("covi_index_only_count did not record COVI index-only evidence".into());
    }
    if covi_count.pointer("/proof/kind").and_then(Value::as_str) != Some("CoviIndexOnlyCount") {
        return Err("covi_index_only_count did not prove CoviIndexOnlyCount".into());
    }
    Ok(())
}

fn require_measured_case<'a>(cases: &'a [Value], id: &str) -> Result<&'a Value, String> {
    let case = cases
        .iter()
        .find(|case| case.get("id") == Some(&json!(id)))
        .ok_or_else(|| format!("benchmark report missing required case {id}"))?;
    if case.get("status").and_then(Value::as_str) != Some("measured") {
        return Err(format!("{id} was not measured"));
    }
    Ok(case)
}

fn case_u64(case: &Value, pointer: &str) -> u64 {
    case.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

fn case_bool(case: &Value, pointer: &str) -> bool {
    case.pointer(pointer)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn markdown_report(report: &Value) -> String {
    let mut out = String::from("# COVE v2 Public Benchmark Report\n\n");
    out.push_str("| Case | Status | Planning ns | Scan ns | Rows |\n");
    out.push_str("| --- | --- | ---: | ---: | ---: |\n");
    if let Some(cases) = report.get("cases").and_then(Value::as_array) {
        for case in cases {
            let id = case.get("id").and_then(Value::as_str).unwrap_or("unknown");
            let status = case
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let metrics = case.get("metrics").unwrap_or(&Value::Null);
            let planning = metrics
                .get("planning_ns")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let scan = metrics.get("scan_ns").and_then(Value::as_u64).unwrap_or(0);
            let rows = metrics
                .get("rows_materialized")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            out.push_str(&format!(
                "| `{id}` | {status} | {planning} | {scan} | {rows} |\n"
            ));
        }
    }
    out
}

fn environment_report() -> Value {
    json!({
        "os": env::consts::OS,
        "arch": env::consts::ARCH,
        "threads": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
    })
}

fn events_batch(row_count: usize) -> Result<RecordBatch, String> {
    let mut ids = Vec::with_capacity(row_count);
    let mut amounts = Vec::with_capacity(row_count);
    let mut buckets = Vec::with_capacity(row_count);
    let mut names = Vec::with_capacity(row_count);
    let mut active = Vec::with_capacity(row_count);
    for row in 0..row_count {
        ids.push(row as i64);
        amounts.push(((row * 37) % 10_000) as i64);
        buckets.push(format!("bucket-{:02}", row % 16));
        names.push(match row % 5 {
            0 => "alpha",
            1 => "beta",
            2 => "gamma",
            3 => "delta",
            _ => "omega",
        });
        active.push(row % 3 != 0);
    }
    RecordBatch::try_from_iter(vec![
        ("id", Arc::new(Int64Array::from(ids)) as ArrayRef),
        ("amount", Arc::new(Int64Array::from(amounts)) as ArrayRef),
        ("bucket", Arc::new(StringArray::from(buckets)) as ArrayRef),
        ("name", Arc::new(StringArray::from(names)) as ArrayRef),
        ("active", Arc::new(BooleanArray::from(active)) as ArrayRef),
    ])
    .map_err(|err| err.to_string())
}

fn write_parquet_file(path: &Path, batch: &RecordBatch) -> Result<(), String> {
    let file =
        fs::File::create(path).map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    let properties = WriterProperties::builder().build();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(properties))
        .map_err(|err| err.to_string())?;
    writer.write(batch).map_err(|err| err.to_string())?;
    writer.close().map_err(|err| err.to_string())?;
    Ok(())
}

struct CoverageCacheFixture {
    cove_bytes: Vec<u8>,
    cache_bytes: Vec<u8>,
}

fn coverage_cache_fixture() -> Result<CoverageCacheFixture, String> {
    let cove_bytes = primitive_events_file_with_name_gamma_coverage(false);
    let state = cove_datafusion::bootstrap::bootstrap_bytes("synthetic-cache", cove_bytes.clone())
        .map_err(|err| err.to_string())?;
    let mut seed = Vec::with_capacity(28);
    seed.extend_from_slice(state.file_id());
    seed.extend_from_slice(&state.file_len().to_le_bytes());
    seed.extend_from_slice(&state.footer_crc32c().to_le_bytes());
    let digest = compute_digest(DigestAlgorithm::Sha256, &seed).map_err(|err| err.to_string())?;
    let mut snapshot_id = [0u8; 16];
    snapshot_id.copy_from_slice(&digest[..16]);
    let dataset_id = *state.file_id();
    let cache = CoverageCacheV2 {
        header: CoveCoverageCacheHeaderV2 {
            cache_format_namespace_ref: 1,
            cache_format_version_major: 2,
            cache_format_version_minor: 0,
            flags: 0,
            cache_id: [7u8; 16],
            dataset_id,
            snapshot_id,
            entry_count: 1,
            created_at_us: 0,
            producer_engine_ref: 0,
            reserved: [0; 32],
            checksum: 0,
        },
        entries: vec![CoverageCacheEntryV2 {
            entry_id: 1,
            dataset_id,
            snapshot_id,
            predicate_normal_form_ref: 1,
            interval_normal_form_ref: u32::MAX,
            coverage_set_ref: 1,
            coverage_granularity: CoverageGranularityV2::Morsel,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            exactness: CoverageExactnessV2::Exact,
            flags: 0,
            actual_coverage_size_bytes: 64,
            actual_read_cost_ns: 1,
            created_at_us: 0,
            valid_until_snapshot_ref: u32::MAX,
            producer_engine_ref: 0,
            checksum: 0,
        }],
    };
    Ok(CoverageCacheFixture {
        cove_bytes,
        cache_bytes: cache.serialize().map_err(|err| err.to_string())?,
    })
}

fn primitive_events_file_with_name_gamma_coverage(bad_checksum: bool) -> Vec<u8> {
    let mut writer = primitive_events_writer();
    for section in name_gamma_coverage_sections(bad_checksum) {
        writer.push_extra_section(section);
    }
    writer.write().unwrap()
}

fn primitive_events_writer() -> ScanProfileCoveWriter {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 1,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 3,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(1, "id", CoveLogicalType::Int64, CovePhysicalKind::NumCode),
                column(2, "name", CoveLogicalType::Utf8, CovePhysicalKind::VarBytes),
                column(
                    3,
                    "active",
                    CoveLogicalType::Bool,
                    CovePhysicalKind::Boolean,
                ),
            ],
        }],
    };
    let mut first = ScanSegment::new(1, 0, 0, 2, 3);
    first.set_column_pages(1, vec![numcode_page(2, numcode_i64(&[1, 2]))]);
    first.set_column_pages(2, vec![varbytes_page(2, varbytes(&["alpha", "beta"]))]);
    first.set_column_pages(3, vec![bool_page(2, bools(&[true, false]))]);

    let mut second = ScanSegment::new(1, 1, 2, 1, 3);
    second.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[3]))]);
    second.set_column_pages(2, vec![varbytes_page(1, varbytes(&["gamma"]))]);
    second.set_column_pages(3, vec![bool_page(1, bools(&[true]))]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_segment(first);
    writer.push_segment(second);
    writer
}

fn column(
    column_id: u32,
    name: &str,
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
) -> ColumnEntry {
    ColumnEntry {
        column_id,
        name: name.into(),
        logical,
        physical,
        nullable: false,
        sort_order: 0,
        collation_id: 0,
        precision: 0,
        scale: 0,
        flags: 0,
    }
}

fn name_gamma_coverage_sections(bad_checksum: bool) -> Vec<SectionPayload> {
    let predicate_form_ref = 1;
    let provider_id = 1;
    let coverage_set_id = 1;
    let snapshot_validity_ref = 1;
    let predicate_form_section =
        predicate_normal_form_ast_section(predicate_form_ref, 1, name_eq_gamma_ast_payload());

    let provider = CoverageProviderDescriptorV2 {
        provider_id,
        provider_kind: CoverageProofKindV2::ValueToFragmentIndex as u16,
        profile: PrimaryProfile::CoverageMetadata as u8,
        granularity: CoverageGranularityV2::Morsel,
        proof_strength: CoverageProofStrengthV2::ExactConservative,
        exactness: CoverageExactnessV2::Exact,
        flags: 0,
        referenced_table_id: 1,
        referenced_column_id: 2,
        referenced_path_ref: u32::MAX,
        logical_type: CoveLogicalType::Utf8 as u16,
        collation_id: 0,
        null_semantics: 0,
        snapshot_validity_ref,
        predicate_form_ref,
        producer_ref: u32::MAX,
        checksum: 0,
    };
    let coverage_set = CoverageSetV2 {
        header: CoverageSetHeaderV2 {
            coverage_set_id,
            provider_id,
            granularity: CoverageGranularityV2::Morsel,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            exactness: CoverageExactnessV2::Exact,
            flags: 0,
            predicate_form_ref,
            snapshot_validity_ref,
            total_fragment_count: 2,
            covered_fragment_count: 0,
            required_fragment_count_estimate: 0,
            coverage_degree_ppm: 500_000,
            tightness_degree_ppm: 1_000_000,
            entries_offset: 0,
            entries_length: 0,
            checksum: 0,
        },
        entries: vec![CoverageSetEntryV2 {
            target_kind: CoverageGranularityV2::Morsel,
            flags: 0,
            file_ref: 0,
            table_id: 1,
            segment_id: 1,
            morsel_id: 0,
            page_ref: u32::MAX,
            object_type_id: u32::MAX,
            path_ref: u32::MAX,
            dimensional_bucket_ref: u32::MAX,
            row_start: 0,
            row_count: 0,
            row_ordinal_bitmap_ref: u32::MAX,
            byte_range_ref: u32::MAX,
            checksum: 0,
        }],
    };
    let coverage_set_bytes = coverage_set.serialize().unwrap();
    let mut coverage_set_checksum = coverage_set_payload_checksum(&coverage_set_bytes);
    if bad_checksum {
        coverage_set_checksum ^= 1;
    }
    let proof = CoverageProofRecordV2 {
        proof_id: 1,
        provider_id,
        coverage_set_id,
        predicate_form_ref,
        proof_kind: CoverageProofKindV2::ValueToFragmentIndex,
        proof_strength: CoverageProofStrengthV2::ExactConservative,
        exactness: CoverageExactnessV2::Exact,
        granularity: CoverageGranularityV2::Morsel,
        null_semantics: 0,
        flags: 0,
        snapshot_validity_ref,
        coverage_set_checksum,
        proof_payload_ref: u32::MAX,
        checksum: 0,
    };

    vec![
        coverage_section(
            SectionKind::CoverageProviderRegistry,
            1,
            provider.serialize().to_vec(),
        ),
        coverage_section(SectionKind::CoverageSet, 1, coverage_set_bytes),
        coverage_section(
            SectionKind::CoverageProofRecord,
            1,
            proof.serialize().unwrap().to_vec(),
        ),
        predicate_form_section,
    ]
}

fn predicate_normal_form_ast_section(
    predicate_form_id: u32,
    table_id: u32,
    payload: Vec<u8>,
) -> SectionPayload {
    let form = PredicateNormalFormV2 {
        predicate_form_id,
        form_kind: PredicateFormKindV2::PredicateAst,
        flags: 0,
        logical_context_ref: table_id,
        payload_offset: PredicateNormalFormV2::LEN as u64,
        payload_length: payload.len() as u64,
        checksum: 0,
    };
    let mut data = Vec::with_capacity(PredicateNormalFormV2::LEN + payload.len());
    data.extend_from_slice(&form.serialize().unwrap());
    data.extend_from_slice(&payload);
    coverage_section(SectionKind::PredicateNormalForm, 1, data)
}

fn name_eq_gamma_ast_payload() -> Vec<u8> {
    let canonical = CanonicalValue::Utf8("gamma").encode().unwrap();
    let node_offset = PredicateAstPayloadHeaderV2::LEN;
    let literal_offset = node_offset + PredicateAstNodeV2::LEN;
    let operand_ref_offset = literal_offset + PredicateLiteralV2::LEN;
    let canonical_offset = operand_ref_offset + 2 * PredicateAstOperandRefV2::LEN;

    let mut payload = Vec::new();
    payload.extend_from_slice(&predicate_ast_header(
        node_offset as u64,
        literal_offset as u64,
        operand_ref_offset as u64,
    ));
    payload.extend_from_slice(&predicate_ast_node());
    payload.extend_from_slice(&predicate_ast_literal(
        canonical_offset as u64,
        canonical.len() as u32,
    ));
    payload.extend_from_slice(&predicate_ast_operand_ref(
        0,
        PredicateOperandKindV2::ColumnOrPath,
        2,
    ));
    payload.extend_from_slice(&predicate_ast_operand_ref(
        1,
        PredicateOperandKindV2::Literal,
        0,
    ));
    payload.extend_from_slice(&canonical);
    payload
}

fn predicate_ast_header(
    node_offset: u64,
    literal_offset: u64,
    operand_ref_offset: u64,
) -> [u8; PredicateAstPayloadHeaderV2::LEN] {
    let mut out = [0u8; PredicateAstPayloadHeaderV2::LEN];
    out[0..4].copy_from_slice(&0u32.to_le_bytes());
    out[4..8].copy_from_slice(&1u32.to_le_bytes());
    out[8..12].copy_from_slice(&1u32.to_le_bytes());
    out[20..24].copy_from_slice(&2u32.to_le_bytes());
    out[24..32].copy_from_slice(&node_offset.to_le_bytes());
    out[32..40].copy_from_slice(&literal_offset.to_le_bytes());
    out[56..64].copy_from_slice(&operand_ref_offset.to_le_bytes());
    let crc = checksum::crc32c(&out);
    out[68..72].copy_from_slice(&crc.to_le_bytes());
    out
}

fn predicate_ast_node() -> [u8; PredicateAstNodeV2::LEN] {
    let mut out = [0u8; PredicateAstNodeV2::LEN];
    out[0..4].copy_from_slice(&0u32.to_le_bytes());
    out[4..6].copy_from_slice(&(PredicateOpV2::Eq as u16).to_le_bytes());
    out[8..10].copy_from_slice(&(CoveLogicalType::Bool as u16).to_le_bytes());
    out[12] = PredicateNullPolicyV2::SqlWhere as u8;
    out[14..16].copy_from_slice(&2u16.to_le_bytes());
    out[16..20].copy_from_slice(&0u32.to_le_bytes());
    out[20..24].copy_from_slice(&2u32.to_le_bytes());
    out[24..28].copy_from_slice(&0u32.to_le_bytes());
    out[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
    out[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
    let crc = checksum::crc32c(&out);
    out[36..40].copy_from_slice(&crc.to_le_bytes());
    out
}

fn predicate_ast_literal(
    canonical_value_offset: u64,
    canonical_value_length: u32,
) -> [u8; PredicateLiteralV2::LEN] {
    let mut out = [0u8; PredicateLiteralV2::LEN];
    out[0..4].copy_from_slice(&0u32.to_le_bytes());
    out[4..6].copy_from_slice(&(ValueTag::Utf8 as u16).to_le_bytes());
    out[6..8].copy_from_slice(&(CoveLogicalType::Utf8 as u16).to_le_bytes());
    out[12..20].copy_from_slice(&canonical_value_offset.to_le_bytes());
    out[20..24].copy_from_slice(&canonical_value_length.to_le_bytes());
    let crc = checksum::crc32c(&out);
    out[24..28].copy_from_slice(&crc.to_le_bytes());
    out
}

fn predicate_ast_operand_ref(
    ordinal: u16,
    operand_kind: PredicateOperandKindV2,
    ref_id: u32,
) -> [u8; PredicateAstOperandRefV2::LEN] {
    let mut out = [0u8; PredicateAstOperandRefV2::LEN];
    out[0..4].copy_from_slice(&0u32.to_le_bytes());
    out[4..6].copy_from_slice(&ordinal.to_le_bytes());
    out[6] = operand_kind as u8;
    out[8..12].copy_from_slice(&ref_id.to_le_bytes());
    let crc = checksum::crc32c(&out);
    out[12..16].copy_from_slice(&crc.to_le_bytes());
    out
}

fn coverage_section(kind: SectionKind, item_count: u64, data: Vec<u8>) -> SectionPayload {
    SectionPayload {
        section_kind: kind as u16,
        profile: PrimaryProfile::CoverageMetadata as u8,
        flags: 0,
        item_count,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data,
    }
}

fn numcode_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::NumCode as u32)
}

fn varbytes_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::VarBytes as u32)
}

fn bool_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::PlainFixed as u32)
}

fn numcode_i64(values: &[i64]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| (*value as u64).to_le_bytes())
        .collect()
}

fn varbytes(values: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    out
}

fn bools(values: &[bool]) -> Vec<u8> {
    values.iter().map(|value| u8::from(*value)).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[allow(dead_code)]
fn _schema_for_docs() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("amount", DataType::Int64, false),
        Field::new("bucket", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("active", DataType::Boolean, false),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_manifest_declares_required_groups() {
        let manifest: Value = serde_json::from_str(PUBLIC_MANIFEST).unwrap();
        let groups = manifest
            .get("query_groups")
            .and_then(Value::as_array)
            .unwrap();
        for required in [
            "full_numeric_scan",
            "parquet_conversion_cost",
            "covi_index_latency",
            "covi_index_only_count",
            "coverage_cache_hit_miss_invalidation",
        ] {
            assert!(groups.iter().any(|group| group.as_str() == Some(required)));
        }
    }

    #[test]
    fn synthetic_cache_fixture_records_planner_hit() {
        let fixture = coverage_cache_fixture().unwrap();
        let dir = env::temp_dir().join(format!("cove-bench-cache-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let cove_path = dir.join("synthetic-cache.cove");
        let cache_path = dir.join("synthetic-cache.cove.cache");
        fs::write(&cove_path, fixture.cove_bytes).unwrap();
        fs::write(&cache_path, fixture.cache_bytes).unwrap();

        let case = run_query_case(
            "coverage_cache_hit",
            "COVE-CACHE hit",
            &cove_path,
            ExplainOptions {
                filters: vec![FilterDsl {
                    column: "name".into(),
                    op: FilterOp::Eq,
                    value: Some("gamma".into()),
                }],
                table_options: CoveTableOptions::default().with_sibling_coverage_cache(),
                ..ExplainOptions::default()
            },
        )
        .unwrap();
        let hits = case
            .pointer("/cost/coverage_metrics/coverage_cache/hits")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        assert!(hits > 0);
        let _ = fs::remove_file(cove_path);
        let _ = fs::remove_file(cache_path);
        let _ = fs::remove_dir(dir);
    }
}
