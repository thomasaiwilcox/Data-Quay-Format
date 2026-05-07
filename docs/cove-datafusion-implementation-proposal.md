# COVE DataFusion Implementation Proposal

Status: accepted implementation plan

Audience: maintainers implementing native DataFusion support for COVE

This document supersedes the old root-level `datafusionproposal.md` as the implementation reference. The old proposal remains useful as background, but this document is the version intended to drive actual work in the current repository.

## 1. Purpose

Build a `cove-datafusion` integration that exposes COVE files and datasets to DataFusion while preserving COVE's existing semantics and exploiting as much of the format as possible for performance.

The integration must:

- remain spec-aligned with the current COVE format;
- avoid introducing any new normative DataFusion-specific wire profile;
- use the existing COVE feature surface, especially FileCode, COVE-E, COVM, COVX, Arrow interop, pruning metadata, and optional indexes;
- follow strict data-oriented design and mechanical-sympathy principles so the hot path is built around cache locality, bounded allocations, predictable branches, and contiguous I/O.

## 2. Design Decisions

### 2.1 No DataFusion-specific COVE profile

There will be no `COVE-DF` profile and no DataFusion-specific required feature bit.

DataFusion support is an engine integration, not a new wire-format feature. The existing format surface is already sufficient:

- COVE-T provides scan semantics and typed column data.
- COVE-E provides FileCode to engine-local execution mappings.
- Spec-defined Arrow interop provides Arrow-facing output rules.
- COVX provides rebuildable accelerator sidecars.
- COVM provides dataset-level advisory planning metadata.

DataFusion may publish a non-normative COVE-E example profile such as:

- `namespace = "org.apache.datafusion"`
- `profile_name = "arrow-dictionary"`
- `ExecutionCodeKind = DictionaryKey`
- `FileCodeMappingKind = MapToArrowDictionary`

That is an example engine registration, not a required COVE profile and not a new feature bit.

### 2.2 Current repository is the starting point

Implementation will build on the current `cove-core` surface, not on a greenfield crate tree.

The current repo already contains the major foundations required for this work:

- wire parsing and validation in `cove-core`;
- predicate outcome logic in `cove-core::predicate`;
- pruning evidence and pruning helpers in `cove-core::pruning`;
- execution-mapping metadata in `cove-core::profile::cove_e` and mount-time representation helpers in `cove-core::mount`;
- Arrow export in `cove-core::interop::arrow`;
- COVM and COVX artifacts in `cove-core::artifact::{covm,covx}`;
- redaction metadata in `cove-core::redaction`;
- a full conformance matrix already reporting the implemented v1 capability surface.

### 2.3 Performance is a first-class requirement

The implementation is not allowed to treat performance as a later cleanup phase.

The source must be designed from the beginning around:

- immutable, compact metadata state;
- structure-of-arrays layout for hot planning data;
- integer ids and offsets in the hot path rather than strings and maps;
- late materialization;
- code-based predicate evaluation for dictionary-backed columns where possible;
- bounded, reusable decode buffers;
- range-coalesced object-store reads;
- minimal cross-thread sharing;
- zero trust in optional metadata for correctness.

## 3. Non-Negotiable Invariants

The implementation must obey the following invariants.

- Unknown required COVE file feature bits are always rejected. This is not configurable.
- Unknown or unsupported required engine profiles are rejected only when the requested operation or output mode requires that profile.
- Unknown optional metadata is ignored unless the requested operation explicitly requires it.
- `TableProviderFilterPushDown::Exact` is reported only when the scan path itself applies the full row predicate with SQL-correct semantics.
- Metadata-only pruning without full row predicate evaluation is always `Inexact`.
- COVM and COVX are advisory only. They may accelerate planning or pruning, but they never change logical results.
- External catalog or snapshot systems remain authoritative for file visibility and delete semantics.
- Redacted values are not treated as ordinary nulls.
- `cove-core` must not depend on DataFusion.
- The Arrow boundary must be isolated from `cove-core` over time.

### 3.1 Correctness-first fallback ladder

Every optimization must have a correctness-preserving fallback.

If an optimization cannot prove safety, the implementation falls back to a slower path rather than failing the query, except for:

- structural corruption;
- unsupported required COVE file features;
- unsupported required engine profiles needed by the requested operation;
- explicit redaction policy violations.

Representative fallback ladders:

- code-based dictionary predicate -> decoded dictionary-value predicate -> residual Arrow/DataFusion filter;
- selected-row decode -> full morsel decode -> full segment or file scan;
- COVX or COVM acceleration -> direct host-file metadata -> direct scan.

## 4. Current Baseline and Hard Prerequisite

### 4.1 Current baseline

Today the workspace has no `cove-datafusion` crate, and `cove-core` still depends directly on Arrow 54 and Parquet 54.

That matters because DataFusion 53.x is on Arrow 58. The first implementation risk is therefore not query planning; it is the dependency boundary between the existing repo and DataFusion.

### 4.2 Hard prerequisite: solve the Arrow seam first

No mergeable `cove-datafusion` crate should land on `main` until the Arrow compatibility seam is handled.

Pure planner design, `DatasetState` modeling, predicate lowering design, benchmark-fixture work, and throwaway adapter spikes may proceed in parallel.

Recommended sequence:

1. Add a new `cove-arrow` crate.
2. Copy or move Arrow-facing interop and export code out of `cove-core` into `cove-arrow`.
3. Update internal callers to use `cove-arrow` directly.
4. Choose one migration path:
  - remove `cove_core::interop::arrow` in the same breaking change; or
  - keep a deprecated duplicate compatibility module temporarily inside `cove-core`, upgraded to Arrow 58, and remove it after downstream migration.
5. Do not make `cove-core` depend on `cove-arrow`, and do not re-export `cove-arrow` APIs from `cove-core` if doing so would create a dependency cycle.
6. Upgrade Arrow consumers to Arrow 58.
7. Add `cove-datafusion` on top of `cove-core` and `cove-arrow`.

The desired steady state is:

- `cove-core`: spec, wire, validation, pruning, mount, artifacts, indexes, execution mapping metadata;
- `cove-arrow`: Arrow schema/export/import and Arrow-specific builders;
- `cove-datafusion`: DataFusion planning, execution, registration, metrics, explain, and version adapters.

Immediate split of `cove-reader` or `cove-writer` is not required for this effort. Those can remain logical modules inside `cove-core` unless later maintenance pressure justifies a separate crate split.

M0 should be treated as real engineering work, not a quick refactor. Arrow export is currently coupled to `EncodedArray`, dictionary types, `CoveError`, and current repo workflows such as Parquet conversion.

## 5. Target Package Architecture

The target package graph is:

```text
cove-core
  -> pure COVE semantics, validation, artifacts, indexes, mount, pruning

cove-arrow
  -> Arrow schema/export/import and Arrow-facing type adaptation

cove-datafusion
  -> DataFusion registration, planning, execution, metrics, explain
```

`cove-datafusion` should be introduced with a narrow module surface rather than a speculative file tree.

Recommended modules:

- `register`: public registration helpers and thin DataFusion session glue.
- `planner`: DataFusion-agnostic scan planning.
- `bootstrap`: footer and dataset bootstrap logic.
- `dataset_state`: immutable dataset metadata state.
- `expr_lowering`: boundary translator from DataFusion expressions to COVE-native predicate programs.
- `prune`: DataFusion-agnostic candidate pruning.
- `task_graph`: DataFusion-agnostic scan task generation.
- `decode`: decode and materialization kernels, DataFusion-agnostic where practical.
- `adapter_v53`: DataFusion 53.x trait and type shims.

The `adapter_v53` module owns the direct DataFusion-facing implementations:

- `table_provider`;
- `file_format`;
- `file_source`;
- `file_opener`;
- `exec`;
- `stream`;
- `statistics`;
- `metrics`;
- `explain`.

### 5.1 Version policy

Initial supported versions are pinned:

- `datafusion = 53.1.x`
- `arrow = 58.x`

Only `adapter_v53` and `register` may import DataFusion traits directly.

Required CI coverage:

- pinned `datafusion = 53.1.x`;
- pinned `arrow = 58.x`;
- adapter-layer compile coverage;
- no unscoped DataFusion trait imports outside `adapter_v53` and `register`.

Later DataFusion upgrades should add new adapter modules only after the 53.x line is stable.

### 5.2 Feature flags

Recommended crate features:

```toml
[features]
default = ["native", "compat"]
native = []
compat = []
covm = []
covx = []
dynamic-filters = []
parquet-compare = []
```

`dynamic-filters` should remain off initially.

## 6. Two Execution Modes

The final implementation should support two modes that share the same lower-level planner and scan kernel.

### 6.1 Compatibility mode

Compatibility mode is the smallest shippable DataFusion path.

It uses DataFusion's file-format integration path to support:

- single-file registration;
- directory scans of `.cove` files;
- `CREATE EXTERNAL TABLE` or equivalent registration flows;
- basic projection pushdown;
- basic filter pushdown classification;
- Arrow output;
- early interoperability testing.

Compatibility mode exists to get COVE working cleanly in existing DataFusion workflows. It is intentionally modest and is not expected to carry every COVE optimization.

FileSource pushdown caveat:

- compatibility mode may return a filter as pushed only when the Cove scan evaluates the full row predicate with SQL-correct semantics;
- pruning-only filters may still be used internally as planning hints, but they must not be reported as pushed unless equivalent residual filtering is guaranteed by the surrounding plan;
- native `TableProvider` mode may expose pruning-only filters as `Inexact`, but FileSource mode has a stricter pushed versus not-pushed contract.

If the FileSource API path cannot safely retain pruning-only filters while preserving residual filtering, compatibility mode should skip pruning-only filter use and leave that optimization to native mode.

### 6.2 Native mode

Native mode is the target architecture.

It uses a custom `TableProvider` and `ExecutionPlan` so that COVE-specific features can be exploited directly:

- COVM-driven dataset bootstrap;
- COVX acceleration;
- file-level pruning before opening all footers;
- lookup indexes and direct candidate routing;
- aggregate synopsis execution paths;
- composite and Top-N pruning;
- query-local execution code mapping via COVE-E;
- external visibility overlays;
- rich explain and performance metrics;
- precise control of ordering, partitioning, fetch, and task generation.

Both modes must share the same bootstrap, predicate lowering, pruning engine, and decode kernels. There should not be two different correctness implementations.

Dynamic filters belong only in native mode initially. They should not be attempted through compatibility mode in the first release line.

## 7. Query Modes to Support

The integration should support four concrete registration modes.

### 7.1 Single COVE file

Register a single `.cove` file directly.

Use cases:

- local inspection;
- benchmarking;
- debugging specific files;
- conformance and regression tests.

### 7.2 Directory or listing of COVE files

Register a directory or explicit list of `.cove` files.

Use cases:

- simple datasets without a `.covm` manifest;
- compatibility mode external tables;
- cold-start interoperability.

### 7.3 COVM dataset

Register a dataset through a `.covm` manifest.

Use COVM for:

- file enumeration;
- basic file fingerprinting;
- row-count and segment-count estimates;
- advisory file-level pruning metadata when present and validated;
- optional sidecar references.

The implementation must only assume fields standardized today. Optional payloads such as partition values, dictionary fingerprints, or extended references are usable only when they are explicitly modeled and validated.

### 7.4 External catalog overlay

Register a COVE-backed dataset controlled by an external catalog or snapshot system.

In this mode:

- the external system chooses visible files;
- COVM may accelerate planning but may not override visibility;
- metadata-only exactness claims are reduced whenever overlay semantics make them uncertain.

## 8. Shared Metadata Model

The core implementation object in `cove-datafusion` should be an immutable `DatasetState` held behind `Arc` and shared by all scans.

### 8.1 Layout rules

Hot planning metadata must be stored in compact, contiguous arrays.

Do not store hot scan metadata as nested trees of heap objects.

Preferred shape:

```rust
// Illustrative only.
struct FileTable {
    file_id: Vec<[u8; 16]>,
    uri: Vec<Arc<str>>,
    file_len: Vec<u64>,
    footer_crc32c: Vec<u32>,
    row_count: Vec<u64>,
    segment_count: Vec<u32>,
    flags: Vec<u32>,
}
```

The same principle applies to zone metadata and index references.

Examples:

- one dense array for row-range starts;
- one dense array for row counts;
- one dense array for null counts;
- one dense array for min and max ranks;
- one dense array for page offsets and lengths;
- one dense array for optional index references.

### 8.2 Candidate representation

Candidate sets should use integer ordinals and adaptive compact containers.

Recommended strategy:

- dense bitmap for large candidate sets with regular ordering;
- sparse `Vec<u32>` or small-set container when selectivity is high;
- explicit conversion only at stage boundaries.

The planner should never carry file names, column names, or schema objects in the hot candidate path.

### 8.3 Cache keys

All footer, dictionary, and sidecar caches should be keyed by the existing COVE fingerprinting model:

- file URI;
- `file_id`;
- `file_len`;
- `footer_crc32c`;
- digest algorithm and digest when present.

This preserves correctness when stale files or stale manifests are encountered.

## 9. Planning Pipeline

Planning must remain lightweight and use only already-available metadata plus small, bounded reads.

Dataset bootstrap should happen during registration, provider construction, or bounded lazy metadata loading. `TableProvider::scan` should normally consume an existing `DatasetState` and produce a `ScanPlan`.

### 9.1 Column planning

The planner must distinguish between output columns, predicate columns, and columns needed only for materialization.

```rust
// Illustrative only.
struct ColumnPlan {
  output_columns: Vec<ColumnId>,
  predicate_columns: Vec<ColumnId>,
  materialization_columns: Vec<ColumnId>,
}
```

Do not overload "projection" to mean every column the scan touches. Filter-only columns may be absent from the final output while still being required internally.

The planning pipeline should be:

1. Bootstrap dataset state from a file tail, footer cache, directory listing, or COVM.
2. Resolve projection to integer column ids.
3. Lower DataFusion expressions into a COVE-specific `PredicateProgram`.
4. Classify each filter as unsupported, inexact, or exact-at-source.
5. Run file-level pruning.
6. Run segment and morsel pruning.
7. Run page-level pruning when metadata is present and safe.
8. Produce a `ScanPlan` and a stable list of `ScanTask`s.
9. Attach residual filter and late-materialization requirements.

Planning must not:

- decode data pages;
- build Arrow arrays;
- eagerly open every file when manifest metadata already excludes them;
- perform broad object-store reads;
- perform large per-query heap construction.

Lifecycle acceptance criteria:

- `TableProvider::scan` must not decode page payloads;
- `TableProvider::scan` must not construct `RecordBatch` output;
- `TableProvider::scan` must not perform long-running object-store scans;
- `TableProvider::scan` may perform small bounded reads only when explicitly configured and when those reads cannot become a broad object-store scan;
- `ExecutionPlan::execute` must construct a stream and return quickly.

## 10. Predicate and Pushdown Model

The shared planner must not expose DataFusion pushdown enums directly. It should expose a COVE-native filter contract first, and each execution mode should map it into its own DataFusion contract.

```rust
// Illustrative only.
enum CoveFilterUse {
  Unsupported,
  PruningOnly,
  FullRowPredicateExact,
}

struct FilterPlan {
  filter_use: CoveFilterUse,
  residual_expr: Option<PhysicalExprRef>,
}
```

The internal COVE pruning engine may continue to use a richer predicate lattice such as `AllMatch`, `NoMatch`, `SomeMatch`, and `Unknown`.

Native `TableProvider` mode maps to DataFusion's logical pushdown states as follows:

| COVE capability | DataFusion pushdown result |
| --- | --- |
| Cannot use predicate safely | `Unsupported` |
| Can only prune candidates | `Inexact` |
| Applies full predicate during scan with SQL-correct semantics | `Exact` |

Rules:

- zone stats alone are `Inexact`;
- exact-set, bloom, lookup, inverted, composite, and Top-N pruning are `Inexact` unless they fully evaluate the row predicate;
- full row predicate evaluation inside the scan may be `Exact`;
- unknown null, collation, NaN, or redaction semantics must never be promoted to `Exact`.

Compatibility FileSource mode maps the same contract more strictly:

- `Unsupported` -> not pushed;
- `PruningOnly` -> not pushed, though it may still be used internally as a planning hint if residual filtering is preserved;
- `FullRowPredicateExact` -> pushed.

This is the central correctness boundary for the DataFusion integration.

## 11. Execution Pipeline

Execution should operate on per-partition local state and avoid global mutable coordination in the hot path.

Recommended execution sequence per partition:

1. Pull the next `ScanTask`.
2. Open or reuse the file handle for that file.
3. Read the smallest contiguous metadata or page ranges needed for the task.
4. Decode predicate columns first.
5. Build a selection bitmap or row index vector.
6. Materialize projected columns only for surviving rows.
7. Apply residual filtering when the pushdown contract is `Inexact`.
8. Emit `RecordBatch` output.

Late materialization is required for performance. The scan should not decode all projected columns before predicate columns have narrowed the row set.

## 12. Feature Coverage Plan

The new implementation should maximize the existing format functionality, not just basic file reading.

### 12.1 FileCode and Arrow dictionary output

Dictionary-backed columns should support three execution shapes:

- decoded logical values;
- Arrow dictionary arrays;
- query-local execution-code mappings when requested and valid.

Equality and `IN` filters on FileCode-backed columns should be lowered to code-based comparisons whenever the literal can be resolved against the file-local dictionary or the active execution mapping.

String comparison in the hot path should be avoided when code comparison is sufficient.

### 12.2 COVE-E execution mapping

COVE-E support should be implemented as a query-local or scan-local optimization layer.

Rules:

- never compare engine-local codes across files unless a valid common scope is proven;
- mapping scope must remain explicit;
- missing or unsupported required engine profiles only fail when the requested operation needs them;
- ordinary scans must still work without COVE-E.

### 12.3 Pruning indexes

The planner should integrate all existing pruning evidence surfaces already present in the repo:

- ColumnDomain and zone stats;
- exact sets;
- bloom filters;
- inverted morsel indexes;
- lookup indexes;
- aggregate synopses;
- composite zone indexes;
- Top-N summaries.

The planner should evaluate these in an order that favors cheap, branch-stable exclusion first.

Recommended order:

1. file-level elimination;
2. cheap zone/domain and null-count checks;
3. exact-set and lookup checks;
4. bloom and inverted checks;
5. aggregate, composite, and Top-N checks;
6. page-level decode decisions.

### 12.4 COVM

COVM should be used aggressively for planning, but only within its advisory contract.

The implementation should use it for:

- dataset bootstrap;
- file enumeration;
- initial file pruning;
- cached statistics;
- warm-path sidecar discovery.

The implementation must ignore stale or unsupported COVM entries and fall back cleanly to direct file planning.

COVM exclusion requires freshness validation. An unvalidated or stale manifest may enumerate candidate files, but it must not be the sole basis for excluding a file unless the referenced file identity has been validated according to `file_id`, `file_len`, `footer_crc32c`, and digest policy.

COVM trust policy should be explicit:

- `Conservative`: COVM may enumerate files, but file exclusion requires a validated or cached file fingerprint.
- `CachedFreshness`: COVM may exclude files whose fingerprint was validated previously and remains cache-valid under the configured policy.
- `ExternalCatalogTrusted`: an external catalog or immutable content-addressed publication mechanism may establish the selected dataset state; COVM still cannot override the external catalog, and opened files must be validated before use.

The default for standalone local development is `Conservative`. The default for production object-store or archive datasets may be `CachedFreshness` or `ExternalCatalogTrusted`, but only when the publication mechanism is explicit.

### 12.5 COVX

COVX should be used as an acceleration layer for precomputed pruning artifacts.

The implementation must validate host-file identity before using it and must degrade to host-file metadata when validation fails.

### 12.6 Aggregate synopsis execution

Metadata-only aggregate answers are allowed only when exactness is proven.

They must be disabled when:

- synopsis accuracy is approximate;
- visibility overlays may hide rows;
- redaction semantics make the answer non-equivalent;
- predicate semantics cannot be proven exactly.

Redaction policy does not disable all aggregates equally. `COUNT(*)` may still be exact when row visibility is unchanged, while `COUNT(redacted_column)`, `MIN/MAX(redacted_column)`, `GROUP BY redacted_column`, and filtered aggregates over a redacted column depend on the active policy.

### 12.7 Redaction

Redaction metadata must remain visible as a semantic boundary.

The DataFusion integration must not silently reinterpret redacted values as ordinary nulls. If a user-facing projection layer later wants placeholder or hidden-value behavior, that is a presentation decision above the storage semantics.

Redaction policy applies not only to projected values but also to statistics, histograms, lookup indexes, exact sets, and Top-N summaries. A value-bearing optimization is disabled whenever the active redaction policy would make its result non-equivalent to the visible query semantics.

### 12.8 Nested columns

Nested columns should initially be decode/export correct through `cove-arrow`. Predicate pushdown for nested columns should be limited to implemented safe cases, such as struct child fields where COVE metadata proves the same semantics. List and map pushdown remain optional and index-dependent.

## 13. External Visibility and Catalog Semantics

When an external catalog controls visibility:

- selected files come from the external system;
- COVM only accelerates the chosen file set;
- exact aggregate answers require overlay-safe proof;
- file- or row-level delete semantics remain outside the COVE file unless explicitly modeled by the overlay.

This rule avoids treating descriptive lakehouse hints as authoritative storage semantics.

## 14. DataFusion API Strategy

Target the DataFusion 53.x API line first, but isolate all version-specific code behind `adapter_v53`.

Rules for the adapter layer:

- keep DataFusion trait signatures and version-specific helper types out of the core planner and decode modules;
- keep `TableProvider`, `ExecutionPlan`, `FileSource`, and `FileOpener` glue thin;
- treat examples in this document as illustrative, not as frozen trait signatures;
- revalidate exact signatures whenever the DataFusion minor version changes.

Trait churn is expected. `adapter_v53` must absorb this instability so the core planner and decode kernels remain DataFusion-agnostic.

### 14.1 Statistics

Expose only safe statistics.

Examples:

- exact row counts may be surfaced as exact;
- COVM-derived byte size estimates are inexact;
- approximate distinct sketches are not exact distinct counts;
- FileCode raw order is not logical order unless a validated domain proves it.

### 14.2 Limit and sort pushdown

Support limit pushdown only when the scan can preserve correctness under the active predicate contract.

Rules:

- limit pushdown is disabled whenever any pushed filter is `Inexact`;
- limit pushdown is allowed only when the scan either has no pushed filter or evaluates the full row predicate exactly before the limit boundary;
- exact sort pushdown means `CoveExec` proves `output_ordering` and may eliminate an upstream sort;
- inexact sort optimization means `CoveExec` reads promising zones first, but the final DataFusion `SortExec` or TopK stage remains.

Support sort pushdown only when the file or dataset ordering is actually proven, not guessed from incidental clustering.

### 14.3 Dynamic filters

Dynamic-filter integration should not block the baseline implementation.

Plan for it, but ship it later behind a feature flag once the base provider path, statistics, pruning, and late materialization are stable.

Dynamic filters are native-mode only initially.

### 14.4 Error mapping

`CoveError` must be mapped to clear `DataFusionError` surfaces.

Recommended mapping:

- structural corruption -> planning-time or execution-time `DataFusionError` with the original COVE error code preserved in the message or structured payload;
- unsupported required feature -> planning-time error;
- unsupported optional acceleration -> metric plus fallback, not query failure;
- redaction policy violation -> query error unless an explicit placeholder or hidden-value policy is configured;
- stale `COVX` or `COVM` -> metric plus fallback, not query failure.

## 15. Mechanical Sympathy Standards

The following standards are mandatory.

### 15.1 Hot-path rules

- no per-row heap allocation;
- no per-row string lookup for dictionary predicates when code comparison is possible;
- no `HashMap` lookup inside inner scan loops;
- no dynamic dispatch in tight decode loops when a monomorphic kernel can be selected earlier;
- no repeated schema or name resolution after planning.

### 15.2 Memory rules

- reuse selection bitmaps, row index vectors, decompress buffers, and Arrow builders per partition;
- prefer fixed-capacity scratch arenas for temporary planning structures;
- bound output batch size to a stable target and reuse capacity;
- keep immutable metadata in read-mostly shared state and mutable execution data in partition-local state.

Arrow validity inversion for COVE null bitmaps should be implemented word-wise over full machine words, with explicit masking of the final partial byte and dedicated tests for padding behavior.

### 15.3 CPU rules

- use code-based comparison for dictionary-backed equality filters;
- use integer ranks for domain-range tests;
- keep branch ordering biased toward the common case of candidate rejection;
- use sequential memory access patterns over pointer chasing;
- use specialized kernels selected once per column plan rather than branching on type for every row.

### 15.4 I/O rules

- coalesce adjacent page reads into a single object-store range request when it reduces round trips;
- avoid opening all files up front;
- preserve sequential reads inside each file as much as possible;
- use manifest and footer metadata to avoid touching files that are already excluded.

### 15.5 Concurrency rules

- partition-local workers own their decode buffers;
- shared state is immutable and reference-counted;
- avoid fine-grained locks in the scan path;
- task size should be chosen to balance parallelism with locality and object-store request overhead.

`spawn_blocking` is not the design center for this implementation. CPU-heavy decode should use a deliberate execution strategy rather than a generic blocking fallback.

## 16. Testing and Benchmarking Plan

This implementation must ship with both correctness and performance gates.

### 16.1 Correctness tests

- DataFusion query results must match direct `cove-core` decode results.
- Exact versus inexact filter behavior must be tested explicitly.
- COVM and COVX stale-state fallback must be covered.
- External overlay visibility restrictions must be covered.
- Redaction behavior must be covered.
- FileCode and execution-code mapping behavior must be covered.
- two-file FileCode equality must be tested explicitly so raw code equality never leaks across files;
- two-file FileCode `GROUP BY` must group by logical values, not raw codes;
- final-byte padding for word-wise null-bitmap inversion must be covered.

### 16.2 Conformance integration

The existing conformance suite should remain the primary wire-format safety net.

`cove-datafusion` tests should consume the same corpus where practical instead of inventing a disconnected fixture set.

### 16.3 Benchmarks

Add benchmark tracks for:

- full scan, cold and warm;
- selective equality on FileCode columns;
- range filters on typed numeric columns;
- point lookup via lookup index;
- aggregate synopsis fast path;
- manifest-pruned multi-file datasets;
- late materialization on wide tables;
- external overlay restricted scans.

Metrics to capture:

- data bytes read;
- metadata bytes read;
- files skipped;
- segments or morsels skipped;
- pages decoded;
- rows decoded;
- rows materialized;
- dictionary lookups performed;
- residual filter rows processed.

### 16.4 Minimum observability contract

Minimum M1 metrics:

- `cove_files_opened`;
- `cove_rows_output`;
- `cove_batches_output`;
- `cove_data_bytes_read`;
- `cove_metadata_bytes_read`.

Minimum M3 metrics:

- `cove_morsels_considered`;
- `cove_morsels_pruned`;
- `cove_pages_decoded`;
- `cove_rows_selected`;
- `cove_rows_materialized`;
- `cove_residual_rows`.

## 17. Implementation Milestones

### M0: Arrow boundary and crate setup

Deliverables:

- add `cove-arrow`;
- move Arrow-specific interop out of `cove-core`;
- upgrade Arrow consumers to 58;
- add `cove-datafusion` scaffold and `adapter_v53`.

Exit criteria:

- workspace builds cleanly on Arrow 58;
- `cove-core` has no direct Arrow or Parquet dependencies;
- `cove-arrow` owns COVE-to-Arrow and Arrow-to-COVE interop APIs;
- `cove-datafusion` may use Arrow types required by DataFusion, but must call `cove-arrow` for COVE Arrow materialization logic;
- conversion crates may depend on Arrow or Parquet directly, but those dependencies must not leak back into `cove-core`;
- `cove-core` does not depend on `cove-arrow` and does not re-export `cove-arrow` APIs in a way that creates a dependency cycle;
- existing Arrow export behavior remains intact;
- `cove-arrow` tests cover null-bitmap inversion, primitive export, FileCode export, redaction failure, and `RecordBatch` construction;
- Arrow- or Parquet-heavy conversion paths are moved out of `cove-core` or explicitly justified as non-core dependencies;
- DataFusion dependency is isolated to `cove-datafusion`.

### M1a: Single-file decoded native scan

Deliverables:

- `cove-datafusion` crate builds and links against the pinned adapter;
- immutable `DatasetState`;
- footer and tail bootstrap helpers;
- native single-file `TableProvider`;
- decoded-value scan path;
- basic statistics and metrics.

Exit criteria:

- `SELECT *` works on a single `.cove` file;
- `EXPLAIN` shows `CoveExec`;
- no residual-filter correctness mistakes;
- `FileCode(0)` behavior is explicitly covered;
- redacted value projection follows policy.

### M1b: Projection and filter classification

Deliverables:

- projection pushdown;
- filter classification;
- value decode and Arrow dictionary output;

Exit criteria:

- projection works on a single `.cove` file;
- filters are classified conservatively;
- filtered query results are correct through residual DataFusion filtering;
- no filter is reported as `Exact` unless the scan path truly evaluates it.

### M2: Compatibility mode and multi-file datasets

Deliverables:

- `FileFormat` path for directory scans;
- file listing support;
- shared planner wired into compatibility mode;
- object-store bootstrap and caching.

Exit criteria:

- DataFusion external-table workflows can query `.cove` datasets.

### M3: Native pruning kernel and late materialization

Deliverables:

- expression lowering;
- ColumnDomain and zone-stat pruning;
- exact-set and bloom pruning;
- selection bitmap production;
- query-local dictionary remap for projected FileCode columns and equality predicates;
- late materialization.

Exit criteria:

- selective filters demonstrably reduce bytes read and pages decoded;
- on selective FileCode equality, `pages_decoded` and `bytes_read` are lower than a full scan;
- results match direct `cove-core` decode plus residual DataFusion filtering.

### M4a: COVM dataset bootstrap and file pruning

Deliverables:

- COVM bootstrap and file pruning;

Exit criteria:

- multi-file datasets can be enumerated and freshness-gated file exclusion works correctly.

### M4b: COVX validation and sidecar loading

Deliverables:

- COVX acceleration;

Exit criteria:

- stale or unsupported sidecars fall back cleanly with metrics.

### M4c: Lookup and inverted indexes

Deliverables:

- lookup and inverted integrations;

Exit criteria:

- point and membership workloads materially reduce candidate rows before decode.

### M4d: Aggregate synopsis, composite, and Top-N paths

Deliverables:

- basic exact aggregate synopsis execution;
- composite pruning;
- Top-N integration.

Exit criteria:

- metadata-only answers are emitted only when exactness is proven;
- inexact Top-N remains semantically correct with a final DataFusion sort stage.

### M4e: External overlay handling

Deliverables:

- external overlay-aware file selection.

Exit criteria:

- overlay-backed datasets preserve visible-row correctness.

### M5: Advanced execution-code policy and metadata-only fast paths

Deliverables:

- execution-code-aware predicate lowering;
- full COVE-E descriptor interpretation and policy handling;
- improved metadata-only aggregate planning, explainability, and dictionary-aware aggregate fast paths;
- richer explain and metrics.

Exit criteria:

- FileCode-heavy workloads avoid unnecessary string work;
- exact metadata-only answers are proven and reported clearly.

### M6: Performance hardening and optional dynamic filters

Deliverables:

- benchmark-driven tuning;
- memory reuse hardening;
- task sizing refinements;
- optional dynamic filter support.

Exit criteria:

- benchmark suite is stable;
- no correctness regressions under more aggressive pushdown.

Dynamic filters remain feature-gated and native-mode only in this milestone.

## 18. Final Target State

The finished implementation should provide:

- clean DataFusion registration for single files, directories, COVM datasets, and overlay-backed datasets;
- full reuse of current `cove-core` semantics and artifacts;
- Arrow dictionary output and value decode paths;
- correctness-safe pushdown classification;
- broad use of COVE pruning and acceleration metadata;
- late materialization and code-based predicate execution where possible;
- explainable metrics and benchmarkable performance.

If a later change improves ergonomics but weakens any of the invariants in this document, the invariants win.
