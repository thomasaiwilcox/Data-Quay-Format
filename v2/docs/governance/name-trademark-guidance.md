# COVE v2.0 Name And Trademark Guidance

COVE v2.0 names should identify artifacts and profiles without implying certification unless the implementation runs the required conformance command set and publishes the tested version. Reference-compatible packages should state the supported conformance level, optional feature bits, and any extension fallback limitations.

Third-party extensions should use namespaced identifiers. They must not reuse registered COVE section kinds, encoding kinds, or feature-scope bits without an accepted registry update. Documentation should distinguish deterministic reference benchmarks from live service claims.

Publication claims should cite:

```sh
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```
