# COVE FastLanes-Style Integer Codec v2

Stable descriptor identity: `org.coveformat.codec.fastlanes-integer.v2`.

This is a COVE-owned integer/date/timestamp/decimal codec inspired by FastLanes-style block encoding. It does not claim byte compatibility with external FastLanes formats. The reference bitstream begins with `CFI2`, followed by row count, null bitmap length, row-offset table length, LSB0 null bitmap, little-endian row offsets, and encoded value bytes.

The broad v2 reference contract is exact logical equivalence against fallback payloads. Block-oriented modes are fixed to 1024 logical values except the final partial block when writers choose mode-specific parameter payloads.
