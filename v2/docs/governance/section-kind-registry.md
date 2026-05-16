# COVE v2.0 Section-Kind Registry

The COVE v2.0 section-kind registry records every stable section identifier, owning profile, requiredness rules, payload encoding, checksum policy, and compatibility behavior. New section kinds must include a normative parser, validation fixture, inspect summary, dump or report behavior where applicable, and a release-gate command when the section is publication-critical.

Section identifiers are globally stable, but feature-scope rules still apply to optional semantics inside each section. Unknown optional sections are skipped through extension fallback. Unknown required sections fail validation with a structured error.

The registry is checked by the conformance suite through:

```sh
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```
