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
- `crates/cove-validate`: validates COVE files (headers, footers, section CRCs, feature consistency, and optional semantic/profile checks)
- `crates/cove-inspect`: prints a readable layout summary for COVE files
- `crates/cove-dump`: dumps metadata or section bytes as hex for debugging
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

Some areas are intentionally marked as partial or scaffold-only. In particular,
Parquet conversion is currently a design surface, not a working converter, and
higher writer profiles still need full writer-emitted page-payload coverage
before they should be described as complete.

Before making or publishing compliance claims, run the release gate:

```sh
sh scripts/release-gates.sh
```

The gate checks formatting, the workspace tests, generated-corpus freshness,
capability-matrix freshness, and the full conformance corpus.

## Read the spec

The canonical description of the format lives in [`Spec.md`](./Spec.md).
