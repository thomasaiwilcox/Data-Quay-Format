# Data-Quay-Format

A query-optimized open specification for table-based and object-based data.

Data Quay Format (Quay Format / QF) is an open, immutable data format for storing data so that query engines can read and reason about it efficiently.

The simplest way to think about it is:

- **like Parquet**, it is a portable offline file format for structured data
- **unlike a generic columnar format**, it is designed from the start around what query engines need to do at read time
- it supports both **table-style analytics** and **object/history-oriented data models**

QF is intended for converted datasets, archives, object-store workloads, and engine-facing storage where pruning, lookups, metadata-driven planning, and efficient execution matter.

## What the project is trying to do

Quay Format aims to define a shared spec for:

- **table-based data** that can be scanned and filtered efficiently
- **object-based data** including richer object and temporal/history-oriented layouts
- **query-engine-friendly storage** with dictionaries, encoded arrays, section metadata, checksums, and optional acceleration artifacts
- **engine-neutral interchange** where logical values stay portable while engines remain free to map them into their own execution model

In short: QF is trying to be an **open spec for queryable offline data**, especially where engine optimisation is a first-class concern.

## Why it exists

Many existing formats are good at interchange or storage efficiency, but do not always expose enough structure for engines to plan and execute queries as directly as they could.

QF is designed to help engines:

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

- `Spec.md`: the main Quay Format specification
- `crates/qf-core`: core format primitives and a minimal writer
- `crates/qf-validate`: validates QF files
- `crates/qf-inspect`: prints a readable layout summary for QF files
- `crates/qf-dump`: dumps metadata or section bytes as hex for debugging

## Included reference crates

- `qf-core`: core format primitives and minimal writer.
- `qf-validate`: validates QF files (headers, footers, section CRCs, and feature consistency).
- `qf-inspect`: prints a readable layout summary for QF files.
- `qf-dump`: dumps metadata or section bytes as hex for debugging.

## Read the spec

The canonical description of the format lives in [`Spec.md`](./Spec.md).
