# Data-Quay-Format

A query-optimized offline data format for table and object-based data.

## Included reference crates

- `qf-core`: core format primitives and minimal writer.
- `qf-validate`: validates QF files (headers, footers, section CRCs, and feature consistency).
- `qf-inspect`: prints a readable layout summary for QF files.
- `qf-dump`: dumps metadata or section bytes as hex for debugging.
