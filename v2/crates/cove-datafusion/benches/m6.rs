#![allow(unused_imports)]

use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[cfg(feature = "parquet-compare")]
use arrow_array::{ArrayRef, BooleanArray, Int64Array, RecordBatch, StringArray};
#[cfg(feature = "covm")]
use cove_core::artifact::covm::{CovmFile, CovmFileEntryV1, CovmHeaderV1, CovmPostscriptV1};
#[cfg(feature = "covm")]
use cove_core::constants::DigestAlgorithm;
use cove_core::{
    constants::{
        CoveEncodingKind, CoveLogicalType, CovePhysicalKind, PrimaryProfile, SectionKind,
        StorageClass, ValueTag,
    },
    dictionary::{
        FileDictionary, FileDictionaryEncoding, FileDictionaryHeaderV1, FileDictionaryIndexEntryV1,
        FileDictionaryKey,
    },
    domain::ColumnDomain,
    index::{
        lookup::{
            LookupEntry, LookupIndex, LookupIndexHeaderV1, LookupIndexKind, LookupKeyKind,
            LookupUniqueness,
        },
        topn::{TopNDirection, TopNSummary, TOPN_ZONE_SUMMARY_LEN},
    },
    row_ref::RowRef,
    table::{ColumnEntry, TableCatalog, TableEntry},
    wire,
    writer::{ScanPageSpec, ScanProfileCoveWriter, ScanSegment, SectionPayload},
    zone_stats::{ZoneStatFlags, ZoneStats, ZoneStatsEntry, ZoneStatsSection},
};
#[cfg(feature = "covm")]
use cove_datafusion::register::register_cove_covm;
use cove_datafusion::{
    bootstrap::{bootstrap_bytes, bootstrap_local_file},
    dataset_state::DatasetState,
    decode::{decode_scan, DecodeStats},
    metadata_aggregate::exact_unfiltered_counts,
    options::CoveTableOptions,
    overlay::{CoveOverlaySnapshot, OverlayFile, OverlayFileIdentity, RowRange, RowVisibility},
    planner::{plan_scan, FilterPlan, NumericPredicateOp, PredicateLiteral, TopNScanHint},
    register::{
        register_cove_file, register_cove_file_with_options, register_cove_overlay_snapshot,
    },
};
use criterion::{black_box, criterion_group, Criterion};
#[cfg(feature = "parquet-compare")]
use datafusion::execution::context::SessionConfig;
#[cfg(feature = "parquet-compare")]
use datafusion::physical_plan::execution_plan::{
    collect as collect_execution_plan, reset_plan_states,
};
#[cfg(feature = "parquet-compare")]
use datafusion::prelude::ParquetReadOptions;
use datafusion::prelude::SessionContext;
#[cfg(feature = "parquet-compare")]
use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "parquet-compare")]
const PARQUET_COMPARE_SCAN_HEAVY_ROWS: usize = 32_768;
#[cfg(feature = "parquet-compare")]
const PARQUET_COMPARE_WIDE_ROWS: usize = 16_384;
#[cfg(feature = "parquet-compare")]
const PARQUET_COMPARE_SEGMENT_ROWS: usize = 4_096;
#[cfg(feature = "parquet-compare")]
const PARQUET_COMPARE_WIDE_COLUMNS: usize = 12;

fn bench_m6_native(c: &mut Criterion) {
    let scan_state = bootstrap_bytes("m6-scan.cove", primitive_events_file())
        .expect("scan fixture should bootstrap");
    let filecode_state = bootstrap_bytes("m6-filecode.cove", dictionary_items_file_with_domain())
        .expect("FileCode fixture should bootstrap");
    let lookup_state = bootstrap_bytes("m6-lookup.cove", numeric_lookup_events_file())
        .expect("lookup fixture should bootstrap");
    let wide_state =
        bootstrap_bytes("m6-wide.cove", wide_events_file()).expect("wide fixture should bootstrap");
    let topn_state =
        bootstrap_bytes("m6-topn.cove", topn_events_file()).expect("TopN fixture should bootstrap");
    let overlay_fixture = OverlayFixture::new();
    let runtime = Runtime::new().expect("benchmark runtime");

    c.bench_function("m6_full_scan", |b| {
        b.iter(|| {
            let decoded = decode_planned(&scan_state, None, Vec::new());
            assert_rows(&decoded.stats, 3, 3);
            black_box(m6_metrics(decoded.stats))
        })
    });

    c.bench_function("m6_projection_scan", |b| {
        b.iter(|| {
            let decoded = decode_planned(&scan_state, Some(vec![1]), Vec::new());
            assert_rows(&decoded.stats, 3, 3);
            assert_eq!(decoded.stats.pages_decoded, 2);
            black_box(m6_metrics(decoded.stats))
        })
    });

    c.bench_function("m6_filecode_equality_filter", |b| {
        b.iter(|| {
            let decoded = decode_planned(
                &filecode_state,
                Some(vec![1]),
                vec![FilterPlan::pruning_file_code_in(0, vec![0], "name = 'red'")],
            );
            assert_rows(&decoded.stats, 1, 1);
            assert_eq!(decoded.stats.morsels_pruned, 1);
            black_box(m6_metrics(decoded.stats))
        })
    });

    c.bench_function("m6_numeric_range_filter", |b| {
        b.iter(|| {
            let decoded = decode_planned(
                &scan_state,
                Some(vec![1]),
                vec![FilterPlan::pruning_numeric(
                    0,
                    NumericPredicateOp::GtEq,
                    PredicateLiteral::Int64(2),
                    "id >= 2",
                )],
            );
            assert_rows(&decoded.stats, 2, 2);
            black_box(m6_metrics(decoded.stats))
        })
    });

    c.bench_function("m6_lookup_backed_point_filter", |b| {
        b.iter(|| {
            let decoded = decode_planned(
                &lookup_state,
                Some(vec![1]),
                vec![FilterPlan::pruning_numeric(
                    0,
                    NumericPredicateOp::Eq,
                    PredicateLiteral::Int64(2),
                    "id = 2",
                )],
            );
            assert_rows(&decoded.stats, 1, 1);
            assert_eq!(decoded.stats.lookup_index_hits, 1);
            black_box(m6_metrics(decoded.stats))
        })
    });

    c.bench_function("m6_late_materialization_wide_rows", |b| {
        b.iter(|| {
            let decoded = decode_planned(
                &wide_state,
                Some(vec![12]),
                vec![FilterPlan::pruning_numeric(
                    0,
                    NumericPredicateOp::Gt,
                    PredicateLiteral::Int64(6),
                    "id > 6",
                )],
            );
            assert_rows(&decoded.stats, 2, 2);
            assert!(decoded.stats.pages_decoded < wide_state.table().columns.len());
            black_box(m6_metrics(decoded.stats))
        })
    });

    c.bench_function("m6_metadata_count_fast_path", |b| {
        b.iter(|| {
            let plan = exact_unfiltered_counts(&scan_state, &[None])
                .expect("metadata count proof")
                .expect("count should be proven from metadata");
            assert_eq!(plan.output_rows(), 1);
            black_box(plan)
        })
    });

    c.bench_function("m6_topn_hinted_scan", |b| {
        b.iter(|| {
            let mut plan = plan_scan(&topn_state, None, Vec::new()).expect("TopN scan plan");
            plan.topn_hint = Some(TopNScanHint {
                column_index: 0,
                descending: true,
                fetch: 1,
            });
            let decoded = decode_scan(&topn_state, &plan).expect("TopN hinted decode");
            assert_rows(&decoded.stats, 2, 2);
            black_box(m6_metrics(decoded.stats))
        })
    });

    c.bench_function("m6_overlay_restricted_scan", |b| {
        b.iter(|| {
            let rows = overlay_fixture.scan_rows(&runtime);
            assert_eq!(rows, 1);
            black_box(rows)
        })
    });

    #[cfg(feature = "covm")]
    {
        let covm_fixture = CovmFixture::new();
        c.bench_function("m6_covm_multi_file_scan", |b| {
            b.iter(|| {
                let rows = covm_fixture.scan_rows(&runtime);
                assert_eq!(rows, 6);
                black_box(rows)
            })
        });
    }
}

#[cfg(feature = "parquet-compare")]
fn bench_m6_parquet_compare(c: &mut Criterion) {
    let runtime = Runtime::new().expect("benchmark runtime");
    let fixture = ParquetCompareFixture::new(&runtime);

    let full_scan_cove = fixture.prepare_query(&runtime, "SELECT * FROM events_cove", 3, 3);
    let full_scan_parquet = fixture.prepare_query(&runtime, "SELECT * FROM events_parquet", 3, 3);
    bench_query_pair(
        c,
        "parquet_compare_full_scan",
        &runtime,
        &fixture.ctx,
        &full_scan_cove,
        &full_scan_parquet,
    );

    let projection_cove = fixture.prepare_query(&runtime, "SELECT name FROM events_cove", 3, 1);
    let projection_parquet =
        fixture.prepare_query(&runtime, "SELECT name FROM events_parquet", 3, 1);
    bench_query_pair(
        c,
        "parquet_compare_projection_scan",
        &runtime,
        &fixture.ctx,
        &projection_cove,
        &projection_parquet,
    );

    let filecode_like_cove = fixture.prepare_query(
        &runtime,
        "SELECT payload FROM items_cove WHERE name = 'red'",
        1,
        1,
    );
    let filecode_like_parquet = fixture.prepare_query(
        &runtime,
        "SELECT payload FROM items_parquet WHERE name = 'red'",
        1,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_low_cardinality_filter",
        &runtime,
        &fixture.ctx,
        &filecode_like_cove,
        &filecode_like_parquet,
    );

    let range_cove =
        fixture.prepare_query(&runtime, "SELECT name FROM events_cove WHERE id >= 2", 2, 1);
    let range_parquet = fixture.prepare_query(
        &runtime,
        "SELECT name FROM events_parquet WHERE id >= 2",
        2,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_numeric_range_filter",
        &runtime,
        &fixture.ctx,
        &range_cove,
        &range_parquet,
    );

    let wide_cove = fixture.prepare_query(
        &runtime,
        "SELECT payload_12 FROM wide_events_cove WHERE id > 6",
        2,
        1,
    );
    let wide_parquet = fixture.prepare_query(
        &runtime,
        "SELECT payload_12 FROM wide_events_parquet WHERE id > 6",
        2,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_wide_projection_filter",
        &runtime,
        &fixture.ctx,
        &wide_cove,
        &wide_parquet,
    );

    let large_full_scan_cove = fixture.prepare_query(
        &runtime,
        "SELECT * FROM large_events_cove",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        4,
    );
    let large_full_scan_parquet = fixture.prepare_query(
        &runtime,
        "SELECT * FROM large_events_parquet",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        4,
    );
    bench_query_pair(
        c,
        "parquet_compare_scan_heavy_full_scan",
        &runtime,
        &fixture.ctx,
        &large_full_scan_cove,
        &large_full_scan_parquet,
    );

    let large_projection_cove = fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_cove",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        1,
    );
    let large_projection_parquet = fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_parquet",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_scan_heavy_projection_scan",
        &runtime,
        &fixture.ctx,
        &large_projection_cove,
        &large_projection_parquet,
    );
    let large_projection_filecode_decoded = fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_filecode_decoded_cove",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_scan_heavy_projection_scan_filecode_decoded",
        &runtime,
        &fixture.ctx,
        &large_projection_filecode_decoded,
        &large_projection_parquet,
    );
    let large_projection_filecode_dictionary = fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_filecode_cove",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_scan_heavy_projection_scan_filecode_dictionary",
        &runtime,
        &fixture.ctx,
        &large_projection_filecode_dictionary,
        &large_projection_parquet,
    );

    let view_fixture = ParquetCompareFixture::new_with_cove_options(
        &runtime,
        CoveTableOptions::default().with_arrow_view_output(),
    );
    let trusted_fixture = ParquetCompareFixture::new_with_cove_options(
        &runtime,
        CoveTableOptions::default().with_trusted_arrow_string_validation(),
    );
    let mmap_fixture = ParquetCompareFixture::new_with_cove_options(
        &runtime,
        CoveTableOptions::default().with_local_file_mmap_reads(),
    );
    let trusted_mmap_fixture = ParquetCompareFixture::new_with_cove_options(
        &runtime,
        CoveTableOptions::default()
            .with_trusted_arrow_string_validation_and_local_file_mmap_reads(),
    );
    let large_projection_view = view_fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_cove",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        1,
    );
    let large_projection_trusted = trusted_fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_cove",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        1,
    );
    let large_projection_mmap = mmap_fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_cove",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        1,
    );
    let large_projection_trusted_mmap = trusted_mmap_fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_cove",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        1,
    );
    let mut arrow_output_group =
        c.benchmark_group("cove_arrow_varbytes_output_scan_heavy_projection");
    arrow_output_group.bench_function("standard-strict", |b| {
        b.iter(|| {
            black_box(execute_prepared_query(
                &runtime,
                &fixture.ctx,
                &large_projection_cove,
            ))
        })
    });
    arrow_output_group.bench_function("standard-trusted", |b| {
        b.iter(|| {
            black_box(execute_prepared_query(
                &runtime,
                &trusted_fixture.ctx,
                &large_projection_trusted,
            ))
        })
    });
    arrow_output_group.bench_function("standard-strict-mmap", |b| {
        b.iter(|| {
            black_box(execute_prepared_query(
                &runtime,
                &mmap_fixture.ctx,
                &large_projection_mmap,
            ))
        })
    });
    arrow_output_group.bench_function("standard-trusted-mmap", |b| {
        b.iter(|| {
            black_box(execute_prepared_query(
                &runtime,
                &trusted_mmap_fixture.ctx,
                &large_projection_trusted_mmap,
            ))
        })
    });
    arrow_output_group.bench_function("view", |b| {
        b.iter(|| {
            black_box(execute_prepared_query(
                &runtime,
                &view_fixture.ctx,
                &large_projection_view,
            ))
        })
    });
    arrow_output_group.bench_function("filecode-dictionary", |b| {
        b.iter(|| {
            black_box(execute_prepared_query(
                &runtime,
                &fixture.ctx,
                &large_projection_filecode_dictionary,
            ))
        })
    });
    arrow_output_group.finish();

    let large_low_cardinality_cove = fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_cove WHERE category = 'group_03'",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS / 8,
        1,
    );
    let large_low_cardinality_parquet = fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_parquet WHERE category = 'group_03'",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS / 8,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_scan_heavy_low_cardinality_filter",
        &runtime,
        &fixture.ctx,
        &large_low_cardinality_cove,
        &large_low_cardinality_parquet,
    );
    let large_low_cardinality_filecode_dictionary = fixture.prepare_query(
        &runtime,
        "SELECT payload FROM large_events_filecode_cove WHERE category = 'group_03'",
        PARQUET_COMPARE_SCAN_HEAVY_ROWS / 8,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_scan_heavy_low_cardinality_filter_filecode_dictionary",
        &runtime,
        &fixture.ctx,
        &large_low_cardinality_filecode_dictionary,
        &large_low_cardinality_parquet,
    );

    let large_range_sql = format!(
        "SELECT payload FROM large_events_{{}} WHERE id >= {}",
        scan_heavy_range_start(PARQUET_COMPARE_SCAN_HEAVY_ROWS)
    );
    let large_range_rows =
        PARQUET_COMPARE_SCAN_HEAVY_ROWS - PARQUET_COMPARE_SCAN_HEAVY_ROWS * 3 / 4;
    let large_range_cove = fixture.prepare_query(
        &runtime,
        &large_range_sql.replace("{}", "cove"),
        large_range_rows,
        1,
    );
    let large_range_parquet = fixture.prepare_query(
        &runtime,
        &large_range_sql.replace("{}", "parquet"),
        large_range_rows,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_scan_heavy_numeric_range_filter",
        &runtime,
        &fixture.ctx,
        &large_range_cove,
        &large_range_parquet,
    );
    let large_range_filecode_dictionary = fixture.prepare_query(
        &runtime,
        &format!(
            "SELECT payload FROM large_events_filecode_cove WHERE id >= {}",
            scan_heavy_range_start(PARQUET_COMPARE_SCAN_HEAVY_ROWS)
        ),
        large_range_rows,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_scan_heavy_numeric_range_filter_filecode_dictionary",
        &runtime,
        &fixture.ctx,
        &large_range_filecode_dictionary,
        &large_range_parquet,
    );

    let large_wide_sql = format!(
        "SELECT payload_12 FROM large_wide_events_{{}} WHERE id > {}",
        PARQUET_COMPARE_WIDE_ROWS * 3 / 4
    );
    let large_wide_rows = PARQUET_COMPARE_WIDE_ROWS - PARQUET_COMPARE_WIDE_ROWS * 3 / 4;
    let large_wide_cove = fixture.prepare_query(
        &runtime,
        &large_wide_sql.replace("{}", "cove"),
        large_wide_rows,
        1,
    );
    let large_wide_parquet = fixture.prepare_query(
        &runtime,
        &large_wide_sql.replace("{}", "parquet"),
        large_wide_rows,
        1,
    );
    bench_query_pair(
        c,
        "parquet_compare_scan_heavy_wide_projection_filter",
        &runtime,
        &fixture.ctx,
        &large_wide_cove,
        &large_wide_parquet,
    );

    let cold_full_scan_cove = ColdQuery {
        sql: Arc::<str>::from("SELECT * FROM events"),
        expected_rows: PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        expected_columns: 4,
        source: CompareSource::Cove,
        path: fixture.large_events.cove.clone(),
    };
    let cold_full_scan_parquet = ColdQuery {
        sql: Arc::<str>::from("SELECT * FROM events"),
        expected_rows: PARQUET_COMPARE_SCAN_HEAVY_ROWS,
        expected_columns: 4,
        source: CompareSource::Parquet,
        path: fixture.large_events.parquet.clone(),
    };
    bench_cold_query_pair(
        c,
        "parquet_compare_cold_context_full_scan",
        &runtime,
        &cold_full_scan_cove,
        &cold_full_scan_parquet,
    );

    let cold_range_sql = Arc::<str>::from(format!(
        "SELECT payload FROM events WHERE id >= {}",
        scan_heavy_range_start(PARQUET_COMPARE_SCAN_HEAVY_ROWS)
    ));
    let cold_range_cove = ColdQuery {
        sql: Arc::clone(&cold_range_sql),
        expected_rows: large_range_rows,
        expected_columns: 1,
        source: CompareSource::Cove,
        path: fixture.large_events.cove.clone(),
    };
    let cold_range_parquet = ColdQuery {
        sql: cold_range_sql,
        expected_rows: large_range_rows,
        expected_columns: 1,
        source: CompareSource::Parquet,
        path: fixture.large_events.parquet.clone(),
    };
    bench_cold_query_pair(
        c,
        "parquet_compare_cold_context_numeric_range_filter",
        &runtime,
        &cold_range_cove,
        &cold_range_parquet,
    );
}

#[cfg(feature = "parquet-compare")]
fn bench_query_pair(
    c: &mut Criterion,
    name: &str,
    runtime: &Runtime,
    ctx: &SessionContext,
    cove: &PreparedQuery,
    parquet: &PreparedQuery,
) {
    let mut group = c.benchmark_group(name);
    group.bench_function("cove", |b| {
        b.iter(|| black_box(execute_prepared_query(runtime, ctx, cove)))
    });
    group.bench_function("parquet", |b| {
        b.iter(|| black_box(execute_prepared_query(runtime, ctx, parquet)))
    });
    group.finish();
}

#[cfg(feature = "parquet-compare")]
fn bench_cold_query_pair(
    c: &mut Criterion,
    name: &str,
    runtime: &Runtime,
    cove: &ColdQuery,
    parquet: &ColdQuery,
) {
    let mut group = c.benchmark_group(name);
    group.bench_function("cove", |b| {
        b.iter(|| black_box(execute_cold_query(runtime, cove)))
    });
    group.bench_function("parquet", |b| {
        b.iter(|| black_box(execute_cold_query(runtime, parquet)))
    });
    group.finish();
}

fn decode_planned(
    state: &DatasetState,
    projection: Option<Vec<usize>>,
    filters: Vec<FilterPlan>,
) -> cove_datafusion::decode::DecodedScan {
    let plan = plan_scan(state, projection.as_ref(), filters).expect("scan plan");
    decode_scan(state, &plan).expect("scan decode")
}

fn assert_rows(stats: &DecodeStats, selected: usize, materialized: usize) {
    assert_eq!(stats.rows_selected, selected);
    assert_eq!(stats.rows_materialized, materialized);
}

fn m6_metrics(stats: DecodeStats) -> (usize, usize, usize, usize, usize, usize, usize) {
    (
        stats.metadata_bytes_read + stats.data_bytes_read,
        stats.pages_decoded,
        stats.rows_selected,
        stats.rows_materialized,
        stats.morsels_pruned,
        stats.range_requests,
        stats.residual_rows,
    )
}

struct OverlayFixture {
    _dir: TempFixtureDir,
    ctx: SessionContext,
}

impl OverlayFixture {
    fn new() -> Self {
        let dir = TempFixtureDir::new("m6-overlay");
        let path = dir.path.join("part1.cove");
        fs::write(&path, primitive_events_file()).expect("write overlay COVE fixture");
        let base = bootstrap_local_file(&path).expect("bootstrap overlay identity");
        let snapshot = CoveOverlaySnapshot {
            snapshot_id: "m6-overlay".into(),
            files: vec![OverlayFile {
                uri: path.display().to_string().into(),
                expected_identity: Some(identity_for_state(&base)),
                visibility: RowVisibility::VisibleRanges(vec![RowRange { start: 2, len: 1 }]),
            }],
        };
        let ctx = SessionContext::new();
        register_cove_overlay_snapshot(&ctx, "events", snapshot, CoveTableOptions::default())
            .expect("register overlay fixture");
        Self { _dir: dir, ctx }
    }

    fn scan_rows(&self, runtime: &Runtime) -> usize {
        runtime.block_on(async {
            self.ctx
                .sql("SELECT id FROM events")
                .await
                .expect("overlay query")
                .collect()
                .await
                .expect("overlay collect")
                .iter()
                .map(|batch| batch.num_rows())
                .sum()
        })
    }
}

#[cfg(feature = "covm")]
struct CovmFixture {
    _dir: TempFixtureDir,
    ctx: SessionContext,
}

#[cfg(feature = "covm")]
impl CovmFixture {
    fn new() -> Self {
        let dir = TempFixtureDir::new("m6-covm");
        let first = dir.path.join("part1.cove");
        let second = dir.path.join("part2.cove");
        fs::write(&first, primitive_events_file()).expect("write first COVM fixture");
        fs::write(&second, primitive_events_file()).expect("write second COVM fixture");
        let manifest = dir.path.join("dataset.covm");
        write_covm_manifest(
            &manifest,
            vec![
                covm_entry_for_path("part1.cove", &first),
                covm_entry_for_path("part2.cove", &second),
            ],
        );
        let ctx = SessionContext::new();
        let provider = register_cove_covm(&ctx, "events", &manifest).expect("register COVM");
        assert_eq!(provider.state().file_count(), 2);
        Self { _dir: dir, ctx }
    }

    fn scan_rows(&self, runtime: &Runtime) -> usize {
        runtime.block_on(async {
            self.ctx
                .sql("SELECT id FROM events")
                .await
                .expect("COVM query")
                .collect()
                .await
                .expect("COVM collect")
                .iter()
                .map(|batch| batch.num_rows())
                .sum()
        })
    }
}

struct TempFixtureDir {
    path: PathBuf,
}

impl TempFixtureDir {
    fn new(label: &str) -> Self {
        let id = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cove-datafusion-{label}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create benchmark fixture directory");
        Self { path }
    }
}

impl Drop for TempFixtureDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(feature = "parquet-compare")]
struct ParquetCompareFixture {
    _dir: TempFixtureDir,
    ctx: SessionContext,
    large_events: ComparePaths,
}

#[cfg(feature = "parquet-compare")]
#[derive(Debug, Clone, Copy, Default)]
struct ParquetCompareSessionOptions {
    target_partitions: Option<usize>,
}

#[cfg(feature = "parquet-compare")]
impl ParquetCompareFixture {
    fn new(runtime: &Runtime) -> Self {
        Self::new_with_options(
            runtime,
            CoveTableOptions::default(),
            ParquetCompareSessionOptions::default(),
        )
    }

    fn new_with_cove_options(runtime: &Runtime, cove_options: CoveTableOptions) -> Self {
        Self::new_with_options(
            runtime,
            cove_options,
            ParquetCompareSessionOptions::default(),
        )
    }

    fn new_with_options(
        runtime: &Runtime,
        cove_options: CoveTableOptions,
        session_options: ParquetCompareSessionOptions,
    ) -> Self {
        let dir = TempFixtureDir::new("m6-parquet-compare");

        let events_cove = dir.path.join("events.cove");
        fs::write(&events_cove, primitive_events_file()).expect("write events COVE fixture");
        let events_parquet = dir.path.join("events.parquet");
        write_parquet_file(&events_parquet, &primitive_events_batch());

        let items_cove = dir.path.join("items.cove");
        fs::write(&items_cove, dictionary_items_file_with_domain())
            .expect("write items COVE fixture");
        let items_parquet = dir.path.join("items.parquet");
        write_parquet_file(&items_parquet, &dictionary_items_batch());

        let wide_cove = dir.path.join("wide_events.cove");
        fs::write(&wide_cove, wide_events_file()).expect("write wide COVE fixture");
        let wide_parquet = dir.path.join("wide_events.parquet");
        write_parquet_file(&wide_parquet, &wide_events_batch());

        let large_events = ComparePaths {
            cove: dir.path.join("large_events.cove"),
            parquet: dir.path.join("large_events.parquet"),
        };
        let large_events_filecode_cove = dir.path.join("large_events_filecode.cove");
        fs::write(
            &large_events.cove,
            scan_heavy_events_file(
                PARQUET_COMPARE_SCAN_HEAVY_ROWS,
                PARQUET_COMPARE_SEGMENT_ROWS,
            ),
        )
        .expect("write large events COVE fixture");
        fs::write(
            &large_events_filecode_cove,
            scan_heavy_events_filecode_file(
                PARQUET_COMPARE_SCAN_HEAVY_ROWS,
                PARQUET_COMPARE_SEGMENT_ROWS,
            ),
        )
        .expect("write large FileCode events COVE fixture");
        write_parquet_file(
            &large_events.parquet,
            &scan_heavy_events_batch(PARQUET_COMPARE_SCAN_HEAVY_ROWS),
        );

        let large_wide_events = ComparePaths {
            cove: dir.path.join("large_wide_events.cove"),
            parquet: dir.path.join("large_wide_events.parquet"),
        };
        fs::write(
            &large_wide_events.cove,
            scan_heavy_wide_events_file(
                PARQUET_COMPARE_WIDE_ROWS,
                PARQUET_COMPARE_WIDE_COLUMNS,
                PARQUET_COMPARE_SEGMENT_ROWS,
            ),
        )
        .expect("write large wide COVE fixture");
        write_parquet_file(
            &large_wide_events.parquet,
            &scan_heavy_wide_events_batch(PARQUET_COMPARE_WIDE_ROWS, PARQUET_COMPARE_WIDE_COLUMNS),
        );

        let ctx = session_options
            .target_partitions
            .map(|target_partitions| {
                SessionContext::new_with_config(
                    SessionConfig::new().with_target_partitions(target_partitions),
                )
            })
            .unwrap_or_default();
        register_cove_file_with_options(&ctx, "events_cove", &events_cove, cove_options.clone())
            .expect("register events_cove");
        register_cove_file_with_options(&ctx, "items_cove", &items_cove, cove_options.clone())
            .expect("register items_cove");
        register_cove_file_with_options(&ctx, "wide_events_cove", &wide_cove, cove_options.clone())
            .expect("register wide_events_cove");
        register_cove_file_with_options(
            &ctx,
            "large_events_cove",
            &large_events.cove,
            cove_options.clone(),
        )
        .expect("register large_events_cove");
        register_cove_file_with_options(
            &ctx,
            "large_events_filecode_decoded_cove",
            &large_events_filecode_cove,
            cove_options.clone(),
        )
        .expect("register large_events_filecode_decoded_cove");
        register_cove_file_with_options(
            &ctx,
            "large_events_filecode_cove",
            &large_events_filecode_cove,
            cove_options.clone().with_arrow_dictionary_output(),
        )
        .expect("register large_events_filecode_cove");
        register_cove_file_with_options(
            &ctx,
            "large_wide_events_cove",
            &large_wide_events.cove,
            cove_options,
        )
        .expect("register large_wide_events_cove");

        runtime.block_on(async {
            ctx.register_parquet(
                "events_parquet",
                events_parquet.to_str().expect("events parquet path"),
                ParquetReadOptions::default(),
            )
            .await
            .expect("register events_parquet");
            ctx.register_parquet(
                "items_parquet",
                items_parquet.to_str().expect("items parquet path"),
                ParquetReadOptions::default(),
            )
            .await
            .expect("register items_parquet");
            ctx.register_parquet(
                "wide_events_parquet",
                wide_parquet.to_str().expect("wide parquet path"),
                ParquetReadOptions::default(),
            )
            .await
            .expect("register wide_events_parquet");
            ctx.register_parquet(
                "large_events_parquet",
                large_events
                    .parquet
                    .to_str()
                    .expect("large events parquet path"),
                ParquetReadOptions::default(),
            )
            .await
            .expect("register large_events_parquet");
            ctx.register_parquet(
                "large_wide_events_parquet",
                large_wide_events
                    .parquet
                    .to_str()
                    .expect("large wide parquet path"),
                ParquetReadOptions::default(),
            )
            .await
            .expect("register large_wide_events_parquet");
        });

        Self {
            _dir: dir,
            ctx,
            large_events,
        }
    }

    fn prepare_query(
        &self,
        runtime: &Runtime,
        sql: &str,
        expected_rows: usize,
        expected_columns: usize,
    ) -> PreparedQuery {
        runtime.block_on(async {
            let df = self.ctx.sql(sql).await.expect("build DataFusion SQL plan");
            df.create_physical_plan()
                .await
                .expect("create physical plan");
            PreparedQuery {
                sql: Arc::<str>::from(sql),
                expected_rows,
                expected_columns,
            }
        })
    }
}

#[cfg(feature = "parquet-compare")]
#[derive(Debug, Clone)]
struct ComparePaths {
    cove: PathBuf,
    parquet: PathBuf,
}

#[cfg(feature = "parquet-compare")]
struct PreparedQuery {
    sql: Arc<str>,
    expected_rows: usize,
    expected_columns: usize,
}

#[cfg(feature = "parquet-compare")]
fn execute_prepared_query(
    runtime: &Runtime,
    ctx: &SessionContext,
    query: &PreparedQuery,
) -> (usize, usize) {
    runtime.block_on(async {
        let batches = ctx
            .sql(query.sql.as_ref())
            .await
            .expect("build benchmark query")
            .collect()
            .await
            .expect("execute benchmark query");
        let rows = batches.iter().map(|batch| batch.num_rows()).sum();
        let columns = batches
            .first()
            .map(|batch| batch.num_columns())
            .unwrap_or(0);
        assert_eq!(rows, query.expected_rows);
        assert_eq!(columns, query.expected_columns);
        (rows, columns)
    })
}

#[cfg(feature = "parquet-compare")]
#[derive(Debug, Clone, Copy)]
enum CompareSource {
    Cove,
    Parquet,
}

#[cfg(feature = "parquet-compare")]
#[derive(Debug, Clone)]
struct ColdQuery {
    sql: Arc<str>,
    expected_rows: usize,
    expected_columns: usize,
    source: CompareSource,
    path: PathBuf,
}

#[cfg(feature = "parquet-compare")]
fn execute_cold_query(runtime: &Runtime, query: &ColdQuery) -> (usize, usize) {
    let ctx = SessionContext::new();
    register_compare_source(runtime, &ctx, "events", query.source, &query.path);
    runtime.block_on(async {
        let batches = ctx
            .sql(query.sql.as_ref())
            .await
            .expect("build cold benchmark query")
            .collect()
            .await
            .expect("execute cold benchmark query");
        let rows = batches.iter().map(|batch| batch.num_rows()).sum();
        let columns = batches
            .first()
            .map(|batch| batch.num_columns())
            .unwrap_or(0);
        assert_eq!(rows, query.expected_rows);
        assert_eq!(columns, query.expected_columns);
        (rows, columns)
    })
}

#[cfg(feature = "parquet-compare")]
fn register_compare_source(
    runtime: &Runtime,
    ctx: &SessionContext,
    table_name: &str,
    source: CompareSource,
    path: &Path,
) {
    match source {
        CompareSource::Cove => {
            register_cove_file(ctx, table_name, path).expect("register cold COVE source");
        }
        CompareSource::Parquet => runtime.block_on(async {
            ctx.register_parquet(
                table_name,
                path.to_str().expect("cold parquet path"),
                ParquetReadOptions::default(),
            )
            .await
            .expect("register cold parquet source");
        }),
    }
}

fn identity_for_state(state: &DatasetState) -> OverlayFileIdentity {
    OverlayFileIdentity {
        file_id: *state.file_id(),
        file_len: state.file_len(),
        footer_crc32c: state.footer_crc32c(),
        digest: None,
    }
}

#[cfg(feature = "covm")]
fn write_covm_manifest(path: &Path, files: Vec<CovmFileEntryV1>) {
    let manifest = CovmFile {
        header: CovmHeaderV1::new([0xC0; 16], 1, files.len() as u32, 0),
        files,
        postscript: CovmPostscriptV1 {
            header_offset: 0,
            header_len: 0,
            entries_offset: 0,
            entries_len: 0,
            file_len: 0,
            flags: 0,
            checksum: 0,
        },
    };
    fs::write(path, manifest.serialize().expect("serialize COVM manifest"))
        .expect("write COVM manifest");
}

#[cfg(feature = "covm")]
fn covm_entry_for_path(uri: &str, path: &Path) -> CovmFileEntryV1 {
    let state = bootstrap_local_file(path).expect("bootstrap COVM member");
    CovmFileEntryV1 {
        file_id: *state.file_id(),
        uri: uri.to_string(),
        file_len: state.file_len(),
        footer_crc32c: state.footer_crc32c(),
        digest_algorithm: DigestAlgorithm::None as u16,
        digest: Vec::new(),
        row_count: state.table().row_count,
        segment_count: state.segments().len() as u32,
        file_stats_ref: u32::MAX,
        file_exact_set_ref: u32::MAX,
        flags: 0,
    }
}

fn primitive_events_file() -> Vec<u8> {
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
                column(
                    1,
                    "id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    2,
                    "name",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
                column(
                    3,
                    "active",
                    CoveLogicalType::Bool,
                    CovePhysicalKind::Boolean,
                    false,
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
    writer.write().expect("write primitive fixture")
}

fn dictionary_items_file_with_domain() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 7,
            namespace: "public".into(),
            name: "items".into(),
            row_count: 2,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "name",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::FileCode,
                    false,
                ),
                column(
                    2,
                    "payload",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
            ],
        }],
    };
    let mut first = ScanSegment::new(7, 0, 0, 1, 2);
    first.set_column_pages(1, vec![filecode_page(1, filecodes(&[0]))]);
    first.set_column_pages(2, vec![varbytes_page(1, varbytes(&["first"]))]);
    let mut second = ScanSegment::new(7, 1, 1, 1, 2);
    second.set_column_pages(1, vec![filecode_page(1, filecodes(&[1]))]);
    second.set_column_pages(2, vec![varbytes_page(1, varbytes(&["second"]))]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&sample_dictionary());
    writer.push_extra_section(column_domain_section());
    writer.push_extra_section(filecode_zone_stats_section());
    writer.push_segment(first);
    writer.push_segment(second);
    writer.write().expect("write FileCode fixture")
}

fn numeric_lookup_events_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 8,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 3,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    2,
                    "payload",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
            ],
        }],
    };
    let mut segment = ScanSegment::new(8, 0, 0, 3, 2);
    segment.set_column_pages(1, vec![numcode_page(3, numcode_i64(&[1, 2, 3]))]);
    segment.set_column_pages(
        2,
        vec![varbytes_page(3, varbytes(&["alpha", "beta", "gamma"]))],
    );

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_extra_section(numcode_lookup_index_section());
    writer.push_segment(segment);
    writer.write().expect("write lookup fixture")
}

fn wide_events_file() -> Vec<u8> {
    let mut columns = vec![column(
        1,
        "id",
        CoveLogicalType::Int64,
        CovePhysicalKind::NumCode,
        false,
    )];
    for index in 1..=12 {
        columns.push(column(
            index + 1,
            &format!("payload_{index}"),
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            false,
        ));
    }

    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 9,
            namespace: "public".into(),
            name: "wide_events".into(),
            row_count: 8,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns,
        }],
    };
    let mut segment = ScanSegment::new(9, 0, 0, 8, 13);
    segment.set_column_pages(
        1,
        vec![numcode_page(8, numcode_i64(&[1, 2, 3, 4, 5, 6, 7, 8]))],
    );
    for index in 1..=12 {
        let values = [
            format!("c{index}_1"),
            format!("c{index}_2"),
            format!("c{index}_3"),
            format!("c{index}_4"),
            format!("c{index}_5"),
            format!("c{index}_6"),
            format!("c{index}_7"),
            format!("c{index}_8"),
        ];
        let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
        segment.set_column_pages(index + 1, vec![varbytes_page(8, varbytes(&refs))]);
    }

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_segment(segment);
    writer.write().expect("write wide fixture")
}

#[cfg(feature = "parquet-compare")]
fn scan_heavy_events_file(row_count: usize, segment_rows: usize) -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 21,
            namespace: "public".into(),
            name: "large_events".into(),
            row_count: row_count as u64,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    2,
                    "category",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
                column(
                    3,
                    "payload",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
                column(
                    4,
                    "active",
                    CoveLogicalType::Bool,
                    CovePhysicalKind::Boolean,
                    false,
                ),
            ],
        }],
    };
    let ids = (1..=row_count).map(|id| id as i64).collect::<Vec<_>>();
    let categories = (0..row_count)
        .map(|idx| format!("group_{:02}", idx % 8))
        .collect::<Vec<_>>();
    let payloads = (0..row_count)
        .map(|idx| format!("payload_{:05}", idx % 2048))
        .collect::<Vec<_>>();
    let actives = (0..row_count).map(|idx| idx % 3 != 0).collect::<Vec<_>>();

    let mut writer = ScanProfileCoveWriter::new(catalog);
    for (segment_idx, row_start) in (0..row_count).step_by(segment_rows).enumerate() {
        let row_end = row_count.min(row_start + segment_rows);
        let segment_len = (row_end - row_start) as u32;
        let mut segment =
            ScanSegment::new(21, segment_idx as u32, row_start as u64, segment_len, 4);
        segment.set_column_pages(
            1,
            vec![numcode_page(
                segment_len,
                numcode_i64(&ids[row_start..row_end]),
            )],
        );
        let category_refs = categories[row_start..row_end]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        segment.set_column_pages(
            2,
            vec![varbytes_page(segment_len, varbytes(&category_refs))],
        );
        let payload_refs = payloads[row_start..row_end]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        segment.set_column_pages(3, vec![varbytes_page(segment_len, varbytes(&payload_refs))]);
        segment.set_column_pages(
            4,
            vec![bool_page(segment_len, bools(&actives[row_start..row_end]))],
        );
        writer.push_segment(segment);
    }
    writer.write().expect("write scan-heavy events fixture")
}

#[cfg(feature = "parquet-compare")]
fn scan_heavy_events_filecode_file(row_count: usize, segment_rows: usize) -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 21,
            namespace: "public".into(),
            name: "large_events".into(),
            row_count: row_count as u64,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    2,
                    "category",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::FileCode,
                    false,
                ),
                column(
                    3,
                    "payload",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::FileCode,
                    false,
                ),
                column(
                    4,
                    "active",
                    CoveLogicalType::Bool,
                    CovePhysicalKind::Boolean,
                    false,
                ),
            ],
        }],
    };
    let ids = (1..=row_count).map(|id| id as i64).collect::<Vec<_>>();
    let categories = (0..row_count)
        .map(|idx| format!("group_{:02}", idx % 8))
        .collect::<Vec<_>>();
    let payloads = (0..row_count)
        .map(|idx| format!("payload_{:05}", idx % 2048))
        .collect::<Vec<_>>();
    let actives = (0..row_count).map(|idx| idx % 3 != 0).collect::<Vec<_>>();
    let dictionary =
        FileDictionaryEncoding::from_keys(categories.iter().chain(payloads.iter()).map(|value| {
            FileDictionaryKey::from_logical_bytes(CoveLogicalType::Utf8, value.as_bytes())
                .expect("scan-heavy dictionary key")
        }))
        .expect("scan-heavy FileCode dictionary");

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&dictionary.dictionary);
    for (segment_idx, row_start) in (0..row_count).step_by(segment_rows).enumerate() {
        let row_end = row_count.min(row_start + segment_rows);
        let segment_len = (row_end - row_start) as u32;
        let mut segment =
            ScanSegment::new(21, segment_idx as u32, row_start as u64, segment_len, 4);
        segment.set_column_pages(
            1,
            vec![numcode_page(
                segment_len,
                numcode_i64(&ids[row_start..row_end]),
            )],
        );
        let category_codes = categories[row_start..row_end]
            .iter()
            .map(|value| {
                dictionary
                    .file_code_for_logical_bytes(CoveLogicalType::Utf8, value.as_bytes())
                    .expect("scan-heavy category FileCode")
            })
            .collect::<Vec<_>>();
        segment.set_column_pages(
            2,
            vec![filecode_page(segment_len, filecodes(&category_codes))],
        );
        let payload_codes = payloads[row_start..row_end]
            .iter()
            .map(|value| {
                dictionary
                    .file_code_for_logical_bytes(CoveLogicalType::Utf8, value.as_bytes())
                    .expect("scan-heavy payload FileCode")
            })
            .collect::<Vec<_>>();
        segment.set_column_pages(
            3,
            vec![filecode_page(segment_len, filecodes(&payload_codes))],
        );
        segment.set_column_pages(
            4,
            vec![bool_page(segment_len, bools(&actives[row_start..row_end]))],
        );
        writer.push_segment(segment);
    }
    writer
        .write()
        .expect("write scan-heavy FileCode events fixture")
}

#[cfg(feature = "parquet-compare")]
fn scan_heavy_wide_events_file(
    row_count: usize,
    payload_columns: usize,
    segment_rows: usize,
) -> Vec<u8> {
    let mut columns = vec![column(
        1,
        "id",
        CoveLogicalType::Int64,
        CovePhysicalKind::NumCode,
        false,
    )];
    for index in 1..=payload_columns {
        columns.push(column(
            (index + 1) as u32,
            &format!("payload_{index}"),
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            false,
        ));
    }

    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 22,
            namespace: "public".into(),
            name: "large_wide_events".into(),
            row_count: row_count as u64,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns,
        }],
    };
    let ids = (1..=row_count).map(|id| id as i64).collect::<Vec<_>>();

    let mut writer = ScanProfileCoveWriter::new(catalog);
    for (segment_idx, row_start) in (0..row_count).step_by(segment_rows).enumerate() {
        let row_end = row_count.min(row_start + segment_rows);
        let segment_len = (row_end - row_start) as u32;
        let mut segment = ScanSegment::new(
            22,
            segment_idx as u32,
            row_start as u64,
            segment_len,
            (payload_columns + 1) as u32,
        );
        segment.set_column_pages(
            1,
            vec![numcode_page(
                segment_len,
                numcode_i64(&ids[row_start..row_end]),
            )],
        );
        for index in 1..=payload_columns {
            let values = (row_start..row_end)
                .map(|row| format!("w{index}_{:05}", row % 4096))
                .collect::<Vec<_>>();
            let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
            segment.set_column_pages(
                (index + 1) as u32,
                vec![varbytes_page(segment_len, varbytes(&refs))],
            );
        }
        writer.push_segment(segment);
    }
    writer.write().expect("write scan-heavy wide fixture")
}

#[cfg(feature = "parquet-compare")]
fn scan_heavy_range_start(row_count: usize) -> i64 {
    (row_count * 3 / 4 + 1) as i64
}

#[cfg(feature = "parquet-compare")]
fn primitive_events_batch() -> RecordBatch {
    RecordBatch::try_from_iter(vec![
        ("id", Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef),
        (
            "name",
            Arc::new(StringArray::from(vec!["alpha", "beta", "gamma"])) as ArrayRef,
        ),
        (
            "active",
            Arc::new(BooleanArray::from(vec![true, false, true])) as ArrayRef,
        ),
    ])
    .expect("primitive events record batch")
}

#[cfg(feature = "parquet-compare")]
fn dictionary_items_batch() -> RecordBatch {
    RecordBatch::try_from_iter(vec![
        (
            "name",
            Arc::new(StringArray::from(vec!["red", "blue"])) as ArrayRef,
        ),
        (
            "payload",
            Arc::new(StringArray::from(vec!["first", "second"])) as ArrayRef,
        ),
    ])
    .expect("dictionary items record batch")
}

#[cfg(feature = "parquet-compare")]
fn wide_events_batch() -> RecordBatch {
    let mut columns = vec![(
        "id".to_string(),
        Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5, 6, 7, 8])) as ArrayRef,
    )];
    for index in 1..=12 {
        let values = (1..=8)
            .map(|row| format!("c{index}_{row}"))
            .collect::<Vec<_>>();
        columns.push((
            format!("payload_{index}"),
            Arc::new(StringArray::from(values)) as ArrayRef,
        ));
    }
    RecordBatch::try_from_iter(columns).expect("wide events record batch")
}

#[cfg(feature = "parquet-compare")]
fn scan_heavy_events_batch(row_count: usize) -> RecordBatch {
    let ids = (1..=row_count).map(|id| id as i64).collect::<Vec<_>>();
    let categories = (0..row_count)
        .map(|idx| format!("group_{:02}", idx % 8))
        .collect::<Vec<_>>();
    let payloads = (0..row_count)
        .map(|idx| format!("payload_{:05}", idx % 2048))
        .collect::<Vec<_>>();
    let actives = (0..row_count).map(|idx| idx % 3 != 0).collect::<Vec<_>>();

    RecordBatch::try_from_iter(vec![
        ("id", Arc::new(Int64Array::from(ids)) as ArrayRef),
        (
            "category",
            Arc::new(StringArray::from(categories)) as ArrayRef,
        ),
        ("payload", Arc::new(StringArray::from(payloads)) as ArrayRef),
        ("active", Arc::new(BooleanArray::from(actives)) as ArrayRef),
    ])
    .expect("scan-heavy events record batch")
}

#[cfg(feature = "parquet-compare")]
fn scan_heavy_wide_events_batch(row_count: usize, payload_columns: usize) -> RecordBatch {
    let mut columns = vec![(
        "id".to_string(),
        Arc::new(Int64Array::from(
            (1..=row_count).map(|id| id as i64).collect::<Vec<_>>(),
        )) as ArrayRef,
    )];
    for index in 1..=payload_columns {
        let values = (0..row_count)
            .map(|row| format!("w{index}_{:05}", row % 4096))
            .collect::<Vec<_>>();
        columns.push((
            format!("payload_{index}"),
            Arc::new(StringArray::from(values)) as ArrayRef,
        ));
    }
    RecordBatch::try_from_iter(columns).expect("scan-heavy wide events record batch")
}

#[cfg(feature = "parquet-compare")]
fn write_parquet_file(path: &Path, batch: &RecordBatch) {
    let file = std::fs::File::create(path).expect("create parquet fixture");
    let properties = WriterProperties::builder()
        .set_max_row_group_row_count(Some(batch.num_rows()))
        .build();
    let mut writer =
        ArrowWriter::try_new(file, batch.schema(), Some(properties)).expect("open parquet writer");
    writer.write(batch).expect("write parquet fixture batch");
    writer.close().expect("close parquet writer");
}

#[cfg(feature = "parquet-compare")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryProfileStage {
    FullQuery,
    PlanningOnly,
    ExecuteOnly,
}

#[cfg(feature = "parquet-compare")]
impl QueryProfileStage {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "full-query" => Some(Self::FullQuery),
            "planning-only" => Some(Self::PlanningOnly),
            "execute-only" => Some(Self::ExecuteOnly),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::FullQuery => "full-query",
            Self::PlanningOnly => "planning-only",
            Self::ExecuteOnly => "execute-only",
        }
    }
}

#[cfg(feature = "parquet-compare")]
#[derive(Debug)]
struct QueryProfileCommand {
    track: String,
    engine: CompareSource,
    stage: QueryProfileStage,
    run_seconds: u64,
    worker_threads: Option<usize>,
    target_partitions: Option<usize>,
    cove_target_morsels_per_partition: Option<usize>,
    cove_arrow_view_output: bool,
    cove_trusted_arrow_string_validation: bool,
    cove_local_file_mmap_reads: bool,
}

#[cfg(feature = "parquet-compare")]
fn maybe_run_query_profile_mode() -> Option<i32> {
    let mut args = env::args().skip(1);
    if args.next().as_deref() != Some("profile-query") {
        return None;
    }
    match parse_query_profile_command(args) {
        Ok(command) => {
            run_query_profile_command(command);
            Some(0)
        }
        Err(message) => {
            eprintln!("{message}");
            Some(2)
        }
    }
}

#[cfg(feature = "parquet-compare")]
fn parse_query_profile_command(
    args: impl Iterator<Item = String>,
) -> Result<QueryProfileCommand, String> {
    let mut track = None;
    let mut engine = None;
    let mut stage = QueryProfileStage::ExecuteOnly;
    let mut run_seconds = None;
    let mut worker_threads = None;
    let mut target_partitions = None;
    let mut cove_target_morsels_per_partition = None;
    let mut cove_arrow_view_output = false;
    let mut cove_trusted_arrow_string_validation = false;
    let mut cove_local_file_mmap_reads = false;

    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--track" => {
                track = Some(next_profile_arg(&mut args, "--track")?);
            }
            "--engine" => {
                let value = next_profile_arg(&mut args, "--engine")?;
                engine = Some(parse_compare_source(&value)?);
            }
            "--stage" => {
                let value = next_profile_arg(&mut args, "--stage")?;
                stage = QueryProfileStage::parse(&value).ok_or_else(|| {
                    format!(
                        "unsupported --stage value '{value}'; expected one of: full-query, planning-only, execute-only"
                    )
                })?;
            }
            "--run-seconds" => {
                let value = next_profile_arg(&mut args, "--run-seconds")?;
                run_seconds =
                    Some(value.parse::<u64>().map_err(|error| {
                        format!("invalid --run-seconds value '{value}': {error}")
                    })?);
            }
            "--worker-threads" => {
                let value = next_profile_arg(&mut args, "--worker-threads")?;
                worker_threads = Some(parse_positive_usize_flag("--worker-threads", &value)?);
            }
            "--target-partitions" => {
                let value = next_profile_arg(&mut args, "--target-partitions")?;
                target_partitions = Some(parse_positive_usize_flag("--target-partitions", &value)?);
            }
            "--cove-target-morsels-per-partition" => {
                let value = next_profile_arg(&mut args, "--cove-target-morsels-per-partition")?;
                cove_target_morsels_per_partition = Some(parse_positive_usize_flag(
                    "--cove-target-morsels-per-partition",
                    &value,
                )?);
            }
            "--cove-arrow-view-output" => {
                cove_arrow_view_output = true;
            }
            "--cove-trusted-arrow-string-validation" => {
                cove_trusted_arrow_string_validation = true;
            }
            "--cove-local-file-mmap-reads" => {
                cove_local_file_mmap_reads = true;
            }
            "--help" | "-h" => {
                return Err(query_profile_usage());
            }
            other => {
                return Err(format!(
                    "unrecognized profile-query argument '{other}'\n\n{}",
                    query_profile_usage()
                ));
            }
        }
    }

    Ok(QueryProfileCommand {
        track: track.ok_or_else(query_profile_usage)?,
        engine: engine.ok_or_else(query_profile_usage)?,
        stage,
        run_seconds: run_seconds.unwrap_or(20),
        worker_threads,
        target_partitions,
        cove_target_morsels_per_partition,
        cove_arrow_view_output,
        cove_trusted_arrow_string_validation,
        cove_local_file_mmap_reads,
    })
}

#[cfg(feature = "parquet-compare")]
fn parse_positive_usize_flag(flag: &str, value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|error| format!("invalid {flag} value '{value}': {error}"))?;
    if parsed == 0 {
        return Err(format!(
            "invalid {flag} value '{value}': expected a positive integer"
        ));
    }
    Ok(parsed)
}

#[cfg(feature = "parquet-compare")]
fn next_profile_arg(
    args: &mut std::iter::Peekable<impl Iterator<Item = String>>,
    flag: &str,
) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value for {flag}\n\n{}", query_profile_usage()))
}

#[cfg(feature = "parquet-compare")]
fn query_profile_usage() -> String {
    "usage: m6 profile-query --track <track> --engine <cove|parquet> [--stage <full-query|planning-only|execute-only>] [--run-seconds <seconds>] [--worker-threads <n>] [--target-partitions <n>] [--cove-target-morsels-per-partition <n>] [--cove-arrow-view-output] [--cove-trusted-arrow-string-validation] [--cove-local-file-mmap-reads]"
        .into()
}

#[cfg(feature = "parquet-compare")]
fn parse_compare_source(value: &str) -> Result<CompareSource, String> {
    match value {
        "cove" => Ok(CompareSource::Cove),
        "parquet" => Ok(CompareSource::Parquet),
        _ => Err(format!(
            "unsupported --engine value '{value}'; expected 'cove' or 'parquet'"
        )),
    }
}

#[cfg(feature = "parquet-compare")]
fn run_query_profile_command(command: QueryProfileCommand) {
    let runtime = match command.worker_threads {
        Some(worker_threads) => RuntimeBuilder::new_multi_thread()
            .worker_threads(worker_threads)
            .enable_all()
            .build()
            .expect("query profile runtime"),
        None => Runtime::new().expect("query profile runtime"),
    };
    let mut cove_options = CoveTableOptions::default();
    if let Some(target) = command.cove_target_morsels_per_partition {
        cove_options = cove_options.with_target_morsels_per_partition(target);
    }
    if command.cove_arrow_view_output {
        cove_options = cove_options.with_arrow_view_output();
    }
    if command.cove_trusted_arrow_string_validation {
        cove_options = cove_options.with_trusted_arrow_string_validation();
    }
    if command.cove_local_file_mmap_reads {
        cove_options = cove_options.with_local_file_mmap_reads();
    }
    let target_partitions = command.target_partitions.or(command.worker_threads);
    let fixture = ParquetCompareFixture::new_with_options(
        &runtime,
        cove_options,
        ParquetCompareSessionOptions { target_partitions },
    );
    let query = profile_query_spec(&command.track, command.engine).unwrap_or_else(|message| {
        panic!("{message}");
    });

    let plan = match command.stage {
        QueryProfileStage::ExecuteOnly => Some(build_physical_plan(
            &runtime,
            &fixture.ctx,
            query.sql.as_ref(),
        )),
        _ => None,
    };

    println!(
        "PROFILE_READY pid={} stage={} track={} engine={} worker_threads={} target_partitions={} cove_target_morsels_per_partition={} cove_arrow_view_output={} cove_trusted_arrow_string_validation={} cove_local_file_mmap_reads={}",
        process::id(),
        command.stage.as_str(),
        command.track,
        compare_source_name(command.engine),
        format_optional_usize(command.worker_threads),
        format_optional_usize(target_partitions),
        format_optional_usize(command.cove_target_morsels_per_partition),
        command.cove_arrow_view_output,
        command.cove_trusted_arrow_string_validation,
        command.cove_local_file_mmap_reads,
    );
    io::stdout().flush().expect("flush profile ready line");
    wait_for_profile_start_signal();

    let (iterations, rows_materialized) = match command.stage {
        QueryProfileStage::FullQuery => {
            let iterations =
                run_full_query_profile_loop(&runtime, &fixture.ctx, &query, command.run_seconds);
            (iterations, None)
        }
        QueryProfileStage::PlanningOnly => {
            let iterations =
                run_planning_profile_loop(&runtime, &fixture.ctx, &query, command.run_seconds);
            (iterations, None)
        }
        QueryProfileStage::ExecuteOnly => {
            let (iterations, rows_materialized) = run_execute_only_profile_loop(
                &runtime,
                &fixture.ctx,
                &query,
                plan.expect("execute-only plan should be prepared"),
                command.run_seconds,
            );
            (iterations, Some(rows_materialized))
        }
    };

    println!(
        "PROFILE_DONE pid={} iterations={} stage={} cove_rows_materialized_per_iteration={}",
        process::id(),
        iterations,
        command.stage.as_str(),
        format_optional_usize(rows_materialized),
    );
}

#[cfg(feature = "parquet-compare")]
fn format_optional_usize(value: Option<usize>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "default".into())
}

#[cfg(feature = "parquet-compare")]
fn wait_for_profile_start_signal() {
    let mut line = String::new();
    let _ = io::stdin()
        .lock()
        .read_line(&mut line)
        .expect("read profile start signal");
}

#[cfg(feature = "parquet-compare")]
fn run_full_query_profile_loop(
    runtime: &Runtime,
    ctx: &SessionContext,
    query: &PreparedQuery,
    run_seconds: u64,
) -> usize {
    let deadline = Instant::now() + Duration::from_secs(run_seconds);
    let mut iterations = 0usize;
    while Instant::now() < deadline {
        black_box(execute_prepared_query(runtime, ctx, query));
        iterations += 1;
    }
    iterations
}

#[cfg(feature = "parquet-compare")]
fn run_planning_profile_loop(
    runtime: &Runtime,
    ctx: &SessionContext,
    query: &PreparedQuery,
    run_seconds: u64,
) -> usize {
    let deadline = Instant::now() + Duration::from_secs(run_seconds);
    let mut iterations = 0usize;
    while Instant::now() < deadline {
        let plan = build_physical_plan(runtime, ctx, query.sql.as_ref());
        black_box(plan);
        iterations += 1;
    }
    iterations
}

#[cfg(feature = "parquet-compare")]
fn run_execute_only_profile_loop(
    runtime: &Runtime,
    ctx: &SessionContext,
    query: &PreparedQuery,
    plan: Arc<dyn datafusion::physical_plan::ExecutionPlan>,
    run_seconds: u64,
) -> (usize, usize) {
    let deadline = Instant::now() + Duration::from_secs(run_seconds);
    let mut iterations = 0usize;
    while Instant::now() < deadline {
        let plan = reset_plan_states(Arc::clone(&plan)).expect("reset execution plan state");
        let batches = runtime.block_on(async {
            collect_execution_plan(plan, ctx.task_ctx())
                .await
                .expect("execute prepared physical plan")
        });
        black_box(assert_query_batches(&batches, query));
        iterations += 1;
    }
    let metric_plan = build_physical_plan(runtime, ctx, query.sql.as_ref());
    let collected_metric_plan = Arc::clone(&metric_plan);
    let batches = runtime.block_on(async {
        collect_execution_plan(metric_plan, ctx.task_ctx())
            .await
            .expect("execute prepared physical plan for metrics")
    });
    black_box(assert_query_batches(&batches, query));
    (
        iterations,
        execution_plan_metric_sum(&collected_metric_plan, "cove_rows_materialized"),
    )
}

#[cfg(feature = "parquet-compare")]
fn execution_plan_metric_sum(
    plan: &Arc<dyn datafusion::physical_plan::ExecutionPlan>,
    metric_name: &str,
) -> usize {
    let own = plan
        .metrics()
        .and_then(|metrics| metrics.sum_by_name(metric_name))
        .map(|metric| metric.as_usize())
        .unwrap_or(0);
    own + plan
        .children()
        .into_iter()
        .map(|child| execution_plan_metric_sum(child, metric_name))
        .sum::<usize>()
}

#[cfg(feature = "parquet-compare")]
fn build_physical_plan(
    runtime: &Runtime,
    ctx: &SessionContext,
    sql: &str,
) -> Arc<dyn datafusion::physical_plan::ExecutionPlan> {
    runtime.block_on(async {
        ctx.sql(sql)
            .await
            .expect("build profiling SQL query")
            .create_physical_plan()
            .await
            .expect("create profiling physical plan")
    })
}

#[cfg(feature = "parquet-compare")]
fn assert_query_batches(batches: &[RecordBatch], query: &PreparedQuery) -> (usize, usize) {
    let rows = batches.iter().map(|batch| batch.num_rows()).sum();
    let columns = batches
        .first()
        .map(|batch| batch.num_columns())
        .unwrap_or(0);
    assert_eq!(rows, query.expected_rows);
    assert_eq!(columns, query.expected_columns);
    (rows, columns)
}

#[cfg(feature = "parquet-compare")]
fn profile_query_spec(track: &str, engine: CompareSource) -> Result<PreparedQuery, String> {
    let suffix = compare_source_name(engine);
    let sql = match track {
        "tiny-full-scan" => format!("SELECT * FROM events_{suffix}"),
        "tiny-projection-scan" => format!("SELECT name FROM events_{suffix}"),
        "tiny-low-cardinality-filter" => {
            format!("SELECT payload FROM items_{suffix} WHERE name = 'red'")
        }
        "tiny-numeric-range-filter" => format!("SELECT name FROM events_{suffix} WHERE id >= 2"),
        "tiny-wide-projection-filter" => {
            format!("SELECT payload_12 FROM wide_events_{suffix} WHERE id > 6")
        }
        "scan-heavy-full-scan" => format!("SELECT * FROM large_events_{suffix}"),
        "scan-heavy-projection-scan" => format!("SELECT payload FROM large_events_{suffix}"),
        "scan-heavy-low-cardinality-filter" => {
            format!("SELECT payload FROM large_events_{suffix} WHERE category = 'group_03'")
        }
        "scan-heavy-numeric-range-filter" => format!(
            "SELECT payload FROM large_events_{suffix} WHERE id >= {}",
            scan_heavy_range_start(PARQUET_COMPARE_SCAN_HEAVY_ROWS)
        ),
        "scan-heavy-wide-projection-filter" => format!(
            "SELECT payload_12 FROM large_wide_events_{suffix} WHERE id > {}",
            PARQUET_COMPARE_WIDE_ROWS * 3 / 4
        ),
        "cold-context-full-scan" | "cold-context-numeric-range-filter" => {
            return Err(format!(
                "track '{track}' is not supported in profile-query mode because it intentionally measures per-iteration setup; use the criterion runner for that track"
            ));
        }
        other => {
            return Err(format!(
                "unsupported profile-query track '{other}'; expected one of: tiny-full-scan, tiny-projection-scan, tiny-low-cardinality-filter, tiny-numeric-range-filter, tiny-wide-projection-filter, scan-heavy-full-scan, scan-heavy-projection-scan, scan-heavy-low-cardinality-filter, scan-heavy-numeric-range-filter, scan-heavy-wide-projection-filter"
            ));
        }
    };

    let (expected_rows, expected_columns) = match track {
        "tiny-full-scan" => (3, 3),
        "tiny-projection-scan" => (3, 1),
        "tiny-low-cardinality-filter" => (1, 1),
        "tiny-numeric-range-filter" => (2, 1),
        "tiny-wide-projection-filter" => (2, 1),
        "scan-heavy-full-scan" => (PARQUET_COMPARE_SCAN_HEAVY_ROWS, 4),
        "scan-heavy-projection-scan" => (PARQUET_COMPARE_SCAN_HEAVY_ROWS, 1),
        "scan-heavy-low-cardinality-filter" => (PARQUET_COMPARE_SCAN_HEAVY_ROWS / 8, 1),
        "scan-heavy-numeric-range-filter" => (
            PARQUET_COMPARE_SCAN_HEAVY_ROWS - PARQUET_COMPARE_SCAN_HEAVY_ROWS * 3 / 4,
            1,
        ),
        "scan-heavy-wide-projection-filter" => (
            PARQUET_COMPARE_WIDE_ROWS - PARQUET_COMPARE_WIDE_ROWS * 3 / 4,
            1,
        ),
        _ => unreachable!("track should be validated above"),
    };

    Ok(PreparedQuery {
        sql: Arc::<str>::from(sql),
        expected_rows,
        expected_columns,
    })
}

#[cfg(feature = "parquet-compare")]
fn compare_source_name(source: CompareSource) -> &'static str {
    match source {
        CompareSource::Cove => "cove",
        CompareSource::Parquet => "parquet",
    }
}

fn topn_events_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 19,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 2,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![column(
                1,
                "id",
                CoveLogicalType::Int64,
                CovePhysicalKind::NumCode,
                false,
            )],
        }],
    };
    let mut low = ScanSegment::new(19, 0, 0, 1, 1);
    low.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[1]))]);
    let mut high = ScanSegment::new(19, 1, 1, 1, 1);
    high.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[9]))]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_topn_summary(&topn_summary(19, 1, 1, 0, TopNDirection::Largest, 9));
    writer.push_segment(low);
    writer.push_segment(high);
    writer.write().expect("write TopN fixture")
}

fn column(
    column_id: u32,
    name: &str,
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
    nullable: bool,
) -> ColumnEntry {
    ColumnEntry {
        column_id,
        name: name.into(),
        logical,
        physical,
        nullable,
        sort_order: 0,
        collation_id: 0,
        precision: 0,
        scale: 0,
        flags: 0,
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

fn filecode_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::FileCode as u32)
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

fn filecodes(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn sample_dictionary() -> FileDictionary {
    FileDictionary {
        header: FileDictionaryHeaderV1 {
            entry_count: 2,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        },
        entries: vec![inline_utf8_entry("red"), inline_utf8_entry("blue")],
        payload: Vec::new(),
    }
}

fn inline_utf8_entry(value: &str) -> FileDictionaryIndexEntryV1 {
    let canonical = canonical_utf8(value);
    let mut inline_data = [0u8; 16];
    inline_data[..canonical.len()].copy_from_slice(&canonical);
    FileDictionaryIndexEntryV1 {
        value_tag: ValueTag::Utf8 as u16,
        storage_class: StorageClass::Inline as u8,
        flags: 0,
        inline_len: canonical.len() as u8,
        reserved0: [0; 3],
        inline_data,
        payload_offset: 0,
        payload_length: 0,
        canonical_hash64: 0,
        reserved1: 0,
    }
}

fn canonical_utf8(value: &str) -> Vec<u8> {
    let mut canonical = wire::encode_u64_leb128(value.len() as u64);
    canonical.extend_from_slice(value.as_bytes());
    canonical
}

fn column_domain_section() -> SectionPayload {
    let domain = ColumnDomain::from_sorted_present_codes(
        &[0, 1],
        2,
        7,
        1,
        CoveLogicalType::Utf8 as u16,
        0,
        0,
    )
    .expect("column domain");
    SectionPayload {
        section_kind: SectionKind::ColumnDomain as u16,
        profile: PrimaryProfile::TableScan as u8,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: domain.serialize().expect("serialize column domain"),
    }
}

fn filecode_zone_stats_section() -> SectionPayload {
    let entries = vec![
        filecode_zone_stats_entry(0, 0),
        filecode_zone_stats_entry(1, 1),
    ];
    let section = ZoneStatsSection { entries };
    SectionPayload {
        section_kind: SectionKind::ZoneStats as u16,
        profile: PrimaryProfile::TableScan as u8,
        flags: 0,
        item_count: 2,
        row_count: 2,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: section.serialize().expect("serialize zone stats"),
    }
}

fn filecode_zone_stats_entry(segment_id: u32, rank: u32) -> ZoneStatsEntry {
    ZoneStatsEntry {
        table_id: 7,
        segment_id,
        morsel_id: 0,
        column_id: 1,
        non_null_count: 1,
        distinct_count: 1,
        run_count: 1,
        stats: ZoneStats {
            scope: cove_core::zone_stats::ZoneScope::Morsel,
            row_count: 1,
            null_count: 0,
            min: None,
            max: None,
            flags: ZoneStatFlags::HAS_DOMAIN_RANGE | ZoneStatFlags::CONSTANT,
        },
        min_domain_rank: rank,
        max_domain_rank: rank,
        exact_set_ref: u32::MAX,
        bloom_ref: u32::MAX,
    }
}

fn numcode_lookup_index_section() -> SectionPayload {
    let index = LookupIndex {
        header: LookupIndexHeaderV1 {
            table_id: 8,
            column_id: 1,
            key_kind: LookupKeyKind::NumCode,
            index_kind: LookupIndexKind::SparseSorted,
            uniqueness: LookupUniqueness::NonUnique,
            flags: 0,
            entry_count: 0,
            entries_offset: 0,
            entries_length: 0,
            rowref_offset: 0,
            rowref_length: 0,
            checksum: 0,
        },
        entries: vec![LookupEntry {
            key: 2,
            rows: vec![RowRef {
                table_id: 8,
                segment_id: 0,
                morsel_id: 0,
                row_in_morsel: 1,
            }],
        }],
    };
    SectionPayload {
        section_kind: SectionKind::LookupIndex as u16,
        profile: PrimaryProfile::ArchiveAcceleration as u8,
        flags: 0,
        item_count: 1,
        row_count: 3,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: index.serialize().expect("serialize lookup index"),
    }
}

fn topn_summary(
    table_id: u32,
    column_id: u32,
    segment_id: u32,
    morsel_id: u32,
    direction: TopNDirection,
    value: u64,
) -> TopNSummary {
    let mut payload = Vec::new();
    payload.extend_from_slice(&value.to_le_bytes());
    payload.extend_from_slice(&1u64.to_le_bytes());
    TopNSummary {
        table_id,
        column_id,
        segment_id,
        morsel_id,
        direction,
        value_count: 1,
        flags: 0,
        payload_offset: TOPN_ZONE_SUMMARY_LEN as u64,
        payload_length: payload.len() as u64,
        checksum: 0,
        payload,
    }
}

#[cfg(feature = "parquet-compare")]
criterion_group!(benches, bench_m6_native, bench_m6_parquet_compare);
#[cfg(not(feature = "parquet-compare"))]
criterion_group!(benches, bench_m6_native);

fn main() {
    #[cfg(feature = "parquet-compare")]
    if let Some(code) = maybe_run_query_profile_mode() {
        std::process::exit(code);
    }

    benches();
    Criterion::default().configure_from_args().final_summary();
}
