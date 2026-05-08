# Cove Format

A query-optimized open specification for table-based and object-based data.

Cove Format (COVE: Canonical Offline Value Encoding) is an open, immutable archive format for storing portable logical values and encoded arrays so query engines can read and reason about data efficiently.

The simplest way to think about it is:

- **like Parquet**, it is a portable offline file format for structured data
- **unlike a generic columnar format**, it is designed from the start around what query engines need to do at read time
- it supports both **table-style analytics** and **object/history-oriented data models**

COVE is intended for converted datasets, archives, object-store workloads, and engine-facing storage where pruning, lookups, metadata-driven planning, and efficient execution matter.

## What the project is trying to do

Cove Format aims to define a shared spec for:

- **table-based data** that can be scanned and filtered efficiently
- **object-based data** including richer object and temporal/history-oriented layouts
- **query-engine-friendly storage** with dictionaries, encoded arrays, section metadata, checksums, and optional acceleration artifacts
- **engine-neutral interchange** where logical values stay portable while engines remain free to map them into their own execution model

In short: COVE is trying to be an **open spec for queryable offline data**, especially where engine optimization is a first-class concern.

## Why it exists

Many existing formats are good at interchange or storage efficiency, but do not always expose enough structure for engines to plan and execute queries as directly as they could.

COVE is designed to help engines:

- skip irrelevant data earlier
- answer more from metadata
- use dictionary/code-oriented execution efficiently
- attach optional sidecar acceleration without changing the meaning of the source file
- work well with immutable archive and object-store patterns

## Core ideas

- **Immutable / write-once-read-many**: files are meant to be durable offline artifacts
- **Open specification**: the format is defined by the spec in this repository
- **Engine-oriented design**: physical layout is shaped to help readers and query engines
- **Portable logical values**: files store portable data, not engine-private runtime identities
- **Profiles and extensions**: the format covers core layout plus table, archive, execution, and object-temporal profiles

## Repository contents

- `Spec.md`: the main Cove Format specification
- `crates/cove-core`: core format primitives, staged validation, a minimal writer, and an early COVE-T scan-profile writer surface
- `crates/cove-arrow`: Arrow schema/export/import and Parquet conversion interop layered on top of `cove-core`
- `crates/cove-datafusion`: DataFusion integration scaffold and version-adapter boundary for future COVE query execution
- `crates/cove-validate`: validates COVE files (headers, footers, section CRCs, feature consistency, and optional semantic/profile checks)
- `crates/cove-inspect`: prints a readable layout summary for COVE files
- `crates/cove-dump`: dumps metadata or section bytes as hex for debugging
- `crates/cove-convert-parquet`: converts supported Parquet files into COVE-T scan-profile files and can emit a conversion report
- `conformance`: generated capability matrix plus whole-file, artifact, and parser-focused accept/reject fixtures

## Implementation status

The repository tracks support with evidence, not a single yes/no claim. See
[`conformance/capability_matrix.md`](./conformance/capability_matrix.md) for
the current status of each spec area across these columns:

- modeled: a type, enum, helper, or design scaffold exists
- parsed: the wire form is read from bytes
- validated: structural or semantic invariants are enforced
- written: a writer can emit the structure
- corpus: conformance fixtures exercise the behavior through the runner

Some areas are intentionally marked as partial or scaffold-only. Parquet
conversion now has a supported reference path for non-null primitive, temporal,
UTF-8, binary, and decimal128 columns, plus a standalone CLI; broader nested and
dictionary-synthesis policies remain visible follow-on areas in the capability
matrix.

Before making or publishing compliance claims, run the release gate:

```sh
sh scripts/release-gates.sh
```

The gate checks formatting, the workspace tests, generated-corpus freshness,
capability-matrix freshness, and the full conformance corpus.

For DataFusion M6 performance work, keep the release gate fast and run the
Criterion suite separately:

```sh
cargo bench -p cove-datafusion --bench m6
```

A short compile-and-smoke profile is useful before full measurement runs:

```sh
cargo bench -p cove-datafusion --bench m6 -- --sample-size 10 --warm-up-time 0.1 --measurement-time 0.1
```

For cross-format DataFusion comparisons on matched COVE and Parquet fixtures,
enable the optional Parquet bench track:

```sh
cargo bench -p cove-datafusion --features parquet-compare --bench m6 parquet_compare
```

A short smoke run for that compare track is:

```sh
cargo bench -p cove-datafusion --features parquet-compare --bench m6 parquet_compare -- --sample-size 10 --warm-up-time 0.1 --measurement-time 0.1
```

For string-heavy local-file scans, the compare track includes the
`standard-strict`, `standard-trusted`, `standard-strict-mmap`, and
`standard-trusted-mmap` COVE variants. Treat Arrow view output as a separate
measurement target rather than an assumed win on those workloads.

Local COVE scans now default to mmap-backed reads. Switch back to positioned
reads only when the file may be concurrently replaced, truncated, or modified.

The compare track now includes heavier `scan_heavy` and `cold_context`
benchmarks on larger matched fixtures. Those are intended for targeted
regression checks rather than every edit-loop run. To focus on one heavier
track:

```sh
cargo bench -p cove-datafusion --features parquet-compare --bench m6 parquet_compare_scan_heavy_full_scan
cargo bench -p cove-datafusion --features parquet-compare --bench m6 parquet_compare_cold_context_full_scan
```

For Instruments profiling on macOS, use the repo wrapper instead of profiling
`cargo` itself. It builds the `m6` bench binary with symbols and can either
launch a Criterion benchmark directly or run a dedicated attach-based query
profiler that keeps fixture setup out of the trace window.

Criterion mode:

```sh
python3 scripts/profile_datafusion_bench.py --track scan-heavy-full-scan --engine cove
python3 scripts/profile_datafusion_bench.py --track cold-context-full-scan --engine parquet
```

Attach after setup, then profile only the hot loop:

```sh
python3 scripts/profile_datafusion_bench.py --runner attached-query --stage execute-only --track scan-heavy-full-scan --engine cove
python3 scripts/profile_datafusion_bench.py --runner attached-query --stage planning-only --track scan-heavy-full-scan --engine parquet
```

The script defaults to `Time Profiler` and writes traces under
`artifacts/instruments/`.

## Read the spec

The canonical description of the format lives in [`Spec.md`](./Spec.md).
