# COVE v2.0 Benchmark Methodology

COVE v2.0 public benchmarks use deterministic generated corpora for CI, standard, and publication profiles. The public corpus includes scan/filter workloads, conversion cost, ORC/Parquet comparison, indexes, coverage cache behavior, COVE-MAP semantics, canonicalisation vectors, negative corrupt vectors, and an offline object-store harness.

The object-store harness records object GETs, range GETs, bytes requested, bytes returned, cold/warm cache state, and coalescing decisions. It is hermetic object-store semantics, not live S3 or MinIO performance. Live object-store publication evidence can be added later without replacing this deterministic gate.

Benchmark artifacts follow COVE v2.0 feature-scope rules and extension fallback policy. The conformance command set remains:

```sh
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```
