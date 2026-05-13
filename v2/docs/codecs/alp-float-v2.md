# COVE ALP-Style Float Codec v2

Stable descriptor identity: `org.coveformat.codec.alp-float.v2`.

This is a COVE-owned exact Float32/Float64 codec. It is inspired by ALP-style floating-point compression but does not claim byte compatibility with external ALP formats. The reference bitstream begins with `CAF2`, followed by row count, null bitmap length, row-offset table length, LSB0 null bitmap, little-endian row offsets, and exact IEEE payload bytes.

Finite values MAY be encoded by decimal-scaled integer parameters in future compatible profiles, but the v2 reference bitstream preserves signed zero, infinities, and NaN payload policy through exact bytes. Fallback equivalence is mandatory.
