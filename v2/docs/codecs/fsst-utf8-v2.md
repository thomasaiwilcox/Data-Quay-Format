# COVE FSST-Style UTF-8 Codec v2

Stable descriptor identity: `org.coveformat.codec.fsst-utf8.v2`.

This is a COVE-owned byte/UTF-8 string codec. It is inspired by FSST-style string tokenisation but does not claim byte compatibility with any external FSST implementation. The reference bitstream begins with `CFS2`, followed by row count, null bitmap length, row-offset table length, LSB0 null bitmap, little-endian row offsets, and concatenated UTF-8 bytes.

All non-null values MUST validate as UTF-8. The decoded logical values and null positions MUST match the validated fallback payload exactly.
