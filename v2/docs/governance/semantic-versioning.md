# COVE v2.0 Semantic Versioning

COVE v2.0 uses semantic versioning for the public file-format surface, conformance suite, and reference implementation packages. Patch releases may add tests, clarify diagnostics, and fix implementation behavior without changing accepted wire layouts. Minor releases may add optional sections, optional feature bits, and optional encodings when readers can apply the extension fallback policy. Major releases are reserved for incompatible wire-format changes.

Feature-scope rules are local to the file, section, or declared profile that owns the feature bit. A reader must not treat a local feature bit as global unless the section explicitly declares that binding. Required unknown features fail validation; optional unknown features follow extension fallback and remain available to inspect/report tools.

Release conformance commands:

```sh
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```
