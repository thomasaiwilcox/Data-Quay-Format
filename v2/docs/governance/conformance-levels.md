# COVE v2.0 Conformance Levels

COVE v2.0 conformance levels describe how much of the format an implementation supports:

- Level 0 validates the core envelope and rejects malformed required sections.
- Level 1 reads COVE-T scan profile files and reports optional metadata through extension fallback.
- Level 2 adds write support, conversion reports, indexes, coverage metadata, and COVE-O/COVE-MAP surfaces.
- Level 3 is publication-grade reference behavior, including release gates, benchmark manifests, and governance checks.

Feature-scope rules are part of every level. Unknown required local features fail. Unknown optional local features remain visible through inspect/report fallback.

Required conformance command set:

```sh
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```
