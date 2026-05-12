# Cove Format

Canonical Offline Value Encoding: an immutable, queryable archive format for
portable logical values, encoded arrays, proof-safe predicate metadata, optional
acceleration artifacts, and optional object/association semantics.

COVE is designed for engines that want to avoid work they can prove is
unnecessary. It is not just a serialization format, and it is not a lakehouse
transaction protocol. The file is a queryable persistent data structure: values,
encodings, dictionaries, indexes, coverage metadata, checksums, and feature
declarations are all part of the read contract.

## What COVE Is

COVE stores immutable offline data in `.cove` files. The baseline format is
engine-neutral: a reader can validate and decode the logical values without
depending on DataFusion, Arrow IPC, Harbor, a catalog service, or an object
store.

The format is shaped around query planning:

- table scans over COVE-T segments and morsels;
- file-local dictionaries and encoded arrays;
- zone statistics, exact sets, blooms, lookup indexes, aggregate synopses, and
  composite indexes;
- optional manifests, sidecars, secondary indexes, and layout metadata;
- optional object-temporal and semantic mapping profiles.

The important distinction is authority. COVE metadata can be used as a proof
only when the spec defines the proof semantics and the reader validates the
metadata under the relevant logical type, collation, null semantics, feature
scope, and snapshot. Advisory metadata can help planning, but it must not change
query results.

## Repository Status

This repository currently contains two workspaces:

- [`v2/`](./v2/) is the current COVE v2 standards-suite workspace. Its
  normative baseline is [`v2/spec.md`](./v2/spec.md).
- [`v1/`](./v1/) preserves the COVE v1 specification and implementation under
  [`v1/Spec.md`](./v1/Spec.md).

COVE v2 uses new magic and major-version fields. v2 readers may choose to
support v1 files, but v1 readers must reject v2 files.

The v2 implementation is staged and evidence-tracked rather than described by a
single blanket compliance claim. The generated matrix in
[`v2/conformance/capability_matrix.md`](./v2/conformance/capability_matrix.md)
is the source of truth for which areas are modeled, parsed, validated, written,
and exercised by corpus fixtures. At the time of this README update, it reports
73 of 82 tracked capability rows as fully gated.

## How It Works

### FileCodes and ExecutionCodes

Repeated values such as strings, categories, identifiers, and other canonical
values can be stored as dense file-local integer codes called `FileCode`s. A
file dictionary maps each `FileCode` back to the canonical logical value.

`FileCode` equality is meaningful inside one COVE file. Cross-file equality
requires resolving to canonical values or mapping into an engine-owned execution
code space. COVE-E defines metadata for this kind of engine mapping, but
execution codes remain runtime-local. They are never the portable truth stored
by COVE.

This separation lets an engine mount a file, map file-local values into its own
native dictionary or symbol space, and then run equality, grouping, filtering,
and joins with integer operations. The current DataFusion adapter has opt-in
FileCode dictionary output for integration testing, but benchmark results do
not support treating that path as the default performance win yet.

### Morsels and Proof-Safe Pruning

COVE-T data is organized into table segments subdivided into morsels. The
default morsel size in the spec and reference writer is 4,096 rows. A morsel is
the unit for scheduling, predicate bitmap production, page pruning, late
materialization, row references, and FileCode-to-ExecutionCode remapping. All
columns in a segment share the same morsel boundaries.

Predicate metadata can prove outcomes such as `DefinitelyNo`,
`DefinitelyYes`, or `Unknown` in the v2 spec vocabulary. A reader may skip data
only when the proof is valid for the requested operation. If metadata is absent,
unsupported, stale, corrupt, or not strong enough, the safe behavior is to scan
the candidate data rather than prune it. Structural corruption fails closed.

COVE v2 expands this into a formal coverage model: a coverage provider may
over-include data, but it must not under-include data when it is used for
pruning, metadata-only answers, lookup routing, or index-only access.

### Object and Association Semantics

COVE works as a table archive without any object layer. For organizations that
need richer semantics over fragmented sources, v2 also includes optional
COVE-O and COVE-MAP profiles.

COVE-MAP describes deterministic conversion from source rows into semantic
objects, properties, associations, evidence, and projection readback metadata.
It separates source-row identity from semantic object identity. Source rows are
provenance; destination object identity is produced by declared deterministic
identity rules and semantic join keys.

The mapping layer is intended to be:

- deterministic: the same declared sources, mapping rules, and function
  versions produce the same semantic assertions;
- versioned and auditable: mapping artifacts carry source catalogs, replay
  fingerprints, function declarations, rule references, and evidence;
- projection-aware: object/association results can be read back through
  deterministic projected table shapes when the mapping declares that behavior.

COVE-MAP is not a probabilistic entity-resolution system, an ETL orchestrator,
or AI-based schema matching. Those systems may produce inputs, but COVE-MAP's
portable contract is deterministic replay, explanation, evidence, and
projection semantics.

### Object Storage and Cheaper Reads

On object storage, every range request has latency and often a per-request cost.
COVE is designed so readers can make fewer requests when metadata proves that
payload pages are irrelevant, or when layout metadata lets nearby reads be
coalesced.

The current DataFusion adapter includes byte-range readers, mmap-backed local
reads, range coalescing, and metrics for requested/coalesced ranges. COVE v2
also defines I/O hints, COVE-L layout plans, scan split indexes, page cluster
directories, and object-store range planning.

This should be treated as a cost model, not a universal benchmark claim. The
local benchmark report does not measure S3 or other remote object stores.
Actual request and transfer savings depend on dataset layout, predicate
selectivity, projected columns, object-store behavior, and whether the optional
layout/index metadata is present and valid.

## v2 Standards Suite Highlights

COVE v2 is a standards suite, not a single mandatory feature pile. Baseline
interoperability is COVE-Core plus COVE-T table scan reading, safe predicate
metadata interpretation, Arrow-compatible export for supported logical types,
and conformance vectors. Optional profiles are implemented or claimed
independently.

- **COVE-Core and COVE-T**: file layout, sections, dictionaries, encoded
  arrays, table catalogs, segments, morsels, page indexes, checksums,
  validation, and table scans.
- **COVE-COVERAGE**: formal coverage providers and sets for conservative
  predicate and index planning.
- **COVE-A / COVX / COVM**: acceleration indexes, sidecars, and dataset
  manifests that must preserve file truth.
- **COVE-I**: optional `.covi` secondary index artifacts. The current repo has
  artifact framing and parser support; full index-only payload coverage remains
  partial in the capability matrix.
- **COVE-E and COVE-H**: generic engine execution-code mapping and a named
  Harbor registration. Generic COVE readers do not require Harbor.
- **COVE-O**: optional object-temporal profile for object catalogs, temporal
  segments, deltas, branches, tombstones, and trust surfaces.
- **COVE-MAP**: optional semantic mapping from source rows into objects,
  associations, evidence, and deterministic projection readback.
- **COVE-CX**: registered codec-extension framework. v2 defines the path for
  FSST-style strings, ALP-style floats, FastLanes-style integer encodings, and
  future codecs, but current broad codec bitstream conformance is still staged.
- **COVE-L**: layout planning, scan splits, page clusters, fast metadata
  indexes, and zero-copy maps as optional planning aids, not schema authority.
- **COVE-R and COVE-CACHE**: runtime compatibility hints and runtime/local
  coverage caches. They are not canonical file truth.
- **Feature scopes**: v2 distinguishes file, section, page, profile, operation,
  and advisory requiredness so ordinary table reads do not fail just because
  unrelated optional profile metadata is unsupported.

## Repository Layout

Important v2 paths:

- [`v2/spec.md`](./v2/spec.md): COVE v2 full specification and current
  normative baseline for implementation and conformance-vector development.
- [`v2/crates/cove-core`](./v2/crates/cove-core): core file structures,
  validation, dictionaries, encodings, indexes, writers, readers, and profiles.
- [`v2/crates/cove-arrow`](./v2/crates/cove-arrow): Arrow export/import and
  Parquet conversion support layered on `cove-core`.
- [`v2/crates/cove-datafusion`](./v2/crates/cove-datafusion): DataFusion table
  provider, file format integration, pruning, range reads, metrics, COVM/COVX
  bootstrap paths, and benchmarks.
- [`v2/crates/cove-map`](./v2/crates/cove-map): reference COVE-MAP execution,
  materialization, evidence, and projection helpers.
- [`v2/crates/cove-codec`](./v2/crates/cove-codec): COVE-CX descriptor and
  registered-envelope validation.
- [`v2/crates/cove-coverage`](./v2/crates/cove-coverage): COVE-COVERAGE
  provider and coverage-set parsing/inspection.
- [`v2/crates/cove-layout`](./v2/crates/cove-layout): COVE-L layout and scan
  split metadata helpers.
- [`v2/crates/cove-index`](./v2/crates/cove-index): COVE-I artifact framing and
  inspection/build helpers.
- [`v2/crates/cove-runtime`](./v2/crates/cove-runtime): COVE-R runtime
  compatibility hints.
- [`v2/crates/cove-cache`](./v2/crates/cove-cache): runtime/local coverage-cache
  artifact helpers.
- [`v2/crates/cove-validate`](./v2/crates/cove-validate): validation CLI.
- [`v2/crates/cove-inspect`](./v2/crates/cove-inspect): readable file layout
  inspection.
- [`v2/crates/cove-dump`](./v2/crates/cove-dump): metadata and section byte
  dumping for debugging.
- [`v2/crates/cove-convert-parquet`](./v2/crates/cove-convert-parquet):
  reference Parquet-to-COVE conversion CLI.
- [`v2/crates/cove-conformance`](./v2/crates/cove-conformance): conformance
  runner and generated capability matrix support.
- [`v2/conformance`](./v2/conformance): generated accept/reject corpus,
  manifest, and capability matrix.
- [`v2/docs/performance/datafusion-benchmark-report.md`](./v2/docs/performance/datafusion-benchmark-report.md):
  current local DataFusion benchmark methodology and results.

## Benchmark Snapshot

The current benchmark report compares COVE through this repository's DataFusion
adapter with DataFusion's native Parquet path. These are local synthetic
fixtures, not universal file-format claims and not object-store measurements.
Ratio is `COVE / Parquet`; lower than `1.00x` means COVE was faster in that
run.

Selected M7 execute-only results:

| Query | COVE | Parquet | Ratio |
| --- | ---: | ---: | ---: |
| `operational_point_lookup` | 288.69 us | 1.565 ms | 0.18x |
| `operational_zero_match` | 22.93 us | 120.17 us | 0.19x |
| `join_selective_dimensions` | 900.47 us | 4.467 ms | 0.20x |
| `join_left_stocked_products` | 455.84 us | 710.74 us | 0.64x |
| `olap_top_customers` | 1.460 ms | 3.027 ms | 0.48x |

Selected M6 COVE-vs-Parquet results:

| Track | COVE | Parquet | Ratio |
| --- | ---: | ---: | ---: |
| `parquet_compare_full_scan` | 227.88 us | 273.39 us | 0.83x |
| `parquet_compare_projection_scan` | 280.25 us | 272.97 us | 1.03x |
| `parquet_compare_low_cardinality_filter` | 234.88 us | 628.14 us | 0.37x |
| `parquet_compare_numeric_range_filter` | 294.33 us | 605.80 us | 0.49x |
| `parquet_compare_wide_projection_filter` | 247.02 us | 607.07 us | 0.41x |

The same report also records cases where Parquet is faster, including the M6
scan-heavy projection path and some OLAP grouping/top-customer full-query
tracks. Planning overhead is visible in the full-query numbers. FileCode
dictionary output is currently opt-in and should not be assumed to be faster
than decoded output.

## Getting Started

Run commands from the v2 workspace:

```sh
cd v2
cargo test --workspace
```

Run the v2 release gate:

```sh
sh scripts/release-gates.sh
```

Run the conformance corpus directly:

```sh
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```

Register a local `.cove` file with DataFusion:

```rust
use cove_datafusion::register::register_cove_file;
use datafusion::prelude::SessionContext;

let ctx = SessionContext::new();
register_cove_file(&ctx, "orders", "orders.cove")?;

let df = ctx
    .sql("SELECT * FROM orders WHERE status = 'active'")
    .await?;
df.show().await?;
```

Run the benchmark suites:

```sh
cargo bench -p cove-datafusion --features parquet-compare --bench m6 -- --noplot
cargo bench -p cove-datafusion --features parquet-compare --bench m7_sql_mix -- --noplot
```

For a faster compile-and-smoke pass:

```sh
cargo bench -p cove-datafusion --features parquet-compare --bench m6 -- --sample-size 10 --warm-up-time 0.1 --measurement-time 0.1
```

## Design Principles

- **Immutable files**: COVE files are write-once-read-many artifacts, not
  mutable database pages.
- **Portable logical truth**: canonical values, nulls, dictionaries, schemas,
  sections, checksums, and validated authoritative metadata define the file.
- **Proofs before pruning**: metadata used to skip data must be conservative,
  validated, and scoped to the requested operation.
- **Fail open for optimization, fail closed for corruption**: unsupported or
  insufficient acceleration falls back to scanning; structural corruption
  rejects.
- **Engine-local execution**: engines may map FileCodes into native runtime
  codes, but those codes are not persisted as portable COVE truth.
- **Subordinate acceleration**: sidecars, manifests, caches, layout plans,
  secondary indexes, and runtime hints can improve reads but must not change
  logical results.
- **Profile-scoped adoption**: readers should reject only the unsupported
  required features that intersect the operation they are actually performing.

## What COVE Is Not

COVE is not a universal Parquet replacement, a WAL, a mutable database file, a
row-level delete protocol, a lakehouse catalog, a lakehouse transaction layer, an
access-control system, an encryption standard, an Arrow IPC replacement, or a
mandatory semantic mapping system.

Parquet and ORC remain mature general-purpose lakehouse formats. COVE is aimed
at immutable archives, converted datasets, object-store planning, predicate-heavy
reads, metadata-answerable queries, deterministic semantic mapping, and engines
that can exploit encoded execution and proof-safe pruning.
