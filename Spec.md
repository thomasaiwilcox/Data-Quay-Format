# Cove Format Specification v1.0
**COVE:** Canonical Offline Value Encoding
Cove Format is a Canonical Offline Value Encoding: an immutable,
queryable offline/archive format for portable logical values, encoded
arrays, proof-carrying predicate metadata, optional acceleration artifacts,
and engine-local execution mappings.

| Field | Value |
| --- | --- |
| Format Name | Cove Format |
| Formal Expansion | Canonical Offline Value Encoding |
| Normative Acronym | COVE |
| Public Short Name | Cove |
| Primary Data File Magic | COV1 |
| Footer Magic | COVF |
| Accelerator Sidecar Magic | CVX1 |
| Dataset Manifest Magic | CVM1 |
| Legacy Draft Identifiers | Non-normative pre-COVE draft artifacts; not valid COVE v1 identifiers |
| Canonical Extension | .cove |
| Short Extension | None in v1; do not introduce .cov unless later required |
| Accelerator Sidecar Extension | .covx |
| Dataset Manifest Extension | .covm |
| MIME Type | application/vnd.cove-format |
| Version | 1.0 |
| Byte Order | Little-endian throughout |
| Mutability | Immutable / write-once-read-many |
| Primary Purpose | Engine-neutral queryable offline/archive format with optional engine execution profiles |

---

## 1. Specification Status

This document defines Cove Format v1.0, hereafter COVE.
COVE means Canonical Offline Value Encoding.
**COVE defines the following profiles and companion artifacts:**

- **COVE-Core:** Common immutable file structure, section directory, dictionary, logical/physical types, encoded arrays, checksums, validation, collation, canonical values, and extension rules.
- **COVE-T:** Engine-neutral table-scan profile.
- **COVE-A:** Archive acceleration profile for synopses, lookup indexes, composite pruning, manifests, and sidecar acceleration.
- **COVE-E:** Universal engine execution profile for mapping FileCodes into implementation-local ExecutionCodes.
- **COVE-H:** Harbor registration under COVE-E. Defines Harbor leased-code execution: FileCode -> Harbor EngineCode.
- **COVE-O:** Object-temporal profile for Harbor-style object history, deltas, branches, CSNs, baselines, snapshots, tombstones, and trust chains.
- **COVX:** Optional accelerator sidecar.
- **COVM:** Optional dataset manifest.
A conforming COVE reader MUST be able to validate and read COVE files without COVX or COVM.
COVX and COVM are optional acceleration and planning artifacts. They MUST NOT change the logical meaning of the referenced COVE files.

---

## 2. Normative Language

| Term | Meaning |
| --- | --- |
| MUST | Required for conformance. |
| MUST NOT | Prohibited for conformance. |
| SHOULD | Recommended default; deviations must be deliberate. |
| SHOULD NOT | Not recommended. |
| MAY | Optional. |
| REQUIRED | Same as MUST. |
| OPTIONAL | Same as MAY. |

---

## 3. Purpose

**COVE is an immutable, queryable, encoded archive format designed for:**
- high-performance offline/archive table scans,
- Parquet/ORC/CSV/Arrow-to-COVE conversion,
- object-store-friendly query planning,
- predicate-heavy workloads,
- point lookup and rare-key access,
- metadata-answerable queries,
- engine-local dictionary/code execution,
- Arrow-compatible decoding,
- optional engine-specific execution mappings,
- Harbor leased-code execution through COVE-H,
- optional object-temporal history through COVE-O.
**COVE is not:**
- a WAL,
- a mutable database file,
- an in-flight transaction recovery log,
- a lakehouse catalog replacement,
- an Arrow IPC replacement,
- a generic Parquet clone,
- a format that persists engine-local ExecutionCodes as authoritative logical data.
**COVE’s guiding principle is:**
Store portable logical values and engine-shaped physical data.
Let each engine own its own execution identity at read or mount time.

---

## 4. Public Positioning

**Cove Format should be positioned as:** A Canonical Offline Value Encoding for immutable, queryable archives: encoded arrays, canonical logical values, proof-carrying predicate metadata, optional accelerator sidecars, and direct support for engine-local dictionary execution.

**It should not be positioned as:** A universal Parquet replacement.

**Recommended positioning:**

- **Parquet / ORC:** universal lakehouse interchange and mature analytical columnar storage.
- **COVE:** high-performance queryable archive and converted-table format for engines that can exploit encoded execution, rich pruning metadata, lookup indexes, aggregate synopses, optional sidecars, and direct dictionary/code-vector execution.

---

## 5. Profile Overview

| Profile | Name | Audience | Purpose |
| --- | --- | --- | --- |
| COVE-Core | Core Format | All readers/writers | File layout, sections, dictionary, encodings, checksums, validation. |
| COVE-T | Table Scan Profile | General engines | Engine-neutral columnar table scan profile. |
| COVE-A | Archive Acceleration Profile | Archive/query engines | Synopses, lookup indexes, manifests, composite pruning, sidecars. |
| COVE-E | Engine Execution Profile | All engines | Universal mapping from FileCodes to engine-local ExecutionCodes. |
| COVE-H | Harbor Execution Registration | Harbor | Harbor leased-code implementation of COVE-E. |
| COVE-O | Object Temporal Profile | Harbor and temporal-object engines | Object history, deltas, branches, CSNs, trust chains. |

---

## 6. Core Concepts

### 6.1 FileCode

```rust
type FileCode = u32;
```

A FileCode is a dense file-local dictionary code.
FileCode(0) = dictionary entry 0
FileCode(1) = dictionary entry 1
FileCode(2) = dictionary entry 2
**Rules:**
- FileCode is local to exactly one COVE file.
- FileCode equality is meaningful only within the same COVE file.
- FileCode equality across files has no semantic meaning.
- FileCode MUST NOT be interpreted as an engine execution code.
- FileCode MUST NOT be used as canonical trust-chain input.
- FileCode(0) is a valid ordinary code.
- FileCode(0) MUST NOT be treated as null.
**Cross-file equality requires:**
- resolving FileCodes to canonical logical values, or
- mapping FileCodes to engine-local ExecutionCodes under a shared engine policy.

---

### 6.2 ExecutionCode

ExecutionCode is an implementation-local runtime code.
**Examples:**
**DuckDB:**
  dictionary vector code / internal categorical representation

**DataFusion / Arrow:**
  dictionary array key or implementation-local dictionary key

**Polars:**
  categorical code

**Harbor:**
  leased Harbor EngineCode

**Custom engine:**
  symbol ID, interned value ID, dictionary key, catalog code, etc.
COVE-Core does not define the meaning of an ExecutionCode.

COVE-E defines the universal mechanism for describing execution-code mappings.

**COVE-H defines Harbor’s implementation:**
FileCode -> Harbor EngineCode

---

### 6.3 Harbor EngineCode

**A Harbor EngineCode is:**
- Harbor-owned,
- tenant/code-space scoped,
- lease-policy governed,
- mount/import resolved,
- possibly epoch-dependent,
- not authoritative COVE file data.
COVE files MUST NOT persist Harbor EngineCodes as authoritative data.

---

### 6.4 NumCode

```rust
type NumCode = u64;
```

NumCode stores raw fixed-width numeric bits.

**Rules:**
- NumCode is interpreted by the declared logical type.
- NumCode MUST NOT be dictionary-resolved.
- NumCode(0) is an ordinary value.
- NumCode(0) MUST NOT be treated as null.

---

### 6.5 Scope

A scope describes the logical ownership or execution boundary associated with a file, profile, dictionary, or execution-code mapping.
**Examples:**
- tenant,
- account,
- organisation,
- workspace,
- catalog,
- dataset,
- engine-specific scope.
The core header uses producer_scope_id and producer_scope_kind.

**For Harbor:**
producer_scope_kind = Tenant
producer_scope_id   = Harbor tenant UUID
**For other engines:**
producer_scope_kind = Workspace / Catalog / Dataset / EngineSpecific
producer_scope_id   = implementation-defined stable ID

---

### 6.6 Null

Null is structural.
**Null bitmap convention:**
bit = 1 means null
bit = 0 means non-null
**Rules:**
- FileCode values are never null sentinels.
- NumCode values are never null sentinels.
- Top-level column absence is represented only by the null bitmap.
- Dictionary ValueTag::Null is not a row-null sentinel.
ValueTag::Null is valid for nested canonical values, explicit JSON/list/map nulls, and canonical value representation.

---

### 6.7 Morsel

A morsel is COVE-T’s fundamental scan unit.
**A morsel is the unit of:**
- scheduling,
- predicate bitmap production,
- page pruning,
- late materialisation,
- FileCode -> ExecutionCode remapping,
- vector decode,
- row reference construction.
All columns in a table segment MUST share the same morsel boundaries.

**Default:**
morsel_row_count = 4096

---

## 7. Core Invariants

### 7.1 COVE is immutable

COVE files are write-once-read-many.
- No in-place mutation.
- No append mutation in v1.
- No in-file delete overlays.
- No mutable visibility maps.
- No mutable execution-code maps.
- No mutable lease maps.
Compaction, import, export, and conversion produce new COVE files.

---

### 7.2 COVE is engine-neutral at the core

COVE-Core and COVE-T MUST be readable without Harbor.
**A non-Harbor reader may choose one of two paths:**
**Portable decode path:**
  FileCode -> dictionary value -> normal engine value / Arrow array

**Native execution path:**
  FileCode -> engine-local ExecutionCode -> native vector
COVE-H is Harbor-specific, but it is registered through the universal COVE-E mechanism.

---

### 7.3 Engine profiles do not define logical truth

**INVARIANT:**
  Engine profiles accelerate or adapt execution; they do not define COVE logical truth.
**A COVE file’s logical values are determined by:**
- COVE-Core,
- file dictionary,
- logical types,
- physical streams,
- encoded arrays,
- validated sections,
- canonical value encoding.
**Engine profiles MAY define how those values are mapped into:**
- engine-local runtime codes,
- native vectors,
- caches,
- mount state,
- dictionary arrays.
Engine profiles MUST NOT be required to recover the logical values of a COVE-T file.

---

### 7.4 Pushdown is conservative

A reader MUST NOT skip data unless validated metadata proves no matching row can exist.
**Rules:**
- Missing optional pushdown metadata fails open to scan.
- Corrupt optional pushdown metadata fails open to scan.
- Unknown optional pushdown metadata fails open to scan.
- Structural corruption fails closed by default.
- Bloom filters may produce false positives but MUST NOT produce false negatives.
- Unsafe min/max metadata MUST NOT be used for exclusion.

---

### 7.5 Pushdown may prove exclusion or inclusion

**COVE-T pushdown returns:**

```rust
enum PredicateZoneOutcome {
    DefinitelyNo = 0,
    DefinitelyYes = 1,
    Unknown = 2,
}
```

**Meaning:**
**DefinitelyNo:**
  no row in the zone can satisfy the predicate.

**DefinitelyYes:**
  every row in the zone satisfies the predicate.

**Unknown:**
  metadata cannot prove exclusion or inclusion.
**Rules:**
- Readers MAY skip zones with DefinitelyNo.
- Readers MAY skip predicate-column decoding for zones with DefinitelyYes.
- Readers MUST evaluate Unknown zones normally.

---

### 7.6 Extensions must be ignorable or required

**INVARIANT:**
  Extension data must be either ignorable or required.
**Rules:**
- If an extension is optional, readers that do not understand it MUST be able
  to ignore it without changing query results.
- If an extension is required to decode projected data or preserve semantics,
  the file MUST set the corresponding required feature bit.

---

### 7.7 JSON is descriptive only

Binary metadata is authoritative.
**JSON metadata MUST NOT be the sole authority for:**
- section offsets,
- section lengths,
- checksums,
- schema,
- column layout,
- dictionary identity,
- pushdown statistics,
- required features,
- execution-code mappings.

---

## 8. Primitive Wire Rules

### 8.1 Endianness

All multi-byte integers are little-endian.

### 8.2 Boolean

Boolean fields are encoded as u8.
0 = false
1 = true
Other values are invalid unless explicitly assigned by an enum.

### 8.3 Varint

Unsigned varints use LEB128-style base-128 encoding.
Signed varints use ZigZag encoding before unsigned varint encoding.
zigzag_i64(x) = (x << 1) ^ (x >> 63)

### 8.4 UUID

UUIDs are stored as raw 16-byte canonical UUID byte order.
UUID values MUST NOT be truncated.

### 8.5 Strings

Strings are UTF-8 byte sequences.
Unless a specific collation is declared, string equality is byte equality.

### 8.6 Checksums

**CRC algorithm:**
CRC32C / Castagnoli
CRC fields are computed over the covered byte range with the CRC field itself set to zero if the covered structure contains its own CRC field.
CRC32C is for corruption detection, not cryptographic trust.

### 8.7 Cryptographic Digests

COVE MAY include cryptographic digests.
**Supported digest algorithms in v1:**

```rust
enum DigestAlgorithm {
    None = 0,
    Sha256 = 1,
    Blake3 = 2,
}
```

**Rules:**
- CRC32C is mandatory for structural corruption detection.
- Cryptographic digests are optional but recommended for public archives.
- Digest manifests MAY cover file, section, page, or Merkle scopes.

### 8.8 Alignment

Writers SHOULD align major sections to at least 8 bytes.
Writers SHOULD align scan payloads to 64 bytes where practical.
Object-store profiles MAY use larger alignment.

---

## 9. Top-Level COVE File Layout

```text
┌─────────────────────────────────────────────────────────────┐
│ Header                                                      │
├─────────────────────────────────────────────────────────────┤
│ Data and metadata sections                                  │
│   - file dictionary index                                   │
│   - file dictionary payload                                 │
│   - collation registry                                      │
│   - extension registry                                      │
│   - engine profile registry                                 │
│   - table catalog                                           │
│   - table segment index                                     │
│   - table segment data                                      │
│   - object catalog                                          │
│   - temporal segment index                                  │
│   - temporal segment data                                   │
│   - zone statistics                                         │
│   - exact sets                                              │
│   - bloom filters                                           │
│   - inverted indexes                                        │
│   - lookup indexes                                          │
│   - aggregate synopses                                      │
│   - composite zone indexes                                  │
│   - Top-N summaries                                         │
│   - digest manifests                                        │
│   - trust/redaction manifests                               │
├─────────────────────────────────────────────────────────────┤
│ Footer                                                      │
│   - binary section directory                                │
│   - optional descriptive JSON metadata                      │
├─────────────────────────────────────────────────────────────┤
│ Postscript                                                  │
│ Postscript version                                          │
│ Postscript length                                           │
│ Magic "COV1"                                                │
└─────────────────────────────────────────────────────────────┘
```

The postscript is discovered by reading the tail of the file.
The postscript points to the footer.
The footer contains the authoritative binary section directory.

---

## 10. Header

Rust-style declarations in this document are descriptive pseudocode only.
Readers and writers MUST parse and emit fields explicitly.
Readers and writers MUST NOT transmute or memory-map unvalidated bytes into native structs.

```rust
struct CoveHeaderV1 {
    magic: [u8; 4],              // "COV1"

    header_len: u16,             // 128 for v1
    version_major: u16,          // 1
    version_minor: u16,          // 0

    primary_profile: u8,
    // 0=mixed/unknown
    // 1=COVE-O object temporal
    // 2=COVE-T table scan
    // 3=COVE-A archive acceleration
    // 4=COVE-E engine execution
    // 5=COVE-H Harbor registered execution profile

    endianness: u8,              // 1=little-endian

    flags: u32,

    required_features: u64,
    optional_features: u64,

    file_id: [u8; 16],

    producer_scope_id: [u8; 16],
    producer_scope_kind: u16,

    reserved_scope_flags: u16,

    created_at_us: i64,

    reserved: [u8; 48],          // MUST be zero

    checksum: u32,               // CRC32C of header with this field zeroed
}
```

**Scope kind:**

```rust
enum ProducerScopeKind {
    None = 0,
    Tenant = 1,
    Account = 2,
    Organisation = 3,
    Workspace = 4,
    Catalog = 5,
    Dataset = 6,
    EngineSpecific = 255,
}
```

**Header rules:**
- magic MUST be "COV1" for COVE v1.
- Non-COVE draft magic values are outside the COVE v1 conformance surface.
- header_len MUST be 128 for v1.
- version_major MUST be 1.
- endianness MUST be 1.
- reserved bytes MUST be zero.
- checksum MUST validate before any other header field is trusted.
- unknown required feature bits MUST cause rejection.

---

## 11. Feature Bits

**Feature bits are divided into:**
**required_features:**
  reader must understand these to correctly read required data

**optional_features:**
  reader may ignore these if the associated section is not needed
**Suggested v1 feature assignments:**

| Bit | Name | Meaning |
| --- | --- | --- |
| 0x0000_0000_0000_0001 | FEATURE_OBJECT_PROFILE | File contains COVE-O sections. |
| 0x0000_0000_0000_0002 | FEATURE_TABLE_PROFILE | File contains COVE-T sections. |
| 0x0000_0000_0000_0004 | FEATURE_ARCHIVE_PROFILE | File contains COVE-A sections. |
| 0x0000_0000_0000_0008 | FEATURE_ENGINE_PROFILE | File contains COVE-E sections. |
| 0x0000_0000_0000_0010 | FEATURE_HARBOR_PROFILE | File contains COVE-H Harbor-specific metadata. |
| 0x0000_0000_0000_0020 | FEATURE_FILE_DICTIONARY | File uses FileCode dictionary. |
| 0x0000_0000_0000_0040 | FEATURE_NUMCODES | File contains NumCode columns. |
| 0x0000_0000_0000_0080 | FEATURE_COLUMN_DOMAINS | File contains ColumnDomain sections. |
| 0x0000_0000_0000_0100 | FEATURE_EXACT_SETS | File contains exact set indexes. |
| 0x0000_0000_0000_0200 | FEATURE_BLOOM_FILTERS | File contains bloom indexes. |
| 0x0000_0000_0000_0400 | FEATURE_INVERTED_INDEXES | File contains inverted morsel indexes. |
| 0x0000_0000_0000_0800 | FEATURE_LOOKUP_INDEXES | File contains point lookup indexes. |
| 0x0000_0000_0000_1000 | FEATURE_AGGREGATE_SYNOPSES | File contains aggregate synopsis sections. |
| 0x0000_0000_0000_2000 | FEATURE_COMPOSITE_ZONES | File contains composite zone indexes. |
| 0x0000_0000_0000_4000 | FEATURE_TOPN_SUMMARIES | File contains Top-N zone summaries. |
| 0x0000_0000_0000_8000 | FEATURE_TRUST_CHAIN | File contains trust-chain data. |
| 0x0000_0000_0001_0000 | FEATURE_REDACTIONS | File contains redacted values/audit references. |
| 0x0000_0000_0002_0000 | FEATURE_NESTED_COLUMNS | File contains list/struct/map columns. |
| 0x0000_0000_0004_0000 | FEATURE_DIGEST_MANIFEST | File contains cryptographic digest manifest. |
| 0x0000_0000_0008_0000 | FEATURE_ARROW_INTEROP_HINTS | File contains Arrow mapping hints. |
| 0x0000_0000_0010_0000 | FEATURE_LAKEHOUSE_HINTS | File contains lakehouse integration hints. |
| 0x0000_0000_0020_0000 | FEATURE_EXTENSION_REGISTRY | File contains extension registry. |
| 0x0000_0000_0040_0000 | FEATURE_CODEC_LZ4 | File uses LZ4-compressed payloads. |
| 0x0000_0000_0080_0000 | FEATURE_CODEC_ZSTD | File uses Zstd-compressed payloads. |

**Rules:**
- Readers MUST reject unknown required feature bits.
- Readers MAY ignore unknown optional feature bits.
- Readers MUST NOT use unknown optional metadata for skipping.

---

## 12. Postscript

**The final bytes of every COVE file are:**
[postscript bytes]
[postscript_version: u16]
[postscript_len: u16]
[magic: "COV1"]
**Rules:**
- postscript_len excludes postscript_version, postscript_len, and trailing magic.
- postscript_len MUST be <= 65535.
- Readers SHOULD be able to discover the footer by reading the final 64 KiB.

```rust
struct CovePostscriptV1 {
    required_features: u64,
    optional_features: u64,

    file_len: u64,

    footer: CoveSectionSpecV1,

    checksum: u32,
}
```

```rust
struct CoveSectionSpecV1 {
    offset: u64,
    length: u64,
    uncompressed_length: u64,

    compression: u8,        // 0=None, 1=LZ4, 2=Zstd
    encryption: u8,         // 0=None in v1
    alignment_log2: u8,
    flags: u8,

    crc32c: u32,
    reserved: u32,          // MUST be zero
}
```

**Postscript validation:**
- file_len MUST equal actual file length.
- footer offset/length MUST be within file_len.
- footer CRC32C MUST validate before footer contents are trusted.
- encryption MUST be 0 in v1.

---

## 13. Footer and Section Directory

The footer contains the authoritative section directory.

```rust
struct CoveFooterHeaderV1 {
    footer_magic: [u8; 4],       // "COVF"

    footer_version: u16,         // 1
    header_len: u16,

    section_count: u32,
    section_entry_len: u16,
    flags: u16,

    metadata_len: u32,           // <= 1 MiB

    reserved: [u8; 24],          // MUST be zero
}
```

// followed by:
//   CoveSectionEntryV1[section_count]
//   metadata_json[metadata_len]

```rust
struct CoveSectionEntryV1 {
    section_id: u32,
    section_kind: u16,

    profile: u8,
    // 0=shared
    // 1=COVE-O
    // 2=COVE-T
    // 3=COVE-A
    // 4=COVE-E
    // 5=COVE-H

    flags: u8,

    offset: u64,
    length: u64,
    uncompressed_length: u64,

    item_count: u64,
    row_count: u64,

    compression: u8,
    encryption: u8,
    alignment_log2: u8,
    reserved0: u8,

    required_features: u64,
    optional_features: u64,

    crc32c: u32,
    reserved1: u32,
}
```

**Rules:**
- The binary section directory is authoritative.
- Section offsets and lengths MUST be bounds-checked.
- Every used section MUST have its CRC validated before use.
- Section ranges MUST NOT overlap unless explicitly permitted by section kind.
- Arithmetic overflow MUST be checked.
- JSON metadata MUST NOT override binary metadata.

---

## 14. Section Kinds

| ID | Name | Profile | Purpose |
| --- | --- | --- | --- |
| 1 | FILE_DICTIONARY_INDEX | shared | Fixed dictionary index entries. |
| 2 | FILE_DICTIONARY_PAYLOAD | shared | Variable/large value payloads. |
| 3 | COLLATION_REGISTRY | shared | Collation/canonicalisation registry. |
| 4 | DIGEST_MANIFEST | shared | Cryptographic digests. |
| 5 | REDACTION_MANIFEST | shared | Redaction audit metadata. |
| 6 | ARROW_INTEROP_HINTS | shared | Arrow mapping hints. |
| 7 | LAKEHOUSE_HINTS | shared | Iceberg/Delta/Hudi/catalog hints. |
| 8 | EXTENSION_REGISTRY | shared | Registered custom extensions. |
| 9 | PROFILE_CAPABILITY_MATRIX | shared | Declared profile support. |
| 10 | TABLE_CATALOG | COVE-T | Table schemas. |
| 11 | TABLE_SEGMENT_INDEX | COVE-T | Segment locators and row ranges. |
| 12 | TABLE_SEGMENT_DATA | COVE-T | Table segment payloads. |
| 13 | COLUMN_DOMAIN | COVE-T | Logical ordering for FileCodes. |
| 14 | ZONE_STATS | COVE-T | Segment/morsel/page stats. |
| 15 | EXACT_SET_INDEX | COVE-T/COVE-A | Exact value-set indexes. |
| 16 | BLOOM_INDEX | COVE-T/COVE-A | Bloom filters. |
| 17 | INVERTED_MORSEL_INDEX | COVE-T/COVE-A | Value-to-morsel indexes. |
| 18 | LOOKUP_INDEX | COVE-A | Point lookup indexes. |
| 19 | AGGREGATE_SYNOPSIS | COVE-A | Counts, histograms, sketches. |
| 20 | COMPOSITE_ZONE_INDEX | COVE-A | Multi-column pruning metadata. |
| 21 | TOPN_ZONE_SUMMARY | COVE-A | Top/bottom zone summaries. |
| 22 | KERNEL_CAPABILITIES | COVE-T/COVE-A | Encoded-kernel capability metadata. |
| 30 | ENGINE_PROFILE_REGISTRY | COVE-E | Registered engine execution profiles. |
| 31 | EXECUTION_CODE_DESCRIPTOR | COVE-E | ExecutionCode description. |
| 32 | EXECUTION_SCOPE_DESCRIPTOR | COVE-E | Execution scope metadata. |
| 33 | CODE_SPACE_DESCRIPTOR | COVE-E | Code-space metadata. |
| 34 | ENGINE_MOUNT_POLICY | COVE-E | Generic mount/execution mapping policy. |
| 40 | OBJECT_TYPE_CATALOG | COVE-O | Object/property catalog. |
| 41 | TEMPORAL_SEGMENT_INDEX | COVE-O | Temporal segment locators. |
| 42 | TEMPORAL_SEGMENT_DATA | COVE-O | Temporal segment payloads. |
| 43 | TEMPORAL_BLOOM_INDEX | COVE-O | Scope/branch/GOID/time bloom filters. |
| 44 | TRUST_MANIFEST | COVE-O | Trust-chain metadata. |
| 50 | HARBOR_MOUNT_HINTS | COVE-H | Harbor-specific lease/mount hints. |
| 255 | VENDOR_EXTENSION | shared | Reserved extension section. |

---

## 15. Metadata JSON

Footer metadata JSON is optional, descriptive, and non-authoritative.
**Example:**

```json
{
  "format_version": "1.0",
  "format_name": "Cove Format",
  "created_by": "cove-writer/<version>",
  "created_at_us": 0,
  "primary_profile": "COVE-T",
  "source": {
    "format": "parquet",
    "schema_fingerprint": "",
    "conversion_policy": ""
  },
  "writer": {
    "morsel_row_count": 4096,
    "segment_target_uncompressed_bytes": 134217728
  },
  "notes": {}
}
```

**Rules:**
- metadata_len MUST be <= 1 MiB.
- Readers MUST ignore unknown metadata keys.
- Metadata JSON MUST NOT be required for correctness.

---

## 16. File Dictionary

The file dictionary maps dense file-local FileCodes to canonical logical values.
FileCode = zero-based ordinal into dictionary index
**Example:**
dictionary[0] = "active"
dictionary[1] = "pending"
dictionary[2] = "closed"

FileCode(0) = "active"
FileCode(1) = "pending"
FileCode(2) = "closed"

### 16.1 Dictionary Header

```rust
struct FileDictionaryHeaderV1 {
    entry_count: u32,
    flags: u32,

    index_entry_len: u16,
    value_hash_algorithm: u16,
    // 0=None
    // 1=xxh3_64
    // 2=sha256_truncated64

    payload_length: u64,

    reserved: [u8; 24],
}
```

### 16.2 Dictionary Index Entry

```rust
struct FileDictionaryIndexEntryV1 {
    value_tag: u16,
    storage_class: u8,
    flags: u8,

    inline_len: u8,
    reserved0: [u8; 3],

    inline_data: [u8; 16],

    payload_offset: u64,
    payload_length: u32,

    canonical_hash64: u64,

    reserved1: u32,
}
```

**Rules:**
- Any FileCode >= entry_count is invalid.
- payload_offset + payload_length MUST be within FILE_DICTIONARY_PAYLOAD.
- canonical_hash64 is an acceleration hint, not proof of equality.
- Equality is determined by value_tag and canonical value bytes.
- Redacted values MUST be marked with StorageClass::Redacted.

### 16.3 Value Tags

```rust
enum ValueTag {
    Null = 0,

    BoolFalse = 1,
    BoolTrue = 2,

    Int64 = 3,
    UInt64 = 4,

    Float32Bits = 5,
    Float64Bits = 6,

    Decimal64 = 7,
    Decimal128 = 8,

    DateDays = 9,
    TimestampMicros = 10,
    TimestampNanos = 11,

    Utf8 = 12,
    Binary = 13,
    Uuid = 14,
    Json = 15,

    List = 16,
    Struct = 17,
    Map = 18,
}
```

### 16.4 Storage Classes

```rust
enum StorageClass {
    Inline = 0,
    Payload = 1,
    Redacted = 2,
}
```

**Rules:**
- Redacted values are present values, not nulls.
- Redacted values MUST have redaction manifest entries.
- Readers MUST NOT silently treat redacted values as null.

---

## 17. Canonical Value Encoding

**Scalar canonical payloads:**
**BoolFalse / BoolTrue:**
  no payload

**Int64:**
  i64 little-endian

**UInt64:**
  u64 little-endian

**Float32Bits:**
  raw IEEE-754 bits as u32 little-endian

**Float64Bits:**
  raw IEEE-754 bits as u64 little-endian

**Decimal64:**
  i64 unscaled value

**Decimal128:**
  i128 two's-complement little-endian unscaled value

**DateDays:**
  i32 days since Unix epoch

**TimestampMicros:**
  i64 microseconds since Unix epoch

**TimestampNanos:**
  i64 nanoseconds since Unix epoch

**Uuid:**
  16 raw UUID bytes

**Utf8:**
  [len: varint][utf8 bytes]

**Binary:**
  [len: varint][bytes]

**Json:**
  [len: varint][utf8 JSON bytes]
**Nested canonical payloads:**
**List:**
  [element_count: varint]
**repeated:**
    [element_value_tag: varint]
    [element_payload]

**Struct:**
  [field_count: varint]
**repeated sorted by field_id ascending:**
    [field_id: varint]
    [field_value_tag: varint]
    [field_payload]

**Map:**
  [pair_count: varint]
**repeated sorted by canonical key bytes:**
    [key_value_tag: varint]
    [key_payload]
    [value_value_tag: varint]
    [value_payload]
**Map rules:**
- Map keys MUST be scalar.
- Map keys MUST NOT be List, Struct, or Map.
- Duplicate canonical keys are invalid.

---

## 18. Logical Types

```rust
enum CoveLogicalType {
    Null = 0,

    Bool = 1,

    Int8 = 2,
    Int16 = 3,
    Int32 = 4,
    Int64 = 5,

    UInt8 = 6,
    UInt16 = 7,
    UInt32 = 8,
    UInt64 = 9,

    Float32 = 10,
    Float64 = 11,

    Decimal64 = 12,
    Decimal128 = 13,

    DateDays = 14,
    TimestampMicros = 15,
    TimestampNanos = 16,

    Utf8 = 17,
    Binary = 18,
    Uuid = 19,
    Json = 20,

    List = 21,
    Struct = 22,
    Map = 23,
}
```

Logical type describes value semantics.

Logical type is independent of physical representation.

**Examples:**
**Utf8 may be physically:**
  FileCode or VarBytes

**Int64 may be physically:**
  NumCode or FileCode

**TimestampMicros may be physically:**
  NumCode

---

## 19. Physical Kinds

```rust
enum CovePhysicalKind {
    FileCode = 0,
    NumCode = 1,
    Boolean = 2,
    FixedBytes = 3,
    VarBytes = 4,
    List = 5,
    Struct = 6,
    Map = 7,
}
```

### 19.1 NumCode Compatibility

**Allowed logical types for NumCode:**
Bool if explicitly declared numeric
Int8/16/32/64
UInt8/16/32/64
Float32/Float64
Decimal64
DateDays
TimestampMicros
TimestampNanos
**Rules:**
- NumCode MUST be interpreted by declared logical_type.
- NumCode MUST NOT be dictionary-resolved.
- Numeric min/max statistics use logical ordering.
**Float rules:**
- Float values preserve raw IEEE bit patterns.
- NaN values are valid.
- Min/max statistics exclude NaN and set HAS_NAN.
- Readers MUST NOT use min/max to exclude NaN-sensitive predicates unless safe.

---

## 20. Encoded Arrays

COVE stores pages as encoded arrays.
**An encoded array has:**
- logical length,
- logical type,
- physical kind,
- encoding tree,
- buffers,
- child arrays,
- statistics reference.

### 20.1 Encoding Kinds

```rust
enum CoveEncodingKind {
    Canonical = 0,

    Validity = 1,
    Constant = 2,

    FileCode = 3,
    NumCode = 4,

    LocalCodebook = 5,
    Rle = 6,
    RunEnd = 7,
    BitPacked = 8,
    Delta = 9,
    FrameOfReference = 10,
    PatchedBase = 11,
    Sparse = 12,
    Sequence = 13,

    PlainFixed = 14,
    PlainVarint = 15,
    VarBytes = 16,

    Lz4Block = 17,
    ZstdBlock = 18,
}
```

### 20.2 Encoding Node Descriptor

```rust
struct CoveEncodingNodeV1 {
    node_id: u16,
    encoding_kind: u16,

    logical_type: u16,
    physical_kind: u8,
    flags: u8,

    logical_len: u32,

    child_count: u16,
    buffer_count: u16,

    params_offset: u32,
    params_length: u32,

    stats_id: u32,
}
```

**Rules:**
- The root node describes the page payload.
- Child nodes and buffers MUST be bounds-checked.
- Each encoding MUST have a canonical decode path.
- Encoded predicate kernels are optional but SHOULD be implemented for common encodings.

### 20.3 Approved v1 Encoding Cascades

**For FileCode columns:**
Constant(FileCode)
Rle(FileCode)
RunEnd(FileCode)
LocalCodebook(BitPacked(local_index -> FileCode))
LocalCodebook(Rle(local_index -> FileCode))
Sparse(fill FileCode + patches)
PlainVarint(FileCode)
**For NumCode columns:**
Constant(NumCode)
Delta(NumCode)
FrameOfReference(NumCode)
PatchedBase(NumCode)
BitPacked(NumCode delta/range)
PlainFixed(NumCode)
PlainVarint(NumCode)
**For booleans:**
Constant(Boolean)
BitPacked(Boolean)
Rle(Boolean)
**For variable bytes:**
VarBytes
LocalCodebook(VarBytes) only if values are page-local and not globally dictionary encoded
Writers SHOULD prefer FileCode over VarBytes for repeated strings, categories, UUID-like dimensions, and low/medium-cardinality values.

### 20.4 LocalCodebook Payload

```rust
struct LocalCodebookPayloadV1 {
  child_encoding_kind: u16,   // Rle or BitPacked
  value_physical_kind: u16,   // FileCode, NumCode, Boolean, or VarBytes
  codebook_len: u32,
  child_payload_len: u32,
  codebook_values: [u8],
  child_payload: [u8],
}
```

**Codebook value layout:**

| value_physical_kind | Codebook entry wire layout |
| --- | --- |
| FileCode | `u32` little-endian FileCode |
| NumCode | `u64` little-endian raw NumCode bits |
| Boolean | `u8`, where `0=false` and `1=true` |
| VarBytes | `u32 byte_len` followed by `byte_len` bytes |

**Rules:**
- `child_encoding_kind` MUST be `Rle` or `BitPacked` and decodes local indexes.
- Each decoded local index MUST be less than `codebook_len`.
- Boolean codebook entries MUST be either 0 or 1.
- VarBytes codebook entries are page-local values and MUST NOT be interpreted as FileCode dictionary entries.
- Readers MUST reject unsupported `value_physical_kind` values and malformed codebook lengths.

---

## 21. Kernel Capability Metadata

COVE-T MAY declare encoding kernel capabilities.

```rust
struct EncodingKernelCapabilityV1 {
    encoding_kind: u16,

    supports_eq: u8,
    supports_in: u8,
    supports_range: u8,
    supports_is_null: u8,

    supports_count: u8,
    supports_min_max: u8,
    supports_selection_decode: u8,
    supports_direct_executioncode_remap: u8,

    decode_cost_class: u8,
    predicate_cost_class: u8,

    reserved: [u8; 6],
}
```

**Rules:**
- Kernel capabilities are advisory.
- A reader MAY ignore them.
- A reader MUST NOT trust capability metadata to skip data without validated stats/index proof.
- supports_direct_executioncode_remap means the page can decode FileCodes directly into engine-local ExecutionCode vectors.
**COVE-H refines this to:**
**supports_direct_enginecode_remap:**
  page can decode FileCodes directly into Harbor EngineCode vectors.

---

## 22. Collation and Canonicalisation Registry

Collation metadata defines safe ordering semantics.
Range pushdown is allowed only when query collation and stored collation agree.

```rust
struct CollationRegistryHeaderV1 {
    entry_count: u32,
    flags: u32,
}
```

```rust
struct CollationRegistryEntryV1 {
    collation_id: u16,

    name_len: u16,
    name: [u8],

    version_len: u16,
    version: [u8],

    flags: u32,
}
```

**Minimum v1 collations:**

| ID | Name | Meaning |
| --- | --- | --- |
| 0 | none | Unordered; range pushdown unavailable. |
| 1 | utf8-bytewise | Bytewise UTF-8 ordering. |
| 2 | unsigned-fixed-bytes | Unsigned bytewise fixed bytes. |
| 3 | signed-numeric | Signed numeric logical order. |
| 4 | unsigned-numeric | Unsigned numeric logical order. |
| 5 | timestamp-chronological | Timestamp chronological order. |

**Rules:**
- String min/max MUST NOT be used for range exclusion unless collation is known and compatible.
- ColumnDomain sections MUST reference a valid collation_id.
- Test vectors MUST cover UTF-8 edge cases, decimals, timestamps, UUIDs, floats, NaN, and nulls.

---

## 23. Column Domains

A ColumnDomain defines logical ordering for FileCode columns.
Raw FileCode numeric order has no semantic meaning.

```rust
struct ColumnDomainHeaderV1 {
    table_or_object_id: u32,
    column_or_property_id: u32,

    logical_type: u16,
    collation_id: u16,

    domain_count: u32,

    sorted_file_codes_offset: u64,
    file_code_to_rank_offset: u64,

    flags: u32,

    checksum: u32,
}
```

**Payload:**
**sorted_file_codes:**
  FileCode[domain_count]

**file_code_to_rank:**
  u32[dictionary_entry_count] or compressed sparse map
**Rules:**
- sorted_file_codes MUST be sorted by logical value order.
- file_code_to_rank maps FileCode -> domain rank.
- Values absent from the column MAY map to INVALID_RANK.
- Readers MUST validate ranks before using domain min/max.
- If no safe ordering exists, range pushdown MUST be disabled.

---

## 24. COVE-T Table Catalog

```rust
struct TableCatalogV1 {
    table_count: u32,
    flags: u32,

    tables: [TableEntryV1],
}
```

```rust
struct TableEntryV1 {
    table_id: u32,

    namespace_len: u16,
    namespace: [u8],

    table_name_len: u16,
    table_name: [u8],

    column_count: u32,
    row_count: u64,

    primary_sort_key_count: u16,
    clustering_key_count: u16,

    flags: u32,

    columns: [TableColumnEntryV1],
}
```

```rust
struct TableColumnEntryV1 {
    column_id: u32,

    column_name_len: u16,
    column_name: [u8],

    logical_type: u16,
    physical_kind: u8,
    nullable: u8,

    sort_order: u16,
    collation_id: u16,

    precision: u16,
    scale: i16,

    flags: u32,
}
```

**Rules:**
- table_id MUST be unique.
- column_id MUST be unique within a table.
- logical_type and physical_kind MUST be compatible.
- nullable=false means all corresponding null counts MUST be zero.

---

## 25. COVE-T Table Segments

A table segment is a contiguous row range for one table.
**Recommended writer targets:**
**segment target uncompressed size:**
  64 MiB to 256 MiB

**morsel row count:**
  4096 default
  8192 for very narrow tables

### 25.1 Table Segment Index Entry

```rust
struct TableSegmentIndexEntryV1 {
    table_id: u32,
    segment_id: u32,

    row_start: u64,
    row_count: u32,

    morsel_count: u32,
    morsel_row_count: u32,

    column_count: u32,

    offset: u64,
    length: u64,

    stats_ref: u32,

    flags: u32,

    checksum: u32,
}
```

### 25.2 Table Segment Header

```rust
struct TableSegmentHeaderV1 {
    table_id: u32,
    segment_id: u32,

    row_start: u64,
    row_count: u32,

    morsel_count: u32,
    morsel_row_count: u32,

    column_count: u32,

    morsel_directory_offset: u64,
    column_directory_offset: u64,
    page_index_offset: u64,
    data_offset: u64,

    flags: u32,

    checksum: u32,
}
```

**Rules:**
- segment_id MUST be unique within table_id.
- row_count MUST equal the sum of row counts in the segment's morsels.
- Last morsel MAY contain fewer rows.
- Segment checksum MUST validate before internal offsets are trusted.

---

## 26. COVE-T Row Morsels

```rust
struct RowMorselEntryV1 {
    morsel_id: u32,

    first_row_in_segment: u32,
    row_count: u32,

    flags: u32,

    stats_ref: u32,

    checksum: u32,
}
```

**Rules:**
- Morsels MUST be ordered by first_row_in_segment.
- Morsel row ranges MUST be contiguous and non-overlapping.
- All columns in a segment MUST use the same morsel boundaries.

---

## 27. COVE-T Column Directory and Pages

### 27.1 Column Directory Entry

```rust
struct TableColumnDirectoryEntryV1 {
    column_id: u32,

    logical_type: u16,
    physical_kind: u8,
    flags: u8,

    page_index_offset: u64,
    page_index_length: u64,

    data_offset: u64,
    data_length: u64,

    stats_ref: u32,
    domain_ref: u32,

    checksum: u32,
}
```

### 27.2 Column Page Index Entry

```rust
struct ColumnPageIndexEntryV1 {
    column_id: u32,
    morsel_id: u32,

    row_count: u32,
    non_null_count: u32,
    null_count: u32,

    encoding_root: u32,

    page_offset: u64,
    page_length: u64,

    uncompressed_length: u64,

    stats_ref: u32,

    flags: u32,

    checksum: u32,
}
```

**Rules:**
- One column page SHOULD exist per column per morsel.
- row_count MUST equal the referenced morsel row_count.
- null_count + non_null_count MUST equal row_count.
- For non-nullable columns, null_count MUST be zero.
- Page checksum covers page payload.

**Page flags:**

| Bits | Name | Meaning |
| --- | --- | --- |
| 0x0000_00FF | PAGE_FLAG_COMPRESSION_CODEC | Page-level `CompressionCodec` value from Section 66. |
| 0xFFFF_FF00 | reserved | Reserved for future required page extensions; MUST be zero in v1. |

**Page codec rules:**
- PAGE_FLAG_COMPRESSION_CODEC applies only to the page payload bytes referenced by `page_offset` and `page_length`.
- Codec `None` requires `page_length == uncompressed_length`.
- LZ4 and Zstd page payloads use the same block codec definitions as Section 66 and require `uncompressed_length` to be the exact decoded byte length.
- If `page_length == 0`, `uncompressed_length` MUST also be zero.
- If `page_length > 0` and the page codec is not `None`, `uncompressed_length` MUST be non-zero.
- Writers that use LZ4 or Zstd page codecs MUST advertise the corresponding `FEATURE_CODEC_LZ4` or `FEATURE_CODEC_ZSTD` bit.
- Readers MUST reject unknown page codec values and any non-zero reserved page flag bits unless a required extension defines the bit and the reader supports that extension.

### 27.3 Page Payload

**A column page payload contains:**
[column page header]
[encoding node descriptors]
[buffer directory]
[buffers]
**Logical row reconstruction:**
1. read null bitmap if present,
2. decode non-null value stream,
3. re-expand values into logical row order.

---

## 28. COVE-T Zone Statistics

**Zone statistics exist at:**
- file/table level,
- segment level,
- morsel level,
- page level.
Morsel-level statistics are the default pruning unit.

```rust
struct ZoneStatsEntryV1 {
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,       // u32::MAX for segment-level stats
    column_id: u32,

    row_count: u32,
    null_count: u32,
    non_null_count: u32,

    distinct_count: u32,
    run_count: u32,

    flags: u32,

    min_value: StatScalarV1,
    max_value: StatScalarV1,

    min_domain_rank: u32,
    max_domain_rank: u32,

    exact_set_ref: u32,
    bloom_ref: u32,
}
```

```rust
struct StatScalarV1 {
    stat_kind: u8,
    flags: u8,
    length: u16,
    data: [u8; 16],
}
```

```rust
enum StatKind {
    None = 0,
    Int64 = 1,
    UInt64 = 2,
    Float64Bits = 3,
    Decimal128 = 4,
    TimestampMicros = 5,
    TimestampNanos = 6,
    DateDays = 7,
    FixedBytes = 8,
}
```

**StatScalar flags:**

| Bit | Name | Meaning |
| --- | --- | --- |
| 0 | STAT_SCALAR_TRUNCATED | This scalar is a truncated bound. |
| 1-7 | reserved | MUST be zero in v1. |

**StatScalar length rules:**

| StatKind | Required length |
| --- | --- |
| None | 0 |
| Int64 | 8 |
| UInt64 | 8 |
| Float64Bits | 8 |
| Decimal128 | 16 |
| TimestampMicros | 8 |
| TimestampNanos | 8 |
| DateDays | 4 |
| FixedBytes | 0..16 |

`MINMAX_TRUNCATED` MUST be set if and only if `STAT_SCALAR_TRUNCATED` is set on either the min or max scalar.

Float64 min/max scalars MUST NOT contain NaN. If a zone contains NaN values, writers MUST set `HAS_NAN` and exclude NaN from min/max.

**Stats flags:**

| Flag | Meaning |
| --- | --- |
| HAS_MIN_MAX | min/max are valid for conservative pruning. |
| HAS_DOMAIN_RANGE | min_domain_rank/max_domain_rank are valid. |
| DISTINCT_EXACT | distinct_count is exact. |
| IS_CONSTANT | all non-null values are equal. |
| IS_SORTED_ASC | non-null values sorted ascending. |
| IS_SORTED_DESC | non-null values sorted descending. |
| HAS_NAN | float data contains NaN. |
| HAS_REDACTED | zone contains redacted values. |
| MINMAX_TRUNCATED | bounds are truncated and require caution. |
| HAS_TOP_N_SUMMARY | top summary available. |
| HAS_BOTTOM_N_SUMMARY | bottom summary available. |

**Rules:**
- For NumCode columns, min/max are interpreted by logical_type.
- For FileCode columns, range stats use domain ranks.
- Raw FileCode min/max MUST NOT be used for logical range pruning.
- Unsafe or truncated bounds MUST NOT be used for exclusion unless rules prove safety.

---

## 29. Neutral Predicate Semantics

**Predicate proof evaluation uses SQL WHERE semantics:**
**TRUE:**
  row is selected

**FALSE or UNKNOWN:**
  row is not selected
**A zone predicate proof returns:**
**DefinitelyNo:**
  no row in the zone can evaluate TRUE

**DefinitelyYes:**
  every row in the zone evaluates TRUE

**Unknown:**
  metadata cannot prove either

### 29.1 Composition Rules

**For A AND B:**
**if A=DefinitelyNo or B=DefinitelyNo:**
  DefinitelyNo

**else if A=DefinitelyYes and B=DefinitelyYes:**
  DefinitelyYes

**else:**
  Unknown
**For A OR B:**
**if A=DefinitelyYes or B=DefinitelyYes:**
  DefinitelyYes

**else if A=DefinitelyNo and B=DefinitelyNo:**
  DefinitelyNo

**else:**
  Unknown
**For NOT A:**
**if A=DefinitelyYes:**
  DefinitelyNo

**if A=DefinitelyNo:**
  DefinitelyYes only when SQL TRUE/FALSE/UNKNOWN semantics are safe

**otherwise:**
  Unknown
**Readers SHOULD be conservative with:**
- NOT,
- nullable columns,
- NaN-sensitive predicates,
- collation-dependent predicates,
- predicates that can evaluate UNKNOWN.

### 29.2 Examples

**For:**
age BETWEEN 18 AND 65
**A non-null zone with:**
min_age = 22
max_age = 51
**is:**
DefinitelyYes
**A zone with:**
max_age = 12
**is:**
DefinitelyNo
**A zone with:**
min_age = 10
max_age = 80
**is:**
Unknown
**For:**
status IN ('active', 'pending')
**A non-null exact set:**
`{active, pending}`
**is:**
DefinitelyYes
**A validated exact set:**
`{closed, cancelled}`
**is:**
DefinitelyNo

---

## 30. Exact Set Indexes

Exact sets represent exact values present in a segment or morsel.

```rust
struct ExactSetIndexHeaderV1 {
    table_id: u32,
    column_id: u32,

    granularity: u8,        // 0=segment, 1=morsel
    key_kind: u8,           // 0=FileCode, 1=NumCode, 2=CanonicalHash
    representation: u8,     // 0=sorted list, 1=bitset, 2=roaring-like
    flags: u8,

    entry_count: u32,

    data_offset: u64,
    data_length: u64,

    checksum: u32,
}
```

**Rules:**
- Exact sets are valid only after checksum validation.
- Corrupt exact sets MUST be ignored.
- Exact sets MUST NOT produce false negatives.
- Exact sets MAY prove DefinitelyNo or DefinitelyYes.
**Recommended writer policy:**
**Build exact sets for:**
  - low-cardinality columns,
  - medium-cardinality predicate columns,
  - equality-heavy dimensions,
  - columns used in IN predicates.

---

## 31. Bloom Indexes

Bloom filters provide conservative membership tests.

```rust
struct BloomIndexHeaderV1 {
    table_id: u32,
    column_id: u32,

    granularity: u8,       // 0=segment, 1=morsel
    hash_domain: u8,       // 0=FileCode, 1=NumCode, 2=CanonicalValueHash
    algorithm: u8,         // 0=split-block
    flags: u8,

    target_fpr_ppm: u32,

    filter_count: u32,

    data_offset: u64,
    data_length: u64,

    checksum: u32,
}
```

**Rules:**
- Bloom filters may produce false positives.
- Bloom filters MUST NOT produce false negatives.
- Corrupt bloom filters MUST be ignored.
- Bloom filters can prove DefinitelyNo.
- Bloom filters generally cannot prove DefinitelyYes.
**Recommended sizing:**
**target_fpr:**
  1% default

**bits_per_item:**
  approximately 10 for 1% FPR

**hash_count:**
  approximately 7 for 1% FPR

**minimum_filter_bits:**
  512

---

## 32. Inverted Morsel Indexes

Inverted morsel indexes map a value to candidate morsels.

```rust
struct InvertedMorselIndexHeaderV1 {
    table_id: u32,
    column_id: u32,

    key_kind: u8,       // 0=FileCode, 1=NumCode
    flags: u8,
    representation: u8,
    reserved: u8,

    entry_count: u32,

    entries_offset: u64,
    bitmap_data_offset: u64,

    checksum: u32,
}
```

```rust
struct InvertedMorselEntryV1 {
    key: u64,
    morsel_bitmap_offset: u64,
    morsel_bitmap_length: u32,

    row_bitmap_offset: u64,
    row_bitmap_length: u32,
}
```

**Rules:**
- Inverted indexes are optional.
- Corrupt inverted indexes MUST be ignored.
- Morsel-level bitmaps are preferred.
- Row-level bitmaps are optional.

---

## 33. Lookup Indexes

Lookup indexes support direct point access.
**Useful predicates:**
WHERE event_id = ?
WHERE patient_id = ?
WHERE order_id = ?
WHERE external_ref = ?

```rust
struct LookupIndexHeaderV1 {
    table_id: u32,
    column_id: u32,

    key_kind: u8,
    // 0=FileCode
    // 1=NumCode
    // 2=CanonicalHash
    // 3=FixedBytes

    index_kind: u8,
    // 0=Hash
    // 1=SparseSorted
    // 2=MinimalPerfectHash

    uniqueness: u8,
    // 0=unknown
    // 1=unique
    // 2=non_unique

    flags: u8,

    entry_count: u64,

    entries_offset: u64,
    entries_length: u64,

    rowref_offset: u64,
    rowref_length: u64,

    checksum: u32,
}
```

**For unique keys:**
key -> CoveTableRowRef
**For non-unique keys:**
key -> rowref list
**Rules:**
- Lookup indexes are optional.
- Lookup indexes MUST be ignored if stale or corrupt.
- Lookup indexes MAY be stored inside COVE or in COVX.
- Lookup indexes MUST NOT change query results.

---

## 34. Aggregate Synopsis Indexes

Aggregate synopses allow metadata-answerable queries and faster aggregation.

```rust
struct AggregateSynopsisEntryV1 {
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,       // u32::MAX for segment-level synopsis
    column_id: u32,

    synopsis_kind: u8,
    key_kind: u8,
    accuracy: u8,         // 0=exact, 1=approximate
    flags: u8,

    row_count: u32,
    null_count: u32,

    payload_offset: u64,
    payload_length: u64,

    checksum: u32,
}
```

```rust
enum SynopsisKind {
    Count = 0,
    MinMax = 1,
    Sum = 2,
    SumAndCount = 3,
    BoolTrueFalseCounts = 4,
    FileCodeHistogram = 5,
    NumCodeHistogram = 6,
    DistinctSketch = 7,
    QuantileSketch = 8,
    TopK = 9,
}
```

**Rules:**
- Exact synopses MAY be used for exact query results.
- Approximate synopses MUST be marked approximate.
- Approximate synopses MUST NOT be used for exact answers.
- Sum/sum-of-squares MUST declare overflow and decimal handling rules.
- Redacted values MUST follow declared redaction aggregation policy.
**Important use cases:**
SELECT count(*) FROM table;

SELECT min(created_at), max(created_at) FROM table;

SELECT status, count(*)
FROM admissions
GROUP BY status;

SELECT count(*)
FROM admissions
WHERE status = 'active';
For low-cardinality FileCode columns, FileCodeHistogram is especially important.

---

## 35. Composite Zone Indexes

Composite zone indexes support multi-column pruning.
**Example:**
WHERE scope = 'A'
  AND event_date = '2026-05-01'
  AND status = 'failed'

```rust
struct CompositeZoneIndexHeaderV1 {
    table_id: u32,

    key_column_count: u16,
    transform_kind: u8,
    // 0=tuple
    // 1=z_order
    // 2=hilbert
    // 3=writer_defined

    flags: u8,

    zone_count: u32,

    key_columns_offset: u64,
    entries_offset: u64,
    entries_length: u64,

    checksum: u32,
}
```

**Each composite zone entry MAY contain:**
- composite min key,
- composite max key,
- optional exact composite set,
- optional composite bloom,
- covered segment/morsel range.
**Rules:**
- Tuple transform uses lexicographic tuple ordering.
- Z-order and Hilbert transforms MUST declare exact encoding rules.
- writer_defined transforms require a required feature bit.
- Unknown composite transforms MUST NOT be used for pruning.
**Recommended use:**
scope + date
site + status
customer + event_type
branch + object_type
partition columns + time

---

## 36. Top-N Zone Summaries

Top-N summaries accelerate ordered limit queries.
**Example:**
SELECT *
FROM events
ORDER BY risk_score DESC
LIMIT 100;

```rust
struct TopNZoneSummaryV1 {
    table_id: u32,
    column_id: u32,
    segment_id: u32,
    morsel_id: u32,

    direction: u8,       // 0=top, 1=bottom
    value_count: u16,
    flags: u8,

    payload_offset: u64,
    payload_length: u64,

    checksum: u32,
}
```

**Rules:**
- Top-N summaries are optional.
- Corrupt summaries MUST be ignored.
- Readers MAY skip zones whose bounds cannot beat the current Top-N threshold.
- Readers MUST preserve stable query semantics, tie handling, and null ordering.

---

## 37. COVE-T Scan Semantics

**A COVE-T scan SHOULD proceed as:**
1. Validate header, postscript, footer, and section directory.
2. Validate table catalog and required dictionary/domain sections.
3. Resolve query literals to FileCodes or NumCodes where possible.
4. Generate scan splits from table segment index.
5. Use COVM file-level pruning if available.
6. Apply segment-level zone stats.
7. Apply morsel-level zone stats.
8. Evaluate predicate proof outcomes.
9. Use exact sets.
10. Use bloom filters.
11. Use inverted morsel indexes.
12. Use lookup indexes for point predicates where available.
13. Use composite zone indexes for multi-column predicates.
14. Use aggregate synopses for metadata-answerable queries.
15. Decode predicate pages for Unknown surviving zones.
16. Build row selection bitmaps.
17. Late-materialise projected columns.
18. Remap FileCodes to ExecutionCodes if supported by the engine.
19. Return engine-native vectors or Arrow-compatible arrays.

### 37.1 Equality on FileCode Column

**For:**
WHERE status = 'active'
**Planner:**
1. Resolve 'active' in COVE dictionary.
2. If absent, predicate is DefinitelyNo for the file.
3. If present, obtain FileCode.
4. Use exact sets, blooms, inverted indexes, and lookup indexes.
5. Decode surviving pages.
6. Remap FileCode -> ExecutionCode for output/execution if supported.

### 37.2 Range on FileCode Column

**For:**
WHERE surname >= 'M' AND surname < 'T'
**Planner:**
1. Use ColumnDomain collation.
2. Convert bounds to domain-rank interval.
3. Compare zone min_domain_rank/max_domain_rank.
4. Skip DefinitelyNo zones.
5. Accept DefinitelyYes zones when safe.
6. Decode Unknown zones.
If no safe ColumnDomain exists, range pushdown MUST fall back to scan.

### 37.3 Numeric Predicate

**For:**
WHERE age BETWEEN 18 AND 65
**Planner:**
1. Use typed NumCode min/max.
2. Evaluate DefinitelyNo/DefinitelyYes/Unknown.
3. Decode Unknown zones.

### 37.4 Null Predicate

WHERE col IS NULL
**Skip zone if:**
null_count = 0
**Accept whole zone if:**
null_count = row_count
WHERE col IS NOT NULL
**Skip zone if:**
null_count = row_count
**Accept whole zone if:**
null_count = 0

### 37.5 Predicate Reordering

Readers MAY reorder conjunctive predicates when reordering preserves semantics.
**Writers SHOULD expose selectivity hints:**
- null_count,
- distinct_count,
- exact set cardinality,
- run_count,
- constant flag,
- sortedness,
- encoded size,
- histogram synopsis.

---

## 38. COVE-E Engine Execution Profile

COVE-E allows an engine to map file-local physical values into implementation-local execution values without changing COVE logical semantics.
**Universal behaviour:**
FileCode -> ExecutionCode
**Examples:**
FileCode -> Harbor EngineCode
FileCode -> DuckDB dictionary vector code
FileCode -> Polars categorical code
FileCode -> Arrow dictionary key
FileCode -> DataFusion dictionary array key
FileCode -> custom engine symbol ID
**Rules:**
- COVE-E is optional.
- COVE-E MUST NOT be required to decode COVE-T logical values.
- COVE-E MAY accelerate scans, output materialisation, joins, grouping, and dictionary execution.
- Unknown required COVE-E profiles cause rejection only when the reader needs that profile.
- Unknown optional COVE-E profiles MUST be ignored.

---

## 39. Engine Profile Registry

```rust
struct EngineProfileRegistryHeaderV1 {
    profile_count: u32,
    flags: u32,
}
```

```rust
struct EngineProfileEntryV1 {
    profile_id: u32,

    namespace_len: u16,
    namespace: [u8],
    // Examples:
    //   "org.coveformat.core"
    //   "io.harbor"
    //   "org.duckdb"
    //   "org.apache.arrow"
    //   "org.apache.datafusion"

    profile_name_len: u16,
    profile_name: [u8],
    // Examples:
    //   "harbor-leased-code"
    //   "arrow-dictionary"
    //   "engine-dictionary-code"

    version_major: u16,
    version_minor: u16,

    required_features: u64,
    optional_features: u64,

    execution_descriptor_ref: u32,
    mount_policy_ref: u32,
    private_payload_ref: u32,

    checksum: u32,
}
```

**Rules:**
- namespace MUST be globally unique.
- Unknown required engine profiles MUST cause rejection only if that profile is required for the requested operation.
- Unknown optional engine profiles MUST be ignored.
- Engine profiles MUST NOT change COVE logical values.
- Engine profiles MAY define faster ways to materialise or compare those values.

---

## 40. ExecutionCode Descriptor

```rust
struct ExecutionCodeDescriptorV1 {
    descriptor_id: u32,

    code_kind: u8,
    code_width_bits: u16,
    byte_order: u8,

    lifetime: u8,
    comparison_scope: u8,
    canonicality: u8,
    null_code_policy: u8,

    flags: u32,

    scope_ref: u32,
    code_space_ref: u32,

    checksum: u32,
}
```

```rust
enum ExecutionCodeKind {
    UnsignedInteger = 0,
    SignedInteger = 1,
    OpaqueBytes = 2,
    DictionaryKey = 3,
    EnginePrivate = 255,
}
```

```rust
enum ExecutionCodeLifetime {
    Query = 0,
    Scan = 1,
    Session = 2,
    Mount = 3,
    LeaseEpoch = 4,
    PersistentEngineLocal = 5,
}
```

```rust
enum ExecutionCodeComparisonScope {
    NotComparable = 0,
    File = 1,
    Dataset = 2,
    Catalog = 3,
    Scope = 4,
    EngineGlobal = 5,
}
```

```rust
enum ExecutionCodeCanonicality {
    Transient = 0,
    Leased = 1,
    CanonicalWithinScope = 2,
    EnginePrivate = 255,
}
```

```rust
enum NullCodePolicy {
    NoNullCode = 0,
    EngineDefinesNullCode = 1,
    NullBitmapOnly = 2,
}
```

**Rules:**
- COVE logical nulls remain structural regardless of execution null-code policy.
- If an engine uses a runtime null code, that code is not a COVE FileCode null sentinel.
- ExecutionCode comparison is valid only within the declared comparison_scope.

---

## 41. Execution Scope Descriptor

```rust
struct ExecutionScopeDescriptorV1 {
    scope_id: u32;

    scope_kind: u16;
    flags: u16;

    stable_id_len: u16;
    stable_id: [u8];

    display_name_len: u16;
    display_name: [u8];

    private_payload_ref: u32;
}
```

```rust
enum ExecutionScopeKind {
    None = 0,
    Tenant = 1,
    Account = 2,
    Organisation = 3,
    Workspace = 4,
    Catalog = 5,
    Dataset = 6,
    EngineSpecific = 255,
}
```

**Examples:**
**Harbor:**
  scope_kind = Tenant
  stable_id  = Harbor tenant UUID

**Generic lakehouse engine:**
  scope_kind = Catalog
  stable_id  = catalog/table namespace ID

**Single-file reader:**
  scope_kind = None

---

## 42. Code Space Descriptor

```rust
struct CodeSpaceDescriptorV1 {
    code_space_id: u32;

    namespace_len: u16;
    namespace: [u8];

    stable_id_len: u16;
    stable_id: [u8];

    epoch: u64;

    flags: u32;

    private_payload_ref: u32;
}
```

**Examples:**
**Harbor:**
  namespace = "io.harbor"
  stable_id = Harbor code-space UUID
  epoch = Harbor lease/code-space epoch

**Arrow dictionary output:**
  namespace = "org.apache.arrow"
  stable_id = dictionary batch or schema identifier
  epoch = 0 or batch/session epoch

**Custom engine:**
  namespace = globally unique engine namespace
  stable_id = implementation-specific code-space ID
**Rules:**
- Code spaces are implementation-local.
- Code-space metadata MUST NOT be required to recover COVE logical values.
- Code-space epoch MAY be used to invalidate stale execution maps.

---

## 43. Engine Mount Policy

```rust
struct EngineMountPolicyV1 {
    policy_id: u32;

    filecode_mapping_kind: u8;
    missing_value_policy: u8;
    stale_mapping_policy: u8;
    reverse_lookup_policy: u8;

    flags: u32;

    dictionary_digest_ref: u32;
    code_space_ref: u32;
    cache_key_ref: u32;

    private_payload_ref: u32;

    checksum: u32;
}
```

```rust
enum FileCodeMappingKind {
    DecodeToValue = 0,
    MapToExecutionCode = 1,
    MapToArrowDictionary = 2,
    EnginePrivate = 255,
}
```

```rust
enum MissingValuePolicy {
    Error = 0,
    DecodeValueOnly = 1,
    RequestLeaseOrIntern = 2,
    ReturnUnmapped = 3,
}
```

```rust
enum StaleMappingPolicy {
    Rebuild = 0,
    Reject = 1,
    IgnoreIfOptional = 2,
}
```

```rust
enum ReverseLookupPolicy {
    NotAvailable = 0,
    BuildFromDictionary = 1,
    EngineProvided = 2,
    CachedExternal = 3,
}
```

**Examples:**
**Generic Arrow reader:**
  filecode_mapping_kind = MapToArrowDictionary
  missing_value_policy = DecodeValueOnly
  stale_mapping_policy = IgnoreIfOptional

**Harbor:**
  filecode_mapping_kind = MapToExecutionCode
  missing_value_policy = RequestLeaseOrIntern
  stale_mapping_policy = Rebuild

---

## 44. COVE-H Harbor Registration

COVE-H is the Harbor registered implementation of COVE-E.
**Registration:**
**namespace:**
  "io.harbor"

**profile_name:**
  "harbor-leased-code"

**ExecutionCode:**
  u64 leased Harbor EngineCode

**Scope:**
  Tenant

**CodeSpace:**
  Harbor dictionary/code-space

**Mapping:**
  FileCode -> Harbor EngineCode

**Lifetime:**
  LeaseEpoch

**Stale policy:**
  Rebuild

**Cache:**
  External Harbor mount cache

### 44.1 Harbor Mount Hints

```rust
struct HarborMountHintsV1 {
    harbor_profile_version_major: u16;
    harbor_profile_version_minor: u16;

    tenant_scope_ref: u32;
    code_space_ref: u32;

    lease_epoch: u64;

    dictionary_digest_ref: u32;
    catalog_digest_ref: u32;

    mount_cache_policy: u8;
    reserved: [u8; 7];

    private_payload_ref: u32;

    checksum: u32;
}
```

**Rules:**
- HarborMountHints are optional outside Harbor.
- Generic readers MUST ignore HarborMountHints.
- Harbor readers MAY use them to build or validate mount caches.
- HarborMountHints MUST NOT be required to decode COVE-T values.
- Harbor readers MUST NOT treat on-disk FileCodes as Harbor EngineCodes.

### 44.2 Harbor Mount Steps

1. Validate COVE structure and required sections.
2. Read table catalog.
3. Read file dictionary.
4. Resolve or lease Harbor EngineCodes for required dictionary values.
5. Build FileCode -> Harbor EngineCode map.
6. Build reverse lookup:
     query literal -> FileCode where possible.
7. Read ColumnDomain and scan index metadata.
8. Validate optional COVX/COVM if present.
9. Expose tables to Harbor query planner.

### 44.3 Harbor Mount Code Map

```rust
type HarborEngineCode = u64;
```

```rust
struct HarborMountCodeMap {
    file_id: [u8; 16],
    table_id: u32,

    dictionary_crc32c: u32,
    lease_epoch: u64,

    filecode_to_enginecode: Vec<HarborEngineCode>,
}
```

**Rules:**
- HarborMountCodeMap is external Harbor metadata.
- HarborMountCodeMap is not authoritative COVE data.
- If missing or stale, it MUST be rebuilt.
- Harbor EngineCodes are not required for offline COVE readability.

---

## 45. Extension Registry

The extension registry allows custom logical types, physical types, encodings, indexes, synopses, predicate kernels, engine profiles, policies, and vendor metadata.

```rust
struct ExtensionRegistryHeaderV1 {
    extension_count: u32;
    flags: u32;
}
```

```rust
struct ExtensionEntryV1 {
    extension_id: u32;

    namespace_len: u16;
    namespace: [u8];

    name_len: u16;
    name: [u8];

    version_major: u16;
    version_minor: u16;

    extension_kind: u16;
    required_feature_bit: u64;
    optional_feature_bit: u64;

    fallback_kind: u16;
    fallback_ref: u32;

    payload_ref: u32;

    checksum: u32;
}
```

```rust
enum ExtensionKind {
    LogicalType = 0,
    PhysicalKind = 1,
    Encoding = 2,
    CompressionCodec = 3,
    Index = 4,
    AggregateSynopsis = 5,
    PredicateKernel = 6,
    EngineProfile = 7,
    RedactionPolicy = 8,
    TrustPolicy = 9,
    VendorMetadata = 255,
}
```

**Rules:**
- Unknown required extensions MUST cause rejection when needed.
- Unknown optional extensions MUST be ignored.
- Extension payloads MUST be length-delimited and checksummed.
- Extensions MUST NOT change the meaning of COVE-Core values unless the extension is required.
- Any custom physical encoding MUST provide a canonical fallback or require a feature bit.
- Any custom logical type SHOULD provide a base logical type or Arrow extension mapping.

---

## 46. Custom Logical Types

```rust
struct ExtensionLogicalTypeV1 {
    extension_id: u32;

    base_logical_type: u16;
    canonical_value_tag: u16;

    collation_id: u16;
    flags: u16;

    arrow_extension_name_len: u16;
    arrow_extension_name: [u8];

    metadata_payload_ref: u32;
}
```

**Rules:**
- If base_logical_type is known, generic readers MAY expose the value as the base type.
- If no safe base type exists and the type is required, unknown readers MUST reject.
- Range pushdown requires known collation/order semantics.
- Custom logical types SHOULD preserve a portable decode path.
**Example:**
**Custom PatientId:**
  namespace = "io.example.health"
  name = "patient-id"
  base_logical_type = Utf8
  canonical_value_tag = Utf8
Generic readers can decode it as UTF-8.

---

## 47. Custom Indexes and Synopses

```rust
struct ExtensionIndexDescriptorV1 {
    extension_id: u32;

    index_kind: u16;
    key_column_count: u16;

    proof_capability: u8;
    // 0=none
    // 1=DefinitelyNo
    // 2=DefinitelyNo+DefinitelyYes

    false_negative_policy: u8;
    // 0=must-not-have-false-negatives
    // 1=may-have-false-negatives, cannot be used for skipping

    flags: u32;

    payload_ref: u32;
}
```

**Rules:**
- An index that may have false negatives MUST NOT be used to skip data.
- Unknown custom indexes MUST be ignored.
- Custom indexes may live in COVE or COVX.
- Custom indexes MUST NOT change query results.

---

## 48. Engine-Neutral Mount / Read Protocol

**A generic COVE-T reader may expose data through:**
- decoded native engine values,
- dictionary/categorical vectors,
- Arrow dictionary arrays,
- Arrow primitive arrays,
- engine-local ExecutionCode vectors.
**Generic mount/read steps:**
1. Validate file structure and required sections.
2. Read table catalog.
3. Read file dictionary.
4. For each selected table/column, decide output representation.
5. Build FileCode -> decoded value map or FileCode -> ExecutionCode map.
6. Build reverse lookup:
     query literal -> FileCode where possible.
7. Read ColumnDomain sections.
8. Read scan index metadata.
9. Validate optional COVX/COVM if present.
10. Expose tables to the query planner.

---

## 49. Arrow Interop Profile

COVE-T SHOULD support Arrow-compatible output.
**Rules:**
- FileCode columns MAY be exposed as Arrow dictionary arrays.
- NumCode columns MAY be exposed as Arrow primitive arrays.
- Boolean columns MAY be exposed as Arrow boolean arrays.
- FixedBytes columns MAY be exposed as Arrow fixed-size binary arrays.
- VarBytes columns MAY be exposed as Arrow binary or UTF-8 arrays.
- Null bitmap MUST be convertible to Arrow validity bitmap.
- Nested List/Struct/Map layouts MUST have a defined Arrow mapping.
**Null bitmap conversion:**
**COVE:**
  1 = null
  0 = non-null

**Arrow:**
  1 = valid
  0 = null
Therefore, Arrow validity bitmaps require inversion unless a reader materialises a new validity bitmap.

**FileCode to Arrow dictionary mapping:**
**Arrow dictionary keys:**
  MAY reuse FileCode values if key width permits.

**Arrow dictionary values:**
  decoded from COVE file dictionary.
**Rules:**
- Arrow interop MUST NOT require Harbor.
- Arrow interop MUST preserve COVE logical type semantics.
- If a COVE logical type cannot be represented exactly in Arrow, the reader MUST either expose an extension type or report a lossy conversion.

---

## 50. Lakehouse Integration Profile

COVE is a file format, not a catalog.
**COVE files MAY be managed by:**
- COVM native manifests,
- Iceberg,
- Delta,
- Hudi,
- engine-specific catalogs,
- object-store inventory systems,
- custom manifests.
**Lakehouse hints MAY include:**
- table schema fingerprint,
- partition values,
- source snapshot identifier,
- data file sequence number,
- delete/visibility overlay reference,
- catalog table identifier,
- source format provenance,
- conversion digest.
**Rules:**
- Lakehouse hints are optional.
- Lakehouse hints MUST NOT override COVE file semantics.
- Visibility/delete overlays are external in v1.
- COVE files remain immutable.
**Recommended usage:**
**Iceberg / Delta / Hudi:**
  May use COVE-T files as data files if the table engine has a COVE reader.

**COVM:**
  Native lightweight COVE dataset manifest for archive/object-store planning.

**COVX:**
  Optional acceleration sidecar for immutable COVE files.

---

## 51. COVE-T Parquet Conversion Profile

COVE-T is intended as a high-performance conversion target for Parquet data.
The converter SHOULD rewrite data into COVE-native scan layout, not copy Parquet’s physical layout.

### 51.1 Conversion Steps

1. Read Parquet schema and metadata.
2. Select target COVE table schema.
3. Select table segment boundaries.
4. Select morsel row count.
5. Decode Parquet pages as needed.
6. Build COVE file-local dictionary.
7. Assign dense FileCodes.
8. Convert repeated string/category/binary/uuid values to FileCode columns.
9. Convert numeric/date/timestamp values to NumCode columns where appropriate.
10. Build ColumnDomain sections for comparable FileCode columns.
11. Build segment and morsel zone stats.
12. Build exact sets for low/medium-cardinality columns.
13. Build bloom filters for high-cardinality equality columns.
14. Build lookup indexes for declared point-lookup columns.
15. Build aggregate synopses for useful low-cardinality and numeric columns.
16. Build composite zone indexes for declared clustering/filter combinations.
17. Build Top-N summaries for ordered hot columns where useful.
18. Encode pages using COVE-approved encodings.
19. Write section directory, footer, postscript, and CRCs.
20. Optionally write COVM/COVX companion artifacts.

### 51.2 Statistics Policy

Converters SHOULD recompute COVE statistics from decoded logical values.
**Converters SHOULD NOT blindly trust source statistics unless they validate:**
- logical type interpretation,
- collation semantics,
- null semantics,
- min/max truncation rules,
- source statistics completeness,
- timestamp/timezone interpretation,
- decimal scale/precision semantics.

### 51.3 Unsupported Nested Shapes

**Unsupported nested shapes MAY be encoded as:**
Json
or
Binary
but MUST be marked pushdown-limited.

---

## 52. Nested Columns

COVE-T uses offset-based nested layouts.

### 52.1 List

**List<T>:**
  parent null bitmap
  offsets: u32[row_count + 1]
  child values: encoded array for T
**Rules:**
- offsets MUST be monotonic.
- offsets[0] MUST be 0.
- offsets[row_count] MUST equal child element count.

### 52.2 Struct

**Struct:**
  parent null bitmap
  child columns, each with row_count rows
**Rules:**
- Struct children share parent row count.
- Parent null handling MUST be declared by encoding.

### 52.3 Map

**Map<K,V>:**
  parent null bitmap
  offsets: u32[row_count + 1]
  key child column
  value child column
**Rules:**
- Map keys MUST be scalar.
- Map keys SHOULD be non-null.
- Duplicate keys within one map value are invalid unless schema flags allow duplicates.
**Pushdown:**
- Struct child fields MAY support full pushdown.
- List/Map element bloom indexes MAY be provided.
- Whole-list/whole-map min/max is usually unsupported.

---

## 53. Sort and Clustering Metadata

```rust
struct SortKeyEntryV1 {
    column_id: u32,
    direction: u8,       // 0=asc, 1=desc
    null_order: u8,      // 0=nulls first, 1=nulls last
    collation_id: u16,
}
```

```rust
struct ClusteringKeyEntryV1 {
    column_id: u32,
    clustering_strength: u8, // 0=unknown, 255=perfect
    reserved: [u8; 3],
}
```

**Rules:**
- Declared sort keys are mandatory claims.
- False sort declarations are format errors.
- Clustering strength is advisory.

---

## 54. Row References

### 54.1 Table Row Reference

```rust
struct CoveTableRowRefV1 {
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,
    row_in_morsel: u16,
}
```

**Use cases:**
- lookup indexes,
- late materialisation,
- row selections,
- diagnostics,
- deferred joins,
- external visibility overlays.
If future morsels exceed u16::MAX rows, v2 must widen this field or introduce a new row reference type.

---

## 55. COVE-O Object Temporal Profile

COVE-O is an optional object-temporal profile.
It is designed for Harbor-style committed object history but may be implemented by other engines with similar temporal object models.
**COVE-O supports:**
- object type catalog,
- scope identity,
- branch identity,
- GOIDs,
- record UUIDs,
- timestamps,
- CSNs,
- baselines,
- snapshots,
- deltas,
- tombstones,
- prev_ref chains,
- optional trust chains,
- optional temporal blooms.

---

## 56. COVE-O Object Type Catalog

```rust
struct ObjectTypeCatalogV1 {
    type_count: u32,
    flags: u32,

    types: [ObjectTypeEntryV1],
}
```

```rust
struct ObjectTypeEntryV1 {
    object_type_id: u32,

    type_name_len: u16,
    type_name: [u8],

    property_count: u16,

    properties: [PropertyEntryV1],
}
```

```rust
struct PropertyEntryV1 {
    property_id: u32,

    property_name_len: u16,
    property_name: [u8],

    logical_type: u16,
    physical_kind: u8,

    nullable: u8,

    collation_id: u16,

    flags: u32,
}
```

**Rules:**
- object_type_id MUST be unique.
- property_id MUST be unique within object_type_id.
- Top-level property declarations MUST NOT use logical Null.

---

## 57. COVE-O Temporal Segment Index

```rust
struct TemporalSegmentIndexEntryV1 {
    segment_id: u32,
    object_type_id: u32,

    time_range_start_us: i64,
    time_range_end_us: i64,

    csn_min: u64,
    csn_max: u64,

    row_count: u32,

    delta_count: u32,
    snapshot_count: u32,
    baseline_count: u32,
    tombstone_count: u32,

    min_goid: [u8; 16],
    max_goid: [u8; 16],

    offset: u64,
    length: u64,

    checksum: u32,
}
```

**Rules:**
- min_goid and max_goid are lexical min/max of full 16-byte GOIDs.
- GOIDs MUST NOT be truncated.
- Time ranges use commit/file-ordering timestamp.

---

## 58. COVE-O Temporal Segments

```rust
struct TemporalSegmentHeaderV1 {
    segment_id: u32,
    object_type_id: u32,

    time_range_start_us: i64,
    time_range_end_us: i64,

    csn_min: u64,
    csn_max: u64,

    row_count: u32,
    morsel_count: u32,
    morsel_row_count: u32,

    column_count: u32,

    row_directory_offset: u64,
    column_directory_offset: u64,
    page_index_offset: u64,
    data_offset: u64,

    flags: u32,

    checksum: u32,
}
```

**Rules:**
- A temporal segment contains exactly one object_type_id.
- For Harbor COVE-H/COVE-O use, producer_scope_kind SHOULD be Tenant and producer_scope_id SHOULD be the Harbor tenant UUID.
- Logical scope values MUST equal producer_scope_id when scope-scoped.
- Rows MUST be ordered by:
    (timestamp_us, csn, branch_key, goid, record_id)
- timestamp_us MUST be monotonic with csn inside a segment.
- prev_ref may point to earlier segments within the same file.
- prev_ref MUST NOT point outside the file.

---

## 59. COVE-O System Columns

Every temporal segment has these logical system columns.

| Column | Name | Physical Kind | Meaning |
| --- | --- | --- | --- |
| 0 | scope_id | implicit/fixed bytes | Producer scope UUID if scope-scoped. |
| 1 | branch_key | FileCode or FixedBytes | Logical branch identity. |
| 2 | goid | FixedBytes | 16-byte global object ID. |
| 3 | record_id | FixedBytes | 16-byte record UUID. |
| 4 | timestamp_us | NumCode | Commit/file-ordering timestamp. |
| 5 | csn | NumCode | Commit Sequence Number. |
| 6 | xmin | NumCode | Transaction provenance. |
| 7 | record_kind | u8/RLE | Delta/snapshot/baseline/tombstone. |
| 8 | prev_ref | nullable fixed struct | Previous chain reference. |

**For Harbor COVE-H/COVE-O:**
scope_id is interpreted as Harbor tenant_id.

### 59.1 Record Kind

```rust
enum RecordKind {
    Delta = 0,
    Snapshot = 1,
    ReservedLegacyMaterializedDelta = 2,
    Baseline = 3,
    Tombstone = 4,
}
```

Staging-only placeholders MUST NOT appear in COVE.

### 59.2 Object Record Reference

```rust
struct CoveRecordRefV1 {
    segment_id: u32,
    row_index: u32,
    target_kind: u8,      // 0=delta-like, 1=snapshot/baseline-like
}
```

**Rules:**
- prev_ref is file-local only.
- Readers MUST reject invalid segment_id or row_index.
- Readers MUST reject mismatched target_kind.

---

## 60. COVE-O Reconstruction Self-Containment

COVE-O v1 files MUST be reconstruction self-contained.
**For every represented object chain, the file MUST contain either:**
- the full chain back to the first record, or
- a Baseline/Snapshot sufficient to reconstruct state before dependent Delta records.
If a chain continues from outside the file, the writer MUST emit a Baseline or Snapshot anchor inside the file.
Mandatory cross-file prev_ref is not supported in v1.

---

## 61. COVE-O Property Columns

Object property columns use the same physical and encoded-array machinery as COVE-T.
**Property values may be:**
**FileCode:**
  file-local dictionary value

**NumCode:**
  raw fixed-width numeric bit pattern

**FixedBytes / VarBytes / nested:**
  special or unsupported cases
**Rules:**
- Nulls are represented only by null bitmaps.
- FileCodes resolve through the file dictionary.
- NumCodes are interpreted by declared logical type.
- Property columns SHOULD be page/morsel aligned with system columns.

---

## 62. COVE-O Temporal Bloom Index

Temporal bloom filters are optional accelerators.
**They answer:**
Can this segment or time bucket contain rows for this scope/branch/goid?
**Recommended bloom key:**
hash(scope_id, branch_key_canonical_value, goid, time_bucket)
**Rules:**
- Single-scope or single-branch files MAY omit scope or branch only if declared.
- Bloom filters may produce false positives.
- Bloom filters MUST NOT produce false negatives.
- Corrupt or missing blooms MUST be ignored.

---

## 63. COVE-O Trust Chain

Trust chains are optional and gated by FEATURE_TRUST_CHAIN.
**Trust columns:**

| Name | Type | Meaning |
| --- | --- | --- |
| trust_hash | nullable [u8; 32] | Hash of canonical delta content. |
| prev_trust_hash | nullable [u8; 32] | Previous trust hash. |
| state_hash | nullable [u8; 32] | Hash of materialised state for baseline/snapshot. |

**Rules:**
- Trust hashes MUST be computed over canonical logical values, not FileCodes.
- Equivalent logical files with different FileCode assignments SHOULD verify to the same logical trust state.
- CRC32C is not a substitute for trust hashes.
**Recommended trust input:**
- scope_id if scope-scoped,
- branch canonical identity,
- object_type_id,
- goid,
- record_id,
- timestamp_us,
- csn,
- record_kind,
- property_id,
- property logical type,
- canonical property value bytes,
- previous trust hash where applicable.

---

## 64. Redaction

A redacted value is present but inaccessible.
It is not null.

```rust
struct RedactionManifestEntryV1 {
    redaction_id: u64,

    section_id: u32,
    local_ref: u64,

    reason_code: u16,

    policy_id_len: u16,
    policy_id: [u8],

    audit_ref_len: u16,
    audit_ref: [u8],

    created_at_us: i64,
}
```

**Rules:**
- Readers MUST NOT silently expose redacted payload bytes.
- Readers MUST NOT silently treat redacted values as null.
- Query engines MAY compare redacted markers only according to policy.

---

## 65. Digest Manifest

The digest manifest provides cryptographic integrity.

```rust
struct DigestManifestHeaderV1 {
    digest_algorithm: u16,   // 1=SHA-256, 2=BLAKE3
    digest_scope: u16,       // 0=file, 1=section, 2=page, 3=merkle

    entry_count: u32,

    entries_offset: u64,
    entries_length: u64,

    root_digest: [u8; 32],

    checksum: u32,
}
```

```rust
struct DigestEntryV1 {
    target_kind: u16,        // section/page/file/custom
    digest_len: u16,

    section_id: u32,
    local_id: u64,

    offset: u64,
    length: u64,

    digest: [u8; digest_len],
}
```

**Rules:**
- Digest manifests are optional.
- Public archive datasets SHOULD include them.
- COVX and COVM SHOULD reference COVE files by cryptographic digest.
- Digest validation failure MUST be reported.
- If digest validation is required by policy, failure MUST reject the file.

---

## 66. Compression

```rust
enum CompressionCodec {
    None = 0,
    Lz4 = 1,
    Zstd = 2,
}
```

**Rules:**
- Readers MUST support None.
- Readers SHOULD support LZ4.
- Zstd requires FEATURE_CODEC_ZSTD.
- Unknown required compression codecs cause rejection.
**Recommended policy:**
**Metadata:**
  None or LZ4

**Hot scan pages:**
  LZ4

**Cold archive pages:**
  Zstd

**Already compact bit-packed pages:**
  MAY be uncompressed

**Indexes:**
  None or LZ4

---

## 67. I/O and Mechanical Sympathy

**COVE writers SHOULD organise files for:**
- tail bootstrap,
- object-store range reads,
- metadata-first pruning,
- predicate-first scans,
- read coalescing,
- late materialisation,
- column projection,
- morsel-level scheduling.
**Optional I/O hints:**

```rust
struct CoveIoHintV1 {
    preferred_read_alignment: u32,
    preferred_coalesce_distance: u32,
    preferred_max_coalesced_read: u32,

    prefetch_group_id: u32,
    page_cluster_id: u32,

    flags: u32,
}
```

Hints are advisory only.

---

## 68. COVX Accelerator Sidecar

COVX is an optional sidecar containing rebuildable acceleration metadata.
**COVX final bytes:**
[postscript bytes]
[postscript_version: u16]
[postscript_len: u16]
[magic: "CVX1"]

### 68.1 COVX Header

```rust
struct CovxHeaderV1 {
    magic: [u8; 4],          // "CVX1"

    header_len: u16,
    version_major: u16,
    version_minor: u16,

    flags: u32,

    accelerator_id: [u8; 16],

    referenced_file_count: u32,

    created_at_us: i64,

    reserved: [u8; 40],

    checksum: u32,
}
```

### 68.2 Referenced File Entry

```rust
struct CovxReferencedFileV1 {
    file_id: [u8; 16],

    file_len: u64,
    footer_crc32c: u32,

    digest_algorithm: u16,
    digest_len: u16,
    digest: [u8; digest_len],
}
```

**COVX may contain:**
- lookup indexes,
- composite zone indexes,
- large histograms,
- full-text indexes,
- vector indexes,
- spatial indexes,
- learned/adaptive indexes,
- workload-specific synopses.
**Rules:**
- COVX MUST be ignored if referenced file digest does not match.
- COVX MUST be ignored if referenced file_id does not match.
- COVX MUST NOT change query semantics.
- COVX acceleration failures MUST fall back to COVE.

---

## 69. COVM Dataset Manifest

COVM is an optional multi-file dataset manifest.
**COVM final bytes:**
[postscript bytes]
[postscript_version: u16]
[postscript_len: u16]
[magic: "CVM1"]

### 69.1 COVM Header

```rust
struct CovmHeaderV1 {
    magic: [u8; 4],          // "CVM1"

    header_len: u16,
    version_major: u16,
    version_minor: u16,

    flags: u32,

    dataset_id: [u8; 16],

    table_count: u32,
    file_count: u32,

    created_at_us: i64,

    reserved: [u8; 32],

    checksum: u32,
}
```

### 69.2 Manifest File Entry

```rust
struct CovmFileEntryV1 {
    file_id: [u8; 16],

    uri_len: u16,
    uri: [u8],

    file_len: u64,

    footer_crc32c: u32,

    digest_algorithm: u16,
    digest_len: u16,
    digest: [u8; digest_len],

    row_count: u64,
    segment_count: u32,

    file_stats_ref: u32,
    file_exact_set_ref: u32,

    flags: u32,
}
```

**COVM MAY contain:**
- table schema fingerprints,
- partition values,
- file-level min/max,
- file-level domain ranges,
- file-level exact sets,
- dictionary fingerprints,
- COVX references,
- object-store hints.
**Rules:**
- COVM MUST be ignored if stale.
- COVM MUST NOT change COVE semantics.
- Query planners MAY use COVM to prune files before opening COVE footers.

---

## 70. Profile Capability Matrix

A public COVE implementation SHOULD declare which profile tier it supports.

| Feature | COVE-Core Reader | COVE-T Scan Reader | COVE-A Archive Reader | COVE-E Reader | COVE-H Harbor Reader |
| --- | --- | --- | --- | --- | --- |
| Validate header/footer/sections | Required | Required | Required | Required | Required |
| Decode FileCode to values | Required | Required | Required | Required | Required |
| Decode NumCode columns | Required | Required | Required | Required | Required |
| Arrow-compatible output | Recommended | Recommended | Recommended | Optional | Optional |
| FileCode -> ExecutionCode | Optional | Recommended | Recommended | Required | Required as Harbor EngineCode |
| Engine profile registry | Optional | Optional | Optional | Required | Required |
| Morsel-aligned scanning | Optional | Required | Required | Optional | Required |
| Zone stats | Optional | Required | Required | Optional | Required |
| Predicate proof outcomes | Optional | Required | Required | Optional | Required |
| Exact sets | Optional | Recommended | Recommended | Optional | Recommended |
| Bloom filters | Optional | Recommended | Recommended | Optional | Recommended |
| Inverted morsel indexes | Optional | Optional | Recommended | Optional | Recommended |
| Lookup indexes | Optional | Optional | Recommended | Optional | Recommended |
| Aggregate synopses | Optional | Optional | Recommended | Optional | Recommended |
| Composite zone indexes | Optional | Optional | Recommended | Optional | Recommended |
| Top-N summaries | Optional | Optional | Recommended | Optional | Recommended |
| COVX sidecars | Optional | Optional | Optional | Optional | Optional |
| COVM manifests | Optional | Optional | Recommended | Optional | Recommended |
| COVE-O object profile | Optional | Optional | Optional | Optional | Recommended for Harbor |
| COVE-H Harbor mount profile | Not required | Not required | Not required | Not required | Required |

---

## 71. Writer Profiles

### 71.1 COVE-Core Minimal Profile

**MUST emit:**
- valid header,
- valid postscript,
- valid footer,
- section directory,
- file dictionary if FileCode columns exist,
- valid checksums,
- valid logical/physical typing,
- valid null bitmaps.

### 71.2 COVE-T Minimal Table Profile

**MUST emit:**
- all COVE-Core requirements,
- table catalog,
- table segment index,
- table segment data,
- column page indexes,
- page checksums,
- null counts,
- segment/morsel row counts.

### 71.3 COVE-T Scan Profile

**Recommended default:**
- all COVE-T Minimal requirements,
- FileCode columns for repeated strings/categories,
- NumCode columns for numeric/timestamp data,
- morsel_row_count = 4096,
- ColumnDomain for comparable FileCode columns,
- morsel-level zone stats,
- predicate proof support,
- exact sets for low/medium-cardinality columns,
- bloom filters for high-cardinality equality columns,
- local codebook encoding for FileCode pages,
- frame-of-reference or delta encoding for NumCode pages,
- LZ4 for hot scan pages.

### 71.4 COVE-A Archive Acceleration Profile

**Recommended for fast offline archives:**
- all COVE-T Scan Profile features,
- COVM manifest,
- digest manifest,
- FileCode histograms,
- lookup indexes,
- composite zone indexes,
- Top-N summaries for ordered hot columns,
- optional COVX sidecar,
- Zstd for cold page payloads where scan latency permits.

### 71.5 COVE-E Engine Execution Profile

**Recommended for engines with dictionary/coded execution:**
- engine profile registry,
- execution code descriptor,
- execution scope descriptor,
- code-space descriptor,
- engine mount policy,
- FileCode -> ExecutionCode mapping strategy,
- optional execution-code cache metadata,
- reverse lookup policy.

### 71.6 COVE-H Harbor Profile

**Recommended for Harbor:**
- all COVE-T Scan Profile features,
- COVE-E engine execution profile,
- FileCode -> Harbor EngineCode mount map,
- Harbor lease epoch tracking,
- Harbor code-space descriptor,
- Harbor mount cache key,
- direct Harbor vector materialisation,
- optional COVE-O object-temporal support.

### 71.7 COVE-O Object Checkpoint Profile

**Recommended for object state:**
- object type catalog,
- temporal segment index,
- self-contained baselines/snapshots,
- FileCode/NumCode property columns,
- temporal blooms,
- trust chain if compliance requires,
- redaction manifest if redactions are present.

---

## 72. Validation Model

### 72.1 Bootstrap Validation

1. Read trailing magic.
2. Read postscript_len and postscript_version.
3. Read postscript.
4. Validate postscript checksum.
5. Validate file_len.
6. Locate footer.
7. Validate footer CRC via postscript section spec.
8. Parse footer and section directory.

### 72.2 Structural Validation

**For every used section:**
- validate offset,
- validate length,
- validate compression,
- validate feature bits,
- validate CRC,
- validate item counts,
- validate internal offsets,
- validate enum ranges,
- validate arithmetic overflow.

### 72.3 COVE-T Semantic Validation

- table IDs unique,
- column IDs unique within table,
- logical/physical pairs valid,
- segment row ranges valid,
- morsel ranges contiguous,
- page row_count matches morsel row_count,
- null_count + non_null_count = row_count,
- FileCodes < dictionary entry_count,
- ColumnDomain ranks valid,
- stats safe before pushdown,
- optional indexes checksum-valid before use.

### 72.4 COVE-E Semantic Validation

- engine profile namespace valid,
- execution descriptor valid,
- scope descriptor valid,
- code-space descriptor valid,
- mount policy valid,
- execution mapping optional or required according to requested operation,
- unknown required profiles rejected only when needed.

### 72.5 COVE-O Semantic Validation

- object_type_id exists,
- property_id exists,
- scope values valid if scope-scoped,
- rows sorted by required order,
- csn/timestamp monotonicity holds,
- prev_ref targets valid rows,
- prev_ref target kind matches,
- reconstruction self-containment holds.

---

## 73. Recovery and Failure Behavior

| Condition | Default Behavior |
| --- | --- |
| Bad header magic | Reject file |
| Bad trailing magic | Reject file |
| Unsupported version | Reject file |
| Unknown required feature | Reject file |
| Unknown optional feature | Ignore if not needed |
| Header checksum mismatch | Reject file |
| Postscript checksum mismatch | Reject file |
| Footer CRC mismatch | Reject file |
| Required section CRC mismatch | Reject file |
| Optional index CRC mismatch | Ignore index and scan |
| Bloom corruption | Ignore bloom and scan |
| Exact set corruption | Ignore exact set and scan |
| Inverted index corruption | Ignore index and scan |
| Lookup index corruption | Ignore index and scan |
| Aggregate synopsis corruption | Ignore synopsis unless required by query-only plan |
| Composite zone corruption | Ignore composite zone and scan |
| Top-N summary corruption | Ignore summary and scan |
| COVE-E optional profile corrupt | Ignore profile |
| COVE-E required profile corrupt | Reject if needed |
| COVX stale/corrupt | Ignore COVX |
| COVM stale/corrupt | Ignore COVM |
| Segment checksum mismatch | Reject segment; fail read unless explicit best-effort mode |
| Page checksum mismatch | Reject page; fail read unless explicit best-effort mode |
| Invalid FileCode | Treat as corruption |
| Invalid NumCode/logical type pairing | Schema error |
| Invalid prev_ref | Reject COVE-O file |
| Unsafe min/max | Do not use for skipping |

Best-effort mode MAY skip corrupt segments only when explicitly requested by recovery/export tooling.
Normal readers fail closed for structural corruption.

---

## 74. Durable Replace Protocol

Writers MUST publish COVE files by durable replace.
**Required protocol:**
1. Write complete candidate file to a temporary path in the target directory.
2. fsync/fdatasync the temporary file after all bytes are written.
3. Optionally reopen and validate header/footer/section CRCs.
4. Atomically rename the temporary file over the destination path.
5. fsync the parent directory to persist the rename.
6. Only after step 5 may the new COVE file be considered durable.
**Rules:**
- Writers MUST NOT claim durability on rename alone.
- If any step fails, the old file remains authoritative.
- Temporary files MUST be ignored, deleted, or quarantined.
- COVE files MUST NOT be modified in place.

---

## 75. Error Codes

| Code | Meaning |
| --- | --- |
| COVE_E_BAD_MAGIC | Missing or invalid magic. |
| COVE_E_BAD_VERSION | Unsupported COVE version. |
| COVE_E_UNKNOWN_REQUIRED_FEATURE | Unknown required feature bit set. |
| COVE_E_CHECKSUM_MISMATCH | Header, postscript, footer, section, segment, or page checksum mismatch. |
| COVE_E_DIGEST_MISMATCH | Cryptographic digest mismatch. |
| COVE_E_OFFSET_RANGE | Offset/length/count exceeds file bounds. |
| COVE_E_ARITH_OVERFLOW | Offset/count/size arithmetic overflow. |
| COVE_E_BAD_SECTION | Section malformed or invalid. |
| COVE_E_BAD_SCHEMA | Catalog/schema malformed. |
| COVE_E_BAD_LOGICAL_PHYSICAL_PAIR | Logical type incompatible with physical kind. |
| COVE_E_DICT_MISS | FileCode missing from dictionary. |
| COVE_E_BAD_FILECODE | FileCode outside dictionary range. |
| COVE_E_BAD_NUMCODE | NumCode invalid for declared logical type. |
| COVE_E_BAD_DOMAIN | ColumnDomain invalid. |
| COVE_E_BAD_STATS | Statistics invalid or unsafe. |
| COVE_E_BAD_INDEX | Optional index invalid or corrupt. |
| COVE_E_BAD_EXTENSION | Extension invalid or required extension unsupported. |
| COVE_E_BAD_ENGINE_PROFILE | Engine profile invalid or unsupported when required. |
| COVE_E_EXECUTION_CODE_MAP | Engine-local code mapping failed. |
| COVE_E_HARBOR_MOUNT_LEASE | Harbor code lease resolution failed. |
| COVE_E_REF_INVALID | COVE-O prev_ref invalid. |
| COVE_E_NOT_SELF_CONTAINED | COVE-O chain lacks baseline/snapshot/full chain. |
| COVE_E_SEGMENT_CORRUPT | Segment structure invalid. |
| COVE_E_PAGE_CORRUPT | Page structure invalid. |
| COVE_E_REDACTION_POLICY | Redacted value cannot be surfaced under current policy. |
| COVE_E_SIDECAR_STALE | COVX/COVM sidecar does not match referenced COVE. |

---

## 76. Compatibility

### 76.1 Versioning

**COVE v1 readers support:**
version_major = 1
**Rules:**
- Readers MUST reject unsupported major versions.
- Readers MAY accept newer minor versions if no unknown required features are set.

### 76.2 Required vs Optional Features

Required features are needed for correctness.
Optional features are accelerators or metadata.
**Examples:**
**Required:**
  - codec needed to decode projected data,
  - nested column support when projected,
  - trust-chain support when verification is requested,
  - engine profile required by requested output mode.

**Optional:**
  - bloom filters,
  - exact sets,
  - lookup indexes,
  - aggregate synopses,
  - Top-N summaries,
  - COVX sidecars,
  - COVM manifests,
  - optional engine profile mappings.

---

## 77. Conformance Requirements

**A conforming COVE-Core reader MUST:**
- validate header checksum,
- validate postscript,
- validate footer,
- parse section directory,
- reject unknown required features,
- bounds-check every used offset/length/count,
- validate CRCs for every used section,
- validate dictionary FileCode ranges,
- enforce null bitmap semantics,
- interpret NumCodes by declared logical type,
- ignore corrupt optional pushdown metadata,
- avoid unsafe min/max pruning,
- fail closed on structural corruption by default.
**A conforming COVE-T reader MUST additionally:**
- parse table catalog,
- validate segment/morsel/page row counts,
- support FileCode decode to dictionary values,
- support direct FileCode/NumCode scan paths,
- preserve correctness when pushdown metadata is missing,
- implement PredicateZoneOutcome conservatively.
**A conforming COVE-A reader SHOULD additionally:**
- use lookup indexes when valid,
- use aggregate synopses when exact and applicable,
- use COVM for file pruning,
- use COVX when valid and beneficial,
- ignore stale or corrupt acceleration artifacts.
**A conforming COVE-E reader MUST additionally:**
- parse engine profile registry when required,
- validate execution descriptors,
- validate scope and code-space descriptors,
- follow mount policy only when understood,
- ignore unknown optional engine profiles,
- reject unknown required engine profiles only when needed by the requested operation,
- never treat ExecutionCodes as COVE logical truth.
**A conforming COVE-H reader MUST additionally:**
- support FileCode -> Harbor EngineCode mapping,
- respect Harbor lease epoch and code-space policy,
- rebuild stale mount maps,
- never treat on-disk FileCodes as Harbor EngineCodes.
**A conforming COVE-O reader MUST additionally:**
- parse object catalog,
- validate temporal segment ordering,
- validate prev_ref targets,
- enforce reconstruction self-containment,
- verify trust chains when requested and present.
**A conforming writer MUST:**
- never emit engine execution codes as authoritative logical data,
- write FileCodes densely into the file dictionary,
- emit valid null bitmaps,
- emit valid CRCs,
- publish by durable replace,
- mark optional indexes accurately,
- avoid false sort/min/max/domain claims,
- recompute safe stats during conversion unless source stats are proven compatible,
- mark required extensions and profiles accurately.

---

## 78. Open Conformance Suite

**An open COVE release SHOULD include:**
1. Reference reader.
2. Reference writer.
3. cove-validate CLI.
4. cove-inspect CLI.
5. cove-convert-parquet CLI.
6. Binary conformance vectors.
7. Property-based fuzz tests.
8. Corruption/negative test corpus.
9. Canonicalisation/collation test corpus.
10. Parquet conversion corpus.
11. Benchmark suite.
**Benchmark categories SHOULD include:**
- full numeric scan,
- string/category scan,
- equality filter,
- IN filter,
- range filter,
- point lookup,
- Top-N,
- group-by low-cardinality FileCode column,
- count/min/max metadata-only query,
- object-store cold scan,
- warm mount-cache scan,
- Parquet-to-COVE conversion cost,
- COVE file-size overhead,
- COVX/COVM acceleration impact,
- ExecutionCode remap overhead,
- Harbor EngineCode remap overhead.
**Conformance vectors SHOULD cover:**
- header/footer/postscript validation,
- dictionary FileCode resolution,
- null bitmap semantics,
- NumCode interpretation,
- ColumnDomain ordering,
- predicate proof outcomes,
- exact set pruning,
- bloom false-positive safety,
- aggregate synopsis exactness,
- lookup index row references,
- extension registry fallback,
- engine profile descriptor validation,
- ExecutionCode scope/comparison rules,
- Harbor COVE-H lease mapping,
- temporal prev_ref validation,
- trust hash canonicalisation,
- redaction handling,
- digest verification,
- Arrow interop mapping.

---

## 79. Utilities and Supporting Artifacts

The public COVE project SHOULD ship the following utilities and artifacts.

### 79.1 Reference Libraries

- **cove-core:** Format primitives, checksums, section directory, dictionary, encoded arrays, validation, collation, extension registry.
- **cove-reader:** Read COVE-Core and COVE-T files.
- **cove-writer:** Write COVE-Core and COVE-T files.
- **cove-arrow:** Export COVE data as Arrow arrays / record batches.
- **cove-engine:** COVE-E engine execution profile helpers.
- **cove-harbor:** COVE-H Harbor mount profile implementation.
- **cove-convert:** Conversion library for Parquet/CSV/Arrow/ORC -> COVE-T.

### 79.2 CLI Tools

- **cove-validate:** Validate structure, CRCs, digests, schema, dictionaries, sections, indexes, profiles, extensions, and conformance.
- **cove-inspect:** Print human-readable file layout, sections, catalog, stats, dictionary summaries, execution profiles, and index summaries.
- **cove-dump:** Dump selected rows, columns, pages, morsels, dictionary values, or encoded array structures.
- **cove-convert-parquet:** Convert Parquet files to COVE-T.
- **cove-convert-arrow:** Convert Arrow IPC/Feather/RecordBatch streams to COVE-T.
- **cove-convert-orc:** Convert ORC files to COVE-T.
- **cove-bench:** Run standard benchmarks against COVE, Parquet, ORC, and optionally other formats.
- **cove-build-covm:** Build or refresh COVM dataset manifest.
- **cove-build-covx:** Build optional COVX accelerator sidecar.
- **cove-verify-digest:** Verify cryptographic digests and Merkle roots.
- **cove-fuzz:** Run corpus and property-based fuzz tests.
- **cove-canonicalise:** Verify canonical value encodings, collation ordering, domain-rank construction, and trust input canonicalisation.

- **cove-profile:** Inspect or generate COVE-E engine profile metadata.
- **cove-arrow-export:** Export COVE-T tables to Arrow-compatible batches.

### 79.3 Engine Integrations

**Recommended initial integrations:**

- **Arrow:** COVE -> Arrow arrays and record batches.
- **DataFusion:** COVE TableProvider.
- **DuckDB:** COVE scan extension / table function.
- **Polars:** COVE scan/read support.
- **Python:** cove.read_table(), cove.scan(), cove.to_arrow(), cove.to_polars().
- **Rust:** cove-core, cove-io, cove-arrow, cove-datafusion, cove-engine.
- **Harbor:** COVE-H direct leased-code mount support.

### 79.4 Dataset and Benchmark Corpus

**Recommended corpora:**

- **synthetic-numeric:** numeric full scan and range predicates.
- **synthetic-categorical:** low/medium-cardinality FileCode workloads.
- **synthetic-wide:** hundreds/thousands of columns with small projections.
- **synthetic-point:** lookup-heavy high-cardinality IDs.
- **synthetic-composite:** multi-column predicates and composite pruning.
- **synthetic-archive:** multi-file object-store-style dataset with COVM.
- **parquet-tpch:** converted TPC-H-style tables.
- **parquet-tpcds:** converted TPC-DS-style tables.
- **parquet-medical-operational:** categorical, temporal, event, and object-history style data.
- **negative-corrupt:** malformed sections, invalid CRCs, bad offsets, invalid FileCodes.
- **canonicalisation:** UTF-8, decimal, timestamp, UUID, NaN, null, map/list/struct cases.
- **engine-profile:** FileCode -> ExecutionCode mapping tests for generic, Arrow, and Harbor profiles.

### 79.5 Governance Artifacts

**For open adoption, the project SHOULD publish:**
- formal binary specification,
- semantic versioning policy,
- feature bit registry,
- section kind registry,
- encoding kind registry,
- extension registry,
- engine profile registry,
- collation registry,
- test vector registry,
- implementation conformance levels,
- performance benchmark methodology,
- security model,
- trademark/name guidance,
- extension proposal process.

---

## 80. Summary of v1 Design Decisions

**COVE v1 chooses:**

- **Neutral public name:** Cove Format, with Harbor represented as COVE-H profile and origin influence.
- **File-local FileCodes:** over persisted engine-owned codes.
- **ExecutionCode abstraction:** so non-Harbor engines can map FileCodes into their own runtime representations.
- **COVE-E universal engine execution profile:** over making Harbor-specific mount behaviour the generic extension mechanism.
- **COVE-H Harbor profile:** Harbor leased-code execution as one registered COVE-E implementation.
- **Scope descriptors:** over hard-coded tenant fields in the universal core.
- **Morsel-aligned pages:** over generic row-group-only scans.
- **Encoded arrays:** over flat codec-only compression.
- **Column domains:** over raw FileCode min/max.
- **Predicate proof outcomes:** over skip-only pruning.
- **Exact sets, blooms, lookup indexes, and aggregate synopses:** over statistics-only acceleration.
- **Composite zone indexes:** over single-column-only pruning.
- **COVX sidecars:** over mutable in-file workload indexes.
- **COVM manifests:** over opening every archive file for planning.
- **Extension registry:** so custom logical types, indexes, synopses, encodings, and engine profiles are safe, discoverable, and either ignorable or required.
- **Arrow interop:** so COVE-T is useful without Harbor.
- **Lakehouse compatibility:** so COVE files can live inside existing catalog/table ecosystems.
- **Binary section directories:** over JSON-authoritative metadata.
- **Digest manifests:** over CRC-only archive integrity.
- **Self-contained object reconstruction:** over mandatory cross-file prev_ref.
- **WORM durable replace:** over in-place mutation.

**The final shape is:**

- **COVE-Core:** immutable binary foundation.
- **COVE-T:** engine-neutral table scan format.
- **COVE-A:** queryable archive acceleration profile.
- **COVE-E:** universal engine execution/mount profile.
- **COVE-H:** Harbor leased-code implementation of COVE-E.
- **COVE-O:** object-temporal profile.
- **COVX:** optional rebuildable accelerator sidecar.
- **COVM:** optional multi-file dataset manifest.
This gives Cove Format a neutral public identity, a strict portable decode path, rich queryable archive acceleration, and a universal execution-profile mechanism while preserving the Harbor-native fast path that inspired the design.
