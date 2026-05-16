# COVE v2.0 Feature-Bit Registry

The COVE v2.0 feature-bit registry assigns bits by feature scope: core file features, scan/table features, object/profile features, COVE-MAP features, and sidecar features each maintain their own namespace. Feature-scope isolation prevents a bit allocated for one section family from changing the interpretation of another family.

Registry updates must document the bit name, scope, required/optional behavior, validation impact, reader fallback, and conformance vectors. Required feature bits need accept and reject fixtures. Optional feature bits need at least inspect/report coverage proving extension fallback behavior for readers that do not implement the feature.

Current v2 publication gates require the conformance command set:

```sh
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```
