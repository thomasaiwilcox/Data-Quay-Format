# COVE v2.0 Security And Privacy Model

COVE v2.0 treats files and sidecars as untrusted input. Readers validate lengths, offsets, checksums, feature-scope declarations, encoding kinds, dictionary references, and optional extension fallback before exposing semantic data. Implementations must avoid panics on malformed artifacts and return structured validation errors.

Privacy-sensitive COVE-O and COVE-MAP data can contain redaction policy, evidence, lineage, and private payload references. Inspect and dump tools must preserve redaction placeholders and avoid leaking suppressed values. Optional private extensions use extension fallback and must remain reportable without requiring semantic decoding.

Security-sensitive changes require the conformance command set:

```sh
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```
