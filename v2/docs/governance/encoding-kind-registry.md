# COVE v2.0 Encoding-Kind Registry

COVE v2.0 encoding kinds define the physical representation used by table pages, object properties, dictionaries, maps, and sidecar payloads. Registry entries include the numeric kind, logical type compatibility, null handling, canonicalization requirements, compression interaction, and exact validation errors for malformed payloads.

Required unknown encodings fail validation. Optional unknown encodings use extension fallback where the containing profile allows a skip, report, or preserved opaque payload. Feature-scope rules decide whether an encoding requirement is local to a section, column, page, or object property.

Conformance command set:

```sh
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```
