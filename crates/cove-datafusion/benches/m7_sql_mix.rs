use criterion::{black_box, criterion_group, criterion_main, Criterion};

#[cfg(feature = "parquet-compare")]
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

#[cfg(feature = "parquet-compare")]
use arrow_array::{ArrayRef, BooleanArray, Int64Array, RecordBatch, StringArray};
#[cfg(feature = "parquet-compare")]
use cove_core::{
    constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind, PrimaryProfile, SectionKind},
    table::{ColumnEntry, TableCatalog, TableEntry},
    writer::{ScanPageSpec, ScanProfileCoveWriter, ScanSegment, SectionPayload},
    zone_stats::{
        StatKind, StatScalar, ZoneScope, ZoneStatFlags, ZoneStats, ZoneStatsEntry, ZoneStatsSection,
    },
};
#[cfg(feature = "parquet-compare")]
use cove_datafusion::{options::CoveTableOptions, register::register_cove_file_with_options};
#[cfg(feature = "parquet-compare")]
use datafusion::{
    physical_plan::{
        execution_plan::{collect as collect_execution_plan, reset_plan_states},
        ExecutionPlan,
    },
    prelude::{ParquetReadOptions, SessionContext},
};
#[cfg(feature = "parquet-compare")]
use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
#[cfg(feature = "parquet-compare")]
use tokio::runtime::Runtime;

#[cfg(feature = "parquet-compare")]
static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "parquet-compare")]
const ORDER_ROWS: usize = 32_768;
#[cfg(feature = "parquet-compare")]
const CUSTOMER_ROWS: usize = 1_024;
#[cfg(feature = "parquet-compare")]
const PRODUCT_ROWS: usize = 256;
#[cfg(feature = "parquet-compare")]
const SEGMENT_ROWS: usize = 4_096;

#[cfg(feature = "parquet-compare")]
fn bench_m7_sql_mix(c: &mut Criterion) {
    let runtime = Runtime::new().expect("benchmark runtime");
    let fixture = SqlMixFixture::new(&runtime, CoveTableOptions::default());
    let trusted_fixture = SqlMixFixture::new(
        &runtime,
        CoveTableOptions::default().with_trusted_arrow_string_validation(),
    );
    let mmap_fixture = SqlMixFixture::new(
        &runtime,
        CoveTableOptions::default().with_local_file_mmap_reads(),
    );

    let queries = sql_mix_queries();
    for query in &queries {
        let cove = fixture.prepare_query(&runtime, &query.sql("cove"));
        let parquet = fixture.prepare_query(&runtime, &query.sql("parquet"));
        bench_query_pair(
            c,
            &format!("m7_sql_mix_full_query/{}", query.name),
            &runtime,
            &fixture.ctx,
            &cove,
            &parquet,
            ExecutionMode::FullQuery,
        );
        bench_query_pair(
            c,
            &format!("m7_sql_mix_execute_only/{}", query.name),
            &runtime,
            &fixture.ctx,
            &cove,
            &parquet,
            ExecutionMode::ExecuteOnly,
        );
    }

    let strict_projection = fixture.prepare_query(
        &runtime,
        "SELECT order_id, status FROM orders_cove WHERE customer_id = 42 ORDER BY created_day DESC LIMIT 10",
    );
    let trusted_projection = trusted_fixture.prepare_query(
        &runtime,
        "SELECT order_id, status FROM orders_cove WHERE customer_id = 42 ORDER BY created_day DESC LIMIT 10",
    );
    let mmap_projection = mmap_fixture.prepare_query(
        &runtime,
        "SELECT order_id, status FROM orders_cove WHERE customer_id = 42 ORDER BY created_day DESC LIMIT 10",
    );
    let mut group = c.benchmark_group("m7_sql_mix_cove_modes/operational_latest_customer");
    group.bench_function("standard-strict", |b| {
        b.iter(|| {
            black_box(execute_full_query(
                &runtime,
                &fixture.ctx,
                &strict_projection,
            ))
        })
    });
    group.bench_function("trusted-strings", |b| {
        b.iter(|| {
            black_box(execute_full_query(
                &runtime,
                &trusted_fixture.ctx,
                &trusted_projection,
            ))
        })
    });
    group.bench_function("strict-mmap", |b| {
        b.iter(|| {
            black_box(execute_full_query(
                &runtime,
                &mmap_fixture.ctx,
                &mmap_projection,
            ))
        })
    });
    group.finish();
}

#[cfg(not(feature = "parquet-compare"))]
fn bench_m7_sql_mix(_c: &mut Criterion) {}

#[cfg(feature = "parquet-compare")]
#[derive(Debug, Clone, Copy)]
enum ExecutionMode {
    FullQuery,
    ExecuteOnly,
}

#[cfg(feature = "parquet-compare")]
fn bench_query_pair(
    c: &mut Criterion,
    name: &str,
    runtime: &Runtime,
    ctx: &SessionContext,
    cove: &PreparedQuery,
    parquet: &PreparedQuery,
    mode: ExecutionMode,
) {
    let mut group = c.benchmark_group(name);
    match mode {
        ExecutionMode::FullQuery => {
            group.bench_function("cove", |b| {
                b.iter(|| black_box(execute_full_query(runtime, ctx, cove)))
            });
            group.bench_function("parquet", |b| {
                b.iter(|| black_box(execute_full_query(runtime, ctx, parquet)))
            });
        }
        ExecutionMode::ExecuteOnly => {
            group.bench_function("cove", |b| {
                b.iter(|| black_box(execute_physical_plan(runtime, ctx, cove)))
            });
            group.bench_function("parquet", |b| {
                b.iter(|| black_box(execute_physical_plan(runtime, ctx, parquet)))
            });
        }
    }
    group.finish();
}

#[cfg(feature = "parquet-compare")]
#[derive(Debug, Clone)]
struct SqlMixQuery {
    name: &'static str,
    template: &'static str,
}

#[cfg(feature = "parquet-compare")]
impl SqlMixQuery {
    fn sql(&self, suffix: &str) -> String {
        self.template.replace("{s}", suffix)
    }
}

#[cfg(feature = "parquet-compare")]
fn sql_mix_queries() -> Vec<SqlMixQuery> {
    vec![
        SqlMixQuery {
            name: "olap_narrow_projection",
            template: "SELECT order_id, customer_id, amount, status FROM orders_{s}",
        },
        SqlMixQuery {
            name: "olap_group_status",
            template: "SELECT status, COUNT(*) AS n, SUM(amount) AS total FROM orders_{s} GROUP BY status",
        },
        SqlMixQuery {
            name: "olap_group_customer",
            template: "SELECT customer_id, COUNT(*) AS n, SUM(amount) AS total FROM orders_{s} GROUP BY customer_id",
        },
        SqlMixQuery {
            name: "olap_top_customers",
            template: "SELECT customer_id, SUM(amount) AS total FROM orders_{s} GROUP BY customer_id ORDER BY total DESC LIMIT 20",
        },
        SqlMixQuery {
            name: "olap_count_distinct_customers",
            template: "SELECT COUNT(DISTINCT customer_id) AS customers FROM orders_{s}",
        },
        SqlMixQuery {
            name: "operational_point_lookup",
            template: "SELECT amount, status FROM orders_{s} WHERE order_id = 12345",
        },
        SqlMixQuery {
            name: "operational_small_in_lookup",
            template: "SELECT order_id, amount FROM orders_{s} WHERE order_id IN (10, 2048, 4096, 8192, 16384, 24576, 32768)",
        },
        SqlMixQuery {
            name: "operational_latest_customer",
            template: "SELECT order_id, status FROM orders_{s} WHERE customer_id = 42 ORDER BY created_day DESC LIMIT 10",
        },
        SqlMixQuery {
            name: "operational_zero_match",
            template: "SELECT order_id, amount FROM orders_{s} WHERE order_id = -1",
        },
        SqlMixQuery {
            name: "join_fact_customer_region",
            template: "SELECT c.region, COUNT(*) AS n, SUM(o.amount) AS total FROM orders_{s} o JOIN customers_{s} c ON o.customer_id = c.customer_id GROUP BY c.region",
        },
        SqlMixQuery {
            name: "join_star_region_category",
            template: "SELECT c.region, p.category, COUNT(*) AS n, SUM(o.amount) AS total FROM orders_{s} o JOIN customers_{s} c ON o.customer_id = c.customer_id JOIN products_{s} p ON o.product_id = p.product_id GROUP BY c.region, p.category",
        },
        SqlMixQuery {
            name: "join_selective_dimensions",
            template: "SELECT COUNT(*) AS n, SUM(o.amount) AS total FROM orders_{s} o JOIN customers_{s} c ON o.customer_id = c.customer_id JOIN products_{s} p ON o.product_id = p.product_id WHERE c.region = 'north' AND p.category = 'software'",
        },
        SqlMixQuery {
            name: "join_left_stocked_products",
            template: "SELECT COUNT(*) AS n FROM orders_{s} o LEFT JOIN products_{s} p ON o.product_id = p.product_id WHERE p.stocked = true",
        },
        SqlMixQuery {
            name: "join_semi_customer_tier",
            template: "SELECT COUNT(*) AS n FROM orders_{s} o WHERE o.customer_id IN (SELECT c.customer_id FROM customers_{s} c WHERE c.tier = 'enterprise')",
        },
        SqlMixQuery {
            name: "join_anti_unstocked_products",
            template: "SELECT COUNT(*) AS n FROM orders_{s} o WHERE o.product_id NOT IN (SELECT p.product_id FROM products_{s} p WHERE p.stocked = true)",
        },
    ]
}

#[cfg(feature = "parquet-compare")]
struct SqlMixFixture {
    _dir: TempFixtureDir,
    ctx: SessionContext,
}

#[cfg(feature = "parquet-compare")]
impl SqlMixFixture {
    fn new(runtime: &Runtime, cove_options: CoveTableOptions) -> Self {
        let dir = TempFixtureDir::new("m7-sql-mix");
        let orders = TablePaths {
            cove: dir.path.join("orders.cove"),
            parquet: dir.path.join("orders.parquet"),
        };
        let customers = TablePaths {
            cove: dir.path.join("customers.cove"),
            parquet: dir.path.join("customers.parquet"),
        };
        let products = TablePaths {
            cove: dir.path.join("products.cove"),
            parquet: dir.path.join("products.parquet"),
        };

        fs::write(&orders.cove, orders_cove_file(ORDER_ROWS, SEGMENT_ROWS))
            .expect("write orders COVE fixture");
        fs::write(
            &customers.cove,
            customers_cove_file(CUSTOMER_ROWS, SEGMENT_ROWS),
        )
        .expect("write customers COVE fixture");
        fs::write(
            &products.cove,
            products_cove_file(PRODUCT_ROWS, SEGMENT_ROWS),
        )
        .expect("write products COVE fixture");

        write_parquet_file(&orders.parquet, &orders_batch(ORDER_ROWS));
        write_parquet_file(&customers.parquet, &customers_batch(CUSTOMER_ROWS));
        write_parquet_file(&products.parquet, &products_batch(PRODUCT_ROWS));

        let ctx = SessionContext::new();
        register_cove_file_with_options(&ctx, "orders_cove", &orders.cove, cove_options)
            .expect("register orders_cove");
        register_cove_file_with_options(&ctx, "customers_cove", &customers.cove, cove_options)
            .expect("register customers_cove");
        register_cove_file_with_options(&ctx, "products_cove", &products.cove, cove_options)
            .expect("register products_cove");

        runtime.block_on(async {
            ctx.register_parquet(
                "orders_parquet",
                orders.parquet.to_str().expect("orders parquet path"),
                ParquetReadOptions::default(),
            )
            .await
            .expect("register orders_parquet");
            ctx.register_parquet(
                "customers_parquet",
                customers.parquet.to_str().expect("customers parquet path"),
                ParquetReadOptions::default(),
            )
            .await
            .expect("register customers_parquet");
            ctx.register_parquet(
                "products_parquet",
                products.parquet.to_str().expect("products parquet path"),
                ParquetReadOptions::default(),
            )
            .await
            .expect("register products_parquet");
        });

        Self { _dir: dir, ctx }
    }

    fn prepare_query(&self, runtime: &Runtime, sql: &str) -> PreparedQuery {
        runtime.block_on(async {
            let batches = self
                .ctx
                .sql(sql)
                .await
                .expect("build setup query")
                .collect()
                .await
                .expect("execute setup query");
            let expected_rows = batches.iter().map(|batch| batch.num_rows()).sum();
            let expected_columns = batches
                .first()
                .map(|batch| batch.num_columns())
                .unwrap_or(0);
            let plan = self
                .ctx
                .sql(sql)
                .await
                .expect("build physical-plan query")
                .create_physical_plan()
                .await
                .expect("create physical plan");
            PreparedQuery {
                sql: Arc::<str>::from(sql),
                expected_rows,
                expected_columns,
                plan,
            }
        })
    }
}

#[cfg(feature = "parquet-compare")]
#[derive(Debug, Clone)]
struct PreparedQuery {
    sql: Arc<str>,
    expected_rows: usize,
    expected_columns: usize,
    plan: Arc<dyn ExecutionPlan>,
}

#[cfg(feature = "parquet-compare")]
fn execute_full_query(
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
        assert_query_batches(&batches, query)
    })
}

#[cfg(feature = "parquet-compare")]
fn execute_physical_plan(
    runtime: &Runtime,
    ctx: &SessionContext,
    query: &PreparedQuery,
) -> (usize, usize) {
    let plan = reset_plan_states(Arc::clone(&query.plan)).expect("reset execution plan state");
    let batches = runtime.block_on(async {
        collect_execution_plan(plan, ctx.task_ctx())
            .await
            .expect("execute physical plan")
    });
    assert_query_batches(&batches, query)
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
#[derive(Debug, Clone)]
struct TablePaths {
    cove: PathBuf,
    parquet: PathBuf,
}

#[cfg(feature = "parquet-compare")]
struct TempFixtureDir {
    path: PathBuf,
}

#[cfg(feature = "parquet-compare")]
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

#[cfg(feature = "parquet-compare")]
impl Drop for TempFixtureDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(feature = "parquet-compare")]
fn orders_cove_file(row_count: usize, segment_rows: usize) -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 71,
            namespace: "public".into(),
            name: "orders".into(),
            row_count: row_count as u64,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "order_id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    2,
                    "customer_id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    3,
                    "product_id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    4,
                    "amount",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    5,
                    "created_day",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    6,
                    "status",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
                column(
                    7,
                    "channel",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
                column(
                    8,
                    "fulfilled",
                    CoveLogicalType::Bool,
                    CovePhysicalKind::Boolean,
                    false,
                ),
            ],
        }],
    };
    let rows = orders_rows(row_count);
    let mut writer = ScanProfileCoveWriter::new(catalog);
    for (segment_idx, row_start) in (0..row_count).step_by(segment_rows).enumerate() {
        let row_end = row_count.min(row_start + segment_rows);
        let segment_len = (row_end - row_start) as u32;
        let mut segment =
            ScanSegment::new(71, segment_idx as u32, row_start as u64, segment_len, 8);
        segment.set_column_pages(
            1,
            vec![numcode_page(
                segment_len,
                numcode_i64(&rows.order_id[row_start..row_end]),
            )],
        );
        segment.set_column_pages(
            2,
            vec![numcode_page(
                segment_len,
                numcode_i64(&rows.customer_id[row_start..row_end]),
            )],
        );
        segment.set_column_pages(
            3,
            vec![numcode_page(
                segment_len,
                numcode_i64(&rows.product_id[row_start..row_end]),
            )],
        );
        segment.set_column_pages(
            4,
            vec![numcode_page(
                segment_len,
                numcode_i64(&rows.amount[row_start..row_end]),
            )],
        );
        segment.set_column_pages(
            5,
            vec![numcode_page(
                segment_len,
                numcode_i64(&rows.created_day[row_start..row_end]),
            )],
        );
        let status_refs = rows.status[row_start..row_end]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        segment.set_column_pages(6, vec![varbytes_page(segment_len, varbytes(&status_refs))]);
        let channel_refs = rows.channel[row_start..row_end]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        segment.set_column_pages(7, vec![varbytes_page(segment_len, varbytes(&channel_refs))]);
        segment.set_column_pages(
            8,
            vec![bool_page(
                segment_len,
                bools(&rows.fulfilled[row_start..row_end]),
            )],
        );
        writer.push_segment(segment);
    }
    writer.push_extra_section(order_id_zone_stats_section(&rows.order_id, segment_rows));
    writer.write().expect("write orders fixture")
}

#[cfg(feature = "parquet-compare")]
fn order_id_zone_stats_section(order_ids: &[i64], segment_rows: usize) -> SectionPayload {
    let mut entries = Vec::new();
    for (segment_idx, row_start) in (0..order_ids.len()).step_by(segment_rows).enumerate() {
        let row_end = order_ids.len().min(row_start + segment_rows);
        if row_start == row_end {
            continue;
        }
        entries.push(order_id_zone_stats_entry(
            segment_idx as u32,
            (row_end - row_start) as u32,
            order_ids[row_start],
            order_ids[row_end - 1],
        ));
    }
    let item_count = entries.len() as u64;
    let row_count = order_ids.len() as u64;
    let section = ZoneStatsSection { entries };
    SectionPayload {
        section_kind: SectionKind::ZoneStats as u16,
        profile: PrimaryProfile::TableScan as u8,
        flags: 0,
        item_count,
        row_count,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: section.serialize().expect("serialize order_id zone stats"),
    }
}

#[cfg(feature = "parquet-compare")]
fn order_id_zone_stats_entry(
    segment_id: u32,
    row_count: u32,
    min: i64,
    max: i64,
) -> ZoneStatsEntry {
    ZoneStatsEntry {
        table_id: 71,
        segment_id,
        morsel_id: 0,
        column_id: 1,
        non_null_count: row_count,
        distinct_count: row_count,
        run_count: row_count,
        stats: ZoneStats {
            scope: ZoneScope::Morsel,
            row_count: u64::from(row_count),
            null_count: 0,
            min: Some(int64_stat(min)),
            max: Some(int64_stat(max)),
            flags: ZoneStatFlags::HAS_MIN_MAX
                | ZoneStatFlags::DISTINCT_EXACT
                | ZoneStatFlags::SORTED_ASC,
        },
        min_domain_rank: u32::MAX,
        max_domain_rank: u32::MAX,
        exact_set_ref: u32::MAX,
        bloom_ref: u32::MAX,
    }
}

#[cfg(feature = "parquet-compare")]
fn int64_stat(value: i64) -> StatScalar {
    StatScalar {
        kind: StatKind::Int64,
        bytes: value.to_le_bytes().to_vec(),
        truncated: false,
    }
}

#[cfg(feature = "parquet-compare")]
fn customers_cove_file(row_count: usize, segment_rows: usize) -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 72,
            namespace: "public".into(),
            name: "customers".into(),
            row_count: row_count as u64,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "customer_id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    2,
                    "region",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
                column(
                    3,
                    "tier",
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
    let rows = customers_rows(row_count);
    let mut writer = ScanProfileCoveWriter::new(catalog);
    for (segment_idx, row_start) in (0..row_count).step_by(segment_rows).enumerate() {
        let row_end = row_count.min(row_start + segment_rows);
        let segment_len = (row_end - row_start) as u32;
        let mut segment =
            ScanSegment::new(72, segment_idx as u32, row_start as u64, segment_len, 4);
        segment.set_column_pages(
            1,
            vec![numcode_page(
                segment_len,
                numcode_i64(&rows.customer_id[row_start..row_end]),
            )],
        );
        let region_refs = rows.region[row_start..row_end]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        segment.set_column_pages(2, vec![varbytes_page(segment_len, varbytes(&region_refs))]);
        let tier_refs = rows.tier[row_start..row_end]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        segment.set_column_pages(3, vec![varbytes_page(segment_len, varbytes(&tier_refs))]);
        segment.set_column_pages(
            4,
            vec![bool_page(
                segment_len,
                bools(&rows.active[row_start..row_end]),
            )],
        );
        writer.push_segment(segment);
    }
    writer.write().expect("write customers fixture")
}

#[cfg(feature = "parquet-compare")]
fn products_cove_file(row_count: usize, segment_rows: usize) -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 73,
            namespace: "public".into(),
            name: "products".into(),
            row_count: row_count as u64,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "product_id",
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
                    "price_band",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    4,
                    "stocked",
                    CoveLogicalType::Bool,
                    CovePhysicalKind::Boolean,
                    false,
                ),
            ],
        }],
    };
    let rows = products_rows(row_count);
    let mut writer = ScanProfileCoveWriter::new(catalog);
    for (segment_idx, row_start) in (0..row_count).step_by(segment_rows).enumerate() {
        let row_end = row_count.min(row_start + segment_rows);
        let segment_len = (row_end - row_start) as u32;
        let mut segment =
            ScanSegment::new(73, segment_idx as u32, row_start as u64, segment_len, 4);
        segment.set_column_pages(
            1,
            vec![numcode_page(
                segment_len,
                numcode_i64(&rows.product_id[row_start..row_end]),
            )],
        );
        let category_refs = rows.category[row_start..row_end]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        segment.set_column_pages(
            2,
            vec![varbytes_page(segment_len, varbytes(&category_refs))],
        );
        segment.set_column_pages(
            3,
            vec![numcode_page(
                segment_len,
                numcode_i64(&rows.price_band[row_start..row_end]),
            )],
        );
        segment.set_column_pages(
            4,
            vec![bool_page(
                segment_len,
                bools(&rows.stocked[row_start..row_end]),
            )],
        );
        writer.push_segment(segment);
    }
    writer.write().expect("write products fixture")
}

#[cfg(feature = "parquet-compare")]
fn orders_batch(row_count: usize) -> RecordBatch {
    let rows = orders_rows(row_count);
    RecordBatch::try_from_iter(vec![
        (
            "order_id",
            Arc::new(Int64Array::from(rows.order_id)) as ArrayRef,
        ),
        (
            "customer_id",
            Arc::new(Int64Array::from(rows.customer_id)) as ArrayRef,
        ),
        (
            "product_id",
            Arc::new(Int64Array::from(rows.product_id)) as ArrayRef,
        ),
        (
            "amount",
            Arc::new(Int64Array::from(rows.amount)) as ArrayRef,
        ),
        (
            "created_day",
            Arc::new(Int64Array::from(rows.created_day)) as ArrayRef,
        ),
        (
            "status",
            Arc::new(StringArray::from(rows.status)) as ArrayRef,
        ),
        (
            "channel",
            Arc::new(StringArray::from(rows.channel)) as ArrayRef,
        ),
        (
            "fulfilled",
            Arc::new(BooleanArray::from(rows.fulfilled)) as ArrayRef,
        ),
    ])
    .expect("orders record batch")
}

#[cfg(feature = "parquet-compare")]
fn customers_batch(row_count: usize) -> RecordBatch {
    let rows = customers_rows(row_count);
    RecordBatch::try_from_iter(vec![
        (
            "customer_id",
            Arc::new(Int64Array::from(rows.customer_id)) as ArrayRef,
        ),
        (
            "region",
            Arc::new(StringArray::from(rows.region)) as ArrayRef,
        ),
        ("tier", Arc::new(StringArray::from(rows.tier)) as ArrayRef),
        (
            "active",
            Arc::new(BooleanArray::from(rows.active)) as ArrayRef,
        ),
    ])
    .expect("customers record batch")
}

#[cfg(feature = "parquet-compare")]
fn products_batch(row_count: usize) -> RecordBatch {
    let rows = products_rows(row_count);
    RecordBatch::try_from_iter(vec![
        (
            "product_id",
            Arc::new(Int64Array::from(rows.product_id)) as ArrayRef,
        ),
        (
            "category",
            Arc::new(StringArray::from(rows.category)) as ArrayRef,
        ),
        (
            "price_band",
            Arc::new(Int64Array::from(rows.price_band)) as ArrayRef,
        ),
        (
            "stocked",
            Arc::new(BooleanArray::from(rows.stocked)) as ArrayRef,
        ),
    ])
    .expect("products record batch")
}

#[cfg(feature = "parquet-compare")]
struct OrdersRows {
    order_id: Vec<i64>,
    customer_id: Vec<i64>,
    product_id: Vec<i64>,
    amount: Vec<i64>,
    created_day: Vec<i64>,
    status: Vec<String>,
    channel: Vec<String>,
    fulfilled: Vec<bool>,
}

#[cfg(feature = "parquet-compare")]
fn orders_rows(row_count: usize) -> OrdersRows {
    let statuses = ["new", "paid", "shipped", "returned"];
    let channels = ["web", "store", "partner", "mobile"];
    let mut rows = OrdersRows {
        order_id: Vec::with_capacity(row_count),
        customer_id: Vec::with_capacity(row_count),
        product_id: Vec::with_capacity(row_count),
        amount: Vec::with_capacity(row_count),
        created_day: Vec::with_capacity(row_count),
        status: Vec::with_capacity(row_count),
        channel: Vec::with_capacity(row_count),
        fulfilled: Vec::with_capacity(row_count),
    };
    for idx in 0..row_count {
        rows.order_id.push((idx + 1) as i64);
        rows.customer_id.push((idx % CUSTOMER_ROWS + 1) as i64);
        rows.product_id
            .push(((idx.wrapping_mul(17) % PRODUCT_ROWS) + 1) as i64);
        rows.amount.push(((idx * 37) % 10_000 + 100) as i64);
        rows.created_day.push((19_000 + idx % 365) as i64);
        rows.status.push(statuses[idx % statuses.len()].to_string());
        rows.channel
            .push(channels[(idx / 3) % channels.len()].to_string());
        rows.fulfilled.push(idx % 5 != 0);
    }
    rows
}

#[cfg(feature = "parquet-compare")]
struct CustomersRows {
    customer_id: Vec<i64>,
    region: Vec<String>,
    tier: Vec<String>,
    active: Vec<bool>,
}

#[cfg(feature = "parquet-compare")]
fn customers_rows(row_count: usize) -> CustomersRows {
    let regions = ["north", "south", "east", "west"];
    let tiers = ["free", "pro", "enterprise"];
    let mut rows = CustomersRows {
        customer_id: Vec::with_capacity(row_count),
        region: Vec::with_capacity(row_count),
        tier: Vec::with_capacity(row_count),
        active: Vec::with_capacity(row_count),
    };
    for idx in 0..row_count {
        rows.customer_id.push((idx + 1) as i64);
        rows.region.push(regions[idx % regions.len()].to_string());
        rows.tier.push(tiers[idx % tiers.len()].to_string());
        rows.active.push(idx % 5 != 0);
    }
    rows
}

#[cfg(feature = "parquet-compare")]
struct ProductsRows {
    product_id: Vec<i64>,
    category: Vec<String>,
    price_band: Vec<i64>,
    stocked: Vec<bool>,
}

#[cfg(feature = "parquet-compare")]
fn products_rows(row_count: usize) -> ProductsRows {
    let categories = [
        "hardware",
        "software",
        "services",
        "accessory",
        "subscription",
        "training",
        "managed",
        "other",
    ];
    let mut rows = ProductsRows {
        product_id: Vec::with_capacity(row_count),
        category: Vec::with_capacity(row_count),
        price_band: Vec::with_capacity(row_count),
        stocked: Vec::with_capacity(row_count),
    };
    for idx in 0..row_count {
        rows.product_id.push((idx + 1) as i64);
        rows.category
            .push(categories[idx % categories.len()].to_string());
        rows.price_band.push((idx % 5 + 1) as i64);
        rows.stocked.push(idx % 7 != 0);
    }
    rows
}

#[cfg(feature = "parquet-compare")]
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

#[cfg(feature = "parquet-compare")]
fn numcode_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::NumCode as u32)
}

#[cfg(feature = "parquet-compare")]
fn varbytes_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::VarBytes as u32)
}

#[cfg(feature = "parquet-compare")]
fn bool_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::PlainFixed as u32)
}

#[cfg(feature = "parquet-compare")]
fn numcode_i64(values: &[i64]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| (*value as u64).to_le_bytes())
        .collect()
}

#[cfg(feature = "parquet-compare")]
fn varbytes(values: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    out
}

#[cfg(feature = "parquet-compare")]
fn bools(values: &[bool]) -> Vec<u8> {
    values.iter().map(|value| u8::from(*value)).collect()
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

criterion_group!(benches, bench_m7_sql_mix);
criterion_main!(benches);
