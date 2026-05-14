# COVE v2.0 Extension Proposal Process

COVE v2.0 extensions start as written proposals with a stable name, owning profile, feature-scope declaration, fallback behavior, compatibility notes, and conformance test plan. A proposal must identify whether it adds a section kind, encoding kind, feature bit, execution profile, sidecar, or metadata payload.

Every extension must define extension fallback: what a reader does when it understands the container but not the extension. Optional extensions must preserve inspect/report visibility. Required extensions must fail with a structured error before semantic execution.

Accepted proposals add fixtures and run the command set:

```sh
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```
