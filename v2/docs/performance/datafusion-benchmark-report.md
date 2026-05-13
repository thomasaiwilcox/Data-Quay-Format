# COVE DataFusion Benchmark Report

Status: current local Criterion report plus public v2 corpus contract

Audience: maintainers evaluating COVE's DataFusion integration and its comparison with Parquet through DataFusion.

## 1. Purpose

This report records the current local benchmark methodology and results for `cove-datafusion`.

The comparison is between:

- COVE files read through the COVE DataFusion table provider and execution code in this repository.
- Parquet files read through DataFusion's built-in Parquet support.

This is not a raw file-format decode benchmark. It measures the format, reader, adapter, DataFusion planning, and execution path used by each side in the benchmark harness.

## 2. Environment

Run date: 2026-05-08

Repository context:

- Git `HEAD`: `06152d7`
- Working tree: included uncommitted COVE Arrow/DataFusion fixes for production-safe FileCode dictionary output.
- DataFusion crate: `53.1.0`
- Arrow crates: `58`
- Parquet crate: `58`
- Criterion crate: `0.5`

Toolchain and host:

- `rustc 1.92.0 (ded5c06cf 2025-12-08)`
- `cargo 1.92.0 (344c4567c 2025-10-21)`
- macOS `26.2` build `25C56`
- Architecture: `arm64`
- CPU model was not captured by the available local commands.

Benchmark execution:

- Criterion benchmarks were run in Cargo `bench` profile.
- `Gnuplot` was not available; Criterion used the plotters backend.
- Timings below use Criterion's `mean.point_estimate` from `target/criterion/*/new/estimates.json`.
- Ratio is `COVE time / Parquet time`. A ratio below `1.00x` means COVE was faster in that run.

## 3. Commands

Verification commands:

```text
cargo test -p cove-core
cargo test -p cove-arrow
cargo test -p cove-datafusion
cargo check --workspace
git diff --check
```

Benchmark commands:

```text
cargo run -p cove-bench -- check
cargo run -p cove-bench -- gen --profile standard --out target/cove-bench/standard
cargo run -p cove-bench -- run --corpus target/cove-bench/standard --report-json target/cove-bench/standard/report.json --report-md target/cove-bench/standard/report.md
cargo bench -p cove-datafusion --features parquet-compare --bench m6 -- --noplot
cargo bench -p cove-datafusion --features parquet-compare --bench m7_sql_mix -- --noplot
```

The previously planned command below was not runnable because `cove-arrow` has no `parquet` feature; Parquet is a normal dependency of that crate.

```text
cargo test -p cove-arrow --features parquet
```

## 4. Methodology

### 4.1 General

Both formats use their own structural and adapter advantages inside DataFusion:

- COVE uses its table provider, scan planner, pruning metadata, lookup indexes, direct Arrow export paths, range reads, materialization selector, and COVE-specific metrics.
- Parquet uses DataFusion's Parquet reader and the Parquet layout written by the benchmark fixture.
- The comparison does not force either format through a deliberately weakened path.

The result is intended to answer: "How does the current COVE DataFusion integration behave against DataFusion's Parquet path for these fixtures and queries?"

### 4.1.1 Public v2 Corpus Harness

`cove-bench` now provides the public reproducible corpus surface required by Spec §78:

- `gen --profile ci|standard|publication --out <dir>` writes deterministic generated artifacts outside the tracked source tree.
- `run --corpus <dir> --report-json <path> --report-md <path>` executes the query matrix and emits machine-readable and Markdown reports.
- `check` runs the CI-sized generate-and-run path and fails if required cases or report fields are missing.

The committed manifest lives in `crates/cove-bench/benchmarks/public-v2-corpus.json`. Generated outputs include COVE, Parquet, COVX/COVM where the converter emits them, a synthetic COVE-CACHE fixture, corpus lock metadata with digests, and benchmark reports with planning, scan, end-to-end, row, coverage, and cache metrics.

### 4.2 M6

`m6` contains lower-level scan and execution benchmarks plus Parquet comparison tracks.

The Parquet comparison fixture includes:

- scan-heavy row count: `32,768`
- wide row count: `16,384`
- segment row count: `4,096`
- wide columns: `12`

The M6 suite includes small native COVE scan-path microbenchmarks and larger COVE-vs-Parquet query tracks. The small native microbenchmarks are useful for tracking internal changes, but they should not be interpreted as Parquet comparisons.

### 4.3 M7 SQL Mix

`m7_sql_mix` uses SQL queries over three tables:

- `orders`: `32,768` rows
- `customers`: `1,024` rows
- `products`: `256` rows
- segment row count: `4,096`

It reports two modes:

- `full_query`: prepare and execute the query.
- `execute_only`: execute a prepared physical plan.

The benchmark includes default COVE and Parquet tracks, plus explicit opt-in FileCode dictionary tracks for selected query families.

## 5. Caveats

- Results are local-machine results and should not be presented as universal performance claims.
- Criterion's terminal "regressed" or "improved" labels compare against whatever local saved baseline was present. They are not used as the basis for this report.
- Some tracks had Criterion warnings that 100 samples could not complete within the default target time. Those results are still recorded, but the confidence interval should be checked before making narrow claims.
- Some M7 tracks show wide confidence intervals and outliers. Treat those as directional until repeated on a controlled machine.
- The `ci` benchmark profile is synthetic and intentionally small enough to run in PR/release gates. The `standard` and `publication` profiles increase row counts but remain deterministic generated corpora, not real customer datasets.
- Object-store cold scans are represented by report fields and disclosure metadata in this first public corpus, but the local harness does not yet drive a remote object-store service.
- FileCode dictionary output is opt-in. It is currently retained for correctness and engine-integration testing, not enabled as the default performance path.
- The production-safe FileCode dictionary fix may trade speed for correctness when a file dictionary contains entries from multiple logical domains.

## 6. Verification Results

All runnable verification commands passed:

| Command | Result |
| --- | --- |
| `cargo test -p cove-core` | Passed |
| `cargo test -p cove-arrow` | Passed |
| `cargo test -p cove-datafusion` | Passed |
| `cargo check --workspace` | Passed |
| `git diff --check` | Passed |
| `cargo test -p cove-arrow --features parquet` | Not applicable: feature does not exist |

## 7. M6 Internal COVE Tracks

These are COVE-only internal tracks and do not compare against Parquet.

| Track | Mean time |
| --- | ---: |
| `m6_full_scan` | 11.27 us |
| `m6_projection_scan` | 10.05 us |
| `m6_filecode_equality_filter` | 8.08 us |
| `m6_numeric_range_filter` | 11.37 us |
| `m6_lookup_backed_point_filter` | 7.83 us |
| `m6_late_materialization_wide_rows` | 14.28 us |
| `m6_metadata_count_fast_path` | 103.95 ns |
| `m6_topn_hinted_scan` | 5.34 us |
| `m6_overlay_restricted_scan` | 210.77 us |

## 8. M6 COVE vs Parquet

| Track | COVE | Parquet | Ratio |
| --- | ---: | ---: | ---: |
| `parquet_compare_full_scan` | 227.88 us | 273.39 us | 0.83x |
| `parquet_compare_projection_scan` | 280.25 us | 272.97 us | 1.03x |
| `parquet_compare_low_cardinality_filter` | 234.88 us | 628.14 us | 0.37x |
| `parquet_compare_numeric_range_filter` | 294.33 us | 605.80 us | 0.49x |
| `parquet_compare_wide_projection_filter` | 247.02 us | 607.07 us | 0.41x |
| `parquet_compare_scan_heavy_full_scan` | 554.51 us | 613.56 us | 0.90x |
| `parquet_compare_scan_heavy_projection_scan` | 365.06 us | 289.06 us | 1.26x |
| `parquet_compare_scan_heavy_low_cardinality_filter` | 2.567 ms | 763.34 us | 3.36x |
| `parquet_compare_scan_heavy_numeric_range_filter` | 673.16 us | 660.01 us | 1.02x |
| `parquet_compare_scan_heavy_wide_projection_filter` | 471.38 us | 696.28 us | 0.68x |
| `parquet_compare_cold_context_full_scan` | 663.31 us | 851.29 us | 0.78x |
| `parquet_compare_cold_context_numeric_range_filter` | 791.14 us | 913.64 us | 0.87x |

## 9. M6 Opt-In FileCode Dictionary Tracks

These tracks are not the default COVE registration path.

| Track | COVE | Parquet | Ratio |
| --- | ---: | ---: | ---: |
| `parquet_compare_scan_heavy_projection_scan_filecode_decoded` | 4.096 ms | 295.43 us | 13.86x |
| `parquet_compare_scan_heavy_projection_scan_filecode_dictionary` | 824.47 us | 296.10 us | 2.78x |
| `parquet_compare_scan_heavy_low_cardinality_filter_filecode_dicti` | 1.081 ms | 742.03 us | 1.46x |
| `parquet_compare_scan_heavy_numeric_range_filter_filecode_diction` | 945.33 us | 660.14 us | 1.43x |

## 10. M6 COVE VarBytes Output Modes

These tracks isolate COVE output-mode choices for the scan-heavy projection fixture.

| Mode | Mean time |
| --- | ---: |
| `standard-strict` | 2.232 ms |
| `standard-trusted` | 1.105 ms |
| `standard-strict-mmap` | 1.200 ms |
| `standard-trusted-mmap` | 1.086 ms |
| `view` | 3.138 ms |
| `filecode-dictionary` | 2.404 ms |

## 11. M7 Full Query

`full_query` includes query preparation and execution.

| Query | COVE | Parquet | Ratio |
| --- | ---: | ---: | ---: |
| `olap_narrow_projection` | 1.551 ms | 1.665 ms | 0.93x |
| `olap_group_status` | 3.866 ms | 3.967 ms | 0.97x |
| `olap_group_customer` | 5.171 ms | 4.181 ms | 1.24x |
| `olap_top_customers` | 3.058 ms | 2.737 ms | 1.12x |
| `olap_count_distinct_customers` | 4.094 ms | 3.784 ms | 1.08x |
| `operational_point_lookup` | 1.045 ms | 3.093 ms | 0.34x |
| `operational_small_in_lookup` | 1.914 ms | 2.753 ms | 0.70x |
| `operational_latest_customer` | 1.134 ms | 1.455 ms | 0.78x |
| `operational_zero_match` | 195.90 us | 420.42 us | 0.47x |
| `join_fact_customer_region` | 1.922 ms | 2.047 ms | 0.94x |
| `join_star_region_category` | 8.807 ms | 10.244 ms | 0.86x |
| `join_selective_dimensions` | 5.772 ms | 10.625 ms | 0.54x |
| `join_left_stocked_products` | 1.125 ms | 6.949 ms | 0.16x |
| `join_semi_customer_tier` | 1.103 ms | 1.529 ms | 0.72x |
| `join_anti_unstocked_products` | 1.131 ms | 1.947 ms | 0.58x |

## 12. M7 Execute Only

`execute_only` executes a prepared physical plan.

| Query | COVE | Parquet | Ratio |
| --- | ---: | ---: | ---: |
| `olap_narrow_projection` | 685.78 us | 1.097 ms | 0.63x |
| `olap_group_status` | 1.929 ms | 1.858 ms | 1.04x |
| `olap_group_customer` | 1.355 ms | 2.056 ms | 0.66x |
| `olap_top_customers` | 1.460 ms | 3.027 ms | 0.48x |
| `olap_count_distinct_customers` | 1.979 ms | 3.206 ms | 0.62x |
| `operational_point_lookup` | 288.69 us | 1.565 ms | 0.18x |
| `operational_small_in_lookup` | 327.71 us | 586.94 us | 0.56x |
| `operational_latest_customer` | 544.92 us | 622.01 us | 0.88x |
| `operational_zero_match` | 22.93 us | 120.17 us | 0.19x |
| `join_fact_customer_region` | 3.279 ms | 2.725 ms | 1.20x |
| `join_star_region_category` | 4.351 ms | 3.869 ms | 1.12x |
| `join_selective_dimensions` | 900.47 us | 4.467 ms | 0.20x |
| `join_left_stocked_products` | 455.84 us | 710.74 us | 0.64x |
| `join_semi_customer_tier` | 380.10 us | 590.16 us | 0.64x |
| `join_anti_unstocked_products` | 517.95 us | 606.91 us | 0.85x |

## 13. M7 Opt-In FileCode Dictionary Tracks

These COVE-only tracks use opt-in FileCode dictionary registration.

| Track | Mean time |
| --- | ---: |
| `filecode_dictionary_full_query/olap_narrow_projection` | 796.87 us |
| `filecode_dictionary_execute_only/olap_narrow_projection` | 600.50 us |
| `filecode_dictionary_full_query/olap_group_status` | 1.841 ms |
| `filecode_dictionary_execute_only/olap_group_status` | 1.347 ms |
| `filecode_dictionary_full_query/operational_latest_customer` | 1.094 ms |
| `filecode_dictionary_execute_only/operational_latest_customer` | 478.05 us |
| `filecode_dictionary_full_query/join_selective_dimensions` | 2.474 ms |
| `filecode_dictionary_execute_only/join_selective_dimensions` | 733.00 us |

## 14. M7 COVE Mode Tracks

These tracks isolate registration/output choices for `operational_latest_customer`.

| Mode | Mean time |
| --- | ---: |
| `standard-strict` | 2.931 ms |
| `trusted-strings` | 1.199 ms |
| `strict-mmap` | 1.568 ms |

## 15. Observations

The following statements are limited to this run and these fixtures.

- COVE is faster than Parquet on most selective, lookup, zero-match, and metadata-sensitive paths.
- COVE is near parity on several full-scan and group-by paths.
- Parquet is faster on the M6 scan-heavy projection path and on some M7 OLAP grouping/top-customer tracks.
- The opt-in FileCode dictionary path is not consistently faster than either default COVE or Parquet on the current fixtures.
- Arrow view output is not a clear win in the current scan-heavy projection fixture.
- The current production default should remain decoded `Utf8/Binary` output unless callers explicitly opt into dictionary output.

## 16. Reproduction Notes

To reproduce this report:

1. Run the verification commands in Section 3.
2. Run the two benchmark commands in Section 3.
3. Read the latest Criterion estimates from `target/criterion/*/new/estimates.json`.
4. Recompute ratios as `COVE / Parquet`.

When comparing across commits, prefer one of the following:

- run both commits on the same machine in close succession;
- clear or explicitly manage Criterion baselines;
- report confidence intervals and raw mean estimates;
- avoid relying on Criterion's local "change" labels unless the saved baseline is known.
