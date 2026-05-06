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
| Semantic Mapping Artifact Magic | CMP1 |
| Legacy Draft Identifiers | Non-normative pre-COVE draft artifacts; not valid COVE v1 identifiers |
| Canonical Extension | .cove |
| Short Extension | None in v1; do not introduce .cov unless later required |
| Accelerator Sidecar Extension | .covx |
| Dataset Manifest Extension | .covm |
| Semantic Mapping Extension | .covemap |
| MIME Type | application/vnd.cove-format |
| Version | 1.0 |
| Byte Order | Little-endian throughout; no byte-order negotiation in v1 |
| Mutability | Immutable / write-once-read-many |
| Primary Purpose | Engine-neutral queryable offline/archive format with optional engine execution profiles and optional semantic source-to-object/association conversion and projection readback |

---

## 1. Specification Status

This document defines Cove Format v1.0, hereafter COVE.
COVE means Canonical Offline Value Encoding.
**COVE defines the following profiles and companion artifacts:**

- **COVE-Core:** Common immutable file structure, section directory, dictionary, logical/physical types, encoded arrays, checksums, validation, collation, canonical values, and extension rules.
- **COVE-T:** Engine-neutral table-scan profile.
- **COVE-A:** Archive acceleration profile for synopses, lookup indexes, composite pruning, manifests, and sidecar acceleration.
- **COVE-E:** Universal engine execution profile for mapping FileCodes into implementation-local ExecutionCodes.
- **COVE-H:** Optional named Harbor registration under COVE-E. Defines Harbor leased-code execution: FileCode -> Harbor EngineCode. COVE-H is not required for generic COVE conformance.
- **COVE-O:** Optional object-temporal extension profile for committed object history, deltas, branches, CSNs, baselines, snapshots, tombstones, and trust chains. COVE-O is not required for generic COVE conformance.
- **COVE-MAP:** Optional deterministic semantic mapping profile and companion `.covemap` artifact for converting one or more external source tables/files/streams into paired object-and-association semantic assertions, properties, temporal facts, and evidence that may be materialised as COVE-O and exposed through optional COVE-T/Arrow/SQL table projections. COVE-MAP is not required for generic COVE conformance.
- **COVX:** Optional accelerator sidecar.
- **COVM:** Optional dataset manifest.
A conforming COVE reader MUST be able to validate and read COVE files without COVX, COVM, or COVE-MAP.
COVX and COVM are optional acceleration and planning artifacts. They MUST NOT change the logical meaning of the referenced COVE files. COVE-MAP artifacts MUST NOT change the logical meaning of already materialised COVE files; they define how source data is converted, replayed, explained, or re-materialised into new COVE outputs.

### 1.1 Profile Maturity and Conformance Surface

COVE v1 is profile-scoped. Implementers MUST NOT treat the existence of an optional profile in this document as a requirement for baseline COVE conformance.
**Baseline v1 interoperability target:**
- COVE-Core structural validation and typed logical decode,
- COVE-T table scan reading,
- safe predicate metadata interpretation,
- Arrow-compatible export for supported logical types,
- a reproducible binary conformance vector set.
**Optional v1 profiles and artifacts:**
- COVE-A archive acceleration,
- COVE-E engine execution-code mapping,
- COVX accelerator sidecars,
- COVM dataset manifests,
- COVE-MAP semantic mapping artifacts when mapping tooling is claimed.
**Named engine registrations:**
- COVE-H is a Harbor-specific COVE-E registration. It demonstrates and standardises one engine profile; it is not a dependency of COVE-Core, COVE-T, COVE-A, or generic COVE-E.
**Optional extension profiles:**
- COVE-O is an optional object-temporal profile. It MAY be implemented by temporal-object engines, but general table readers SHOULD ignore COVE-O sections unless the requested operation explicitly requires object-temporal semantics.
- COVE-MAP is an optional v1 profile with a stable conceptual and conformance boundary: artifact magic, feature bit, validation boundary, identity model, and operation-level rules are part of v1. The reusable `.covemap` artifact framing is defined in this specification. Exact reusable mapping-definition payload schemas and binary `MAP_*` payload schemas SHOULD be defined by a companion COVE-MAP schema specification or by registered required extensions. General COVE readers SHOULD ignore COVE-MAP artifacts or sections unless the requested operation explicitly requires mapping validation, mapping replay, mapping explanation, source-to-object/association conversion, or mapping-defined projection readback.

A file that contains optional profile sections MUST advertise the corresponding feature bits. A reader that does not implement an advertised optional profile MUST either ignore the profile when it is not required for the requested operation, or reject the requested operation with a profile-not-supported error.
Implementations that claim COVE-MAP support SHOULD state whether they support only the stable v1 profile boundary or also one or more companion reusable-mapping payload schemas.

### 1.2 Named Engine and Product-Specific Terms

COVE is an engine-neutral format. Product-specific names are allowed only in named profiles, examples, registries, or non-normative implementation guidance.

Harbor is a named engine/profile registration that supplied the initial leased-code execution use case. Generic COVE text SHOULD use engine-neutral terms such as engine, scope, ExecutionCode, code-space, mapping, and profile. Harbor-specific concepts such as Harbor tenant UUID, Harbor EngineCode, Harbor lease, and Harbor mount cache apply only to COVE-H or to examples explicitly labelled as Harbor examples.

A COVE-Core, COVE-T, COVE-A, or generic COVE-E implementation MUST NOT require Harbor software, Harbor identity, Harbor tenancy, Harbor leases, or Harbor code spaces.

### 1.3 Standards Boundary

This specification admits only features that define portable wire semantics, validation behaviour, interoperability obligations, conformance levels, or extension contracts. Ecosystem tasks such as engine plugins, UI viewers, orchestration hooks, benchmark dashboards, and language bindings are valuable, but they do not belong in the normative core unless they introduce a stable artifact or reader/writer obligation.

**Rules:**
- COVE-Core and COVE-T MUST remain implementable without a lakehouse catalog, named engine profile, accelerator sidecar, object-temporal engine, or product-specific integration.
- New stable profiles MUST define feature bits, fallback behaviour, failure behaviour, security/privacy impact where relevant, and conformance vectors.
- Optional acceleration and ecosystem integration metadata MUST remain ignorable unless explicitly required by a feature bit or by the requested operation.

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
- optional engine-specific execution mappings through COVE-E,
- optional named engine execution registrations such as COVE-H,
- optional object-temporal history through COVE-O,
- optional deterministic multi-source semantic mapping into object-based COVE through COVE-MAP.
**COVE is not:**
- a WAL,
- a mutable database file,
- an in-flight transaction recovery log,
- a lakehouse catalog replacement,
- a lakehouse/table transaction protocol,
- a row-level delete or visibility protocol,
- an access-control system or encryption standard in v1,
- an Arrow IPC replacement,
- a generic Parquet clone,
- a format that persists engine-local ExecutionCodes as authoritative logical data,
- a mandatory ETL orchestrator, master-data-management system, probabilistic entity-resolution system, or AI-based schema matching system.
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
- **COVE-MAP:** optional deterministic source-row semantics for organisations that want to convert fragmented source tables, files, and streams into portable object-and-association COVE, and optionally expose that object-association truth through deterministic projected tables, without adopting a named runtime engine.

---

## 5. Profile Overview

| Profile | Name | Audience | Purpose |
| --- | --- | --- | --- |
| COVE-Core | Core Format | All readers/writers | File layout, sections, dictionary, encodings, checksums, validation. |
| COVE-T | Table Scan Profile | General engines | Engine-neutral columnar table scan profile. |
| COVE-A | Archive Acceleration Profile | Archive/query engines | Synopses, lookup indexes, manifests, composite pruning, sidecars. |
| COVE-E | Engine Execution Profile | All engines | Universal mapping from FileCodes to engine-local ExecutionCodes. |
| COVE-H | Harbor Execution Registration | Harbor implementations | Optional Harbor leased-code implementation of COVE-E. |
| COVE-O | Object Temporal Profile | Temporal-object engines | Optional object history, deltas, branches, CSNs, trust chains. |
| COVE-MAP | Semantic Mapping Profile | Conversion/governance/object/projection tools | Optional deterministic multi-source row semantics, identity joins, evidence, materialisation into object-and-association COVE, and deterministic readback as projected tables. |

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

**Custom engine:**
  symbol ID, interned value ID, dictionary key, catalog code, etc.

**COVE-H example:**
  leased Harbor EngineCode
COVE-Core does not define the meaning of an ExecutionCode.

COVE-E defines the universal mechanism for describing execution-code mappings.

**COVE-H defines Harbor’s implementation:**
FileCode -> Harbor EngineCode

---

### 6.3 Named Engine Example: Harbor EngineCode

This subsection is COVE-H specific. It is not part of COVE-Core, COVE-T, COVE-A, or generic COVE-E conformance.
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

**COVE-H example:**
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

**Bitmap layout:**
- Row i uses bit (i & 7) of byte (i >> 3).
- Bits are numbered least-significant-bit first within each byte.
- Unused high bits in the final byte MUST be zero.
- Implementations SHOULD name this structure null_bitmap or cove_null_bitmap, not validity_bitmap.

**Rationale:**
COVE stores a nullness bitmap rather than an Arrow-style validity bitmap because null is a structural exception and because all-zero freshly allocated bitmap memory represents the common all-non-null case. This convention also allows a null bitmap to be used directly as a null-rejection mask during predicate evaluation. The tradeoff is intentional: Arrow export requires inversion or materialisation of an Arrow validity bitmap, and conformance vectors MUST cover that conversion.

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

**Motivation:**
A morsel is an execution and pruning grain, not merely a compression block. It is intentionally smaller and more regular than a large table segment so engines can schedule work, build predicate bitmaps, remap FileCodes, and late-materialise selected columns without opening unrelated data. The 4096-row default balances:
- low per-morsel metadata overhead,
- cache-friendly predicate bitmaps and row-selection masks,
- simple row references with u16 row offsets,
- vectorised execution in 1024-row or 2048-row engines,
- practical packing of narrow columns.

**Relationship to other batching concepts:**
- Table segments may contain many morsels.
- Column pages are encoded per column and align to morsel row ranges unless an extension explicitly defines a different safe layout.
- Arrow RecordBatches are an output representation chosen by the reader; a reader MAY expose one morsel per RecordBatch, combine adjacent morsels, or split a morsel for downstream limits, provided logical row order and row-reference semantics are preserved.
- Morsel boundaries are the default unit for zone statistics, exact sets, bloom membership summaries, lookup row references, and predicate proof bitmaps.

**Vector alignment:**
- morsel_row_count SHOULD be a power of two.
- morsel_row_count SHOULD be a whole multiple of any declared execution vector size hint.
- Engine profiles that promise direct engine-vector materialisation MUST declare their execution vector size, or declare that no fixed vector size is assumed.
- Except for the final morsel in a segment, writers SHOULD NOT emit partial execution vectors within a morsel.
- The default 4096-row morsel is intentionally compatible with 2048-row and 1024-row execution vectors.

---


### 6.8 Semantic Object Identity and COVE-MAP Join Keys

COVE-MAP introduces a portable distinction between source-local row identity and semantic object identity.

A **source row identity** identifies a row, record, event, or payload within a declared source snapshot or source load. It is provenance, not object identity.

A **semantic object identity** identifies the destination object that source evidence contributes to. In COVE-MAP, semantic identity is produced only by declared identity rules and deterministic join keys.

A **semantic join key** is an ordered tuple of one or more canonicalised source values used to assert that source rows describe the same destination object. A join-key definition may bind the same semantic roles to columns from different source schemas, but each join key tuple is computed per source row or source record using only that source's declared bindings. Cross-source matching occurs because different source-specific bindings map into the same ordered semantic roles, not because values from multiple sources are combined before identity resolution.

**Example:**

```text
Customer.name_email_key:
  object_type: Customer
  confidence_class: strong_deterministic
  auto_merge: true
  components:
    - semantic_role: Customer.Name
      source_columns:
        crm.customers.name
        support.requester_name
      normalisation: cove.fn.person_name.v1
    - semantic_role: Customer.Email
      source_columns:
        crm.customers.email
        orders.customer_email
        support.requester_email
      normalisation: cove.fn.email.v1
  null_policy: all_components_required
```

Under this rule, a CRM row and a Support row with the same canonical `Customer.Name` and `Customer.Email` values produce the same strong deterministic identity key and may be merged into one `Customer` object. The same name alone would not merge unless a separate rule explicitly allowed it.

**Rules:**
- COVE-MAP join keys MUST be computed from canonical logical values, not FileCodes, source display bytes, locale defaults, or engine-local ExecutionCodes.
- Multi-column join keys MUST preserve the declared component order and MUST use length-delimited canonical component bytes before hashing or comparison.
- A join key that permits automatic object merge MUST declare its object type, component list, normalisation functions, null policy, confidence class, merge policy, and conflict policy.
- A confidence class in COVE-MAP is a declared deterministic rule class, not a probability. It MUST NOT be produced by hidden probabilistic or AI matching unless the mapping labels the result as candidate-only evidence.
- Candidate join keys MAY be emitted as evidence, but candidate keys MUST NOT change canonical object identity unless promoted by an explicit deterministic mapping rule in the declared mapping version.
- If two join keys would merge objects in violation of a declared do-not-merge rule, the mapper MUST apply the declared conflict behaviour: reject, keep separate, or emit conflict evidence.

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

**Write finalisation:**
- A writer MAY stream input records into temporary builder state, temporary files, or uncommitted segment buffers.
- A .cove object is valid only after the complete section directory, footer, postscript, and covered checksums have been written and validated.
- COVE v1 does not define partially visible incremental writes, append-in-place, or reader recovery from an unfinished .cove object.
- Streaming or incremental dataset publication MAY be built above COVE using new immutable COVE files plus COVM or an external catalog, but readers MUST NOT infer visibility from partially written COVE data.

Future versions MAY define appendable or streaming containers, but such containers MUST use new magic, feature bits, or profile rules so v1 readers cannot mistake them for immutable v1 COVE files.
See Section 50.4 for the v1 append, streaming, CDC, and compaction boundary when COVE files are used inside a dataset or external table system.

---

### 7.2 COVE is engine-neutral at the core

COVE-Core and COVE-T MUST be readable without Harbor.
**A non-Harbor reader may choose one of two paths:**
**Portable decode path:**
  FileCode -> dictionary value -> normal engine value / Arrow array

**Native execution path:**
  FileCode -> engine-local ExecutionCode -> native vector
COVE-H is Harbor-specific, but it is registered through the universal COVE-E mechanism.

**Specification style rule:**
Generic COVE-Core, COVE-T, COVE-A, and COVE-E text SHOULD avoid Harbor terminology except when contrasting a generic rule with the COVE-H registration. This keeps the portable format boundary clear for non-Harbor implementers.

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

### 7.3.1 Semantic mappings do not redefine materialised truth

COVE-MAP definitions describe how external source data is converted into COVE outputs. They do not redefine the logical values already present in a materialised COVE-Core, COVE-T, or COVE-O file.

**Rules:**
- A COVE-T reader MUST NOT need COVE-MAP to decode table values.
- A COVE-O reader MUST NOT need COVE-MAP to reconstruct object records that have already been materialised.
- COVE-MAP may be required for mapping replay, mapping explanation, source-to-object conversion, or audit of source evidence.
- Mapping identity comparisons MUST use canonical logical values and declared mapping functions. They MUST NOT compare FileCodes across files.

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
COVE v1 deliberately chooses one canonical byte order instead of storing a byte-order marker or negotiating host endianness. This keeps section parsing, memory-mapped fixed-width fields, checksum coverage, and conformance vectors deterministic.
**Rules:**
- Writers MUST emit little-endian values.
- Readers on big-endian hosts MUST byte-swap multi-byte scalar fields into host order before interpretation.
- Encoded byte streams whose algorithm defines its own byte order MUST follow that algorithm's registered COVE encoding definition.
- Future formats that want byte-order negotiation MUST use new magic, a new major version, or an explicitly incompatible required feature bit.

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
    // 6=COVE-MAP evidence/projection carrier inside a .cove file;
    //   reusable mapping definitions normally live in .covemap

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
| 0x0000_0000_0100_0000 | FEATURE_SEMANTIC_MAP | File or companion artifact contains COVE-MAP mapping, mapping evidence, identity-equivalence, source-conversion metadata, or object-association projection definitions. |
| 0x0000_0000_0200_0000 | FEATURE_PAGE_PAYLOAD_ELISION | File may contain stats-only constant pages or value-stream-elided pages whose reconstruction depends on page flags and validated page-level stats. |

**Rules:**
- Readers MUST reject unknown required feature bits.
- Readers MAY ignore unknown optional feature bits.
- FEATURE_SEMANTIC_MAP indicates the presence of COVE-MAP-related metadata. Whether that metadata is required depends on the requested operation and any required embedded profile or extension rules. Ordinary COVE-T or COVE-O reads MAY ignore optional mapping evidence, identity-equivalence, or projection metadata when mapping replay, explanation, conversion, or projection readback is not requested.
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
    // 6=COVE-MAP

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
**Directory granularity and lazy metadata:**
- The footer section directory SHOULD remain coarse-grained. Writers SHOULD NOT create one section entry per page or per morsel when a table segment index, column directory, or page index can describe the same data.
- Detailed table, segment, column, page, and morsel metadata SHOULD be stored in ordered arrays inside their profile sections.
- Readers MAY load the footer and top-level section directory eagerly, then lazily materialise table segment, column, and page metadata only for referenced tables, projected columns, and candidate morsels.
- Segment, morsel, and page lookup arrays SHOULD be ordered by table_id, segment_id, column_id, and morsel_id as applicable, so readers can use binary search without tree-shaped metadata structures.
- Lazy loading MUST NOT weaken validation. Any section or subsection used for pruning, decoding, or planning MUST be bounds-checked and checksum-validated before use.

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
| 60 | MAP_SOURCE_CATALOG | COVE-MAP | Source system/file/table/stream declarations and source-load fingerprints. |
| 61 | MAP_FUNCTION_REGISTRY | COVE-MAP | Declared deterministic normalisation, canonicalisation, hashing, and derivation functions. |
| 62 | MAP_IDENTITY_RULE_CATALOG | COVE-MAP | Object identity, multi-column join-key, confidence-class, merge, and do-not-merge rules. |
| 63 | MAP_ROW_SEMANTICS_CATALOG | COVE-MAP | Source row semantics: object, event, link, association, composite, dispatch, key/value fragment, projection, and evidence-only rules. |
| 64 | MAP_ASSERTION_LOG | COVE-MAP | Optional canonical semantic assertion stream produced by applying mapping rules. |
| 65 | MAP_IDENTITY_EQUIVALENCE_INDEX | COVE-MAP | Deterministic identity-key to destination-GOID/equivalence-set index. |
| 66 | MAP_EVIDENCE_INDEX | COVE-MAP | Source row, rule, digest, and output assertion evidence. |
| 67 | MAP_CONVERSION_REPORT | COVE-MAP | Conversion diagnostics, conflicts, candidate matches, rejected rows, and fidelity report. |
| 68 | MAP_PROJECTION_CATALOG | COVE-MAP | Object-and-association to table projection definitions and read-surface declarations. |
| 255 | VENDOR_EXTENSION | shared | Reserved extension section. |

MAP_* section payloads are COVE-MAP profile payloads. The authoritative reusable mapping definition normally lives in a `.covemap` artifact. MAP_* sections embedded in a `.cove` file are intended for mapping evidence, projection catalogs, conversion reports, identity-equivalence indexes, or embedded mapping snapshots tied to that file or dataset state; they MUST NOT silently override an explicitly referenced reusable mapping definition unless a required profile or extension defines that authority rule. A writer MUST NOT place MAP_* sections in an ordinary COVE file unless it advertises FEATURE_SEMANTIC_MAP and the payload schema is defined by the referenced COVE-MAP artifact/profile version or by a registered required extension. General COVE readers MUST ignore optional MAP_* sections for ordinary COVE-T or COVE-O reads. COVE-MAP-aware tools MUST validate MAP_* payload schemas, source fingerprints, function registries, and evidence references before using them for conversion, replay, projection, or explanation.

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
    reserved: u16,       // MUST be 0
}
```

Encoded length: 30 bytes.

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

### 20.5 Writer Encoding Selection Policy

Encoding selection is writer policy; it does not change file semantics. Readers observe only the emitted encoding tree, buffers, checksums, and validated metadata.
Writers SHOULD choose encodings per column page, normally one column per morsel. Different morsels of the same column MAY use different approved encoding cascades.
**Recommended analysis pass:**
1. collect page-local facts: row_count, null_count, non_null_count, distinct_count estimate or exact count, run_count, min/max, domain range, sortedness, value width, and candidate encoded sizes;
2. test every approved encoding cascade that is applicable to the page's logical and physical type;
3. assign each candidate a deterministic score representing estimated stored bytes plus optional read-cost penalties for the writer's declared hot/cold policy;
4. choose the lowest-score candidate;
5. emit only the chosen encoding tree and its buffers.
**Rules:**
- A candidate encoder MUST NOT be selected unless it has a canonical decode path available to conforming readers for the chosen profile, or is guarded by a required extension feature bit.
- Writers SHOULD evaluate Constant first. If a page is all-null or all non-null values are equal, Constant or stats-only constant storage SHOULD win unless another representation is proven smaller and equally decodable.
- Writers SHOULD NOT apply a general block codec to an already compact page when the codec increases size or materially harms the declared hot-scan cost class.
- Adaptive selection metadata MAY be recorded in non-authoritative writer metadata for observability, but readers MUST NOT require it for decoding.

### 20.6 Constant and Payload-Elided Storage

Constant encoding is a first-class storage optimisation, not only a predicate-statistics hint.
**Rules:**
- Constant pages MAY omit value buffers when the value can be reconstructed from Constant parameters or, for stats-only all-non-null pages, from a validated page-level ZoneStatsEntry under the rules in 27.2.
- Stats-only constant pages are allowed only for all-null pages or all-non-null pages. Mixed null/non-null constant pages MAY elide the value stream but MUST retain enough null-position information to reconstruct logical row order.
- If the constant value is stored in Constant parameters, the page-level ZoneStatsEntry SHOULD still set IS_CONSTANT and SHOULD use matching min_value and max_value when min/max are valid.
- If the constant value is stored only in stats, the stats entry is decode-required canonical data for that page. It MUST be checksummed, bounds-checked, type-checked, and collation-checked before decoding.
- Readers MUST NOT use raw FileCode min/max as the logical constant for comparable FileCode columns. The constant must be a FileCode equality value or a canonical/domain-ranked value according to the column's declared physical kind and domain rules.

### 20.7 Optional Specialized Encoding Gate

FSST-style string encoding and ALP-style floating-point encoding are accepted as high-priority v1 extension candidates, but they are not COVE-Core v1 encodings until their exact wire formats, parameters, canonical decode algorithms, feature bits, and conformance vectors are specified.
**Rules:**
- Writers MUST NOT emit FSST, ALP, Chimp, Patas, or similar specialised encodings as core CoveEncodingKind values unless this specification or a registered required extension defines their byte-level format.
- Optional specialised encodings MUST provide either a canonical fallback encoding for the same logical page or set a required feature bit that causes readers without support to reject safely.
- FSST-style encodings SHOULD be considered only for variable-byte data that is not already better represented through the file dictionary and FileCode path.
- ALP-style encodings SHOULD be considered for Float32 and Float64 NumCode columns only when the algorithm is lossless for the exact IEEE bit patterns, including signed zero, infinities, and NaN payload handling as specified by the extension.
- Chimp/Patas-style encodings remain experimental/vendor-extension candidates for v1 unless COVE adds normative bitstream definitions and float conformance vectors.

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
| 0x0000_0100 | PAGE_FLAG_STATS_ONLY_CONSTANT | No page payload exists; the page is reconstructed from page index counts and, for all-non-null pages, a validated page-level ZoneStatsEntry. Requires FEATURE_PAGE_PAYLOAD_ELISION. |
| 0x0000_0200 | PAGE_FLAG_ALL_NULL | Every row in the page is null. Requires FEATURE_PAGE_PAYLOAD_ELISION when used to omit payload or null-position data. |
| 0x0000_0400 | PAGE_FLAG_ALL_NON_NULL | Every row in the page is non-null. Requires FEATURE_PAGE_PAYLOAD_ELISION when used to omit payload or null-position data. |
| 0x0000_0800 | PAGE_FLAG_VALUE_STREAM_ELIDED | The non-null value stream is elided because the non-null value is constant. A null bitmap may still be present unless ALL_NULL or ALL_NON_NULL is set. Requires FEATURE_PAGE_PAYLOAD_ELISION. |
| 0xFFFF_F000 | reserved | Reserved for future required page extensions; MUST be zero in v1 unless a required extension defines the bit and the reader supports that extension. |

**Page codec rules:**
- PAGE_FLAG_COMPRESSION_CODEC applies only to the page payload bytes referenced by `page_offset` and `page_length`.
- Codec `None` requires `page_length == uncompressed_length`.
- LZ4 and Zstd page payloads use the same block codec definitions as Section 66 and require `uncompressed_length` to be the exact decoded byte length.
- If `page_length == 0`, `uncompressed_length` MUST also be zero.
- If `page_length > 0` and the page codec is not `None`, `uncompressed_length` MUST be non-zero.
- Writers that use LZ4 or Zstd page codecs MUST advertise the corresponding `FEATURE_CODEC_LZ4` or `FEATURE_CODEC_ZSTD` bit.
- Readers MUST reject unknown page codec values and any non-zero reserved page flag bits unless a required extension defines the bit and the reader supports that extension.

**Page flag consistency:**
- Page-elision flags are decode-affecting metadata. Writers that use PAGE_FLAG_STATS_ONLY_CONSTANT, PAGE_FLAG_ALL_NULL to omit null-position data, PAGE_FLAG_ALL_NON_NULL to omit null-position data, or PAGE_FLAG_VALUE_STREAM_ELIDED MUST set FEATURE_PAGE_PAYLOAD_ELISION in required_features. A reader that does not support FEATURE_PAGE_PAYLOAD_ELISION MUST reject the file before decoding those pages.
- PAGE_FLAG_ALL_NULL and PAGE_FLAG_ALL_NON_NULL are mutually exclusive.
- PAGE_FLAG_ALL_NULL requires null_count == row_count and non_null_count == 0. The null bitmap MAY be omitted only when FEATURE_PAGE_PAYLOAD_ELISION is required; any present null bitmap MUST contain only null bits for rows in the page with unused final-byte bits zeroed.
- PAGE_FLAG_ALL_NON_NULL requires null_count == 0 and non_null_count == row_count. The null bitmap MAY be omitted because every row is non-null; any present null bitmap MUST contain only zero bits with unused final-byte bits zeroed.
- If neither PAGE_FLAG_ALL_NULL nor PAGE_FLAG_ALL_NON_NULL is set, the counts still determine how much null-position information is required. A mixed null/non-null page MUST include a validated null-position representation; a page with null_count == 0 MAY omit the null bitmap.
- Page flags MUST be internally consistent with row_count, null_count, non_null_count, page_length, uncompressed_length, encoding_root, checksum, and any referenced stats_ref. A mismatch is page corruption; flags are not hints and MUST NOT override the counts or validated payload metadata.
- PAGE_FLAG_VALUE_STREAM_ELIDED requires the non-null value to be reconstructable from Constant encoding parameters or, only when PAGE_FLAG_STATS_ONLY_CONSTANT is also set, from the validated page-level ZoneStatsEntry rules below.

**Rules for payload-elided pages:**
- page_length MAY be zero only when PAGE_FLAG_STATS_ONLY_CONSTANT is set.
- If PAGE_FLAG_STATS_ONLY_CONSTANT is set, PAGE_FLAG_COMPRESSION_CODEC MUST be `CompressionCodec::None`, page_offset and uncompressed_length MUST be zero, encoding_root MUST be u32::MAX, and checksum MUST be CRC32C of the empty byte string.
- PAGE_FLAG_STATS_ONLY_CONSTANT requires either PAGE_FLAG_ALL_NULL or PAGE_FLAG_ALL_NON_NULL. Mixed null/non-null constant pages still need a null-position representation and therefore MUST NOT be stats-only.
- For all-null stats-only pages, null_count MUST equal row_count and non_null_count MUST be zero.
- For all-non-null stats-only pages, non_null_count MUST equal row_count, null_count MUST be zero, and stats_ref MUST reference a validated page-level ZoneStatsEntry with IS_CONSTANT and min_value == max_value under the declared logical type and collation rules.
- For Float32 and Float64 stats-only constant pages, the stats entry MUST preserve the exact raw IEEE value bits needed for reconstruction. If exact bits are not represented, including NaN payloads or signed-zero distinctions, the constant value MUST be stored in Constant parameters instead of stats-only storage.
- When PAGE_FLAG_STATS_ONLY_CONSTANT is set on an all-non-null page, the referenced stats entry is decode-required canonical data for that page, not optional pushdown metadata. A reader that cannot validate it MUST reject the page rather than fail open.

### 27.3 Page Payload

**A column page payload contains:**
[column page header]
[encoding node descriptors]
[buffer directory]
[buffers]

```rust
struct ColumnPagePayloadHeaderV1 {
    magic: [u8; 4],          // "CPG1"
    version_major: u16,      // 1
    header_len: u16,         // 36
    flags: u16,              // reserved, MUST be 0
    root_node_id: u16,
    node_count: u16,
    buffer_count: u16,
    row_count: u32,
    nodes_offset: u32,
    buffer_directory_offset: u32,
    buffers_offset: u32,
    reserved: u32,           // MUST be 0
}

enum PageBufferKind {
    NullBitmap = 0,
    Values = 1,
    Offsets = 2,
    ChildLayout = 3,
    Other = 255,
}

struct PageBufferDescriptorV1 {
    buffer_id: u16,           // dense 0..buffer_count-1
    kind: u16,                // PageBufferKind
    flags: u32,               // reserved, MUST be 0
    offset: u64,              // byte offset within this page payload
    length: u64,
    checksum: u32,            // CRC32C of this buffer
    reserved: u32,            // MUST be 0
}
```

**Container rules:**
- `nodes_offset` MUST equal `header_len`.
- `buffer_directory_offset` MUST equal `nodes_offset + node_count * 30`.
- `buffers_offset` MUST equal `buffer_directory_offset + buffer_count * 32`.
- `root_node_id` MUST identify exactly one `CoveEncodingNodeV1`, and that node's `logical_len` MUST equal the page row count.
- Buffer descriptors MUST be dense by `buffer_id`, in ascending non-overlapping offset order, and every buffer MUST lie inside the page payload.
- A non-elided page payload MUST be fully consumed by its buffer descriptors; trailing bytes are invalid.
- A buffer descriptor checksum mismatch is a page checksum failure.

**Logical row reconstruction:**
1. if PAGE_FLAG_STATS_ONLY_CONSTANT is set, reconstruct all rows from page index counts and, for all-non-null pages, the validated page-level stats entry;
2. otherwise read the null bitmap if present,
3. decode the non-null value stream,
4. re-expand values into logical row order.

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
FileCode -> Arrow dictionary key
FileCode -> DataFusion dictionary array key
FileCode -> DuckDB dictionary vector code
FileCode -> Polars categorical code
FileCode -> custom engine symbol ID
FileCode -> Harbor EngineCode under the optional COVE-H registration
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
**Generic lakehouse engine:**
  scope_kind = Catalog
  stable_id  = catalog/table namespace ID

**Single-file reader:**
  scope_kind = None

**COVE-H example:**
  scope_kind = Tenant
  stable_id  = Harbor tenant UUID

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
**Arrow dictionary output:**
  namespace = "org.apache.arrow"
  stable_id = dictionary batch or schema identifier
  epoch = 0 or batch/session epoch

**Custom engine:**
  namespace = globally unique engine namespace
  stable_id = implementation-specific code-space ID

**COVE-H example:**
  namespace = "io.harbor"
  stable_id = Harbor code-space UUID
  epoch = Harbor lease/code-space epoch
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

**COVE-H example:**
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
    SemanticMapping = 10,
    MappingFunction = 11,
    SourceConnector = 12,
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

### 49.1 Relationship to Arrow IPC

Arrow IPC is an interchange and transport format for Arrow record batches. COVE is a durable, immutable, query-planning-oriented storage format.
Use Arrow IPC when the primary requirement is to move or persist already-materialised Arrow arrays or RecordBatches with minimal semantic translation.
Use COVE when the primary requirement is offline/archive storage with file-level dictionaries, predicate-proof metadata, encoded arrays, optional lookup/synopsis indexes, digests, sidecars, and dataset manifests.
**Rules:**
- COVE readers MAY export Arrow arrays, RecordBatches, streams, or files.
- Arrow IPC is not a canonical serialisation of a COVE file. COVE section metadata, predicate proofs, FileCode domains, digests, and optional acceleration artifacts remain authoritative only in COVE/COVX/COVM.
- COVE writers MAY ingest Arrow IPC/Feather/RecordBatch streams as source data, but they SHOULD recompute COVE statistics, dictionaries, domains, and indexes from logical values rather than preserving Arrow batch boundaries as COVE morsel boundaries by default.
- A COVE-to-Arrow conversion MUST report or represent any COVE logical type, collation, extension type, or metadata guarantee that cannot be expressed exactly in Arrow.
**Zero-copy interop:**
- Zero-copy Arrow export is an implementation optimisation, not a COVE conformance requirement.
- A reader MAY expose COVE buffers to Arrow without copying only when the COVE physical layout, offsets, endianness, nullability representation, alignment, lifetime, and dictionary key width are compatible with the Arrow array being produced.
- When COVE null bitmaps, encoded pages, FileCode widths, nested offsets, or dictionary values do not match the target Arrow layout, the reader MUST materialise compatible Arrow buffers rather than exposing incompatible COVE bytes as Arrow memory.
- Writers SHOULD NOT weaken COVE encoding, statistics, or predicate metadata solely to maximise zero-copy Arrow export.

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

COVE is a file format, not a catalog or table format.
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

### 50.1 COVM vs Lakehouse Catalogs

COVM is not a transaction log, table catalog, or lakehouse protocol. It is a COVE-native planning manifest for a set of immutable COVE files.
**Rules:**
- When COVE files are managed by Iceberg, Delta, Hudi, or another table format, that external catalog remains authoritative for table snapshot selection, transactions, schema evolution, deletes, and visibility.
- COVM MAY mirror catalog-derived file lists and pruning metadata for faster COVE-native planning, but COVM MUST NOT override the external catalog's selected snapshot or visibility rules.
- Standalone COVM datasets MAY use immutable COVM publication to identify a dataset state, but this is a lightweight archive/dataset mechanism, not a replacement for ACID table protocols.
- Lakehouse hints inside COVE are descriptive hints. They MUST be validated against the external catalog before being used as authoritative table metadata.

### 50.2 COVE as Data Files in Table Formats

COVE v1 intentionally does not define a COVE Table Layer with ACID commits, catalog state, snapshot isolation, schema evolution, partition evolution, or transaction logs. Those responsibilities belong to an external table format or catalog.

Official integration specifications MAY define how COVE-T files are used as data files inside Iceberg, Delta, Hudi, Hive-style catalogs, Unity-style catalogs, or engine-specific catalogs. Such adapter specifications MUST:
- keep .cove files immutable,
- identify data files by URI plus stable file_id, file_len, footer_crc32c, and digest where available,
- map external table schema fields to COVE table_id/column_id without changing the COVE file schema,
- apply the external catalog's snapshot, partition, delete, visibility, time-travel, and schema-evolution rules before returning rows,
- treat LAKEHOUSE_HINTS, COVM entries, and metadata JSON as hints unless the external catalog explicitly accepts them,
- reject or ignore any COVE hint that conflicts with the selected external snapshot.

A future COVE-native table protocol, if one is ever standardised, MUST be a separate companion specification with its own conformance level, commit protocol, and feature gates. It MUST NOT weaken the immutability or standalone readability of COVE data files.

### 50.3 External Delete and Visibility Overlay Semantics

External row-level deletes, deletion vectors, equality deletes, access filters, and visibility overlays are outside COVE-Core and COVE-T v1. They MAY be referenced by lakehouse hints or manifests, but their semantics are defined by the external table format, catalog, or application protocol.

COVE predicate metadata and indexes describe the physical rows present in the immutable COVE file before external visibility filtering. When an external overlay is active:
- PredicateZoneOutcome::DefinitelyNo remains safe for pruning because no physical row in the zone satisfies the predicate.
- PredicateZoneOutcome::DefinitelyYes remains safe only as a claim that every remaining visible row from that physical zone satisfies the predicate; it does not prove that any visible row remains.
- Unknown remains Unknown.
- Exact sets, blooms, ColumnDomain ranges, and zone stats MAY be used to reject impossible predicates over the physical file, but they MUST NOT be interpreted as exact visible-table domains unless the overlay is proven empty or overlay-aware metadata is available.
- Lookup indexes and inverted morsel indexes return physical row candidates. Readers MUST apply the external visibility/delete overlay before returning rows.
- Aggregate synopses over a COVE file are exact only for the physical COVE rows. They MUST NOT answer visible-table aggregate queries when a non-empty external overlay is active unless an overlay-aware correction or proof is applied.

External overlays that reference physical positions SHOULD identify the target COVE file by file_id plus file length, footer CRC, and cryptographic digest where available. Rewritten or compacted COVE files receive new physical row references; overlays for old files MUST NOT be silently applied to rewritten files.

### 50.4 Append, Streaming, CDC, and Compaction Boundary

**The accepted mutable-data pattern for COVE v1 is immutable-file publication:**
- append by writing additional complete COVE files and publishing a new COVM state or external table snapshot,
- update/delete by external table-format overlays or by rewriting affected data into new COVE files,
- compact by writing replacement COVE files and publishing a new manifest/catalog state,
- ingest streams by buffering or micro-batching into temporary writer state, then finalising complete COVE files.

**Rules:**
- A .cove file MUST NOT be appended in place after finalisation.
- A partially written object MUST NOT be treated as a valid COVE file.
- Patch, delta, CDC, or operation-log files MAY be represented as ordinary COVE-T data files when an external protocol defines their meaning, but COVE-Core/COVE-T readers MUST treat them as ordinary data unless that external protocol is explicitly in scope.
- COVM readers MUST select one published dataset state. They MUST NOT merge multiple COVM generations as an implicit transaction log unless a separate protocol says to do so.
- Compaction MUST preserve logical table semantics according to the governing manifest or catalog; it MUST NOT mutate the replaced COVE files.

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
18. Analyze each column page/morsel and encode using COVE-approved encodings under the writer encoding selection policy.
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

### 51.4 Optional Physical Row Reordering

COVE-T writers MAY reorder rows within a table segment to improve compression, clustering, and pruning, but only when row order is not part of the dataset's logical contract.
**Rules:**
- Reordering MUST be opt-in writer behaviour.
- Reordering MUST NOT change the logical multiset of rows or any declared primary/lookup key semantics.
- Reordering MUST happen before morsel IDs, page indexes, zone stats, exact sets, bloom filters, lookup indexes, aggregate synopses, row references, and digest/trust inputs are generated.
- Writers MUST NOT apply physical row reordering to COVE-O temporal/object segments unless the profile explicitly proves that object history order, CSNs, baselines, deltas, tombstones, and trust chains remain semantically identical.
- If source row order is externally observable or needed for reproducibility, the writer SHOULD either disable reordering or materialise a source ordinal column before reordering.
- Writers SHOULD evaluate the benefit before committing a reorder. A reorder SHOULD be kept only when the estimated encoded size, pruning quality, or declared workload score improves enough to justify the additional write cost.
- Writer metadata MAY record the reorder policy and sort keys, but this metadata is descriptive and MUST NOT be required for logical decoding.
**Recommended heuristic:**
- Prefer stable clustering keys with low or medium cardinality and common predicate use.
- Avoid high-cardinality timestamp-only ordering unless time filtering is the dominant workload; coarse time buckets followed by other clustering keys are usually safer.
- Do not reorder nested, temporal, or trust-sensitive data unless the profile explicitly permits it.

### 51.5 Conversion Fidelity and Reporting

Converters are adoption-critical but are not allowed to redefine COVE semantics. A converter MUST NOT claim lossless conversion unless the declared conversion policy preserves logical values, nulls, schema semantics, decimal precision/scale, timestamp units/timezone interpretation, nested structure, map-key rules, and redaction/trust semantics for the supported source features.

**A converter SHOULD produce a machine-readable conversion report containing:**
- source format, source file identifiers, and source digests where available,
- source schema fingerprint and target COVE schema fingerprint,
- row count and column count,
- conversion policy version,
- unsupported or lossy source features,
- nested-shape fallbacks to Json or Binary,
- timestamp/timezone and decimal policies,
- collation and canonicalisation policies,
- row reordering policy, if any,
- generated COVE feature bits, section kinds, and acceleration artifacts,
- validation result for the produced COVE/COVX/COVM artifacts.

**Rules:**
- Source physical encodings, compression codecs, page boundaries, and statistics do not need to be preserved. COVE statistics and indexes SHOULD be recomputed from decoded logical values.
- Bidirectional tools such as Parquet <-> COVE, ORC <-> COVE, Arrow IPC <-> COVE, and CSV <-> COVE MUST distinguish logical round-trip fidelity from physical-layout preservation.
- If a source feature cannot be represented exactly in COVE-Core/COVE-T, the converter MUST either use a required extension, use a declared lossy fallback, or reject the conversion.

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

### 52.4 Fixed-Size Lists, Vectors, Tensors, and Embeddings

COVE-Core v1 does not define Vector, Tensor, or Embedding as additional scalar logical types. Dense vectors SHOULD be represented by existing nested or extension mechanisms rather than by adding ad hoc core scalar types.

**Recommended representation:**
- For maximum generic compatibility, store a dense fixed-length vector as List<Float32> or List<Float64> with ordinary List offsets and a schema-level fixed-length assertion.
- For space- and scan-optimised storage, a FixedSizeList or Tensor extension MAY elide offsets or use a specialised physical layout only when it declares a required feature bit or a safe List/Binary fallback.

**A FixedSizeList, Tensor, or Embedding extension MUST declare:**
- element logical type,
- dimension count and shape,
- row-major/column-major or other layout order,
- nullable element policy,
- whether vector length is fixed or variable,
- distance/similarity metrics if indexes depend on them,
- normalisation policy if cosine/dot-product semantics depend on it,
- Arrow base type or Arrow extension mapping where exported.

Approximate nearest-neighbour, vector, spatial, learned, or similarity indexes MUST be optional COVX or registered extension indexes. They MAY return candidates, but they MUST NOT be used for predicate exclusion, nearest-neighbour completeness, or metadata-only answers unless their descriptor proves exactness and a no-false-negative policy for the declared metric/query class.

### 52.5 Semi-Structured and Document Values

COVE Json is an opaque UTF-8 JSON payload unless a required extension declares stronger semantics. Core COVE readers MUST NOT assume semantic JSON equality, object-key ordering, numeric normalisation, path typing, or JSON path pushdown from the Json logical type alone.

**Rules:**
- JSON/path indexes MAY be stored as optional COVX or registered extension indexes. They MUST NOT change the logical Json payload.
- A semantic JSON/document extension MUST define canonicalisation, duplicate-key policy, missing-vs-null semantics, numeric normalisation, path type rules, and safe predicate outcomes.
- Without such an extension, Json columns are pushdown-limited to nullness, byte-level equality if declared safe, and indexes that explicitly state their proof semantics.
- COVE-O object-temporal semantics MUST NOT be used as an implicit replacement for general JSON/document semantics.

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
**Rules:**
- CoveTableRowRefV1 identifies a physical row position inside one immutable COVE file.
- External catalogs, delete vectors, lookup overlays, or audit systems that persist row references SHOULD pair the row reference with file_id and a validating file fingerprint such as file_len, footer_crc32c, or cryptographic digest.
- Row references are not stable across conversion, row reordering, compaction, or file rewrite unless an external protocol explicitly maps old references to new references.
- Readers MUST NOT apply row references from one file to another file solely because schemas or paths match.
If future morsels exceed u16::MAX rows, v2 must widen this field or introduce a new row reference type.

---

## 55. COVE-O Object Temporal Profile

COVE-O is an optional object-temporal extension profile. It is not part of baseline COVE-Core/COVE-T/COVE-A/COVE-E conformance.
COVE-O is designed for committed object history workloads and may be implemented by any engine with compatible temporal object semantics. Harbor is one possible implementation, not a dependency.
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
- optional temporal blooms,
- optional materialisation of COVE-MAP semantic assertions as object, property, link, association, evidence, or projection records.

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

    flags: u32,

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

**Object type flags:**

| Bit | Name | Meaning |
| --- | --- | --- |
| 0x0000_0001 | OBJECT_TYPE_FLAG_ENTITY_OBJECT | Type is primarily an entity/object identity surface. |
| 0x0000_0002 | OBJECT_TYPE_FLAG_EVENT_OBJECT | Type is primarily an event/transaction object. |
| 0x0000_0004 | OBJECT_TYPE_FLAG_LINK_OBJECT | Type is a first-class connector/link object. |
| 0x0000_0008 | OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT | Type materially represents an association between endpoint objects. |
| 0x0000_0010 | OBJECT_TYPE_FLAG_EVIDENCE_OBJECT | Type primarily carries evidence/provenance materialisation. |
| 0x0000_0020 | OBJECT_TYPE_FLAG_PROJECTION_OBJECT | Type is a materialised projection/read-surface helper rather than canonical object truth. |

**Property flags:**

| Bit | Name | Meaning |
| --- | --- | --- |
| 0x0000_0001 | PROPERTY_FLAG_ASSOCIATION_FROM_GOID | Property is the source/from endpoint GOID. |
| 0x0000_0002 | PROPERTY_FLAG_ASSOCIATION_TO_GOID | Property is the target/to endpoint GOID. |
| 0x0000_0004 | PROPERTY_FLAG_ASSOCIATION_TYPE | Property identifies the association type or role family. |
| 0x0000_0008 | PROPERTY_FLAG_ASSOCIATION_VALID_FROM | Property is the association validity start timestamp. |
| 0x0000_0010 | PROPERTY_FLAG_ASSOCIATION_VALID_TO | Property is the association validity end timestamp. |
| 0x0000_0020 | PROPERTY_FLAG_ASSOCIATION_OBSERVED_AT | Property records observation/materialisation time. |
| 0x0000_0040 | PROPERTY_FLAG_EVIDENCE_REF | Property references evidence/provenance material. |
| 0x0000_0080 | PROPERTY_FLAG_MAPPING_RULE_REF | Property references the mapping rule or projection rule that produced the materialised value. |

**Rules:**
- object_type_id MUST be unique.
- property_id MUST be unique within object_type_id.
- Top-level property declarations MUST NOT use logical Null.
- A writer that claims association readback MUST set OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT or OBJECT_TYPE_FLAG_LINK_OBJECT on every materialised association type and MUST flag association endpoint and semantics properties with the corresponding PROPERTY_FLAG_* bits.
- Readers SHOULD use ObjectTypeEntryV1.flags and PropertyEntryV1.flags, not property names alone, as the authoritative cues for association, evidence, and projection readback. Property names such as `from_goid`, `to_goid`, `association_type`, `source_evidence_id`, and `mapping_rule_id` remain recommended conventions only.
- An object type flagged OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT SHOULD expose exactly one PROPERTY_FLAG_ASSOCIATION_FROM_GOID property and exactly one PROPERTY_FLAG_ASSOCIATION_TO_GOID property unless a required extension defines a multi-endpoint association form.
- OBJECT_TYPE_FLAG_LINK_OBJECT and OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT MAY be set together when a type is both a first-class object and an association carrier. Other combinations that materially change readback semantics SHOULD be documented by the profile or extension that emits them.

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
- For scope-scoped COVE-O use, producer_scope_kind and producer_scope_id SHOULD identify the scope that owns the object history.
- COVE-H/COVE-O Harbor deployments commonly use producer_scope_kind = Tenant and producer_scope_id = Harbor tenant UUID.
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

**Profile-specific scope interpretation:**
- Generic COVE-O readers treat scope_id as the declared producer/object scope.
- COVE-H/COVE-O Harbor deployments interpret scope_id as Harbor tenant_id.

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


### 61.1 COVE-MAP Association and Evidence Materialisation

COVE-O v1 does not require a dedicated native edge section. When COVE-MAP produces association assertions and the destination is object-based COVE, a writer MUST materialise those associations using declared COVE-O object types unless a future association-capable COVE-O extension is explicitly required.

**Recommended object-type pattern:**

```text
Object type: CustomerPlacedOrder
Required properties:
  association_type        Utf8 or registered enum
  from_goid              FixedBytes(16)
  to_goid                FixedBytes(16)
  valid_from_us          nullable Timestamp
  valid_to_us            nullable Timestamp
  observed_at_us         nullable Timestamp
  source_evidence_id     nullable FixedBytes or Utf8
  mapping_rule_id        nullable Utf8
```

The property names above are recommended conventions, not the only interoperable spelling. When association readback is claimed, ObjectTypeEntryV1.flags and PropertyEntryV1.flags are authoritative for identifying association objects, endpoint properties, validity fields, evidence references, and mapping-rule references.

Link objects such as `OrderLine`, `Membership`, `CustomerAddress`, or `AccountManagerAssignment` MAY carry additional properties and MAY create multiple association-like references through `from_goid`, `to_goid`, or named endpoint properties.

**Rules:**
- Association materialisation MUST be declared in the COVE-MAP row semantics or output profile.
- Association endpoint GOIDs MUST be produced by the same deterministic identity-resolution run as the objects they connect.
- Evidence fields SHOULD point to MAP_EVIDENCE_INDEX entries or to declared source row digests when explanation is required.
- A COVE-O reader that does not understand COVE-MAP may still read the materialised association/link objects as ordinary object records.

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

### 64.1 Security and Privacy Boundary

COVE v1 provides corruption detection, optional cryptographic digests, redaction markers, and trust metadata. It does not define a complete access-control, key-management, or encrypted-storage protocol.

**Rules:**
- The encryption fields in v1 section specs and postscript specs MUST be 0. Encrypted sections, encrypted columns, authenticated encryption modes, key identifiers, key rotation, and associated-data rules require a future required extension or profile.
- Redaction is a logical/audit marker, not access control. If sensitive bytes are present unencrypted in a COVE file, COVE redaction metadata alone does not prevent disclosure.
- Column-level or row-level access control is external to COVE v1. Engines enforcing access policy MUST apply that policy before exposing decoded values, indexes, synopses, dictionaries, or metadata that could reveal protected data.
- Indexes, dictionaries, exact sets, blooms, histograms, Top-N summaries, and aggregate synopses may reveal value distributions. Writers handling sensitive datasets SHOULD omit or coarsen acceleration metadata according to policy.
- Differentially private, sampled, masked, or otherwise privacy-preserving statistics MUST be marked as approximate or policy-protected. They MUST NOT be used as exact aggregate synopses, exact value sets, or predicate-proof metadata unless the proof remains valid under the declared privacy transformation.

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

**Codec selection:**
- Compression codecs wrap already encoded byte buffers or sections; they are distinct from CoveEncodingKind array encodings.
- Writers SHOULD evaluate the codec choice after array encoding selection.
- Writers SHOULD leave already compact bit-packed, RLE, run-end, local-codebook, or stats-only constant pages uncompressed when a block codec does not reduce size.
- Writers MAY use Zstd for cold archive sections and LZ4 for hot scan sections, but MUST advertise any required codec feature bits.

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

### 67.1 Small Page Packing

COVE does not require fixed-size allocation blocks. Writers therefore SHOULD NOT import a database-style block allocation model that wastes space for narrow columns or tiny morsel pages.
**Recommended writer policy:**
- Pack small column pages contiguously inside TABLE_SEGMENT_DATA rather than aligning every page to a large block boundary.
- Small pages from different columns and morsels MAY share a page cluster when each ColumnPageIndexEntry still identifies the exact page_offset, page_length, uncompressed_length, flags, and checksum.
- Writers SHOULD use a tunable target cluster size for read coalescing and object-store range requests. The target is a writer/I/O policy, not a required allocation unit.
- Large pages MAY be placed in dedicated aligned ranges when doing so improves direct reads or decompression.
- Packing MUST NOT merge checksums across independently addressable pages unless an additional enclosing checksum is provided; each page checksum remains authoritative for that page's bytes.
- Packing MUST preserve morsel and column page boundaries at the logical level even when physical bytes are adjacent.

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
- COVX vector, ANN, spatial, learned, or workload-specific indexes MUST have a registered descriptor that declares proof capability, false-negative policy, metric/query class where relevant, and fallback behaviour.
- Approximate or candidate-generating COVX indexes MAY accelerate ranking or candidate selection, but MUST NOT advertise DefinitelyNo, DefinitelyYes, exact Top-N, or metadata-answerable semantics unless the index is exact for the declared query class.

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
- COVE-MAP artifact references, including projection catalogs where present,
- object-store hints.
**Rules:**
- COVM MUST be ignored if stale.
- COVM MUST NOT change COVE semantics.
- Query planners MAY use COVM to prune files before opening COVE footers.

### 69.3 COVM Publication and Atomic Update Discipline

COVM describes a dataset state. Updating a dataset means publishing a new dataset state; it MUST NOT mutate the logical meaning of any referenced immutable COVE file.
**Preferred publication model:**
1. write a complete new COVM object or file;
2. validate its header, section directory, footer/postscript, checksums, and referenced COVE digests when present;
3. publish it by an atomic rename, catalog pointer update, compare-and-swap object metadata update, or other external atomic reference mechanism.
**Rules:**
- COVM readers MUST validate freshness using referenced file_id, file_len, footer_crc32c, and digest fields when digest_algorithm is not None before trusting manifest pruning.
- A stale, corrupt, partially written, or unsupported COVM MUST be ignored; readers MUST fall back to opening COVE files directly.
- The dual-root/header rotation pattern MAY be used only as an optional local-filesystem publication protocol for mutable COVM pointers. It MUST NOT be required for canonical .covm objects and MUST NOT be applied to immutable COVE data files.
- If a local mutable COVM pointer file uses dual roots, each root slot MUST include a generation counter, COVM location or footer section spec, file length, digest or CRC, and checksum. Readers MUST choose the highest-generation root that fully validates; if neither validates, the COVM pointer is ignored.
- Object-store deployments SHOULD prefer immutable COVM objects plus an atomic catalog/reference update over in-place 4 KiB header writes.


### 69.4 COVM References to COVE-MAP

COVM MAY reference COVE-MAP artifacts for lineage, planning, conversion replay, or explanation.

**A COVM mapping reference SHOULD include:**
- mapping artifact URI or logical reference,
- mapping artifact digest and digest algorithm,
- mapping_id and mapping_version,
- source-set identity or source-load digest,
- output COVE file IDs produced by the mapping run,
- mapping execution ID,
- conversion report reference.

**Rules:**
- COVM MUST NOT be the sole authority for semantic mapping rules unless a future required profile explicitly defines that behaviour.
- COVM MUST NOT change the logical meaning of referenced COVE files.
- If a COVM mapping reference is stale, corrupt, or unsupported, readers MUST still be able to read the referenced COVE files. Only mapping replay/explanation operations fail or degrade.
- A COVE-MAP converter SHOULD emit a COVM dataset manifest when it materialises more than one output COVE file or when source lineage must be preserved across a dataset.

---


## 70. COVE-MAP Deterministic Semantic Mapping Profile

COVE-MAP is an optional profile and companion artifact for deterministic semantic mapping from one or more external source datasets into object-and-association-based COVE. In COVE-MAP, objects and associations are a paired semantic model: objects carry identity and properties; associations carry durable meaning between objects. Projected tables are read surfaces over that pair, not a replacement for it.

COVE-MAP is not part of baseline COVE-Core, COVE-T, COVE-A, COVE-E, COVE-H, or COVE-O conformance. A general COVE reader MUST NOT require COVE-MAP support to read ordinary materialised COVE-T or COVE-O files.

COVE-MAP is used when a tool needs to validate, replay, explain, perform source-to-object/association conversion, or expose COVE-O through deterministic table projections.

**Typical flow:**

```text
source tables/files/streams
  + COVE-MAP source catalog
  + source-local row semantics
  + deterministic semantic join keys
  + identity/conflict/provenance rules
  -> paired object-association semantic assertions
  -> COVE-O object-temporal output with materialised association/link records
  -> optional COVE-T/Arrow/SQL projections as read surfaces
  -> optional COVM manifest and evidence indexes
```

**Destination and read-surface rule:**
When the destination is object-based COVE, the materialised output SHOULD be valid COVE-O and SHOULD preserve both objects and associations as the semantic truth surface. Optional table projections MAY be emitted as COVE-T, Arrow record batches, SQL-accessible views, or other table-shaped read surfaces for engines that want relational scans over the object-association output. These projections MUST NOT redefine object identity, association identity, temporal history, or canonical property truth.

### 70.1 Artifact Boundary

COVE-MAP is an optional v1 profile with a stable conceptual and conformance boundary. Artifact identifiers, artifact framing, validation boundary, identity rules, projection/evidence rules, and operation-level fallback/rejection behaviour in this section are normative for v1. Exact reusable mapping-definition payload schemas and binary schemas for `MAP_*` payload bodies SHOULD be defined by a companion COVE-MAP schema specification or by a registered required extension.

A reusable mapping definition SHOULD be stored in a separate `.covemap` artifact. Embedded `MAP_*` sections inside a `.cove` file are typically file-local evidence, projection catalogs, conversion reports, identity-equivalence indexes, or embedded mapping snapshots tied to that file or dataset state. Unless a required profile or extension explicitly says otherwise, the `.covemap` artifact is the authoritative reusable mapping definition.

**COVEMAP final bytes:**
[postscript bytes]
[postscript_version: u16]
[postscript_len: u16]
[magic: "CMP1"]

`.covemap` uses the same tail-discovery pattern as COVE files. The postscript points to the CovemapHeaderV1 region rather than to a COVE footer.

```rust
struct CovemapPostscriptV1 {
  required_features: u64,
  optional_features: u64,
  file_len: u64,
  header_offset: u64,
  header_length: u64,
  checksum: u32,
}
```

```rust
struct CovemapHeaderV1 {
  magic: [u8; 4],          // "CMP1"

  header_len: u16,
  version_major: u16,
  version_minor: u16,

  flags: u32,

  mapping_id: [u8; 16],

  required_features: u64,
  optional_features: u64,

  section_count: u32,

  mapping_version_len: u16,
  reserved0: u16,

  created_at_us: i64,

  reserved: [u8; 32],

  checksum: u32,
}
// followed by:
//   mapping_version[mapping_version_len]
//   CovemapSectionEntryV1[section_count]
```

```rust
struct CovemapSectionEntryV1 {
  section_id: u32,         // MAP_* or VENDOR_EXTENSION
  offset: u64,
  length: u64,
  uncompressed_length: u64,
  compression: u8,
  required: u8,
  reserved: u16,
  checksum: u32,
}
```

**Suggested mapping artifact identifiers:**

| Field | Value |
| --- | --- |
| Artifact magic | `CMP1` |
| Extension | `.covemap` |
| Primary role | Deterministic source-row to semantic-assertion mapping |
| Output role | Produce COVE-O object/association output and optional COVE-T/COVM/COVX/projection artifacts |

A `.covemap` artifact may be referenced by COVM or by output COVE metadata using digest-verified references. COVM may reference mappings for lineage and replay, but COVM MUST NOT be the sole authority for semantic interpretation unless a future required profile defines that behaviour.

COVE-MAP artifacts MUST be immutable for a declared mapping version. A new mapping version may produce different output, but the mapping version, source snapshot/load identity, deterministic functions, and conflict rules must make the difference explainable.

The reference implementation defines its supported JSON companion payload schema in `docs/covemap-json-schema-v1.md`. That schema is the authority for JSON `MAP_*` payloads accepted by the reference `cove-map` tool; other payload encodings or richer grammars require a registered extension or a companion schema version.

**Rules:**
- `magic` MUST be `CMP1`.
- `mapping_version` identifies the reusable mapping-definition version; a new version MUST produce a new immutable artifact.
- `.covemap` postscript discovery MUST use absolute byte offsets from the start of the artifact. `header_offset` and `header_length` in CovemapPostscriptV1 MUST be within `file_len` and MUST locate the CovemapHeaderV1 region for the artifact version being read.
- `section_id` SHOULD reference `MAP_*` section kinds or `VENDOR_EXTENSION`.
- `offset` and `length` in CovemapSectionEntryV1 are absolute byte offsets from the start of the `.covemap` artifact unless a future required extension defines otherwise.
- `compression` in CovemapSectionEntryV1 uses the Section 66 `CompressionCodec` registry.
- If `compression` is `None`, `length` MUST equal `uncompressed_length`.
- If `compression` is not `None`, `uncompressed_length` MUST be the exact decoded byte length.
- If `length == 0`, `uncompressed_length` MUST also be zero.
- A `.covemap` artifact MUST be discoverable and integrity-checkable without consulting a COVE data file.
- The artifact framing defined here is stable for v1. Payload bodies MAY use JSON, YAML, canonical CBOR, or another registered encoding, but the payload schema MUST be versioned, checksum-validated, and attributable to the mapping/profile version or required extension that defines it.
- An implementation MAY claim COVE-MAP profile support without claiming every companion reusable-mapping payload schema. It SHOULD state which companion schema versions or required extensions it supports.

### 70.2 Source Catalog

A COVE-MAP source catalog describes the inputs that a mapping can consume.

**Supported source kinds may include:**
- COVE-T files,
- SQL tables or query snapshots,
- Parquet files,
- ORC files,
- CSV files,
- JSON/NDJSON exports,
- Arrow IPC/Feather data,
- application logs,
- event streams,
- API payload snapshots,
- other structured or semi-structured sources described by extension.

**A source entry SHOULD declare:**
- source_id,
- source_kind,
- source_uri or logical source reference,
- source_owner or producer,
- source_schema or schema fingerprint,
- source_load_id or snapshot identity,
- source_row_identity rule,
- source ordering rule when order-sensitive,
- source timestamp roles,
- source payload digest policy,
- source trust/sensitivity labels where applicable.

**Rules:**
- A mapping that claims replayability MUST identify source inputs by stable snapshot/load identity and digest or equivalent immutable source fingerprint.
- A source row number alone SHOULD NOT be the only source row identity unless it is paired with a source file digest and schema fingerprint.
- SQL/live source mappings MUST specify the snapshot, extraction query, transaction watermark, or export digest needed to reproduce the same rows.

### 70.3 Source-Local Row Semantics

Row semantics define what a source row means before identity resolution.

COVE-MAP row semantics are an engine-neutral inversion of operational row-semantics systems: source rows are interpreted into semantic assertions rather than live engine mutations.

**Row semantics kinds:**

| Kind | Meaning |
| --- | --- |
| Object | Row contributes to one independent destination object. |
| EventObject | Row creates a point-in-time event or transaction object. |
| LinkObject | Row creates a first-class connector object between other objects. |
| AssociationOnly | Row creates an association assertion without a separate object, unless materialised as a link object for COVE-O v1. |
| Composite | Row contributes to multiple objects and associations. |
| Dispatched | A discriminator value selects one of several row semantics rules. |
| KeyValueFragment | Row is an entity-attribute-value or sparse-property fragment. |
| ProjectionOnly | Row is a read-only projection and does not create new semantic truth unless declared. |
| EvidenceOnly | Row provides source evidence for existing objects/properties/associations. |
| Tombstone | Row represents deletion, closure, revocation, or absence according to a declared policy. |

**Rules:**
- A row semantics rule MUST declare the assertion kinds it may produce.
- Object and association assertions are designed to be consumed together. A mapping that declares association output MUST NOT treat those associations merely as optional foreign-key hints; they are durable semantic facts subject to identity, temporal, evidence, governance, and projection rules.
- Composite and dispatched semantics MUST be deterministic for each input row.
- ProjectionOnly rows MUST NOT create canonical object identity unless an explicit identity rule says they do.
- Tombstone semantics MUST declare whether the tombstone applies to an object, property, association, source-local record, or evidence assertion.

### 70.4 Semantic Assertions

COVE-MAP applies row semantics and identity rules to produce semantic assertions.

**Assertion kinds:**
- object assertion,
- property assertion,
- association assertion,
- temporal assertion,
- identity-key assertion,
- identity-equivalence assertion,
- candidate-match assertion,
- tombstone assertion,
- evidence assertion,
- conflict assertion.

A semantic assertion is not necessarily the final COVE-O row. It is the deterministic intermediate meaning produced from source data. A COVE-MAP converter may materialise assertions as COVE-O object records, COVE-O link/association object records, COVE-T projections, evidence indexes, conversion reports, or future association sections.

**Rules:**
- Assertion identity MUST be deterministic for a given source row identity, mapping rule ID, mapping version, and assertion payload.
- Assertion canonical bytes MUST use COVE canonical value encoding for logical values where applicable.
- A materialiser MUST NOT discard conflicts, candidate matches, or rejected rows silently when the mapping claims auditability.

### 70.5 Identity Rules and Multi-Column Semantic Join Keys

Identity rules determine which source rows contribute to the same destination object.

An identity rule may define one or more semantic join keys. A join key is a deterministic ordered tuple of canonicalised components.
A join key tuple is computed per source row or source record. Cross-source matching occurs because different source-specific column bindings map into the same ordered semantic roles, not because values from multiple sources are combined before identity resolution.

**Identity rule classes:**

| Class | May auto-merge? | Typical use |
| --- | --- | --- |
| authoritative | Yes | Source primary key or governed master key. |
| strong_deterministic | Yes, if declared | Exact canonical match on a high-confidence tuple such as email + name, national ID + date of birth, or external ID + issuer. |
| weak_deterministic | Not by default | Name + postcode, phone-only, or other collision-prone deterministic tuples. |
| source_scoped | Only within source scope | Source-local ID with no cross-source merge authority. |
| candidate | No | Suggested possible match retained as evidence. |
| do_not_merge | Prevents merge | Explicit negative match, conflict rule, privacy boundary, or known collision. |

**Multi-column join-key requirements:**
- object_type,
- identity_rule_id,
- key_family or semantic key name,
- confidence_class,
- auto_merge flag,
- component_count,
- declared component order,
- logical type for each component,
- semantic role for each component,
- source column bindings for each source,
- normalisation/canonicalisation function for each component,
- null/missing policy,
- duplicate/collision policy,
- do-not-merge behaviour,
- tie-breaker policy.

**Canonical tuple construction:**

```text
join_key_tuple_bytes =
  version_marker
  || object_type_id
  || identity_rule_id
  || component_count
  || for each component in declared order:
       component_role_id
       logical_type_id
       null_marker or length-prefixed canonical_value_bytes
```

If hashed, the hash input MUST be the canonical tuple bytes. Implementations MUST NOT hash display strings, source bytes, FileCodes, or engine-local ExecutionCodes as a substitute.

**Example: Customer high-confidence match**

```yaml
identity_rules:
  - id: customer.name_email.v1
    object_type: Customer
    class: strong_deterministic
    auto_merge: true
    null_policy: all_components_required
    components:
      - role: Customer.Name
        logical_type: Utf8
        normalise: cove.fn.person_name.v1
        bindings:
          crm.customers: name
          support.tickets: requester_name
      - role: Customer.Email
        logical_type: Utf8
        normalise: cove.fn.email.v1
        bindings:
          crm.customers: email
          orders.orders: customer_email
          support.tickets: requester_email
    do_not_merge:
      - rule: customer.email_marked_shared_or_role_account
      - rule: source_policy_boundary_conflict
```

A CRM row and a Support row whose canonical `Customer.Name` and canonical `Customer.Email` components match create the same strong deterministic join key and may contribute to one `Customer` object. A row with the same name but different email does not match this key. A row with the same email but a do-not-merge marker is kept separate or rejected according to policy.

A single source row may emit more than one identity-key assertion for the same row-semantics object output, for example a governed source ID, an email key, and a name-plus-email key. Those keys are separate evidence items; they become co-referential only under the equivalence rules below.

### 70.6 Identity Resolution Algorithm

A COVE-MAP implementation that claims deterministic identity resolution MUST implement an equivalent deterministic algorithm.

**Recommended abstract algorithm:**
1. For each source row, compute source row identity and source evidence digest.
2. Apply row semantics to produce identity-key, object, property, association, temporal, and evidence assertions.
3. Compute every declared join key using declared canonicalisation functions and null policies.
4. Partition keys by object type and identity rule scope.
5. Add merge edges only for authoritative or strong deterministic keys whose `auto_merge` policy is true.
6. Add candidate edges only as candidate-match assertions.
7. Apply do-not-merge constraints before forming final equivalence sets.
8. For each valid equivalence set, choose a canonical identity anchor using declared precedence: identity class, rule precedence, source priority, canonical key bytes, and source row identity tie-breakers.
9. Generate the destination GOID from the canonical anchor or from a declared external authoritative key.
10. Emit identity-equivalence and evidence records linking all contributing keys, source rows, and mapping rules.

**Rules:**
- The algorithm MUST produce the same equivalence sets and GOIDs for the same source data, source order declarations, mapping version, function versions, and conflict policy.
- If input row order can affect output, the mapping MUST declare a deterministic row ordering or reject replayability claims.
- Do-not-merge constraints take precedence over auto-merge edges.
- Identity-key assertions emitted for the same source row and the same row-semantics object output MAY declare co-reference. Co-referenced keys participate in the same identity equivalence graph only when their rule classes permit merge and no do-not-merge constraint applies.
- Candidate matches MUST NOT participate in GOID selection.
- A mapping MAY declare that unresolved identity conflicts reject the conversion, keep source-scoped objects separate, or emit conflict evidence. The default safe behaviour is rejection for canonical object output.

### 70.7 GOID Generation for Mapped Objects

COVE-O GOIDs produced by COVE-MAP SHOULD be deterministic within a declared mapping namespace.

**Recommended GOID input:**
- mapping namespace UUID or dataset namespace,
- mapping_id,
- mapping_version or declared identity-stability version,
- object_type_id,
- canonical identity anchor kind,
- canonical identity anchor bytes,
- optional source scope when identity is source-scoped.

**Rules:**
- GOIDs MUST NOT be derived from FileCodes.
- GOIDs SHOULD NOT be derived from non-canonical display strings.
- GOIDs generated from personal data SHOULD use a keyed or governance-approved digest policy when raw key exposure is a concern.
- If a mapping version changes identity precedence or canonicalisation functions, generated GOIDs may change unless an explicit identity-stability policy or alias index is used.
- A converter SHOULD emit an identity-equivalence index when multiple source keys or join keys map to the same GOID.

### 70.8 Property Mapping and Conflict Rules

A row may contribute property assertions to destination objects.

**Property mapping SHOULD declare:**
- destination object type and property ID/name,
- source column binding,
- logical type and conversion policy,
- normalisation or derivation function,
- temporal role if the value is time-qualified,
- source priority,
- null/missing semantics,
- conflict handling,
- evidence retention.

**Conflict behaviours:**
- source priority wins,
- latest observed value wins with deterministic tie-breaker,
- valid-time precedence,
- reject on conflict,
- keep multi-valued property,
- keep source-specific facets,
- canonicalise equivalent values,
- retain non-winning values as evidence.

**Rules:**
- Conflict rules MUST be declared when multiple sources may write the same canonical property.
- Time-based conflict rules MUST declare the temporal axis used and tie-breakers for equal timestamps.
- Null MUST NOT overwrite a non-null value unless the mapping explicitly defines null as clearing, tombstoning, or unknown.
- Non-winning values SHOULD be retained as evidence when auditability is claimed.

### 70.9 Association Mapping

COVE-MAP associations describe durable relationships between destination objects. Associations are first-class semantic outputs paired with objects. They SHOULD be inspectable, explainable, and projectable in the same way as objects.

**Association mapping SHOULD declare:**
- association type,
- endpoint object types,
- endpoint identity rules or aliases,
- direction and cardinality,
- association properties,
- temporal validity fields,
- duplicate handling,
- source evidence,
- materialisation strategy.

For COVE-O v1 destinations, association assertions SHOULD be materialised as link/association object types as described in Section 61.1 unless a future association-specific extension is required. A reader that exposes COVE-O as an object-association surface SHOULD present these materialised records as associations even though their v1 storage form is object records.

**Rules:**
- Association endpoints MUST resolve through deterministic identity resolution.
- Association duplicate handling MUST be deterministic.
- Association validity time MUST NOT be confused with COVE-O commit/file-ordering timestamp.
- Association readback MUST preserve declared direction, endpoint roles, association type, materialised association/link GOID where present, temporal validity, and evidence linkage.

### 70.10 Object-Association Read Surfaces and Projection Rules

COVE-MAP supports two complementary directions:

1. **Source-to-object/association mapping:** external rows become deterministic semantic assertions and are materialised as COVE-O objects, link/association records, temporal facts, and evidence.
2. **Object/association-to-table projection:** existing COVE-O object-association data is exposed as deterministic table-shaped read views for SQL, BI, Arrow, dataframe, debugging, or export workflows.

A projection rule defines a read-time or materialised view over the object-association semantic surface. It does not create a new source of truth unless the projected output is explicitly materialised as COVE-T with lineage back to the COVE-O source and projection definition.

Projection expression syntax in the examples below is non-normative pseudocode. A formal projection expression grammar, if standardised, MUST be defined by a companion COVE-MAP schema specification or a required extension.

**Reader surfaces:**

| Surface | Exposes | Required for baseline COVE-O? |
| --- | --- | --- |
| Object surface | Objects, properties, temporal history, GOIDs, tombstones | Yes for COVE-O readers. |
| Association surface | Associations/link records, endpoint roles, direction, cardinality, validity, evidence | Recommended for COVE-MAP-derived COVE-O; required when association readback is claimed. |
| Projection surface | Deterministic rows derived from objects and associations | Optional; required only when mapping-defined projection support is claimed. |
| Evidence surface | Source rows, mapping rules, assertion IDs, conflicts, and provenance | Optional; required only when explanation/audit support is claimed. |

**A projection rule SHOULD declare:**
- projection_id,
- output table or view name,
- output schema,
- row grain,
- anchor object type or association type,
- selected properties,
- association traversals,
- temporal mode or point-in-time cut,
- conflict/value selection policy,
- null and missing-value policy,
- cardinality explosion policy,
- duplicate handling,
- ordering policy,
- evidence inclusion policy,
- whether the projection is read-only, materialised, exportable as COVE-T, or exportable as Arrow/SQL rows.

**Recommended row grains:**

| Row grain | Meaning |
| --- | --- |
| `one_row_per_object` | One row per object of the anchor type. |
| `one_row_per_association` | One row per association of the anchor type. |
| `one_row_per_link_object` | One row per materialised link/association object. |
| `one_row_per_property_version` | One row per historical property value/version. |
| `one_row_per_event_object` | One row per event or transaction object. |
| `one_row_per_object_as_of_time` | One row per object at a declared temporal cut. |
| `one_row_per_evidence_assertion` | One row per source evidence or mapping assertion. |

**Example: object summary projection**

```yaml
projections:
  - id: customer_summary.v1
    output_table: customer_summary
    row_grain: one_row_per_object
    anchor:
      object_type: Customer
    temporal_mode:
      as_of: latest_committed
    columns:
      - name: customer_goid
        value: Customer.goid
      - name: display_name
        value: Customer.display_name
        conflict_policy: canonical_value
      - name: email
        value: Customer.email
        conflict_policy: canonical_value
      - name: order_count
        value: count(association(CustomerPlacedOrder))
      - name: latest_ticket_opened_at
        value: max(association(CustomerOpenedSupportTicket).SupportTicket.opened_at)
```

**Example: association edge projection**

```yaml
projections:
  - id: customer_order_edges.v1
    output_table: customer_order_edges
    row_grain: one_row_per_association
    anchor:
      association_type: CustomerPlacedOrder
    columns:
      - name: customer_goid
        value: association.source_goid
      - name: order_goid
        value: association.target_goid
      - name: association_goid
        value: association.goid
      - name: order_date
        value: Order.order_date
      - name: evidence_source
        value: evidence.source_id
```

**Rules:**
- Projection support is optional. A COVE-O reader MAY expose only the object surface unless it claims association, projection, or evidence readback support.
- A projection rule MUST be deterministic for a given COVE-O dataset state, mapping/projection version, temporal cut, and function registry.
- A projection rule MUST declare how multi-valued associations are handled: explode rows, aggregate, choose deterministic first/last, reject, or emit nested/list values where the target format supports them.
- A projection rule MUST declare whether it uses latest values, full history, valid-time state, observed-time state, or COVE-O commit/file-ordering state.
- A projected table view MUST NOT change object identity, association identity, canonical property truth, tombstone semantics, or evidence lineage.
- If a projected view is materialised as COVE-T, the COVE-T output SHOULD include lineage to the source COVE-O files, COVM dataset state where applicable, projection_id, projection_version, mapping/projection artifact digest, and temporal cut.

### 70.11 Temporal Roles

Source time fields must declare their temporal role.

**Temporal roles:**
- source event time,
- valid-from time,
- valid-to time,
- observed-at time,
- ingested-at time,
- source transaction time,
- mapping execution time,
- COVE-O commit/file-ordering timestamp.

Only a field explicitly mapped to COVE-O commit/file-ordering timestamp may populate COVE-O `timestamp_us`. Other temporal roles must be represented as properties, association validity fields, evidence fields, or future temporal-axis extensions.

### 70.12 Provenance and Evidence

COVE-MAP SHOULD preserve evidence linking output objects, properties, associations, identity decisions, conflicts, and tombstones back to source data.

**Minimum evidence for explainable output SHOULD include:**
- source_id,
- source_kind,
- source schema fingerprint,
- source load/snapshot identity,
- source row identity,
- source row digest or payload digest,
- mapping_id,
- mapping_version,
- mapping rule ID,
- mapping execution ID,
- output assertion ID,
- output object GOID or association/link GOID where materialised.

**Rules:**
- Evidence entries MUST be deterministic for a given mapping run.
- Evidence visibility MUST respect source governance/redaction policy.
- If evidence cannot be retained because of privacy/security policy, the mapping SHOULD retain a redacted evidence stub with digest and policy reference where allowed.

### 70.13 Deterministic Function Registry

COVE-MAP may reference deterministic functions for normalisation, canonicalisation, hashing, type coercion, and simple derivation.

**Function declarations SHOULD include:**
- function_id,
- function_version,
- input logical types,
- output logical type,
- null policy,
- Unicode normalisation policy,
- locale/collation policy,
- timezone policy when applicable,
- hash/digest algorithm when applicable,
- deterministic failure behaviour.

**Rules:**
- Functions used for identity MUST be declared and versioned.
- Functions used for identity MUST NOT depend on undeclared locale defaults, mutable external services, random values, network calls, wall-clock time, or implementation-defined ordering.
- A mapper MUST reject conversion if it cannot execute a required identity or property function exactly as declared.

### 70.14 Security, Governance, and Privacy

Semantic mapping can combine sources and reveal relationships not obvious in any single source.

**Rules:**
- A mapper MUST NOT silently weaken source access boundaries.
- If mapped output combines sources with different sensitivity labels, the output MUST preserve the most restrictive applicable policy metadata, emit declared governance reconciliation metadata, or reject conversion.
- Evidence indexes, identity-equivalence indexes, dictionaries, join-key digests, and conversion reports may leak sensitive information and must be governed like data.
- Join keys derived from personal or regulated data SHOULD use digest/redaction policies that avoid exposing raw identity components to unauthorised readers.
- COVE-MAP is not an access-control system. Readers and platforms remain responsible for enforcing policy.

### 70.15 Conversion Tool Contract

A COVE-MAP converter that targets object-and-association-based COVE SHOULD implement the following pipeline:

1. Validate mapping artifact and deterministic function registry.
2. Validate source snapshots, schema fingerprints, and source digests.
3. Read source rows using declared source row identity and ordering.
4. Apply source-local row semantics.
5. Compute semantic join keys and source evidence digests.
6. Resolve deterministic identity and produce GOIDs/equivalence sets.
7. Apply property and association conflict rules.
8. Produce semantic assertions and conversion diagnostics.
9. Materialise COVE-O object records and link/association object records.
10. Validate object-association readback semantics for the materialised output when association readback is claimed.
11. Optionally materialise or register COVE-MAP projection rules for COVE-T/Arrow/SQL relational query engines.
12. Emit evidence indexes and conversion report when auditability is claimed.
13. Emit COVM manifest references when a dataset has multiple output files or lineage artifacts.
14. Validate the produced COVE outputs independently of the mapping artifact.

**Recommended tools:**
- `cove-map validate`,
- `cove-map preview`,
- `cove-map plan-keys`,
- `cove-map convert`,
- `cove-map explain`,
- `cove-map diff`,
- `cove-map project`,
- `cove-map test`.

### 70.16 Non-Goals

COVE-MAP v1 deliberately does not define:
- probabilistic entity resolution as canonical identity,
- AI-based automatic mapping as canonical identity,
- a general ETL orchestration system,
- a master-data-management workflow,
- a business glossary standard,
- mutable catalog transactions,
- live database writes,
- a mandatory Harbor dependency,
- treating projected tables as more authoritative than the underlying object-association model.

Future extensions may support candidate suggestions, interactive approval workflows, or external resolver integrations, but such features MUST NOT silently change deterministic object identity in a COVE-MAP output.

---

## 71. Profile Capability Matrix

A public COVE implementation SHOULD declare which profile tier it supports.

| Feature | COVE-Core Reader | COVE-T Scan Reader | COVE-A Archive Reader | COVE-E Reader | COVE-H Harbor Reader | COVE-MAP Tool |
| --- | --- | --- | --- | --- | --- | --- |
| Validate header/footer/sections | Required | Required | Required | Required | Required | Required for COVE outputs |
| Decode FileCode to values | Required | Required | Required | Required | Required | Required when reading COVE sources/outputs |
| Decode NumCode columns | Required | Required | Required | Required | Required | Required when reading COVE sources/outputs |
| Arrow-compatible output | Recommended | Recommended | Recommended | Optional | Optional | Recommended for previews/projections |
| FileCode -> ExecutionCode | Optional | Recommended | Recommended | Required | Required as Harbor EngineCode | Optional; never identity truth |
| Engine profile registry | Optional | Optional | Optional | Required | Required | Optional |
| Morsel-aligned scanning | Optional | Required | Required | Optional | Required | Optional |
| Zone stats | Optional | Required | Required | Optional | Required | Optional |
| Predicate proof outcomes | Optional | Required | Required | Optional | Required | Optional |
| Exact sets | Optional | Recommended | Recommended | Optional | Recommended | Optional |
| Bloom filters | Optional | Recommended | Recommended | Optional | Recommended | Optional |
| Inverted morsel indexes | Optional | Optional | Recommended | Optional | Recommended | Optional |
| Lookup indexes | Optional | Optional | Recommended | Optional | Recommended | Optional |
| Aggregate synopses | Optional | Optional | Recommended | Optional | Recommended | Optional |
| Composite zone indexes | Optional | Optional | Recommended | Optional | Recommended | Optional |
| Top-N summaries | Optional | Optional | Recommended | Optional | Recommended | Optional |
| COVX sidecars | Optional | Optional | Optional | Optional | Optional | Optional |
| COVM manifests | Optional | Optional | Recommended | Optional | Recommended | Recommended for multi-file outputs |
| COVE-O object profile | Optional | Optional | Optional | Optional | Optional unless object-temporal semantics are requested | Required when destination is object-based COVE |
| COVE-O association readback | Optional | Not required | Optional | Optional | Recommended for object-association semantics | Required when association readback is claimed |
| COVE-MAP projection readback | Not required | Optional | Optional | Optional | Optional unless table projection is requested | Required when mapping-defined projection support is claimed |
| COVE-MAP semantic mapping | Not required | Not required | Not required | Not required | Not required unless mapping explanation is requested | Required |
| COVE-H Harbor mount profile | Not required | Not required | Not required | Not required | Required only for COVE-H | Not required |

---

## 72. Writer Profiles

### 72.1 COVE-Core Minimal Profile

**MUST emit:**
- valid header,
- valid postscript,
- valid footer,
- section directory,
- file dictionary if FileCode columns exist,
- valid checksums,
- valid logical/physical typing,
- valid null bitmaps, unless nullness is fully determined by valid page flags in a COVE-T stats-only constant page.

### 72.2 COVE-T Minimal Table Profile

**MUST emit:**
- all COVE-Core requirements,
- table catalog,
- table segment index,
- table segment data,
- column page indexes,
- page checksums,
- null counts,
- segment/morsel row counts.

This profile MUST NOT require COVE-A, COVE-E, COVE-H, COVE-O, COVX, COVM, or any required custom extension.

#### 72.2.1 COVE-T Starter Interoperability Subset

A first public reader/writer SHOULD target this subset before claiming broader COVE ecosystem readiness:
- COVE-Core plus COVE-T Minimal Table Profile,
- primitive Bool/Int/UInt/Float/Decimal/Date/Timestamp types,
- Utf8/Binary/Uuid through FileCode or VarBytes,
- ordinary List/Struct/Map only when Arrow-compatible mappings are implemented,
- uncompressed and LZ4 payloads,
- valid null bitmaps and all-null/all-non-null page flags,
- morsel_row_count = 4096 unless explicitly declared otherwise,
- morsel-level zone stats for numeric and comparable FileCode columns,
- Arrow-compatible export,
- no required COVE-A, COVE-E, COVE-H, COVE-O, COVX, or COVM dependencies.

Writers producing starter-subset files SHOULD avoid required extensions and exotic encodings. Readers implementing the starter subset MUST still reject unknown required feature bits and MUST remain correct when optional acceleration metadata is absent.

### 72.3 COVE-T Scan Profile

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
- adaptive per-page encoding selection,
- stats-only constant pages for all-null and all-non-null constant pages where supported,
- small page packing inside table segment data,
- LZ4 for hot scan pages.

### 72.4 COVE-A Archive Acceleration Profile

**Recommended for fast offline archives:**
- all COVE-T Scan Profile features,
- COVM manifest,
- digest manifest,
- FileCode histograms,
- lookup indexes,
- composite zone indexes,
- Top-N summaries for ordered hot columns,
- optional COVX sidecar,
- safe COVM publication using immutable manifests or an external atomic reference update,
- Zstd for cold page payloads where scan latency permits.

### 72.5 COVE-E Engine Execution Profile

**Recommended for engines with dictionary/coded execution:**
- engine profile registry,
- execution code descriptor,
- execution scope descriptor,
- code-space descriptor,
- engine mount policy,
- FileCode -> ExecutionCode mapping strategy,
- optional execution-code cache metadata,
- reverse lookup policy.

### 72.6 COVE-H Harbor Profile

**Recommended for Harbor:**
- all COVE-T Scan Profile features,
- COVE-E engine execution profile,
- FileCode -> Harbor EngineCode mount map,
- Harbor lease epoch tracking,
- Harbor code-space descriptor,
- Harbor mount cache key,
- direct Harbor vector materialisation,
- optional COVE-O object-temporal support.

### 72.7 COVE-O Object Checkpoint Profile

**Recommended for object state:**
- object type catalog,
- temporal segment index,
- self-contained baselines/snapshots,
- FileCode/NumCode property columns,
- temporal blooms,
- trust chain if compliance requires,
- redaction manifest if redactions are present.


### 72.8 COVE-MAP Object Conversion Profile

**Recommended for deterministic multi-source conversion into object-and-association-based COVE:**
- COVE-MAP mapping artifact or embedded mapping sections,
- source catalog with source identity, source kind, schema fingerprint, source load/snapshot identity, and source row identity rules,
- deterministic function registry with function IDs and versions,
- row semantics catalog defining whether rows produce objects, event objects, link objects, associations, composite records, dispatch records, key/value fragments, projections, tombstones, or evidence-only assertions,
- identity rule catalog with authoritative, strong deterministic, weak deterministic, source-scoped, candidate, and do-not-merge rules,
- multi-column semantic join keys for high-confidence cross-source object matching,
- deterministic conflict rules for property values and identity collisions,
- evidence index linking output objects/properties/associations to source rows and mapping rule IDs,
- COVE-O materialisation when the destination is object-based COVE, including materialised link/association object records when associations are produced,
- optional object-association readback metadata for readers that expose associations as a first-class surface,
- optional projection catalog for deterministic object/association-to-table readback,
- optional COVE-T projections for query compatibility,
- optional COVM manifest referencing mapping artifact, source set, conversion report, and output files.

A COVE-MAP writer that claims object-conversion conformance MUST produce COVE-O output that is valid without requiring the mapping artifact for ordinary object reconstruction. If the writer claims association readback, it MUST preserve sufficient metadata for associations/link records to be exposed as associations rather than only as generic objects. The mapping artifact may be required for replay, explanation, conflict audit, projection readback, or source-row traceability.

---

## 73. Validation Model

### 73.1 Bootstrap Validation

1. Read trailing magic.
2. Read postscript_len and postscript_version.
3. Read postscript.
4. Validate postscript checksum.
5. Validate file_len.
6. Locate footer.
7. Validate footer CRC via postscript section spec.
8. Parse footer and section directory.

### 73.2 Structural Validation

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

### 73.3 COVE-T Semantic Validation

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

### 73.4 COVE-E Semantic Validation

- engine profile namespace valid,
- execution descriptor valid,
- scope descriptor valid,
- code-space descriptor valid,
- mount policy valid,
- execution mapping optional or required according to requested operation,
- unknown required profiles rejected only when needed.

### 73.5 COVE-O Semantic Validation

- object_type_id exists,
- property_id exists,
- scope values valid if scope-scoped,
- rows sorted by required order,
- csn/timestamp monotonicity holds,
- prev_ref targets valid rows,
- prev_ref target kind matches,
- reconstruction self-containment holds.

### 73.6 COVE-MAP Semantic Validation

A COVE-MAP-aware validator MUST validate the mapping artifact and any embedded mapping sections before using them for conversion, replay, or explanation.

**Validation requirements:**
- source IDs unique within the mapping artifact,
- source schema fingerprints present when source replay is claimed,
- source row identity rules deterministic and non-empty,
- mapping_id and mapping_version present,
- mapping function IDs declared with explicit versions,
- no undeclared random, wall-clock, locale-default, network, or mutable external dependency,
- identity rules reference existing object types and semantic roles,
- multi-column join-key components have declared logical types, canonicalisation, null policy, and ordering,
- auto-merge rules use authoritative or deterministic confidence classes only,
- candidate rules do not alter canonical object identity,
- do-not-merge constraints are checked before equivalence classes are materialised,
- property conflict rules are declared for multi-source canonical properties,
- association endpoints resolve to deterministic object identities,
- output COVE-O object records satisfy COVE-O validation,
- evidence entries refer to valid source IDs, source row identities or digests, mapping rule IDs, and output assertion IDs.

---

## 74. Recovery and Failure Behavior

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
| COVE-MAP artifact stale/corrupt | Ignore for ordinary reads; reject mapping replay/explanation/conversion if required |
| COVE-MAP identity conflict | Apply declared conflict behaviour; reject if no safe declared behaviour exists |
| Segment checksum mismatch | Reject segment; fail read unless explicit best-effort mode |
| Page checksum mismatch | Reject page; fail read unless explicit best-effort mode |
| Invalid FileCode | Treat as corruption |
| Invalid NumCode/logical type pairing | Schema error |
| Invalid prev_ref | Reject COVE-O file |
| Unsafe min/max | Do not use for skipping |

Best-effort mode MAY skip corrupt segments only when explicitly requested by recovery/export tooling.
Normal readers fail closed for structural corruption.

---

## 75. Durable Replace Protocol

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

## 76. Error Codes

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
| COVE_E_MAP_INVALID | COVE-MAP mapping artifact or embedded mapping section is malformed. |
| COVE_E_MAP_FUNCTION_UNDECLARED | Mapping references an undeclared or unsupported deterministic function. |
| COVE_E_MAP_IDENTITY_CONFLICT | Declared identity rules produce an unresolved merge/do-not-merge conflict. |
| COVE_E_MAP_SOURCE_STALE | Source snapshot, schema fingerprint, or source digest does not match the mapping run. |
| COVE_E_MAP_EVIDENCE_INVALID | Mapping evidence references a missing source, rule, row, assertion, or output object. |

---

## 77. Compatibility

### 77.1 Versioning

**COVE v1 readers support:**
version_major = 1
**Rules:**
- Readers MUST reject unsupported major versions.
- Readers MAY accept newer minor versions if no unknown required features are set.

### 77.2 Required vs Optional Features

Required features are needed for correctness.
Optional features are accelerators or metadata.
**Examples:**
**Required:**
  - codec needed to decode projected data,
  - nested column support when projected,
  - trust-chain support when verification is requested,
  - engine profile required by requested output mode,
  - COVE-MAP artifact required by requested mapping replay, source-to-object conversion, or mapping explanation operation.

**Optional:**
  - bloom filters,
  - exact sets,
  - lookup indexes,
  - aggregate synopses,
  - Top-N summaries,
  - COVX sidecars,
  - COVM manifests,
  - optional engine profile mappings,
  - COVE-MAP mapping artifacts and evidence when ordinary table/object reading does not request mapping replay or explanation.

---

## 78. Conformance Requirements

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
**A COVE-MAP-aware tool MUST additionally:**
- validate mapping artifacts and embedded mapping sections before use,
- compute identity join keys from canonical logical values,
- apply declared normalisation and canonicalisation function versions,
- preserve declared component order for multi-column join keys,
- keep candidate matches separate from canonical object identity unless explicitly promoted by deterministic mapping rules,
- enforce do-not-merge constraints before automatic object merge,
- materialise object-based destinations as valid COVE-O files when COVE-O output is requested,
- preserve evidence sufficient to explain source row -> object/property/association output when explanation is claimed,
- reject or report unresolved identity/property conflicts according to declared policy,
- never require Harbor for COVE-MAP conversion or COVE-O output.
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

## 79. Open Conformance Suite

A public interoperability release of COVE SHOULD NOT claim broad v1 readiness without a working reference reader, reference writer, and binary conformance suite. The wire format is defined by this specification, but adoption depends on reproducible test artifacts. An implementation SHOULD NOT claim COVE-Core or COVE-T conformance until it passes the applicable public vectors for that level.

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
11. COVE-MAP multi-source conversion corpus when COVE-MAP tooling is claimed.
12. Benchmark suite.
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
- Harbor EngineCode remap overhead,
- COVE-MAP source-to-object conversion cost and identity-resolution cost when COVE-MAP tooling is claimed.

### 79.1 Minimum Binary Test Vector Contract

**Each public conformance vector SHOULD include:**
- one or more binary .cove/.covx/.covm files,
- a machine-readable expected logical result set or expected validation error,
- expected cove-inspect/cove-dump metadata summaries,
- declared conformance level and required feature bits,
- producer version and vector version,
- checksum and digest expectations where applicable.

Negative vectors SHOULD name the expected error class rather than depending on exact implementation wording. Optional-profile vectors MUST state which profile is being tested; COVE-H and COVE-O vectors are optional unless an implementation claims those profiles.

**Conformance vectors SHOULD cover:**
- header/footer/postscript validation,
- dictionary FileCode resolution,
- null bitmap semantics, including bit order, final-byte padding, all-null/all-non-null flags, and Arrow validity inversion,
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
- Arrow interop mapping,
- Arrow IPC conversion boundaries,
- lakehouse/COVM manifest freshness and visibility rules,
- external delete/visibility overlay safety for pruning, lookup, and aggregate synopses,
- row-reference file fingerprint validation,
- conversion fidelity reports and lossy conversion rejection,
- FixedSizeList/vector/tensor extension fallback behaviour,
- approximate COVX index proof-capability restrictions,
- Json opaque semantics and semantic-JSON extension behaviour,
- security/privacy boundary cases including redaction, omitted sensitive indexes, and approximate/private statistics,
- streaming-writer finalisation and partially written file rejection,
- COVE-MAP source catalog validation, deterministic function registry validation, multi-column join-key canonicalisation, candidate-vs-canonical identity separation, do-not-merge enforcement, source evidence traceability, object-and-association-based COVE-O output validation, association readback validation, and projection-rule validation.

---

## 80. Utilities and Supporting Artifacts

The public COVE project SHOULD ship the following utilities and artifacts.

### 80.1 Reference Libraries

- **cove-core:** Format primitives, checksums, section directory, dictionary, encoded arrays, validation, collation, extension registry.
- **cove-reader:** Read COVE-Core and COVE-T files.
- **cove-writer:** Write COVE-Core and COVE-T files.
- **cove-arrow:** Export COVE data as Arrow arrays / record batches.
- **cove-engine:** COVE-E engine execution profile helpers.
- **cove-harbor:** Optional COVE-H Harbor mount profile implementation.
- **cove-convert:** Conversion library for Parquet/CSV/Arrow/ORC -> COVE-T.
- **cove-map:** Optional COVE-MAP library for deterministic source-row semantics, multi-source identity joins, evidence tracking, materialisation into COVE-O object/association outputs, and deterministic object/association-to-table projections.

### 80.2 CLI Tools

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
- **cove-conversion-report:** Emit machine-readable conversion fidelity reports for source-to-COVE and COVE-to-source conversions.
- **cove-map-validate:** Validate COVE-MAP artifacts, source declarations, deterministic function registries, identity rules, row semantics, and output profiles.
- **cove-map-preview:** Show source-row to semantic-assertion output before materialisation.
- **cove-map-convert:** Convert multiple declared sources into object-and-association-based COVE-O output and optional COVE-T projections.
- **cove-map-explain:** Explain how source rows, join keys, rules, conflicts, and evidence produced an object/property/association output or projected table row.
- **cove-map-diff:** Compare outputs from two mapping versions or two source snapshots.
- **cove-map-test:** Run mapping fixtures with known input rows, expected join-key values, expected GOIDs, expected conflicts, and expected COVE-O output.
- **cove-map-plan-keys:** Inspect multi-column join keys, component null rates, duplicate rates, candidate conflicts, and do-not-merge collisions before conversion.
- **cove-map-project:** Expose COVE-O object/association data as deterministic projected tables, Arrow record batches, or materialised COVE-T outputs using declared projection rules.

- **cove-explain-pruning:** Explain file, segment, morsel, and page pruning decisions, including which statistic, domain, exact set, bloom, lookup index, synopsis, COVM entry, or COVX artifact produced DefinitelyNo, DefinitelyYes, or Unknown.
- **cove-plan-cost:** Estimate projected I/O, morsel pruning, index utility, and expected scan work for representative predicates.

### 80.3 Engine Integrations

**Recommended initial integrations:**

- **Arrow:** COVE -> Arrow arrays and record batches.
- **DataFusion:** COVE TableProvider.
- **DuckDB:** COVE scan extension / table function.
- **Polars:** COVE scan/read support.
- **Spark / Trino / Presto / ClickHouse:** Optional read-only adapters or table-format data-file adapters once COVE-T conformance vectors are stable.
- **Python:** cove.read_table(), cove.scan(), cove.to_arrow(), cove.to_polars().
- **Java / Scala:** Table-format and engine adapters where JVM ecosystem integration is required.
- **Go:** Lightweight validation, inspection, and service-side read bindings.
- **Rust:** cove-core, cove-io, cove-arrow, cove-datafusion, cove-engine.
- **WASM / embedded:** Optional lightweight COVE-Core/COVE-T validation and projection readers with optional profiles disabled by default.
- **Harbor:** Optional COVE-H direct leased-code mount support.

**Integration guidance:**
- Engine integrations SHOULD start read-only until COVE-Core/COVE-T conformance vectors pass.
- An engine integration MUST NOT reinterpret optional acceleration metadata as required table semantics.
- Table-format adapters MUST apply external catalog visibility and delete rules before returning rows.

### 80.4 Dataset and Benchmark Corpus

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
- **semantic-mapping:** CRM/orders/support style multi-source datasets where `Customer.Name` + `Customer.Email` produces a strong deterministic object match, with candidate-name-only and do-not-merge negative cases.
- **engine-profile:** FileCode -> ExecutionCode mapping tests for generic and Arrow profiles; Harbor vectors are required only for COVE-H claims.

**Benchmark reporting:**
- Public performance claims SHOULD publish dataset versions, query definitions, selected columns/predicates, hardware, storage medium, cold/warm cache state, thread count, engine version, COVE writer settings, comparator format settings, and reproducible scripts.
- Benchmarks SHOULD separate file-size, conversion cost, cold planning latency, warm planning latency, scan CPU, decompression CPU, index build cost, and end-to-end query latency.
- A benchmark MUST NOT claim format-level superiority when the result depends on a non-portable engine shortcut that is unavailable to the compared format, unless the shortcut is explicitly disclosed.

### 80.5 Governance Artifacts

**For open adoption, the project SHOULD publish:**
- formal binary specification,
- semantic versioning policy,
- feature bit registry,
- section kind registry,
- encoding kind registry,
- extension registry,
- engine profile registry,
- collation registry,
- COVE-MAP deterministic function registry,
- COVE-MAP identity confidence-class and row-semantics registry,
- test vector registry,
- implementation conformance levels,
- performance benchmark methodology,
- security model,
- trademark/name guidance,
- extension proposal process.

Governance rules SHOULD ensure that required feature bits, section kinds, encoding IDs, and profile registrations are not controlled by a single proprietary engine or vendor-specific implementation. Named engine registrations are allowed, but they MUST remain optional unless a reader explicitly claims that named profile.
**Governance for new stable features SHOULD require:**
- an extension proposal or specification patch,
- assigned feature bits and registry entries where applicable,
- fallback and unknown-reader behaviour,
- security/privacy review for features that expose, hide, encrypt, redact, or approximate data,
- positive and negative conformance vectors,
- reference implementation support,
- interoperability evidence from at least one independent implementation before broad ecosystem conformance claims are made.

---

## 81. Summary of v1 Design Decisions

**COVE v1 chooses:**

- **Neutral public name:** Cove Format, with Harbor represented only as an optional named COVE-H profile.
- **File-local FileCodes:** over persisted engine-owned codes.
- **ExecutionCode abstraction:** so non-Harbor engines can map FileCodes into their own runtime representations.
- **COVE-E universal engine execution profile:** over making any one engine's mount behaviour the generic extension mechanism.
- **COVE-H Harbor profile:** optional Harbor leased-code execution as one registered COVE-E implementation.
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
- **No COVE table protocol in v1:** over duplicating Iceberg/Delta/Hudi-style ACID catalog responsibilities inside the file spec.
- **External visibility overlays:** so delete vectors and table snapshots can be applied safely without changing immutable COVE file semantics.
- **Binary section directories:** over JSON-authoritative metadata.
- **Digest manifests:** over CRC-only archive integrity.
- **Self-contained object reconstruction:** over mandatory cross-file prev_ref.
- **WORM durable replace:** over in-place mutation.
- **Extension-gated vectors, tensors, semantic JSON, encryption, and advanced indexes:** over adding immature workload-specific semantics to COVE-Core v1.
- **COVE-MAP as an optional conversion/projection profile:** over embedding multi-source identity resolution, business-object mapping, source reconciliation, association readback, or object-to-table projection semantics into COVE-Core or COVE-T.
- **Deterministic multi-column semantic join keys:** over probabilistic or hidden matching for canonical object identity.

**The final shape is:**

- **COVE-Core:** immutable binary foundation.
- **COVE-T:** engine-neutral table scan format.
- **COVE-A:** queryable archive acceleration profile.
- **COVE-E:** universal engine execution/mount profile.
- **COVE-H:** optional Harbor leased-code implementation of COVE-E.
- **COVE-O:** optional object-temporal extension profile.
- **COVE-MAP:** optional deterministic semantic mapping and multi-source object-conversion profile.
- **COVX:** optional rebuildable accelerator sidecar.
- **COVM:** optional multi-file dataset manifest.
This gives Cove Format a neutral public identity, a strict portable decode path, rich queryable archive acceleration, a universal execution-profile mechanism, and an optional path from fragmented source data into object-based COVE while allowing named engine fast paths such as COVE-H without making them dependencies of the core format.
