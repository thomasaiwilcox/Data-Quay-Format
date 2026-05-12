# COVE Standards Suite v2.0 — Full Specification
> **Specification status:** This is the full-detail combined v2.0 specification and the current normative baseline for implementation and conformance-vector development. It is derived from the supplied COVE v1.0 specification, the COVE v2 rewrite, and the Harbor Row Semantics material. It intentionally preserves the original COVE identity while adding optional registered codec, layout-planning, zero-copy, runtime-registry, conservative query-coverage, optional secondary-index, runtime/local coverage-cache, and richer deterministic row-semantics mechanisms.
>
> **Non-reduction rule:** This is not a micro-spec, summary, or reduced profile. The document remains a full specification. Implementation staging and future split documents are conformance and organisation tools only; they MUST NOT reduce the normative detail below the original.
>
> **Document model:** This combined specification is written as one document for review and implementation, but each major part is designed to split cleanly into a standalone full standard: COVE-Core, COVE-T, COVE-COVERAGE, COVE-A, COVE-I, COVE-E, COVE-H, COVE-O, COVE-MAP, COVE-CX, COVE-L, COVE-R, COVE-CACHE, COVE-Interop, and COVE-Conformance.

**COVE:** Canonical Offline Value Encoding
Cove Format is a Canonical Offline Value Encoding: an immutable,
queryable offline/archive format for portable logical values, encoded
arrays, proof-carrying predicate and coverage metadata, optional acceleration
artifacts, optional secondary indexes, and engine-local execution mappings.

| Field | Value |
| --- | --- |
| Format Name | Cove Format |
| Formal Expansion | Canonical Offline Value Encoding |
| Normative Acronym | COVE |
| Public Short Name | Cove |
| Primary Data File Magic | COV2 |
| Footer Magic | CV2F |
| Accelerator Sidecar Magic | CVX2 |
| Dataset Manifest Magic | CVM2 |
| Semantic Mapping Artifact Magic | CMP2 |
| Secondary Index Artifact Magic | CVI2 |
| Runtime Coverage Cache Artifact Magic | None normative in v2; implementation-defined if persisted locally |
| Legacy Draft Identifiers | Non-normative pre-COVE draft artifacts; not valid COVE v2 identifiers |
| Canonical Extension | .cove |
| Short Extension | None in v2; do not introduce .cov unless later required |
| Accelerator Sidecar Extension | .covx |
| Dataset Manifest Extension | .covm |
| Semantic Mapping Extension | .covemap |
| Secondary Index Extension | .covi |
| Runtime Coverage Cache Extension | None normative in v2; implementation-defined and non-canonical if persisted locally |
| MIME Type | application/vnd.cove-format |
| Version | 2.0 full-detail combined specification |
| Byte Order | Little-endian throughout; no byte-order negotiation in v2 |
| Mutability | Immutable / write-once-read-many |
| Primary Purpose | Engine-neutral queryable offline/archive format with optional engine execution profiles, optional semantic source-to-object/association conversion, registered lossless codec extensions, optional coverage proofs, optional secondary indexes, optional layout/split planning metadata, optional runtime/local coverage caches, and optional runtime registry interoperability hints |
| V2 Compatibility Posture | COVE v2 uses new magic and major version fields. v2 readers MAY support v1 files; v1 readers MUST reject v2 files. |
| V2 Identity Rule | Catalog/schema, canonical logical values, predicate-proof metadata, validated coverage proofs used for pruning or metadata-only answers, COVE-O truth, COVE-MAP mapping/replay truth when requested, COVM publication state, and digest/trust/redaction surfaces remain authoritative. Codec, layout, zero-copy, COVX acceleration, writer cost metadata, and runtime-registry additions are non-authoritative unless explicitly required for decode or for the requested operation. |
| Standards Suite Rule | COVE-Core and COVE-T are the first public implementation target, but not a smaller spec. Other profiles, including COVE-COVERAGE, COVE-I, COVE-CACHE, COVE-MAP, COVE-CX, and COVE-L, are optional standards with explicit feature bits, fallback behaviour, validation boundaries, conformance claims, and full normative detail when defined. |

The generated v2 capability matrix in `conformance/capability_matrix.md` is the implementation-status record for this workspace. It distinguishes fully gated conformance from partial, unit-only, and vector-family scoped implementation evidence.


---

## 0. Standards Suite Scope, Detail Preservation, and Split Plan

COVE v2 is a **standards suite** for immutable, canonical, queryable archive data. The primary `.cove` file stores portable logical values and validated physical encodings. Companion artifacts may describe acceleration, manifests, semantic mappings, codec registrations, layout plans, and runtime compatibility. Only explicitly authoritative surfaces define logical truth. Optional surfaces are ignorable unless required by feature bit or by the requested operation.

This combined specification is intentionally one document for design review and implementation. It SHOULD later be split into the following standalone standards without changing the meaning of the combined specification. A split standard MUST remain a full specification for its scope, not a micro-spec, overview, or thin adapter note. Each split standard MUST carry its own normative structures, validation rules, feature bits, fallback behaviour, failure behaviour, conformance requirements, and test-vector obligations.

| Part | Future standard | Scope |
| --- | --- | --- |
| Part 0 | COVE-Overview | Positioning, identity, terminology, conformance tiers, and the standards-suite map. |
| Part 1 | COVE-Core | File layout, primitives, canonical values, feature model, dictionary, extension registry, checksums, digests, validation, and error model. |
| Part 2 | COVE-T | Table catalog, segments, morsels, pages, encoded arrays, null semantics, ColumnDomains, predicate proofs, and table scans. |
| Part 3 | COVE-COVERAGE | Formal conservative query coverage semantics: coverage sets, tightness, coverage degree, proof strength, provider metadata, interval forms, and do-no-harm planning. |
| Part 4 | COVE-A | Archive/query acceleration: exact sets, blooms, inverted indexes, lookup indexes, synopses, composite zones, Top-N summaries, COVX, and COVM planning. |
| Part 5 | COVE-I | Optional secondary index artifacts, root indexes, value-to-fragment mappings, index-only capabilities, and snapshot validity. |
| Part 6 | COVE-E | Generic FileCode-to-ExecutionCode execution mapping for engines. |
| Part 7 | COVE-H | Harbor leased-code registration under COVE-E. |
| Part 8 | COVE-O | Object-temporal profile: object catalogs, temporal segments, deltas, baselines, snapshots, branches, tombstones, and trust chains. |
| Part 9 | COVE-MAP | Deterministic semantic mapping from source rows into object/property/association/evidence assertions, dimensional coverage maps, and projections. |
| Part 10 | COVE-CX | Registered lossless codec extension framework, codec descriptors, registered encoding envelopes, fallback payloads, and codec conformance vectors. |
| Part 11 | COVE-L | Layout planning, scan splits, page clusters, fast metadata indexes, zero-copy buffer maps, and object-store range planning. |
| Part 12 | COVE-R | Runtime/session registry guidance and optional runtime compatibility hints. |
| Part 13 | COVE-CACHE | Optional mutable runtime/local predicate coverage cache with snapshot-bound validity and explicit non-authority. |
| Part 14 | COVE-Interop | Arrow, Parquet/ORC/CSV/Arrow IPC conversion, lakehouse integration, external visibility overlays, and publication rules. |
| Part 15 | COVE-Conformance | Reader/writer levels, conformance vectors, negative corpora, registries, governance, and benchmark methodology. |

### 0.1 Full-Detail Specification Rule

COVE v2 MUST NOT become a micro-spec. The combined specification and any future split standards MUST remain detailed enough for independent implementation without private knowledge.

**Rules:**
- A normative profile MUST define its binary structures, field meanings, enum values, validation rules, required and optional feature bits, fallback behaviour, failure behaviour, and conformance requirements.
- A split document such as `COVE-MAP`, `COVE-CX`, or `COVE-L` MUST be a full standard for that domain, not a summary of the combined specification.
- Implementation staging, starter subsets, and recommended first targets are adoption tools only. They MUST NOT remove detail from the standard.
- Extension schema specifications MAY define additional extension payload grammars where the main document intentionally reserves an extension point, but they MUST NOT replace or narrow normative profile grammars defined in this combined specification. COVE-MAP v2 mapping payloads are defined by this document, not by a separate required schema.
- A feature that is not specified in enough detail for independent implementation MUST remain explicitly provisional, experimental, or registry-reserved, and MUST NOT be required for broad conformance.
- No future editorial split may delete a normative structure, validation rule, fallback rule, failure rule, or conformance requirement merely because it is optional to implement.

### 0.2 First Public Implementation Target, Not Specification Reduction

The first public implementation and interoperability target for COVE v2 MAY be staged so implementers can ship a correct reader/writer before every optional profile is implemented. This staging is **not** a reduction of the specification. COVE v2 remains a full-detail standards suite, and optional profiles remain fully specified when this document defines them.

A first implementation target SHOULD prioritise:

- COVE-Core structural validation;
- COVE-T table scan reading and writing;
- FileCode and NumCode decode;
- structural null bitmap handling;
- page checksums and section validation;
- safe predicate metadata interpretation;
- morsel-level zone statistics for common primitive and comparable FileCode columns;
- Arrow-compatible export for supported logical types;
- a reproducible binary conformance vector set.

COVE-A, COVE-E, COVE-H, COVE-O, COVE-MAP, COVE-CX, COVE-L, COVE-R, COVX, and COVM remain optional implementation/conformance claims unless an implementation explicitly claims those standards or a requested operation requires them. Their optionality does not make them lesser, sketch-level, or non-normative. Where the combined specification defines their wire structures and behaviour, they MUST be specified at full detail.

### 0.3 Authoritative and Advisory Surfaces

COVE v2 preserves the original COVE principle that portable logical truth must not depend on an engine, a sidecar, a layout tree, or a runtime plugin registry.

**Authoritative surfaces include:**

- the COVE file header, postscript, footer, and binary section directory;
- required feature declarations and validated required sections;
- table and object catalogs;
- canonical logical values and canonical value encodings;
- file-local FileCode dictionaries and ColumnDomain ordering;
- NumCode interpretation by declared logical type;
- structural null bitmaps and page reconstruction rules;
- predicate-proof and coverage-proof metadata when it is used to skip, include, or answer data;
- COVE-O object-temporal reconstruction rules;
- COVE-MAP mapping artifact semantics when mapping conversion, replay, explanation, or projection readback is requested;
- digest manifests, trust chains, redaction manifests, and COVM publication state when those policies are requested or required.

**Advisory or non-authoritative surfaces include, unless explicitly required for decode or for the requested operation:**

- COVX acceleration indexes and workload-specific sidecars;
- COVE-I secondary index artifacts unless their exactness, snapshot validity, and proof contract are validated for the requested operation;
- COVE-CACHE runtime/local coverage caches;
- COVM planning hints other than the selected publication state itself;
- COVE-L layout-plan nodes;
- scan split indexes;
- page-cluster directories;
- zero-copy buffer maps;
- runtime compatibility hints and runtime registry bindings;
- engine execution profiles and ExecutionCodes;
- writer cost-model metadata;
- advisory statistics, coverage estimates, and cost estimates not marked proof-safe or not validated.

A reader MUST NOT use advisory metadata to change query results. A reader MAY use advisory metadata for planning, performance, diagnostics, or runtime dispatch after validation.

### 0.4 Value Preservation Rule

This v2 combined specification MUST preserve the value of the original COVE v1 design:

- immutable write-once-read-many `.cove` files;
- engine-neutral COVE-Core and COVE-T readability;
- file-local FileCodes, with FileCode(0) as an ordinary value and never a null sentinel;
- ExecutionCodes as engine-local runtime values, never portable logical truth;
- structural nulls represented by a null bitmap where `1 = null`;
- morsel-aligned scanning, pruning, late materialisation, and execution remap;
- ColumnDomain-based logical ordering for FileCode columns;
- conservative predicate-proof pushdown with `DefinitelyNo`, `DefinitelyYes`, and `Unknown`;
- optional exact sets, blooms, lookup indexes, synopses, composite zones, Top-N summaries, COVX, and COVM;
- COVE-O self-contained object reconstruction;
- COVE-MAP deterministic identity, object/association semantics, evidence, and projection readback;
- Arrow and lakehouse interoperability without making Arrow IPC or a table protocol the COVE identity;
- durable replace publication and rejection of partially written COVE files;
- public conformance vectors and negative validation corpus.

### 0.5 New v2 Maturity Rule

COVE v2 adds modern mechanisms only when they are subordinate to COVE logical truth:

- COVE-CX registered codecs MAY improve compression and scan performance, but codec names and plugin IDs are not sufficient wire semantics.
- COVE-L layout plans MAY improve object-store and lazy-read planning, but layout nodes are not schema authority or predicate proof.
- Zero-copy maps MAY reduce export cost, but target compatibility must be proven before exposing COVE buffers directly.
- COVE-R runtime/session guidance MAY help implementations instantiate codecs, kernels, functions, and adapters, but process-global runtime state MUST NOT define on-disk semantics.
- COVE-MAP MAY use Harbor-inspired row semantics, but Harbor runtime write behaviour remains a named implementation influence, not a COVE-Core requirement.

### 0.6 Standards Boundary for Harbor Row Semantics

The Harbor Row Semantics model is valuable because it clearly separates what a source row **is** from how meaning is derived from it. COVE-MAP adopts the engine-neutral version of that idea for offline deterministic mapping.

**Boundary rule:**

- Harbor Row Semantics answers: *what should a SQL mutation do inside Harbor now?*
- COVE-MAP answers: *what deterministic semantic assertions does this source row produce for archive materialisation, replay, explanation, or projection?*

COVE-MAP MUST NOT require Harbor software, Harbor tenancy, Harbor leases, or Harbor object graph runtime behaviour. Harbor may implement COVE-MAP and COVE-O efficiently through COVE-H, but that is a named profile registration, not a core dependency.


### 0.6A Accepted and Constrained Additions from Coverage Review

The coverage-centred additions are accepted with constraints so they strengthen COVE without weakening the original format.

**Accepted into the v2 suite:** COVE-COVERAGE, predicate normal forms, interval predicate forms, balanced coverage plan candidates, sidecar validity, index-only capability declarations, COVE-I secondary indexes, COVX kernel descriptors, dimensional coverage maps, late materialisation/export capabilities, and stronger benchmark metrics.

**Accepted but constrained:** COVE-CACHE is useful as runtime/local snapshot-bound state, but it is not a canonical COVE artifact and MUST NOT be required for logical correctness. Hardware acceleration descriptors are useful, but they are optional capability metadata and MUST NOT make a file vendor-hardware-dependent unless a non-portable required extension explicitly says so.

**Not adopted as core requirements:** mandatory global secondary indexes, mandatory hardware acceleration, a table/lakehouse transaction protocol, mutable in-file caches, a universal query optimiser encoded in bytes, and any claim that COVE is generally faster than Parquet or replaces Iceberg/Delta/Hudi.

### 0.7 Coverage-Aware v2 Identity

COVE v2 SHOULD be understood as a **coverage-aware value/archive format**, not merely as a columnar layout with optional statistics. A coverage-aware format describes not only how values are stored, but which validated fragments are sufficient to answer or evaluate a predicate without reading the whole dataset.

**Coverage principle:**

A COVE coverage artifact may over-include data, but it MUST NOT under-include data when it is used for pruning, metadata-only answers, lookup routing, or index-only access. An artifact that may under-include data is approximate or advisory and MUST NOT be used to skip candidate data unless a required extension explicitly defines a bounded-loss query semantics and the query requests that semantics.

**Coverage-aware identity surfaces:**

- COVE-T zone stats, exact sets, blooms, inverted morsel indexes, lookup indexes, aggregate synopses, and composite zones may act as coverage providers when their proof semantics are validated.
- COVE-A and COVX may carry rebuildable acceleration providers, but they remain semantics-preserving.
- COVE-I may carry optional secondary indexes that map values, intervals, object paths, or dimensional buckets to files, segments, pages, morsels, row ranges, row ordinals, objects, or projection fragments.
- COVE-MAP may define object, association, semantic path, and dimensional mappings that allow coverage over non-flat or object-derived data.
- COVE-L may describe how coverage fragments correspond to byte ranges, page clusters, scan splits, and object-store requests.
- COVE-CACHE may remember previously validated coverage sets for a dataset snapshot, but it is mutable runtime/local state and never canonical truth.

**Rules:**
- Coverage metadata MUST declare its granularity, proof strength, exactness, snapshot validity, referenced logical context, and fallback behaviour.
- Coverage metadata MUST be checksummed and bounds-checked before use.
- Coverage metadata MUST be interpreted under the declared logical type, collation, null semantics, canonicalisation rules, and feature/profile version.
- A coverage provider MUST NOT silently substitute physical-code comparisons for logical comparisons unless the encoding explicitly declares them safe.
- Ignoring coverage metadata MUST preserve logical correctness; it may only reduce performance.
- COVE-Core and COVE-T remain decodable without COVE-I, COVX, COVM, COVE-MAP, or any runtime-local COVE-CACHE state unless a requested operation explicitly requires one of those optional surfaces.


## 1. Specification Status

This document defines Cove Format v2.0, hereafter COVE.
COVE means Canonical Offline Value Encoding.
**COVE defines the following profiles and companion artifacts:**

- **COVE-Core:** Common immutable file structure, section directory, dictionary, logical/physical types, encoded arrays, checksums, validation, collation, canonical values, and extension rules.
- **COVE-T:** Engine-neutral table-scan profile.
- **COVE-COVERAGE:** Optional formal coverage-semantics profile for conservative predicate coverage sets, tightness/coverage metrics, proof strength, interval forms, provider metadata, and do-no-harm planning. COVE-COVERAGE is the common proof vocabulary used by COVE-T, COVE-A, COVX, COVE-I, COVM, COVE-MAP, and COVE-CACHE when those profiles expose coverage.
- **COVE-A:** Archive acceleration profile for synopses, lookup indexes, composite pruning, manifests, and sidecar acceleration.
- **COVE-E:** Universal engine execution profile for mapping FileCodes into implementation-local ExecutionCodes.
- **COVE-H:** Optional named Harbor registration under COVE-E. Defines Harbor leased-code execution: FileCode -> Harbor EngineCode. COVE-H is not required for generic COVE conformance.
- **COVE-O:** Optional object-temporal extension profile for committed object history, deltas, branches, CSNs, baselines, snapshots, tombstones, and trust chains. COVE-O is not required for generic COVE conformance.
- **COVE-MAP:** Optional deterministic semantic mapping profile and companion `.covemap` artifact for converting one or more external source tables/files/streams into paired object-and-association semantic assertions, properties, temporal facts, and evidence that may be materialised as COVE-O and exposed through optional COVE-T/Arrow/SQL table projections. COVE-MAP is not required for generic COVE conformance.
- **COVE-CX:** Optional registered codec-extension profile for lossless specialised encodings, codec capability descriptors, canonical fallback rules, and conformance vectors. COVE-CX is the v2 path for FSST-style string encoding, ALP-style floating-point encoding, FastLanes-style integer packing/frame-of-reference/delta encoding, and future codecs.
- **COVE-L:** Optional layout-plan and scan-split profile. COVE-L describes lazy read planning, page clusters, split generation, and object-store range grouping without replacing the COVE table catalog, segment index, page index, or predicate-proof metadata.
- **COVE-R:** Optional runtime registry/session interoperability guidance and artifacts. COVE-R describes how implementations advertise supported codec, profile, index, kernel, FFI, and engine-adapter capabilities without making runtime state part of COVE logical truth.
- **COVE-I:** Optional secondary index artifact profile and `.covi` artifact for value-to-fragment, path-to-fragment, dimensional-bucket, row-range, and index-only access metadata.
- **COVE-CACHE:** Optional mutable runtime/local coverage-cache profile for snapshot-bound predicate containment and coverage reuse. COVE-CACHE is never canonical file truth.
- **COVX:** Optional accelerator sidecar.
- **COVM:** Optional dataset manifest.
A conforming COVE reader MUST be able to validate and read COVE files without COVX, COVM, COVE-I, COVE-CACHE, or COVE-MAP.
COVX, COVM, and COVE-I are optional acceleration, planning, index, or manifest artifacts. COVE-CACHE is optional runtime/local state, not canonical file truth. None of these surfaces may change the logical meaning of referenced COVE files. COVE-MAP artifacts MUST NOT change the logical meaning of already materialised COVE files; they define how source data is converted, replayed, explained, or re-materialised into new COVE outputs.

### 1.1 Profile Maturity and Conformance Surface

COVE v2 is profile-scoped. Implementers MUST NOT treat the existence of an optional profile in this document as a requirement for baseline COVE conformance.
**Baseline v2 interoperability target:**
- COVE-Core structural validation and typed logical decode,
- COVE-T table scan reading,
- safe predicate metadata interpretation,
- Arrow-compatible export for supported logical types,
- a reproducible binary conformance vector set.
**Optional v2 profiles and artifacts:**
- COVE-A archive acceleration,
- COVE-COVERAGE coverage proof vocabulary,
- COVE-I secondary index artifacts,
- COVE-E engine execution-code mapping,
- COVX accelerator sidecars,
- COVM dataset manifests,
- COVE-MAP semantic mapping artifacts when mapping tooling is claimed,
- COVE-CX registered codec extensions,
- COVE-L layout plans, split indexes, page cluster directories, and zero-copy maps,
- COVE-R runtime compatibility manifests and session/registry hints,
- COVE-CACHE runtime/local predicate coverage caches.
**Named engine registrations:**
- COVE-H is a Harbor-specific COVE-E registration. It demonstrates and standardises one engine profile; it is not a dependency of COVE-Core, COVE-T, COVE-A, or generic COVE-E.
**Optional extension profiles:**
- COVE-O is an optional object-temporal profile. It MAY be implemented by temporal-object engines, but general table readers SHOULD ignore COVE-O sections unless the requested operation explicitly requires object-temporal semantics.
- COVE-MAP is an optional v2 profile with a stable conceptual and conformance boundary: artifact magic, feature bit, validation boundary, identity model, operation-level rules, reusable `.covemap` artifact framing, and standard `MAP_*` payload schemas are part of this specification. General COVE readers SHOULD ignore COVE-MAP artifacts or sections unless the requested operation explicitly requires mapping validation, mapping replay, mapping explanation, source-to-object/association conversion, or mapping-defined projection readback.

A file that contains optional profile sections MUST advertise the corresponding feature bits. A reader that does not implement an advertised optional profile MUST either ignore the profile when it is not required for the requested operation, or reject the requested operation with a profile-not-supported error.
Implementations that claim COVE-MAP support SHOULD state which standard `MAP_*` section kinds and registered extension payload encodings they support. A COVE-MAP v2 artifact validator MUST support the standard payload schema defined in Section 70 for the section kinds it claims.

**Standards-suite conformance note:** An implementation SHOULD state conformance at the narrowest honest level, for example `COVE-Core v2 reader`, `COVE-T starter reader`, `COVE-T scan writer`, `COVE-CX-aware reader`, `COVE-MAP artifact validator`, or `COVE-H Harbor registration`. A product MUST NOT claim broad COVE v2 support merely because it can parse the header or use one named engine profile. A narrow conformance claim is not a narrow specification; it is an honest implementation boundary.

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


### 1.4 V2 Delta and Identity Guardrails

COVE v2 adds a narrow set of next-generation mechanisms that improve performance, implementation ergonomics, and ecosystem integration while preserving COVE's identity as a canonical offline value encoding.

**V2 additions are deliberately scoped:**
- **Registered codec extensions** replace vague specialised-encoding aspirations with a concrete envelope for lossless codecs, feature bits, fallback rules, and test vectors.
- **Layout-plan metadata** gives readers a hierarchical planning surface for lazy object-store reads and scan split generation, but it is not an authoritative data model.
- **Fast metadata indexes and page-cluster directories** improve wide-schema and range-read behaviour, but they mirror validated COVE sections rather than replacing them.
- **Zero-copy buffer maps** allow Arrow/engine-friendly buffer exposure when safe, but they do not weaken null, dictionary, canonical-value, or checksum semantics.
- **Runtime registry/session guidance** gives implementations a clean way to manage codecs, indexes, kernels, functions, FFI adapters, and engine integrations without global state or engine-specific leakage into COVE-Core.

**The following COVE surfaces remain authoritative in v2:**
- the COVE file header, postscript, footer, and binary section directory;
- the table catalog and object catalog;
- canonical logical values and canonical value encodings;
- file-local FileCode dictionaries and ColumnDomain ordering;
- null bitmaps and page reconstruction rules;
- predicate-proof metadata and safe `PredicateZoneOutcome` rules;
- COVE-O object-temporal reconstruction rules;
- COVE-MAP deterministic identity, association, projection, and evidence rules;
- digest manifests, trust chains, redaction manifests, and COVM publication state.

**The following v2 surfaces are explicitly non-authoritative unless a required decode feature bit says otherwise:**
- COVE-L layout-plan nodes;
- scan split indexes;
- page-cluster directories;
- zero-copy buffer maps;
- runtime compatibility hints;
- COVX acceleration indexes;
- engine execution profiles and ExecutionCodes;
- writer cost-model metadata;
- registry/session implementation state.

**Anti-clone boundary:** COVE v2 MUST NOT become a generic layout-tree file format, MUST NOT replace the table catalog with a dtype-only schema model, MUST NOT make runtime registry string IDs the primary compatibility mechanism, MUST NOT require any particular external columnar library, and MUST NOT treat advisory layout/statistics metadata as a substitute for COVE's proof-carrying pruning semantics.

### 1.5 V2 Non-Goals

COVE v2 deliberately does not define:
- a mutable append-in-place file;
- a transaction log or table protocol;
- a mandatory Arrow IPC replacement;
- a mandatory Vortex, Parquet, ORC, Arrow, DuckDB, DataFusion, Harbor, Spark, or Trino dependency;
- a schema-less or dtype-only replacement for COVE-T's table catalog;
- a general plugin system whose unknown runtime identifiers are enough to decode required file data;
- lossy float or string encodings in the core format;
- probabilistic identity resolution as canonical COVE-MAP identity;
- advisory statistics that can skip data without conservative proof.

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
- optional deterministic multi-source semantic mapping into object-based COVE through COVE-MAP,
- optional registered lossless codec extensions through COVE-CX,
- optional layout/split planning metadata through COVE-L,
- optional explicit runtime registry/session compatibility through COVE-R.
**COVE is not:**
- a WAL,
- a mutable database file,
- an in-flight transaction recovery log,
- a lakehouse catalog replacement,
- a lakehouse/table transaction protocol,
- a row-level delete or visibility protocol,
- an access-control system or encryption standard in v2,
- an Arrow IPC replacement,
- a generic Parquet clone,
- a Vortex clone or wrapper,
- a format whose schema authority is a layout tree or dtype-only model,
- a format that persists engine-local ExecutionCodes as authoritative logical data,
- a mandatory ETL orchestrator, master-data-management system, probabilistic entity-resolution system, or AI-based schema matching system.
**COVE’s guiding principle is:**
Store portable logical values and engine-shaped physical data.
Let each engine own its own execution identity at read or mount time.
Let specialised codecs, coverage proofs, indexes, caches, and layout plans accelerate access without changing portable logical truth.

---

## 4. Public Positioning

**Cove Format should be positioned as:** A Canonical Offline Value Encoding for immutable, queryable archives: encoded arrays, canonical logical values, proof-carrying predicate and coverage metadata, optional accelerator/index sidecars, and direct support for engine-local dictionary execution.

**It should not be positioned as:** A universal Parquet replacement.

**Recommended positioning:**

- **Parquet / ORC:** universal lakehouse interchange and mature analytical columnar storage.
- **COVE:** high-performance queryable archive and converted-table format for engines that can exploit encoded execution, proof-carrying coverage metadata, lookup indexes, aggregate synopses, optional sidecars, direct dictionary/code-vector execution, registered codec extensions, optional secondary indexes, and optional layout/split planning metadata.
- **COVE-MAP:** optional deterministic source-row semantics for organisations that want to convert fragmented source tables, files, and streams into portable object-and-association COVE, and optionally expose that object-association truth through deterministic projected tables, without adopting a named runtime engine.

---

## 5. Profile Overview

| Profile | Name | Audience | Purpose |
| --- | --- | --- | --- |
| COVE-Core | Core Format | All readers/writers | File layout, sections, dictionary, encodings, checksums, validation. |
| COVE-T | Table Scan Profile | General engines | Engine-neutral columnar table scan profile. |
| COVE-COVERAGE | Coverage Semantics Profile | Query planners/archive engines | Conservative query coverage vocabulary, proof strength, tightness/coverage metrics, interval predicate forms, and do-no-harm planning metadata. |
| COVE-A | Archive Acceleration Profile | Archive/query engines | Synopses, lookup indexes, manifests, composite pruning, sidecars. |
| COVE-E | Engine Execution Profile | All engines | Universal mapping from FileCodes to engine-local ExecutionCodes. |
| COVE-H | Harbor Execution Registration | Harbor implementations | Optional Harbor leased-code implementation of COVE-E. |
| COVE-O | Object Temporal Profile | Temporal-object engines | Optional object history, deltas, branches, CSNs, trust chains. |
| COVE-MAP | Semantic Mapping Profile | Conversion/governance/object/projection tools | Optional deterministic multi-source row semantics, identity joins, evidence, materialisation into object-and-association COVE, and deterministic readback as projected tables. |
| COVE-CX | Codec Extension Profile | Reader/writer/engine implementers | Registered lossless specialised encodings with feature bits, fallback rules, capability descriptors, and conformance vectors. |
| COVE-L | Layout Plan Profile | Query planners/object-store readers | Optional hierarchical layout plans, scan splits, page clusters, and zero-copy metadata that never replace catalog/page/index authority. |
| COVE-R | Runtime Registry Guidance | Library and adapter implementers | Explicit session/registry model for codecs, kernels, profiles, engine adapters, FFI, and capability discovery. |
| COVE-I | Secondary Index Profile | Archive/query/index builders | Optional secondary index artifacts, root indexes, value/path/dimensional mappings, and index-only access declarations. |
| COVE-CACHE | Runtime Coverage Cache Profile | Engine/runtime implementers | Optional snapshot-bound mutable predicate coverage cache for local planning reuse; never canonical truth. |

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
      normalisation: cove.fn.person_name.v2
    - semantic_role: Customer.Email
      source_columns:
        crm.customers.email
        orders.customer_email
        support.requester_email
      normalisation: cove.fn.email.v2
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
- No append mutation in v2.
- No in-file delete overlays.
- No mutable visibility maps.
- No mutable execution-code maps.
- No mutable lease maps.
Compaction, import, export, and conversion produce new COVE files.

**Write finalisation:**
- A writer MAY stream input records into temporary builder state, temporary files, or uncommitted segment buffers.
- A .cove object is valid only after the complete section directory, footer, postscript, and covered checksums have been written and validated.
- COVE v2 does not define partially visible incremental writes, append-in-place, or reader recovery from an unfinished .cove object.
- Streaming or incremental dataset publication MAY be built above COVE using new immutable COVE files plus COVM or an external catalog, but readers MUST NOT infer visibility from partially written COVE data.

Future versions MAY define appendable or streaming containers, but such containers MUST use new magic, feature bits, or profile rules so v2 readers cannot mistake them for immutable v2 COVE files.
See Section 50.4 for the v2 append, streaming, CDC, and compaction boundary when COVE files are used inside a dataset or external table system.

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


### 7.4.1 Coverage is conservative

COVE coverage metadata generalises predicate pruning from a single zone decision to an explicit set of fragments that is guaranteed to contain every possible matching value or row for a declared predicate context.

**INVARIANT:**
A coverage set used for correctness-sensitive pruning, routing, index-only answers, metadata-only answers, or lookup narrowing MUST be conservative for the declared snapshot and predicate context.

**Rules:**
- A conservative coverage set MAY contain false positives: fragments that are read even though they contain no matching row.
- A conservative coverage set MUST NOT contain false negatives: fragments outside the set that could contain matching rows.
- A tight coverage set is a conservative coverage set that contains only necessary fragments under the declared proof model.
- A coverage artifact with approximate, advisory, engine-local, stale, or unvalidated proof strength MUST NOT be used to skip data.
- Coverage metadata that is corrupt, unsupported, stale, or mismatched to the selected snapshot MUST fail open to a wider conservative plan or full scan.
- Coverage metadata MUST NOT override structural validation, page reconstruction rules, external visibility overlays, or COVE-MAP/COVE-O semantic truth.

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


### 7.8 COVE v2 layout and codec additions are subordinate to logical truth

COVE-CX codecs and COVE-L layout plans are performance and implementation mechanisms. They MUST NOT redefine COVE logical values.

**Rules:**
- A registered codec MAY change how a page is encoded; it MUST NOT change the decoded logical sequence, null positions, canonical value bytes, collation semantics, FileCode dictionary meaning, NumCode interpretation, or trust/digest inputs.
- A layout-plan node MAY describe how to group reads, generate scan splits, or traverse page clusters; it MUST NOT replace the table catalog, object catalog, segment indexes, page indexes, or row-reference rules.
- A runtime registry/session MAY decide how to instantiate codecs, kernels, and engine adapters; it MUST NOT become part of COVE logical truth.
- If a registered codec or layout-plan section is corrupt and optional, a reader MUST ignore it and fall back. If the codec is required to decode selected data, the reader MUST reject safely.

### 7.9 Catalog and schema authority

COVE v2 keeps explicit schema authority. A table-shaped COVE file is defined by COVE-T table catalog entries and column IDs, not by a dtype-only tree, runtime layout node, or engine adapter schema.

**Rules:**
- A COVE-T reader MUST resolve table and column identity from the table catalog.
- COVE-L layout nodes MUST reference existing table IDs, column IDs, segments, morsels, pages, or sections.
- A layout node that references a missing or mismatched catalog entry is invalid and MUST NOT be used.
- Engine-facing schemas exported to Arrow, SQL, DataFusion, DuckDB, Polars, Spark, Trino, or another runtime are projections of COVE catalog/schema authority, not replacements for it.

---

## 8. Primitive Wire Rules

### 8.1 Endianness

All multi-byte integers are little-endian.
COVE v2 deliberately chooses one canonical byte order instead of storing a byte-order marker or negotiating host endianness. This keeps section parsing, memory-mapped fixed-width fields, checksum coverage, and conformance vectors deterministic.
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

**Checksum coverage discipline:**

| Structure | Coverage rule |
| --- | --- |
| Header | CRC32C over the header bytes with the header checksum field zeroed. |
| Postscript | CRC32C over the postscript bytes with the postscript checksum field zeroed. |
| Footer section spec | CRC32C over the stored footer bytes after section-level decompression rules are applied only when the spec says the CRC covers decoded bytes; otherwise over stored bytes. COVE-Core v2 default is stored bytes. |
| Section entry CRC | CRC32C over the stored section payload bytes unless the section kind explicitly declares decoded-byte coverage. |
| Page checksum | CRC32C over the stored page payload bytes referenced by the page index after page-level compression wrapping; buffer descriptor checksums cover individual decoded page buffers when present. |
| Page buffer checksum | CRC32C over the exact buffer bytes described by the buffer descriptor. |
| Codec envelope checksum | CRC32C over the registered encoding envelope with its checksum field zeroed. |
| Optional enclosing cluster checksum | CRC32C over the stored cluster byte range; individual page checksums remain authoritative. |

**Rules:**
- A structure with an embedded checksum MUST define whether the checksum field is zeroed during computation.
- Writers MUST NOT mix stored-byte and decoded-byte CRC coverage for the same structure kind unless a required extension defines the distinction.
- Cryptographic digests MAY cover file, section, page, decoded logical value, or Merkle scopes, but the digest manifest MUST declare the exact scope.


### 8.7 Cryptographic Digests

COVE MAY include cryptographic digests.
**Supported digest algorithms in v2:**

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
│ Magic "COV2"                                                │
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

COVE v2 uses a new primary magic and a widened header. The widened header gives readers a bootstrap pointer to optional extended feature and metadata-index sections without making those sections mandatory for baseline COVE-Core/COVE-T reads.

```rust
struct CoveHeaderV2 {
    magic: [u8; 4],              // "COV2"

    header_len: u16,             // 160 for v2
    version_major: u16,          // 2
    version_minor: u16,          // 0 for v2.0

    primary_profile: u8,
    // 0=mixed/unknown
    // 1=COVE-O object temporal
    // 2=COVE-T table scan
    // 3=COVE-A archive acceleration
    // 4=COVE-E engine execution
    // 5=COVE-H Harbor registered execution profile
    // 6=COVE-MAP evidence/projection carrier inside a .cove file
    // 7=COVE-CX codec extension carrier
    // 8=COVE-L layout/split planning carrier
    // 9=COVE-R runtime compatibility carrier
    // 10=COVE-COVERAGE coverage metadata carrier
    // 11=COVE-I secondary index carrier

    endianness: u8,              // 1=little-endian

    flags: u32,

    required_features: u64,      // low feature word
    optional_features: u64,      // low feature word

    file_id: [u8; 16],

    producer_scope_id: [u8; 16],
    producer_scope_kind: u16,

    reserved_scope_flags: u16,

    created_at_us: i64,

    feature_set_section_id: u32,        // 0 if no EXTENDED_FEATURE_SET section
    profile_capability_section_id: u32, // 0 if no PROFILE_CAPABILITY_MATRIX section
    fast_metadata_section_id: u32,      // 0 if no FAST_METADATA_INDEX section
    v2_flags: u32,

    reserved: [u8; 64],          // MUST be zero

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
- magic MUST be `"COV2"` for COVE v2 files.
- COVE v1 magic values are not valid COVE v2 magic values. A v2 reader MAY implement a separate v1 compatibility reader.
- header_len MUST be 160 for v2.0.
- version_major MUST be 2.
- endianness MUST be 1.
- reserved bytes MUST be zero.
- checksum MUST validate before any other header field is trusted.
- Header `required_features` are **always file-required**. An unknown bit in the header low-word `required_features` MUST cause rejection during bootstrap. There is no scoped-requiredness escape hatch for header-required bits.
- Operation-, profile-, section-, and page-scoped requiredness MUST be expressed through section entries, page flags/envelopes, profile descriptors, `PROFILE_CAPABILITY_MATRIX`, or `SECTION_FEATURE_BINDING`, not by placing unknown or operation-only bits in the header `required_features`.
- Writers SHOULD place optional profile-presence bits such as COVE-MAP, COVE-H, COVE-L, COVE-R, COVX, COVE-I, and COVE-CACHE references in `optional_features` unless ordinary baseline file parsing or selected logical decode truly requires them.
- Writers MUST NOT place operation-only requirements, such as mapping replay, trust-chain verification, Harbor mount, projection readback, runtime adapter selection, index-only answering, or zero-copy export, in header `required_features`. Doing so makes the whole file unreadable to readers that do not know the bit.
- if `feature_set_section_id != 0`, the referenced `EXTENDED_FEATURE_SET` section MUST be validated before any extended required feature is acted on.
- `feature_set_section_id`, `profile_capability_section_id`, and `fast_metadata_section_id` are bootstrap **section identifiers**, not byte offsets and not replacements for the footer section directory. A reader still discovers authoritative section offsets through the postscript and footer. If a referenced optional section is absent, corrupt, or unsupported, a reader MUST fall back to ordinary footer/section parsing unless the section is marked required for the requested operation.
- Header fields MUST NOT be used to override footer section directory entries or profile-specific catalog metadata.

---

## 11. Feature Bits

**Feature bits are divided into:**
**required_features:**
  reader must understand these to correctly read required data

**optional_features:**
  reader may ignore these if the associated section is not needed
**Assigned v2 feature bits:**

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
| 0x0000_0000_0400_0000 | FEATURE_CODEC_EXTENSION_REGISTRY | File contains COVE-CX codec extension descriptors. |
| 0x0000_0000_0800_0000 | FEATURE_REGISTERED_ENCODINGS | File contains pages encoded with registered COVE-CX encodings. Required when any projected page cannot be decoded through core encodings alone. |
| 0x0000_0000_1000_0000 | FEATURE_LAYOUT_PLAN | File contains COVE-L layout-plan metadata. |
| 0x0000_0000_2000_0000 | FEATURE_SCAN_SPLIT_INDEX | File contains precomputed scan split metadata. |
| 0x0000_0000_4000_0000 | FEATURE_PAGE_CLUSTER_DIRECTORY | File contains page cluster metadata for range-read coalescing. |
| 0x0000_0000_8000_0000 | FEATURE_ZERO_COPY_BUFFER_MAP | File contains optional zero-copy/export buffer compatibility metadata. |
| 0x0000_0001_0000_0000 | FEATURE_FAST_METADATA_INDEX | File contains optional wide-schema/random-access metadata index. |
| 0x0000_0002_0000_0000 | FEATURE_RUNTIME_COMPATIBILITY_HINTS | File contains COVE-R runtime compatibility hints. |
| 0x0000_0004_0000_0000 | FEATURE_EXTENDED_FEATURE_SET | File contains feature words beyond the low 64-bit header fields. |
| 0x0000_0008_0000_0000 | FEATURE_CODEC_FALLBACK_PAYLOADS | File contains explicit fallback payloads for optional registered encodings. |
| 0x0000_0010_0000_0000 | FEATURE_COVERAGE_METADATA | File or companion artifact contains COVE-COVERAGE coverage sets, proofs, provider descriptors, or coverage plan candidates. |
| 0x0000_0020_0000_0000 | FEATURE_COVERAGE_PLAN_CANDIDATES | File or companion artifact contains costed coverage plan candidates and do-no-harm fallback metadata. |
| 0x0000_0040_0000_0000 | FEATURE_SECONDARY_INDEX_ARTIFACT | Dataset or file references a COVE-I `.covi` secondary index artifact. |
| 0x0000_0080_0000_0000 | FEATURE_INDEX_ONLY_CAPABILITY | File or companion artifact declares exact or approximate index-only query-answer capabilities. |
| 0x0000_0100_0000_0000 | FEATURE_COVERAGE_CACHE_HINTS | File or manifest may reference a runtime/local COVE-CACHE compatibility or invalidation surface. |

**Rules:**
- Readers MUST reject unknown header `required_features` bits unconditionally during bootstrap.
- Readers MUST reject unknown section-, page-, profile-, or operation-required feature bits only when the requested operation needs the section, page, profile, or operation carrying those bits.
- Readers MAY ignore unknown optional feature bits.
- FEATURE_SEMANTIC_MAP indicates the presence of COVE-MAP-related metadata. Whether that metadata is required depends on the requested operation and any required embedded profile or extension rules. Ordinary COVE-T or COVE-O reads MAY ignore optional mapping evidence, identity-equivalence, or projection metadata when mapping replay, explanation, conversion, or projection readback is not requested.
- Readers MUST NOT use unknown optional metadata for skipping.
- COVE-CX registered encodings that are needed to decode projected data MUST be represented by required feature bits unless an independently validated canonical fallback payload is present and selected.
- COVE-L layout, scan-split, page-cluster, zero-copy, and runtime compatibility metadata MUST be optional unless the requested operation explicitly asks for that metadata.
- COVE-COVERAGE metadata MUST be validated before use. Unknown optional coverage metadata MUST be ignored and MUST NOT be used for pruning.
- COVE-I secondary index artifacts and COVE-CACHE coverage caches MUST be optional and snapshot-bound. Unsupported, stale, or corrupt index/cache metadata MUST be ignored for ordinary reads.
- If `FEATURE_EXTENDED_FEATURE_SET` is set, readers MUST validate the extended feature set before accepting or rejecting unknown extended required features.


### 11.1 Extended Feature Set

The low 64-bit header feature words cover bootstrap features. COVE v2 also allows an `EXTENDED_FEATURE_SET` section for future feature banks.

Feature words are globally numbered. Global feature word 0 is the low 64-bit header/postscript word. Global feature word `N` contains feature bits `64*N` through `64*N + 63`. This global numbering is used by the `EXTENDED_FEATURE_SET`, `SECTION_FEATURE_BINDING`, profile capability matrices, and companion artifacts. Local arrays may store only the words needed by a binding, but every local word range MUST declare the global word number it represents.

```rust
struct ExtendedFeatureSetHeaderV2 {
    word_count: u32,
    required_word_count: u32,
    optional_word_count: u32,
    flags: u32,
    checksum: u32,
}
// followed by:
//   required_feature_words: u64[required_word_count]
//   optional_feature_words: u64[optional_word_count]
```

**Rules:**
- Feature word 0 MUST equal the low feature words in the header and postscript.
- `required_feature_words[i]` and `optional_feature_words[i]` represent global feature word `i`; missing words beyond the declared counts are interpreted as zero.
- `word_count` is the declared logical feature-word horizon for this artifact: valid global feature-word indexes are `0` through `word_count - 1`. It MUST be greater than or equal to both `required_word_count` and `optional_word_count`. If `word_count` is greater than either array count, the missing trailing words for that array are zero. Writers SHOULD set `word_count` to the smallest value that covers every non-zero required or optional feature word and every globally numbered feature word referenced by a section binding, profile capability matrix, or companion artifact reference.
- Readers MUST reject an extended feature set when `word_count == 0`, when a non-zero bit appears outside the declared horizon, or when a `SECTION_FEATURE_BINDING` references a global feature word greater than or equal to `word_count`.
- Unknown required bits in global feature word 0 are header-required and MUST reject unconditionally during bootstrap.
- Unknown required bits in global feature words greater than 0 MUST cause rejection according to their declared scope. If no narrower binary binding exists, the default scope is `FileRequired`.
- Unknown optional bits MAY be ignored.
- A later `SECTION_FEATURE_BINDING` MAY scope extended feature words greater than 0, but it MUST NOT reinterpret, narrow, or defer unknown header-required bits in global word 0.
- The extended feature set MUST NOT be represented only in JSON metadata.
- A writer SHOULD use the low feature word for commonly required bootstrap features and extended words for profile-specific, vendor, or future features.

### 11.2 Feature Scope and Requiredness Model

COVE v2 distinguishes the scope of requiredness. This avoids the failure mode where an ordinary table reader rejects a valid table scan because the file also contains required metadata for a different operation such as mapping replay, trust verification, Harbor mount, index-only answering, zero-copy export, or projection readback.

```rust
enum FeatureScope {
    FileRequired = 0,      // required for bootstrap, baseline parse, and selected logical decode
    SectionRequired = 1,   // required only when the section is used
    PageRequired = 2,      // required only when the page is projected, filtered, or reconstructed
    ProfileRequired = 3,   // required only when the profile is claimed or requested
    OperationRequired = 4, // required only for a named operation such as mapping replay or trust verification
    AdvisoryOnly = 5,      // never required for correctness; unsupported readers ignore
}
```

**Default binding of feature words:**

| Location | Default scope | Meaning |
| --- | --- | --- |
| Header `required_features` word 0 | `FileRequired` | Required for bootstrap or ordinary logical decode of the file. Unknown bits always reject during bootstrap and cannot be narrowed by later bindings. |
| Header `optional_features` word 0 | `AdvisoryOnly` or scoped by section/profile | Presence or optional capability advertisement. Unknown bits are ignored. |
| `EXTENDED_FEATURE_SET.required_feature_words` without narrower binding | `FileRequired` | Required for the file or artifact as a whole. Unknown bits reject before use. |
| `CoveSectionEntryV2.required_features` | `SectionRequired` | Required only to use that section. Unknown bits reject use of that section, not unrelated operations. |
| `ColumnPageIndexEntryV2` page flags / registered codec envelope | `PageRequired` when decode-affecting | Required only to decode, predicate-evaluate, reconstruct, or validate that page. |
| `PROFILE_CAPABILITY_MATRIX` or profile descriptor | `ProfileRequired` | Required only when the profile is explicitly requested or claimed. |
| `SectionFeatureBindingV2.scope = OperationRequired` | `OperationRequired` | Required only for the named operation or capability referenced by the binding payload/profile matrix. |

**Precedence rules:**
1. Header `required_features` have the highest bootstrap precedence. If a writer puts an unknown bit there, a generic reader MUST reject. Header-required bits cannot be narrowed by `PROFILE_CAPABILITY_MATRIX`, `SectionFeatureBindingV2`, section entries, or page envelopes.
2. Section and page requiredness cannot make an otherwise undecodable file safe to parse; it can only scope rejection for optional sections/pages that are not needed by the selected operation.
3. `SectionFeatureBindingV2` can narrow or extend section-level requiredness for extended feature banks, but it cannot override, reinterpret, or defer any header `required_features` bit.
4. Profile and operation requiredness applies only after the reader has selected an operation, output mode, requested profile, or advertised conformance claim.
5. Advisory features MUST NOT cause rejection and MUST NOT be used for pruning, index-only answers, or decode unless independently validated by a supported proof or codec contract.

**Rules:**
- A `FileRequired` unknown feature MUST cause rejection before logical decode.
- A `SectionRequired` unknown feature MUST cause rejection only when the reader needs that section.
- A `PageRequired` unknown feature MUST cause rejection only when the reader needs that page for projection, predicate evaluation, reconstruction, or validation.
- A `ProfileRequired` unknown feature MUST cause rejection only when the reader claims or requests that profile.
- An `OperationRequired` unknown feature MUST cause rejection only for the operation that requires it.
- Ordinary COVE-T reads MUST NOT fail solely because optional COVE-MAP, COVE-H, COVE-L, COVE-R, COVX, COVE-I, COVM, or COVE-CACHE metadata is unsupported, stale, corrupt, or missing.
- If a registered codec is required to decode a projected page and no valid fallback exists, that codec is `PageRequired` and the page operation MUST reject when unsupported.
- If COVE-MAP metadata is required only for replay, conversion, explanation, or projection readback, ordinary COVE-T/COVE-O decoding MUST remain possible without it.
- If a trust-chain, redaction, digest, COVE-I index-only answer, COVX kernel, COVE-L zero-copy map, COVE-R runtime adapter, or COVE-CACHE entry is requested by operation or policy, unsupported required features reject that operation only.
- A writer that wants a file to be broadly readable SHOULD advertise optional profiles in header `optional_features`, then express their requiredness in section entries, profile matrices, or operation-specific bindings.

### 11.2.1 Requiredness Validation Order

A conforming reader SHOULD evaluate requiredness in this order:

1. Validate header magic, length, version, endianness, reserved bytes, and header checksum.
2. Reject unknown header `required_features` bits unconditionally.
3. Discover and validate the postscript and footer section directory.
4. Validate `EXTENDED_FEATURE_SET` if advertised or referenced.
5. Build the feature-scope table from header words, footer section entries, `PROFILE_CAPABILITY_MATRIX`, and `SectionFeatureBindingV2` records.
6. Select the requested operation: ordinary table scan, object reconstruction, mapping replay, projection readback, index-only answer, trust verification, Harbor mount, Arrow zero-copy export, etc.
7. Reject only the unknown required features whose scope intersects the selected operation.
8. Ignore unsupported advisory features and unsupported optional sections.

A reader MAY implement a stricter policy for safety, but such a policy MUST be reported as an implementation policy rather than a COVE wire-format requirement.

### 11.3 Section-Level Extended Feature Binding

The low 64-bit feature words in `CoveSectionEntryV2` are sufficient for common bootstrap and section features. When section-, profile-, page-, or operation-scoped requiredness uses extended feature words, a `SECTION_FEATURE_BINDING` section provides the binary-authoritative binding. The binding section is not a way to make an unknown header-required feature safe; it applies only after header validation has succeeded.

A `SECTION_FEATURE_BINDING` payload has one header, one binding array, an optional local payload-reference array, and a feature-word data area. All offsets in this subsection are byte offsets relative to the start of the `SECTION_FEATURE_BINDING` section payload unless explicitly stated otherwise.

```rust
struct SectionFeatureBindingSectionHeaderV2 {
    magic: [u8; 4],              // "SFB2"
    version_major: u16,          // 2
    version_minor: u16,          // 0
    header_len: u16,
    entry_len: u16,

    binding_count: u32,
    payload_ref_count: u32,
    feature_word_count: u32,

    bindings_offset: u64,
    payload_refs_offset: u64,    // 0 when payload_ref_count == 0
    feature_words_offset: u64,   // 0 when feature_word_count == 0
    payload_data_offset: u64,    // 0 when there is no local payload data
    payload_data_length: u64,

    flags: u32,
    checksum: u32,
}
```

```rust
enum SectionFeatureBindingPayloadKindV2 {
    None = 0,
    ProfileRequirement = 1,
    OperationRequirement = 2,
    PageRequirement = 3,
    ExtensionRequirement = 4,
    CodecRequirement = 5,
    CoverageRequirement = 6,
    IndexRequirement = 7,
    RuntimeRequirement = 8,
    VendorDefined = 255,
}

struct SectionFeatureBindingPayloadRefV2 {
    binding_payload_ref: u32,     // dense 1..payload_ref_count; 0 is absent
    payload_kind: u16,            // SectionFeatureBindingPayloadKindV2
    operation_kind: u16,          // OperationKindV2 or None
    profile: u8,                  // section/profile id using CoveSectionEntryV2.profile values
    flags: u8,
    reserved: u16,
    payload_offset: u64,          // into payload_data area
    payload_length: u64,
    checksum: u32,
}
```

```rust
struct SectionFeatureBindingV2 {
    binding_id: u32,              // dense 0..binding_count-1
    section_id: u32,              // 0 when binding applies to a profile/artifact rather than one section
    scope: u8,                    // FeatureScope
    profile: u8,                  // 0=shared or CoveSectionEntryV2.profile value
    operation_kind: u16,           // OperationKindV2; must be None unless scope=OperationRequired

    required_word_count: u32,
    optional_word_count: u32,
    required_feature_word_index: u32, // index into local feature-word array, or u32::MAX
    optional_feature_word_index: u32, // index into local feature-word array, or u32::MAX
    required_first_feature_word_number: u32, // global feature word number, or u32::MAX
    optional_first_feature_word_number: u32, // global feature word number, or u32::MAX

    binding_payload_ref: u32,      // 0 when absent; local SectionFeatureBindingPayloadRefV2 reference
    target_local_ref: u64,         // page_id, profile_id, codec_id, index_root_id, etc.; u64::MAX when not applicable
    flags: u32,
    checksum: u32,
}
```

```rust
enum OperationKindV2 {
    None = 0,
    OrdinaryTableScan = 1,
    ObjectReconstruction = 2,
    MappingReplay = 3,
    MappingExplanation = 4,
    ProjectionReadback = 5,
    TrustVerification = 6,
    RedactionPolicyEvaluation = 7,
    HarborMount = 8,
    EngineExecutionMapping = 9,
    IndexOnlyAnswer = 10,
    CoveragePlanning = 11,
    ZeroCopyExport = 12,
    RuntimeAdapterSelection = 13,
    VendorDefined = 255,
}
```

**Reference spaces:**
- `binding_id` is local to one `SECTION_FEATURE_BINDING` section and MUST be dense.
- `binding_payload_ref` is local to the `payload_refs` array of the same `SECTION_FEATURE_BINDING` section. `0` means absent. Non-zero values MUST be in `1..payload_ref_count` and MUST identify exactly one `SectionFeatureBindingPayloadRefV2`.
- `required_feature_word_index` and `optional_feature_word_index` are indexes into the local `u64[feature_word_count]` array beginning at `feature_words_offset`. The binding uses the contiguous local ranges `[index, index + word_count)`. `u32::MAX` is valid only when the corresponding word count is zero.
- `required_first_feature_word_number` and `optional_first_feature_word_number` identify the global feature-word number represented by the first word in the corresponding local range. The local word at `feature_word_index + i` represents global feature word `first_feature_word_number + i`. `u32::MAX` is valid only when the corresponding word count is zero.
- A `SECTION_FEATURE_BINDING` MUST NOT bind global feature word 0. Low-word section scoping is expressed by `CoveSectionEntryV2.required_features`, `CoveSectionEntryV2.optional_features`, page flags, codec envelopes, or other low-word fields. Unknown header-required bits in global word 0 always reject before bindings are interpreted.
- If multiple bindings for the same target and scope mention the same global feature-word number, the effective word is the bitwise OR of those validated bindings. Bindings MUST NOT rely on local array position as semantic feature-bank identity.
- `section_id` references a `CoveSectionEntryV2.section_id` in the same `.cove` artifact. `section_id = 0` is allowed only for profile-, artifact-, or operation-wide bindings where the payload reference identifies the target.
- `target_local_ref` is interpreted only by the `payload_kind` and `scope`. For example, it may be a page reference, codec ID, index root ID, coverage provider ID, profile ID, or runtime hint ID. If the required interpretation is unknown, the binding MUST be treated as unsupported for that scoped operation.

**Rules:**
- Section-level extended feature bindings MUST be checksummed and bounds-checked before use.
- `magic` MUST be `"SFB2"`; unsupported major versions make the binding section unsupported.
- The binding array, payload-ref array, feature-word array, and payload-data area MUST be non-overlapping and within the section payload.
- A section with unsupported required extended bits MUST be rejected only at the scope declared by the binding.
- `operation_kind` MUST be `None` unless `scope == OperationRequired`.
- If `required_word_count > 0`, then `required_feature_word_index` and `required_first_feature_word_number` MUST NOT be `u32::MAX`, the local word range MUST be in bounds, and `required_first_feature_word_number` MUST be greater than 0.
- If `optional_word_count > 0`, then `optional_feature_word_index` and `optional_first_feature_word_number` MUST NOT be `u32::MAX`, the local word range MUST be in bounds, and `optional_first_feature_word_number` MUST be greater than 0.
- If a word count is zero, the corresponding local index and global first-word number MUST both be `u32::MAX`.
- `binding_payload_ref`, when non-zero, MUST reference a validated binary payload that defines the operation, profile, page, codec, index, runtime adapter, or extension contract. JSON metadata MUST NOT define requiredness.
- A writer SHOULD use this binding only when low-word section features are insufficient or when extended features need operation-, profile-, page-, or artifact-specific scope.
- `SectionFeatureBindingV2` MUST NOT narrow, override, reinterpret, or defer any unknown bit in header `required_features`.
- The extended feature set MUST remain binary-authoritative; JSON metadata MUST NOT define requiredness.

---

## 12. Postscript

**The final bytes of every COVE file are:**
[postscript bytes]
[postscript_version: u16]
[postscript_len: u16]
[magic: "COV2"]
**Rules:**
- postscript_len excludes postscript_version, postscript_len, and trailing magic.
- postscript_len MUST be <= 65535.
- Readers SHOULD be able to discover the footer by reading the final 64 KiB.

```rust
struct CovePostscriptV2 {
    required_features: u64,
    optional_features: u64,

    file_len: u64,

    footer: CoveSectionSpecV2,

    checksum: u32,
}
```

```rust
struct CoveSectionSpecV2 {
    offset: u64,
    length: u64,
    uncompressed_length: u64,

    compression: u8,        // 0=None, 1=LZ4, 2=Zstd
    encryption: u8,         // 0=None in v2
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
- encryption MUST be 0 in v2.

---

## 13. Footer and Section Directory

The footer contains the authoritative section directory.

```rust
struct CoveFooterHeaderV2 {
    footer_magic: [u8; 4],       // "CV2F"

    footer_version: u16,         // 2
    header_len: u16,

    section_count: u32,
    section_entry_len: u16,
    flags: u16,

    metadata_len: u32,           // <= 1 MiB

    reserved: [u8; 24],          // MUST be zero
}
```

// followed by:
//   CoveSectionEntryV2[section_count]
//   metadata_json[metadata_len]

```rust
struct CoveSectionEntryV2 {
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
    // 7=COVE-CX
    // 8=COVE-L
    // 9=COVE-R
    // 10=COVE-COVERAGE
    // 11=COVE-I

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
| 23 | EXTENDED_FEATURE_SET | shared | Feature words beyond the low 64-bit header/postscript fields. |
| 24 | CODEC_EXTENSION_REGISTRY | COVE-CX | Registered lossless codec descriptors, fallback contracts, and codec conformance references. |
| 25 | LAYOUT_PLAN | COVE-L | Optional hierarchical logical read-planning nodes. |
| 26 | SCAN_SPLIT_INDEX | COVE-L | Optional precomputed scan split descriptors. |
| 27 | PAGE_CLUSTER_DIRECTORY | COVE-L | Optional physical page clustering and range-read coalescing metadata. |
| 28 | ZERO_COPY_BUFFER_MAP | COVE-L/shared | Optional Arrow/engine buffer export compatibility metadata. |
| 29 | FAST_METADATA_INDEX | shared | Optional random-access metadata index for wide schemas and large page directories. |
| 35 | COVERAGE_PROVIDER_REGISTRY | COVE-COVERAGE | Coverage providers, proof kinds, proof strength, exactness, and validity declarations. |
| 36 | COVERAGE_SET | COVE-COVERAGE | Coverage set entries over files, segments, pages, morsels, row ranges, objects, paths, or dimensional buckets. |
| 37 | COVERAGE_PLAN_CANDIDATE | COVE-COVERAGE | Optional costed candidate plans for safe do-no-harm coverage planning. |
| 38 | PREDICATE_NORMAL_FORM | COVE-COVERAGE | Canonical predicate AST/CNF/interval/encoded forms used by coverage proofs and caches. |
| 39 | INDEX_ONLY_CAPABILITY | COVE-I/COVE-A | Declarations for metadata/index-only exact or approximate query answers. |
| 45 | SECTION_FEATURE_BINDING | shared | Section/profile/operation-scoped extended feature requiredness bindings. |
| 46 | COVERAGE_PROOF_RECORD | COVE-COVERAGE | Proof records binding predicate forms, providers, coverage sets, validity, and proof semantics. |
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

MAP_* section payloads are COVE-MAP profile payloads whose standard schema is defined by Section 70. The authoritative reusable mapping definition normally lives in a `.covemap` artifact. MAP_* sections embedded in a `.cove` file are intended for mapping evidence, projection catalogs, conversion reports, identity-equivalence indexes, or embedded mapping snapshots tied to that file or dataset state; they MUST NOT silently override an explicitly referenced reusable mapping definition unless a required profile or extension defines that authority rule. A writer MUST NOT place MAP_* sections in an ordinary COVE file unless it advertises FEATURE_SEMANTIC_MAP and the payload conforms to the COVE-MAP v2 schema or to a registered required extension. General COVE readers MUST ignore optional MAP_* sections for ordinary COVE-T or COVE-O reads. COVE-MAP-aware tools MUST validate MAP_* payload schemas, source fingerprints, function registries, and evidence references before using them for conversion, replay, projection, or explanation.

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
struct FileDictionaryHeaderV2 {
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
struct FileDictionaryIndexEntryV2 {
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
- In v2, `Bool if explicitly declared numeric` is declared with the
  per-column/property numeric flag: `TableColumnEntryV2.flags bit 0`,
  `TableColumnDirectoryEntryV2.flags bit 0`, and `PropertyEntryV2.flags bit 8`.
  Catalog and segment declarations for the same column/property MUST agree.
- NumCode MUST be interpreted by declared logical_type.
- NumCode MUST NOT be dictionary-resolved.
- Numeric min/max statistics use logical ordering.
**Float rules:**
- Float values preserve raw IEEE bit patterns.
- NaN values are valid.
- Min/max statistics exclude NaN and set HAS_NAN.
- Readers MUST NOT use min/max to exclude NaN-sensitive predicates unless safe.


### 19.2 Portable NumCode Encoding Metadata

NumCode is portable only because its interpretation is declared by COVE metadata. A reader MUST NOT infer logical comparison safety from the raw numeric width or from an engine-specific representation.

```rust
struct NumCodeEncodingDescriptorV2 {
    descriptor_id: u32,
    logical_type: u16,
    physical_width_bits: u16,
    signedness: u8,          // 0=unsigned, 1=signed, 2=raw_bits, 3=decimal_scaled
    byte_order: u8,          // 1=little-endian in v2
    scale: i16,
    offset_kind: u8,         // 0=none, 1=signed_i64, 2=unsigned_u64, 3=decimal128
    flags: u8,
    min_logical_ref: u32,
    max_logical_ref: u32,
    overflow_policy: u8,     // 0=reject, 1=wrap_invalid, 2=saturate_invalid, 3=extension_defined
    null_representation: u8, // 0=null_bitmap_only in core v2
    reserved: u16,
    checksum: u32,
}
```

**Descriptor flags:**

| Bit | Name | Meaning |
| --- | --- | --- |
| 0x0001 | ORDER_PRESERVING | Physical order is identical to logical order for non-null values under the declared logical type. |
| 0x0002 | EQUALITY_PRESERVING | Physical equality is identical to logical equality for non-null values. |
| 0x0004 | RANGE_COMPARISON_SAFE | Range predicates may be evaluated over the encoded physical domain without logical decode. |
| 0x0008 | ENCODED_PREDICATE_SAFE | Declared encoded predicate kernels are equivalent to baseline logical evaluation. |
| 0x0010 | ADAPTIVE_WIDTH | Values may be stored in an adaptive width stream such as u8/u16/u32/u64 or i8/i16/i32/i64 under this descriptor. |
| 0x0020 | BITPACKED_WIDTH | Values may be bit-packed with a declared bit width. |
| 0x0040 | DELTA_OR_FOR | Values may use delta, frame-of-reference, or scaled integer transforms under declared codec rules. |
| 0x0080 | FLOAT_RAW_BITS | Float values preserve raw IEEE bits and require float-specific predicate safety rules. |

**Rules:**
- `null_representation` MUST be `null_bitmap_only` for COVE-Core/COVE-T v2. NumCode values are never null sentinels.
- Physical equality, ordering, and range comparison are usable only when the corresponding descriptor flags are set and the logical type, collation, NaN, signed-zero, decimal scale, timestamp unit, and overflow rules are understood.
- Encoded predicate kernels MUST NOT run on NumCode streams unless `ENCODED_PREDICATE_SAFE` or a codec-specific equivalent is declared and validated.
- A descriptor mismatch between catalog, page, codec, and kernel metadata is corruption for the affected operation.
- A reader MAY ignore NumCode encoding descriptors and decode through the baseline logical path.

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

    RegisteredEncoding = 19,
}
```

### 20.2 Encoding Node Descriptor

```rust
struct CoveEncodingNodeV2 {
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

### 20.3 Approved v2 Encoding Cascades

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
struct LocalCodebookPayloadV2 {
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


### 20.5.1 Page Reconstruction Authority

A page's decoded logical sequence is reconstructed from one of three authority classes:

```rust
enum PageReconstructionSource {
    Payload = 0,          // normal page payload and buffers
    ConstantParams = 1,   // explicit constant parameters in the encoding tree
    StatsConstant = 2,    // validated decode-required stats entry for all-null/all-non-null constant pages
}
```

**Rules:**
- `Payload` is the default.
- `ConstantParams` is canonical page data even when it is small enough to appear in an encoding parameter block.
- `StatsConstant` is allowed only under the strict page-elision rules in Section 27.2.
- When `StatsConstant` is used, the referenced stats entry is no longer optional pushdown metadata. It is decode-required canonical reconstruction data for that page.
- A reader MUST reject a `StatsConstant` page when the stats entry is missing, corrupt, unsafe, truncated, collation-incompatible, or unable to represent the exact logical value.
- Writers SHOULD prefer `ConstantParams` when exact value reconstruction from stats would be ambiguous, especially for floats, decimals, fixed bytes, redacted values, and extension logical types.

### 20.6 Constant and Payload-Elided Storage

Constant encoding is a first-class storage optimisation, not only a predicate-statistics hint.
**Rules:**
- Constant pages MAY omit value buffers when the value can be reconstructed from Constant parameters or, for stats-only all-non-null pages, from a validated page-level ZoneStatsEntry under the rules in 27.2.
- Stats-only constant pages are allowed only for all-null pages or all-non-null pages. Mixed null/non-null constant pages MAY elide the value stream but MUST retain enough null-position information to reconstruct logical row order.
- If the constant value is stored in Constant parameters, the page-level ZoneStatsEntry SHOULD still set IS_CONSTANT and SHOULD use matching min_value and max_value when min/max are valid.
- If the constant value is stored only in stats, the stats entry is decode-required canonical data for that page. It MUST be checksummed, bounds-checked, type-checked, and collation-checked before decoding.
- Readers MUST NOT use raw FileCode min/max as the logical constant for comparable FileCode columns. The constant must be a FileCode equality value or a canonical/domain-ranked value according to the column's declared physical kind and domain rules.

### 20.7 Registered Codec Extension Gate

COVE v2 replaces the v1 specialised-encoding placeholder with a formal COVE-CX codec-extension profile.

**Core rule:** specialised encodings are allowed only when their byte-level wire format, parameters, canonical decode algorithm, feature bits, fallback behaviour, and conformance vectors are defined by COVE v2 itself, by a companion COVE-CX codec specification, or by a registered required extension.

**High-priority candidate v2 codec registrations:**
- `org.coveformat.codec.fsst-utf8.v2` for lossless string/byte encodings where FileCode dictionary encoding is not better.
- `org.coveformat.codec.alp-float.v2` for lossless Float32/Float64 NumCode encodings that preserve exact IEEE bit patterns, including signed zero, infinities, NaN class, and any payload handling declared by the codec specification.
- `org.coveformat.codec.fastlanes-integer.v2` for lossless integer/date/timestamp/decimal NumCode encodings using bit packing, frame-of-reference, delta, patched-base, or related vectorised integer techniques.

These names are **candidate registration identifiers** until companion COVE-CX codec specifications define exact bitstream bytes, parameter schemas, offset bases, block termination rules, fallback equivalence, positive vectors, and negative vectors. A candidate registration MUST NOT be treated as broadly v2-supported merely because the identifier appears in this document.

```rust
enum CodecSpecificationStatusV2 {
    Candidate = 0,             // named here but not yet broad-conformance-ready
    ProvisionalRegistered = 1, // exact spec exists but interoperability evidence is incomplete
    StableRegistered = 2,      // exact spec, vectors, and conformance evidence exist
    Deprecated = 3,
    VendorPrivate = 255,
}
```

**Rules:**
- Writers MUST NOT emit FSST-style, ALP-style, FastLanes-style, Chimp/Patas-style, or similar specialised encodings as core `CoveEncodingKind` values unless the exact byte-level format is registered and gated.
- A `Candidate` or `ProvisionalRegistered` codec MUST NOT be required for broad COVE-Core/COVE-T conformance. It MAY be used only with a validated core fallback payload or inside explicitly experimental/vendor conformance levels.
- A `StableRegistered` encoding needed for decoding projected data MUST either provide a validated canonical fallback payload or set a required feature bit that causes unsupported readers to reject safely.
- Optional registered encodings MAY be used inside COVX or fallback-bearing pages for experimentation, but unsupported readers MUST still recover the same logical values through core COVE encodings.
- Codec names are not enough for interoperability. The registry entry MUST identify the exact codec specification version and conformance vector set.
- Lossy codecs are prohibited for COVE-Core/COVE-T decode unless a required logical extension explicitly defines lossy semantics and every affected column is marked accordingly. COVE v2 core specialised codecs are assumed lossless.

### 20.8 COVE-CX Codec Extension Descriptor

```rust
struct CodecExtensionDescriptorV2 {
    codec_id: u32,

    namespace_len: u16,
    namespace: [u8],

    name_len: u16,
    name: [u8],

    version_major: u16,
    version_minor: u16,

    codec_family: u16,
    // 0=string_symbol_table
    // 1=float_alp_like
    // 2=integer_fastlanes_like
    // 3=bitstream_transform
    // 4=vendor_defined

    logical_type_mask: u64,
    physical_kind_mask: u64,

    requirement: u8,
    // 0=optional_with_fallback
    // 1=required_for_decode
    // 2=sidecar_only

    fallback_policy: u8,
    // 0=no_fallback
    // 1=core_encoding_payload_present
    // 2=dictionary_or_canonical_decode_path
    // 3=external_required_extension

    parameter_schema_kind: u8,
    // 0=none
    // 1=cove_binary_params
    // 2=canonical_cbor
    // 3=json_descriptive_only

    flags: u8,

    specification_status: u8,   // CodecSpecificationStatusV2
    reserved0: [u8; 3],

    required_feature_bit: u64,
    optional_feature_bit: u64,

    spec_digest_algorithm: u16,
    spec_digest_len: u16,
    spec_digest: [u8; spec_digest_len],

    conformance_vector_ref: u32,
    fallback_ref: u32,
    private_payload_ref: u32,

    checksum: u32,
}
```

**Rules:**
- `namespace + name + version` MUST identify one exact codec definition when `specification_status` is `ProvisionalRegistered` or `StableRegistered`.
- `Candidate` descriptors are allowed only for experimental, sidecar-only, or fallback-bearing use and MUST NOT be required for ordinary COVE-Core/COVE-T decode.
- `spec_digest` SHOULD identify the exact codec specification or canonical bitstream definition used by the writer.
- `conformance_vector_ref` SHOULD reference positive and negative codec test vectors.
- `fallback_ref` MUST be valid when `fallback_policy` requires a fallback.
- A codec descriptor MUST declare whether it supports equality kernels, range kernels, selection decode, direct FileCode-to-ExecutionCode remap, or only full decode through `KERNEL_CAPABILITIES` or a codec-specific capability payload.
- A codec descriptor MUST declare any restrictions on null handling, value ordering, NaN handling, signed zero handling, byte ordering, padding bits, and final-block termination.
- A registered codec MUST be deterministic and side-effect-free.


### 20.8.1 Registered Encoding Dispatch

Registered codec payloads MUST be reachable through an explicit encoding node. A page whose root value stream is encoded by a registered codec MUST use `CoveEncodingKind::RegisteredEncoding` at the appropriate encoding node and MUST provide a `RegisteredEncodingEnvelopeV2` in that node's parameter payload or in a referenced page buffer.

**Rules:**
- `RegisteredEncoding` is not itself a codec; it is the COVE dispatch envelope for an exact registered codec descriptor.
- A reader MUST validate the codec descriptor and envelope before touching codec-specific bytes.
- A reader MUST NOT dispatch solely on runtime registry names, implementation class names, or vendor strings.
- If the codec is unsupported and a valid fallback payload exists, the reader MAY use the fallback.
- If the codec is unsupported and no valid fallback exists, the reader MUST reject only the operation that needs the encoded page.

### 20.9 Registered Encoding Page Envelope

Registered codecs use a common page-level envelope so readers can reject, fall back, or dispatch safely before touching codec-specific bytes.

```rust
struct RegisteredEncodingEnvelopeV2 {
    codec_id: u32,
    codec_version_major: u16,
    codec_version_minor: u16,

    logical_len: u32,
    non_null_count: u32,

    params_offset: u32,
    params_length: u32,

    encoded_payload_offset: u64,
    encoded_payload_length: u64,

    fallback_payload_offset: u64,  // 0 when absent
    fallback_payload_length: u64,  // 0 when absent

    decoded_uncompressed_length: u64,

    flags: u32,
    checksum: u32,
}
```

**Rules:**
- The envelope is part of the page payload and is covered by the page checksum.
- `logical_len` MUST match the page index row_count and root encoding node logical length.
- If a fallback payload is present, the fallback MUST decode to exactly the same logical sequence and null positions as the registered codec payload.
- If the registered codec is unsupported and a valid fallback payload is present, a reader MAY use the fallback.
- If the registered codec is unsupported and no valid fallback exists, a reader MUST reject any operation that needs the page.
- A reader MUST NOT choose an optional registered payload over a core fallback unless it supports the exact codec version.


### 20.10 Codec Pipeline Classification and Acceleration Neutrality

COVE-CX distinguishes logical encodings, lightweight physical encodings, compression, integrity transforms, and acceleration-only transforms. This prevents a writer from treating a hardware path or runtime plugin as the wire-format definition.

```rust
enum CodecTransformClassV2 {
    LogicalEncoding = 0,
    PhysicalLightweightEncoding = 1,
    BlockCompression = 2,
    ChecksumIntegrityTransform = 3,
    AccelerationOnlyTransform = 4,
    VendorDefined = 255,
}

struct CodecPipelineStageV2 {
    stage_id: u16,
    transform_class: u8,
    codec_id: u32,
    input_physical_kind: u16,
    output_physical_kind: u16,
    independent_decode_unit_rows: u32,
    preferred_block_size_bytes: u32,
    supports_random_access: u8,
    supports_encoded_scan: u8,
    supports_partial_decode: u8,
    supports_selective_decode: u8,
    canonical_decoder_required: u8,
    optional_accelerated_decoder: u8,
    fallback_decoder_ref: u32,
    conformance_vector_ref: u32,
    checksum: u32,
}
```

**Rules:**
- A COVE file MUST define the canonical byte-level decode path independently of any SIMD, GPU, Intel IAA/QPL, ARM extension, FPGA, or other hardware accelerator.
- Implementations MAY use hardware acceleration when it is semantically equivalent to the canonical decoder and all alignment, lifetime, page, null, checksum, and fallback requirements are satisfied.
- A hardware-specific decoder MUST NOT be required for baseline COVE-Core/COVE-T decode unless a non-portable required extension explicitly declares that dependency and unsupported readers reject safely.
- Codec pipeline metadata is advisory unless the registered codec itself is required for projected data. Unsupported advisory pipeline stages MUST be ignored.
- `supports_encoded_scan`, `supports_partial_decode`, and `supports_selective_decode` are capability claims. They MUST NOT be used as predicate-proof metadata.

---

## 21. Kernel Capability Metadata

COVE-T MAY declare encoding kernel capabilities.

```rust
struct EncodingKernelCapabilityV2 {
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


### 21.1 V2 Codec and Kernel Capability Binding

COVE-CX codec descriptors and `KERNEL_CAPABILITIES` MAY be linked so engines can decide whether to evaluate predicates on encoded data, decode into canonical values, or materialise engine-local vectors.

```rust
struct CodecKernelCapabilityV2 {
    codec_id: u32,
    encoding_kind: u16,

    supports_eq: u8,
    supports_in: u8,
    supports_range: u8,
    supports_is_null: u8,
    supports_like_or_prefix: u8,
    supports_selection_decode: u8,
    supports_direct_executioncode_remap: u8,
    supports_zero_copy_export: u8,

    decode_cost_class: u8,
    predicate_cost_class: u8,
    random_access_cost_class: u8,
    reserved0: u8,

    min_reader_version_major: u16,
    min_reader_version_minor: u16,

    checksum: u32,
}
```

**Rules:**
- Capability metadata is advisory and MUST NOT be trusted as proof for skipping rows.
- Predicate skipping still requires validated COVE predicate-proof metadata.
- A false capability declaration is a writer/tooling error but MUST NOT change query results; readers MAY ignore capability metadata.
- Zero-copy export capability means only that the codec/page/buffer layout may be exposed without copy when all other nullability, alignment, lifetime, and target format rules also hold.

---

## 22. Collation and Canonicalisation Registry

Collation metadata defines safe ordering semantics.
Range pushdown is allowed only when query collation and stored collation agree.

```rust
struct CollationRegistryHeaderV2 {
    entry_count: u32,
    flags: u32,
}
```

```rust
struct CollationRegistryEntryV2 {
    collation_id: u16,

    name_len: u16,
    name: [u8],

    version_len: u16,
    version: [u8],

    flags: u32,
}
```

**Minimum v2 collations:**

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
struct ColumnDomainHeaderV2 {
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
struct TableCatalogV2 {
    table_count: u32,
    flags: u32,

    tables: [TableEntryV2],
}
```

```rust
struct TableEntryV2 {
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

    columns: [TableColumnEntryV2],
}
```

```rust
struct TableColumnEntryV2 {
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
struct TableSegmentIndexEntryV2 {
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
struct TableSegmentHeaderV2 {
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
struct RowMorselEntryV2 {
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
struct TableColumnDirectoryEntryV2 {
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
struct ColumnPageIndexEntryV2 {
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
| 0xFFFF_F000 | reserved | Reserved for future required page extensions; MUST be zero in v2 unless a required extension defines the bit and the reader supports that extension. |

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
- A stats-only constant page MUST declare or imply `PageReconstructionSource::StatsConstant` and MUST NOT be treated as a normal optional statistics optimisation.
- A stats-only constant page MUST NOT use truncated `StatScalar` values for reconstruction. Truncated min/max may remain advisory pruning metadata, but they cannot be the only source of a decoded constant value.
- If the page contains redacted values, the reconstruction source MUST preserve the redaction marker and policy reference. A redacted constant MUST NOT be reconstructed as null or as the unredacted value.

### 27.3 Page Payload

**A column page payload contains:**
[column page header]
[encoding node descriptors]
[buffer directory]
[buffers]

```rust
struct ColumnPagePayloadHeaderV2 {
    magic: [u8; 4],          // "CPG2"
    version_major: u16,      // 2
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

struct PageBufferDescriptorV2 {
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
- `root_node_id` MUST identify exactly one `CoveEncodingNodeV2`, and that node's `logical_len` MUST equal the page row count.
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
struct ZoneStatsEntryV2 {
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

    min_value: StatScalarV2,
    max_value: StatScalarV2,

    min_domain_rank: u32,
    max_domain_rank: u32,

    exact_set_ref: u32,
    bloom_ref: u32,
}
```

```rust
struct StatScalarV2 {
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
| 1-7 | reserved | MUST be zero in v2. |

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


### 29.3 COVE-COVERAGE Query Coverage Semantics

COVE-COVERAGE defines the common vocabulary for conservative query coverage. A coverage set identifies the fragments that are sufficient to evaluate a predicate, answer an index-only query, route a lookup, or produce a conservative scan plan for a declared dataset snapshot.

```rust
enum CoverageGranularityV2 {
    Dataset = 0,
    Object = 1,
    File = 2,
    Segment = 3,
    RowGroup = 4,
    Page = 5,
    Morsel = 6,
    RowRange = 7,
    RowOrdinalSet = 8,
    MapNode = 9,
    DimensionalBucket = 10,
    ObjectPath = 11,
    Association = 12,
    ProjectionFragment = 13,
    ExternalFragment = 255,
}

enum CoverageProofKindV2 {
    MinMaxExclusion = 0,
    DictionaryMembership = 1,
    BloomMaybe = 2,
    ZoneMap = 3,
    ExactSet = 4,
    ValueToFragmentIndex = 5,
    RangeBucketLayout = 6,
    SemanticPathMapping = 7,
    ObjectDimensionMapping = 8,
    AggregateSynopsis = 9,
    LookupIndex = 10,
    CompositeZone = 11,
    EngineObservedCache = 12,
    ExternalIndex = 13,
    RuntimeHint = 14,
    VendorDefined = 255,
}

enum CoverageProofStrengthV2 {
    ExactTight = 0,
    ExactConservative = 1,
    ProbabilisticConservative = 2,
    AdvisoryOnly = 3,
    EngineLocal = 4,
    ApproximateMayUnderInclude = 5,
}

enum CoverageExactnessV2 {
    Exact = 0,
    ApproximateOverInclusiveOnly = 1,
    ApproximateMayUnderInclude = 2,
    Unknown = 255,
}
```

```rust
struct CoverageProviderDescriptorV2 {
    provider_id: u32,
    provider_kind: u16,          // CoverageProofKindV2 or registered extension kind
    profile: u8,                 // COVE-T/COVE-A/COVX/COVE-I/COVM/COVE-MAP/COVE-CACHE/etc.
    granularity: u8,
    proof_strength: u8,
    exactness: u8,
    flags: u16,

    referenced_table_id: u32,
    referenced_column_id: u32,   // u32::MAX when path/object/dataset scoped
    referenced_path_ref: u32,    // 0 when not path scoped
    logical_type: u16,
    collation_id: u16,
    null_semantics: u8,
    snapshot_validity_ref: u32,
    predicate_form_ref: u32,
    producer_ref: u32,
    checksum: u32,
}

struct CoverageSetHeaderV2 {
    coverage_set_id: u32,
    provider_id: u32,
    granularity: u8,
    proof_strength: u8,
    exactness: u8,
    flags: u8,

    predicate_form_ref: u32,
    snapshot_validity_ref: u32,

    total_fragment_count: u64,
    covered_fragment_count: u64,
    required_fragment_count_estimate: u64,

    coverage_degree_ppm: u32,
    tightness_degree_ppm: u32,

    entries_offset: u64,
    entries_length: u64,
    checksum: u32,
}

struct CoverageSetEntryV2 {
    target_kind: u16,            // CoverageGranularityV2
    flags: u16,
    file_ref: u32,
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,
    page_ref: u32,
    object_type_id: u32,
    path_ref: u32,
    dimensional_bucket_ref: u32,
    row_start: u64,
    row_count: u64,
    row_ordinal_bitmap_ref: u32,
    byte_range_ref: u32,
    checksum: u32,
}
```

**Definitions:**
- A **coverage set** is a set of fragments that is sufficient to contain every row, object, association, projection row, or value that can satisfy the declared predicate context for the declared snapshot.
- A **tight coverage set** is a conservative coverage set that contains only necessary fragments according to the declared proof model.
- **coverage_degree_ppm** estimates how much of the full search space is covered by the set, expressed in parts per million. Smaller values normally imply less data to read.
- **tightness_degree_ppm** estimates how close the coverage set is to the tight set under the declared model. Higher values normally imply less over-inclusion.
- **coverage_confidence** and cost estimates are planning metadata only. They are not proof.

**Rules:**
- A reader MAY skip fragments outside a validated conservative coverage set only when `proof_strength` is `ExactTight`, `ExactConservative`, or `ProbabilisticConservative` with a no-false-negative contract that is understood.
- A reader MUST NOT skip fragments based on `AdvisoryOnly`, `EngineLocal`, `ApproximateMayUnderInclude`, stale, corrupt, or unsupported coverage metadata.
- Bloom-derived coverage MAY be conservative for exclusion only if the Bloom implementation guarantees no false negatives for the declared hash domain and snapshot. Bloom membership positives are candidates, not proof of match.
- Coverage metrics and cost estimates MUST NOT be used as correctness proof.
- Coverage sets MUST declare snapshot validity and MUST NOT be reused across dataset snapshots, schema changes, semantic-map versions, sidecar versions, or external visibility overlays unless the validity descriptor explicitly proves compatibility.
- Coverage entries that reference COVE rows or pages MUST identify the target file by `file_id` plus file length, footer CRC, and digest where available.
- Coverage over object/association/projection data MUST be interpreted through the declared COVE-O/COVE-MAP identity, temporal, projection, and evidence rules.

### 29.3.1 Coverage Set Entry Grammar and Invariants

`CoverageSetEntryV2` is a tagged union encoded as one fixed-width structure. The `target_kind` field determines which identifier fields are meaningful. Unused identifier fields MUST use the absent sentinel below and MUST NOT be interpreted by readers.

**Absent sentinels:**
- Identifier fields ending in `_id` use `u32::MAX` when absent.
- Reference fields ending in `_ref` use `u32::MAX` when absent unless the enclosing section explicitly defines `0` as the absent reference for that reference space.
- `row_start` MUST be `0` and `row_count` MUST be `0` when the entry is not row-range scoped.
- `checksum` covers the fixed entry with the checksum field zeroed.

| `target_kind` | Required fields | Optional fields | Required absent fields |
| --- | --- | --- | --- |
| `Dataset` | none beyond `snapshot_validity_ref` in the header | `byte_range_ref` for whole-dataset planning ranges | table/segment/morsel/page/object/path/bucket/row fields |
| `Object` | `object_type_id`, `path_ref` or object identity payload via `byte_range_ref` | `file_ref` when object rows are materialised in a file | segment/page/morsel row fields unless physically scoped |
| `File` | `file_ref` | `byte_range_ref` for file-level range hints | table/segment/morsel/page/row fields |
| `Segment` | `file_ref`, `table_id`, `segment_id` | `byte_range_ref` | morsel/page/row fields |
| `RowGroup` | `file_ref`, `table_id`, `segment_id`, `row_start`, `row_count` | `byte_range_ref` | morsel/page unless also declared by extension |
| `Page` | `file_ref`, `table_id`, `segment_id`, `page_ref` | `morsel_id`, `byte_range_ref` | row ordinal set unless explicitly page-row scoped |
| `Morsel` | `file_ref`, `table_id`, `segment_id`, `morsel_id` | `byte_range_ref` | page/row ordinal fields |
| `RowRange` | `file_ref`, `table_id`, `segment_id`, `row_start`, `row_count` | `morsel_id` if range is morsel-local | `row_ordinal_bitmap_ref` |
| `RowOrdinalSet` | `file_ref`, `table_id`, `row_ordinal_bitmap_ref` | `segment_id`, `morsel_id` | `row_count` unless the bitmap descriptor declares count |
| `MapNode` | `path_ref` | `file_ref`, `byte_range_ref` | table row fields unless map node is materialised as table rows |
| `DimensionalBucket` | `dimensional_bucket_ref` | `file_ref`, `table_id`, `segment_id`, `byte_range_ref` | row fields unless bucket maps to row ranges |
| `ObjectPath` | `object_type_id`, `path_ref` | `file_ref`, `byte_range_ref` | table row fields unless materialised |
| `Association` | `object_type_id` or `path_ref` identifying association type | endpoint references via extension payload | table row fields unless materialised |
| `ProjectionFragment` | `path_ref` or projection fragment ref | `file_ref`, `table_id`, `segment_id` | unused physical fields |
| `ExternalFragment` | `byte_range_ref` or extension payload | implementation-defined with required extension | all fields not declared by extension |

**Entry ordering and duplicate rules:**
- Entries in one `CoverageSetHeaderV2` payload MUST be sorted by `(target_kind, file_ref, table_id, segment_id, morsel_id, page_ref, object_type_id, path_ref, dimensional_bucket_ref, row_start, row_count)` after substituting absent sentinels.
- Exact duplicate entries are invalid.
- Row ranges for the same physical scope MUST be sorted by `row_start`, non-overlapping, and coalesced when adjacent unless the writer sets a diagnostic flag explaining why ranges are intentionally split.
- Row ordinal sets for the same physical scope MUST NOT overlap unless a required extension declares multiset semantics. COVE-Core/COVE-T coverage sets use mathematical set semantics, not bag semantics.
- A `CoverageSetHeaderV2` coverage set is the union of its entries.
- `total_fragment_count`, `covered_fragment_count`, `coverage_degree_ppm`, and `tightness_degree_ppm` are metrics; they MUST NOT be used to infer missing entries.

### 29.3.2 Coverage Set Algebra for Predicate Planning

Coverage set algebra is defined only for validated coverage sets over the same snapshot, schema fingerprint, semantic-map fingerprint when applicable, external visibility overlay state, predicate logical context, and compatible granularity.

```rust
enum CoverageSetOperationV2 {
    Union = 0,
    Intersection = 1,
    Difference = 2,
    Complement = 3,
    Coarsen = 4,
    Refine = 5,
}
```

**Rules:**
- For `A OR B`, a reader MAY use the union of the validated coverage sets for `A` and `B`.
- For `A AND B`, a reader MAY use the intersection of validated coverage sets only when both sets share compatible granularity and proof semantics. Otherwise it MUST use the narrower understood conservative set, a coarsened conservative set, or full scan fallback.
- For `NOT A`, a reader MUST NOT compute a complement coverage set unless the provider explicitly declares a complete universe, compatible null/UNKNOWN semantics, external visibility overlay compatibility, and exact complement proof. The default outcome for NOT is `Unknown`.
- `Difference` is allowed only for diagnostic or planner-estimation use unless the provider supplies exact set-difference proof under SQL three-valued semantics.
- `Coarsen` may convert row/page/morsel coverage to a broader granularity such as segment or file when all covered lower-level fragments map into the broader fragments. Coarsening is safe but may reduce tightness.
- `Refine` may split a broader fragment into narrower fragments only when a validated provider proves that no satisfying values exist outside the refined subset.
- If two coverage providers disagree, a reader MUST choose a conservative over-inclusive plan, ignore one provider, or scan. It MUST NOT use disagreement to under-include data.

### 29.3.3 Coverage Proof Records

A coverage set used for pruning or an index-only answer SHOULD be linked to an explicit proof record. A proof record binds the predicate form, provider, coverage set, snapshot validity, and proof semantics.

```rust
struct CoverageProofRecordV2 {
    proof_id: u32,
    provider_id: u32,
    coverage_set_id: u32,
    predicate_form_ref: u32,
    proof_kind: u16,
    proof_strength: u8,
    exactness: u8,
    granularity: u8,
    null_semantics: u8,
    flags: u16,
    snapshot_validity_ref: u32,
    coverage_set_checksum: u32,
    proof_payload_ref: u32,
    checksum: u32,
}
```

**Rules:**
- `coverage_set_checksum` MUST match the validated coverage set that is used.
- `proof_payload_ref` MAY reference provider-specific evidence such as min/max ranges, dictionary value sets, Bloom descriptor, index root, dimensional bucket definition, or COVE-MAP semantic path mapping.
- A proof record with unsupported `proof_kind`, unsupported collation, unsafe null semantics, stale validity, or checksum mismatch MUST NOT be used for pruning or exact answering.
- Approximate or may-under-include proof records MAY be used for advisory ranking or candidate generation only.

### 29.4 Predicate Normal Forms and Interval Predicates

Coverage providers need a stable predicate representation. COVE-COVERAGE defines several forms so readers can choose the weakest form that is sufficient and safe.

```rust
enum PredicateFormKindV2 {
    PredicateAst = 0,
    PredicateCnf = 1,
    IntervalPredicateForm = 2,
    EncodedPredicateForm = 3,
    EnginePrivate = 255,
}

struct PredicateNormalFormV2 {
    predicate_form_id: u32,
    form_kind: u16,
    flags: u16,
    logical_context_ref: u32,
    payload_offset: u64,
    payload_length: u64,
    checksum: u32,
}

struct IntervalPredicateV2 {
    column_or_path_ref: u32,
    logical_type: u16,
    collation_id: u16,
    null_policy: u8,          // 0=null_excluded, 1=null_included, 2=sql_unknown, 3=extension_defined
    bound_kind: u8,           // 0=lower_upper, 1=point, 2=open_range, 3=multi_interval_ref
    flags: u16,
    lower_inclusive: u8,
    upper_inclusive: u8,
    reserved: u16,
    lower_value_ref: u32,     // canonical value ref or u32::MAX for unbounded
    upper_value_ref: u32,     // canonical value ref or u32::MAX for unbounded
    checksum: u32,
}
```

### 29.4.1 Canonical Predicate Payload Grammar

`PredicateNormalFormV2.payload_offset` and `payload_length` identify one of the following payload grammars according to `form_kind`. All offsets are relative to the containing `PREDICATE_NORMAL_FORM` section payload unless the section kind explicitly says otherwise. Every payload is length-delimited and checksummed by `PredicateNormalFormV2.checksum`; nested payload records with their own checksum cover their own fixed fields with the checksum field zeroed.

```rust
enum PredicateOpV2 {
    TrueLiteral = 0,
    FalseLiteral = 1,
    IsNull = 2,
    IsNotNull = 3,
    Eq = 4,
    NotEq = 5,
    Lt = 6,
    LtEq = 7,
    Gt = 8,
    GtEq = 9,
    Between = 10,
    InSet = 11,
    And = 12,
    Or = 13,
    Not = 14,
    LikePrefix = 15,
    Contains = 16,
    IsNaN = 17,
    IsFinite = 18,
    FunctionCall = 19,
    LiteralValue = 20,
    ColumnRef = 21,
    Extension = 255,
}

enum PredicateNullPolicyV2 {
    SqlWhere = 0,          // TRUE selects; FALSE/UNKNOWN do not select
    NullExcluded = 1,
    NullIncluded = 2,
    NullOnly = 3,
    NullRejected = 4,
    ExtensionDefined = 255,
}

enum PredicateOperandKindV2 {
    Node = 0,
    Literal = 1,
    LiteralList = 2,
    ColumnOrPath = 3,
    Function = 4,
    IntervalSet = 5,
    Extension = 255,
}

struct PredicateAstPayloadHeaderV2 {
    root_node_id: u32,
    node_count: u32,
    literal_count: u32,
    literal_list_count: u32,
    function_count: u32,
    operand_ref_count: u32,

    node_offset: u64,
    literal_offset: u64,
    literal_list_offset: u64,
    function_offset: u64,
    operand_ref_offset: u64,

    flags: u32,
    checksum: u32,
}

struct PredicateAstOperandRefV2 {
    parent_node_id: u32,
    ordinal: u16,
    operand_kind: u8,       // PredicateOperandKindV2
    flags: u8,
    ref_id: u32,            // node_id, literal_id, literal_list_id, column_or_path_ref, function_ref, interval_set_id, or extension ref
    checksum: u32,
}

struct PredicateAstNodeV2 {
    node_id: u32,
    op: u16,                    // PredicateOpV2
    flags: u16,
    result_logical_type: u16,
    collation_id: u16,
    null_policy: u8,
    reserved0: u8,

    operand_count: u16,
    first_operand_index: u32,    // index into PredicateAstOperandRefV2 array, or u32::MAX

    column_or_path_ref: u32,     // fast-path mirror; u32::MAX when unused
    literal_ref: u32,            // fast-path mirror; u32::MAX when unused
    function_ref: u32,           // fast-path mirror; u32::MAX when unused
    aux_ref: u32,                // literal list, interval set, extension payload, or u32::MAX

    checksum: u32,
}

struct PredicateLiteralV2 {
    literal_id: u32,
    value_tag: u16,
    logical_type: u16,
    flags: u32,
    canonical_value_offset: u64,
    canonical_value_length: u32,
    checksum: u32,
}

struct PredicateLiteralListV2 {
    literal_list_id: u32,
    first_literal_index: u32,
    literal_count: u32,
    flags: u32,
    checksum: u32,
}

struct PredicateFunctionRefV2 {
    function_ref: u32,
    namespace_len: u16,
    namespace: [u8],
    name_len: u16,
    name: [u8],
    version_major: u16,
    version_minor: u16,
    deterministic: u8,
    flags: u8,
    required_extension_ref: u32,
    checksum: u32,
}
```

**Predicate AST reference spaces:**
- `root_node_id` MUST identify exactly one `PredicateAstNodeV2` unless the payload flag explicitly declares a fragment list.
- `node_id`, `literal_id`, `literal_list_id`, and `function_ref` are local to one predicate payload and MUST be unique within their own tables.
- If `operand_count == 0`, `first_operand_index` MUST be `u32::MAX`. If `operand_count > 0`, `first_operand_index` MUST NOT be `u32::MAX` and `first_operand_index + operand_count` MUST lie within the operand-ref array.
- Operand references for one node MUST be contiguous, sorted by `ordinal`, and have ordinals `0..operand_count-1` without gaps.
- The operand-ref table is the canonical predicate encoding. `column_or_path_ref`, `literal_ref`, `function_ref`, and `aux_ref` are redundant fast-path mirrors only. A mirror field MUST be `u32::MAX` or MUST exactly match the corresponding canonical operand. A mirror field MUST NOT satisfy an operator's arity requirement by itself.
- A reader MUST validate and interpret predicate semantics from operand references. If a non-`u32::MAX` mirror disagrees with the operand-ref table, the predicate payload is malformed and MUST NOT be used for pruning or exact answering.

**Operator arity and operand binding:**

| Operator | Required operands | Binding rules |
| --- | --- | --- |
| `TrueLiteral`, `FalseLiteral` | 0 | No column, literal, function, or interval operands. |
| `LiteralValue` | 1 literal operand | Produces the canonical literal value for expression-to-expression predicates. `literal_ref` MAY mirror the operand but is not canonical. |
| `ColumnRef` | 1 column/path operand | Produces a column/path value; range use still requires declared collation/order semantics. `column_or_path_ref` MAY mirror the operand but is not canonical. |
| `IsNull`, `IsNotNull`, `IsNaN`, `IsFinite` | 1 column/path or node | Operand 0 is the value being tested. Null and NaN semantics MUST be explicit. |
| `Eq`, `NotEq`, `Lt`, `LtEq`, `Gt`, `GtEq`, `LikePrefix`, `Contains` | 2 | Operand 0 is normally a column/path or expression node; operand 1 is normally a literal, literal-value node, or expression node. Simple column-literal atoms SHOULD mirror operands through `column_or_path_ref` and `literal_ref`. |
| `Between` | 3 | Operand 0 is column/path or expression; operand 1 is lower literal/expression; operand 2 is upper literal/expression. Flags bit 0 means lower inclusive; bit 1 means upper inclusive. Missing bounds MUST use `IntervalPredicateV2`, not an omitted AST operand. |
| `InSet` | 2 | Operand 0 is column/path or expression; operand 1 is `LiteralList`. Literal-list values MUST be canonical, sorted by declared equality/collation where applicable, and duplicate-free unless an extension defines multiset semantics. |
| `And`, `Or` | 2 or more node operands | N-ary logical operators are canonical. Writers SHOULD flatten nested same-op nodes and sort proof-safe atoms deterministically when doing so preserves semantics. |
| `Not` | 1 node operand | Readers MUST be conservative under SQL UNKNOWN semantics. `NOT` over nullable or NaN-sensitive expressions often remains `Unknown` for pruning. |
| `FunctionCall` | 1 function operand plus zero or more argument operands | Operand 0 MUST be a `Function` operand identifying the function. Argument operands follow the function operand by ordinal unless the function payload defines a different order. `function_ref` MAY mirror operand 0 but is not canonical. Functions used for pruning MUST be deterministic and fully versioned. |
| `Extension` | extension-defined | The required extension MUST define arity, operand kinds, null semantics, and proof safety. Unsupported extension nodes evaluate to `Unknown` for pruning. |

**Predicate AST rules:**
- AST nodes MUST form a finite acyclic graph with exactly one root unless the payload is explicitly a list of predicate fragments.
- Literal values MUST be COVE canonical value bytes. Display strings, raw source bytes, raw FileCodes, and engine-local ExecutionCodes are not valid predicate literals.
- `And` and `Or` nodes are n-ary and MUST use node operands. Binary logical trees MAY be normalised to n-ary form.
- `LiteralValue`, `ColumnRef`, `Between`, `InSet`, n-ary `And`/`Or`, `Not`, and `FunctionCall` MUST follow the arity table above through canonical operand references; malformed arity is a predicate-payload validation error.
- A `FunctionCall` used for pruning MUST reference a deterministic, versioned function with declared null, collation, timezone, and failure behaviour.
- Unknown predicate operations, unknown deterministic functions, malformed arity, and unsupported extension nodes MUST evaluate to `Unknown` for pruning.

### 29.4.2 CNF/DNF Payload Grammar

```rust
enum PredicateNormalisationKindV2 {
    Cnf = 0,
    Dnf = 1,
    FlatConjunction = 2,
    FlatDisjunction = 3,
}

struct PredicateNormalisedPayloadHeaderV2 {
    normalisation_kind: u8,
    flags: u8,
    reserved: u16,
    clause_count: u32,
    term_count: u32,
    clause_offset: u64,
    term_offset: u64,
    checksum: u32,
}

struct PredicateClauseEntryV2 {
    clause_id: u32,
    first_term_index: u32,
    term_count: u32,
    flags: u32,
    checksum: u32,
}

struct PredicateTermV2 {
    term_id: u32,
    ast_node_ref: u32,
    negated: u8,
    null_policy: u8,
    proof_safe: u8,
    reserved: u8,
    checksum: u32,
}
```

**CNF/DNF rules:**
- CNF and DNF payloads MUST reference AST atom nodes through `ast_node_ref`; they MUST NOT invent different literal semantics.
- Terms within a clause SHOULD be sorted by `(column_or_path_ref, op, literal canonical bytes)` for deterministic equality.
- Duplicate terms SHOULD be removed by writers and MAY be ignored by readers.
- A term marked `proof_safe = 0` MUST NOT be used for coverage exclusion or inclusion, but MAY remain in the predicate form for full evaluation.

### 29.4.3 Multi-Interval Predicate Payload Grammar

`IntervalPredicateV2.bound_kind = multi_interval_ref` references an `IntervalPredicateSetV2` payload. Multi-interval sets are used for `IN`, disjoint ranges, dimensional buckets, and predicate-containment caches.

```rust
struct IntervalPredicateSetV2 {
    interval_set_id: u32,
    column_or_path_ref: u32,
    logical_type: u16,
    collation_id: u16,
    null_policy: u8,
    flags: u8,
    interval_count: u32,
    intervals_offset: u64,
    checksum: u32,
}

struct IntervalBoundPairV2 {
    lower_value_ref: u32,       // u32::MAX for unbounded
    upper_value_ref: u32,       // u32::MAX for unbounded
    lower_inclusive: u8,
    upper_inclusive: u8,
    flags: u16,
    checksum: u32,
}
```

**Interval rules:**
- Intervals in one set MUST be sorted by lower bound under the declared collation and logical type.
- Intervals MUST be non-overlapping. Adjacent intervals SHOULD be coalesced when inclusivity makes them equivalent to a single range.
- `u32::MAX` unbounded sentinels are allowed only where the bound direction permits unbounded range semantics.
- Float intervals MUST declare NaN and signed-zero behaviour. Min/max or interval exclusion MUST NOT be used for NaN-sensitive predicates unless safe rules are declared.
- String intervals require a known compatible collation. Bytewise UTF-8 range rules are not a substitute for locale collation unless the column declares bytewise collation.

### 29.4.4 Encoded Predicate Form Payload Grammar

```rust
struct EncodedPredicateFormV2 {
    encoded_predicate_id: u32,
    baseline_predicate_ref: u32,
    table_id: u32,
    column_id: u32,
    logical_type: u16,
    physical_kind: u8,
    encoding_kind: u16,
    codec_id: u32,              // 0 when core encoding only
    flags: u32,
    equivalence_kind: u8,        // 0=exact_logical_equivalence, 1=conservative_no_false_negative, 2=advisory_only
    null_semantics: u8,
    collation_id: u16,
    params_offset: u64,
    params_length: u64,
    checksum: u32,
}
```

**Encoded predicate rules:**
- Encoded predicate evaluation is allowed only when the page encoding, NumCode descriptor, codec descriptor, and kernel capability all declare equivalence to baseline logical evaluation or conservative no-false-negative behaviour for the specific predicate class.
- `advisory_only` encoded predicate forms MUST NOT be used for pruning.
- Encoded predicates MUST preserve COVE structural null semantics and SQL TRUE/FALSE/UNKNOWN selection rules.
- FileCode encoded predicates may compare raw FileCodes for equality only after query literals are resolved through the same file dictionary. Range predicates over FileCode columns require ColumnDomain/domain-rank semantics.

**Rules:**
- `PredicateAst` is the general canonical predicate form.
- `PredicateCnf` is a normalised conjunction/disjunction form suitable for proof composition.
- `IntervalPredicateForm` is the range-compatible subset used by range pruning, dimensional buckets, coverage caches, and range indexes.
- `EncodedPredicateForm` may be used only when the underlying physical encoding declares the predicate physically safe and equivalent to baseline logical evaluation.
- Interval predicates MUST use canonical logical values, declared collation, declared null semantics, and length-delimited canonical bytes. They MUST NOT compare source display bytes, raw FileCodes, or engine-local ExecutionCodes.
- A predicate form with unknown functions, unknown collation, unsupported extension logical types, or unsafe null/NaN semantics MUST evaluate as `Unknown` for pruning unless a required extension defines safe behaviour.

### 29.5 Coverage Plan Candidates and Do-No-Harm Fallback

A tight coverage set is not always the best plan if computing it is more expensive than scanning a broader set. COVE therefore exposes costed coverage plan candidates without mandating a planner algorithm.

```rust
struct CoveragePlanCandidateV2 {
    candidate_id: u32,
    predicate_fragment_ref: u32,
    provider_id: u32,
    provider_type: u16,
    flags: u16,

    estimated_lookup_cost_ns: u64,
    estimated_coverage_size_bytes: u64,
    estimated_read_cost_ns: u64,
    estimated_decode_cost_ns: u64,
    estimated_materialisation_cost_ns: u64,
    baseline_scan_cost_estimate_ns: u64,

    max_allowed_estimated_cost_ns: u64,
    confidence_ppm: u32,
    calibration_epoch: u64,
    observed_error_bounds_ref: u32,
    fallback_policy: u8,
    reserved: [u8; 3],
    checksum: u32,
}

enum CoverageFallbackPolicyV2 {
    AdvisoryOnly = 0,
    FallbackRequired = 1,
    FullScanFallback = 2,
    WiderCoverageFallback = 3,
    RejectIfRequired = 4,
}
```

**Rules:**
- Coverage plan candidates are planning hints, not proof.
- A reader MAY ignore all plan candidates and derive a plan from ordinary COVE-T/COVE-A metadata.
- A reader SHOULD prefer plans whose estimated combined lookup, read, decode, and materialisation cost is lower than the baseline scan cost, but it is not required to use the writer's cost model.
- A reader MUST fall back to a wider conservative plan or full scan when a selected coverage provider is unavailable, stale, corrupt, too expensive under local policy, or unsupported.
- A cost estimate error MUST NOT change query results. It may only affect performance.
- If a plan candidate requires correctness trust in a sidecar, index, or cache, that sidecar/index/cache MUST validate under the selected snapshot before the plan is used.

---

## 30. Exact Set Indexes

Exact sets represent exact values present in a segment or morsel.

```rust
struct ExactSetIndexHeaderV2 {
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
struct BloomIndexHeaderV2 {
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
struct InvertedMorselIndexHeaderV2 {
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
struct InvertedMorselEntryV2 {
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
struct LookupIndexHeaderV2 {
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


### 33.1 COVE-I Secondary Index Artifact

COVE-I is an optional secondary index profile. A `.covi` artifact may contain global, dataset-level, file-level, object-level, path-level, row-range, or dimensional-bucket indexes. COVE-I exists because some indexes are too large, workload-specific, mutable-to-rebuild, or cross-file to belong in every `.cove` file.

**COVE-I final bytes:**
[postscript bytes]
[postscript_version: u16]
[postscript_len: u16]
[magic: "CVI2"]

A `.covi` artifact uses the same tail-discovery discipline as COVE: readers discover the postscript from the final bytes, validate the postscript, locate the header/section directory, and then validate only the index roots and payload sections needed by the requested operation.

```rust
struct CoviPostscriptV2 {
    required_features: u64,
    optional_features: u64,
    file_len: u64,
    header_offset: u64,
    header_length: u64,
    checksum: u32,
}
```

```rust
struct CoviHeaderV2 {
    magic: [u8; 4],          // "CVI2"
    header_len: u16,         // fixed header length for this version
    version_major: u16,
    version_minor: u16,

    flags: u32,
    index_artifact_id: [u8; 16],
    dataset_id: [u8; 16],
    snapshot_id: [u8; 16],

    section_count: u32,
    referenced_file_count: u32,
    snapshot_validity_count: u32,
    index_root_count: u32,
    capability_count: u32,

    section_directory_offset: u64,
    section_directory_length: u64,
    referenced_files_offset: u64,
    snapshot_validity_offset: u64,
    index_roots_offset: u64,
    capabilities_offset: u64,
    string_table_section_ref: u32,

    created_at_us: i64,
    reserved: [u8; 24],
    checksum: u32,
}
```

```rust
enum CoviSectionKindV2 {
    ReferencedFiles = 0,
    SnapshotValidity = 1,
    StringTable = 2,
    IndexRoots = 3,
    IndexCapabilities = 4,
    KeyBlock = 5,
    EntryBlock = 6,
    PostingsBlock = 7,
    RowRangeBlock = 8,
    RowOrdinalSetBlock = 9,
    BitmapBlock = 10,
    AggregateAnswerBlock = 11,
    CoverageSetBlock = 12,
    DimensionalBucketBlock = 13,
    ObjectPathBlock = 14,
    ExtensionBlock = 255,
}

struct CoviSectionEntryV2 {
    section_id: u32,
    section_kind: u16,       // CoviSectionKindV2
    flags: u16,
    offset: u64,
    length: u64,
    uncompressed_length: u64,
    item_count: u64,
    compression: u8,         // CompressionCodec
    encryption: u8,          // 0=None in v2
    alignment_log2: u8,
    reserved0: u8,
    required_features: u64,
    optional_features: u64,
    crc32c: u32,
    checksum: u32,
}
```

```rust
struct CoviReferencedFileV2 {
    file_ref: u32,           // dense zero-based file reference used by postings
    flags: u32,
    file_id: [u8; 16],
    file_len: u64,
    footer_crc32c: u32,
    digest_algorithm: u16,
    digest_len: u16,
    digest_offset: u64,      // digest bytes in StringTable or binary payload block
    uri_ref: u32,            // optional URI string-table ref, u32::MAX when absent
    schema_fingerprint_ref: u32,
    checksum: u32,
}

struct CoviSnapshotValidityV2 {
    snapshot_validity_ref: u32,
    dataset_id: [u8; 16],
    snapshot_id: [u8; 16],
    schema_fingerprint_ref: u32,
    semantic_map_fingerprint_ref: u32,
    external_visibility_ref: u32,
    data_checksum_root_ref: u32,
    valid_from_us: i64,
    valid_until_us: i64,     // i64::MAX when open-ended for immutable snapshot ref
    flags: u32,
    checksum: u32,
}
```

```rust
enum CoviIndexedTargetKindV2 {
    TableColumn = 0,
    ObjectProperty = 1,
    ObjectPath = 2,
    AssociationEndpoint = 3,
    ProjectionColumn = 4,
    SemanticDimension = 5,
    DimensionalTuple = 6,
    ExternalTarget = 255,
}

enum CoviIndexKindV2 {
    Hash = 0,
    Sorted = 1,
    SparseSorted = 2,
    Trie = 3,
    RangeBucket = 4,
    Bitmap = 5,
    MinimalPerfectHash = 6,
    AggregateOnly = 7,
    Extension = 255,
}

struct CoviIndexRootV2 {
    index_root_id: u32,
    indexed_target_kind: u16,      // CoviIndexedTargetKindV2
    index_kind: u16,               // CoviIndexKindV2
    coverage_granularity: u8,      // CoverageGranularityV2
    proof_strength: u8,            // CoverageProofStrengthV2
    exactness: u8,                 // CoverageExactnessV2
    flags: u8,

    table_id: u32,
    column_id: u32,
    object_type_id: u32,
    property_id: u32,
    path_ref: u32,
    semantic_dimension_ref: u32,

    logical_type: u16,
    physical_kind: u8,
    key_encoding_kind: u8,
    comparator_kind: u16,
    collation_id: u16,
    null_semantics: u8,
    sort_order: u8,

    value_count: u64,
    distinct_count: u64,
    null_count: u64,

    min_key_ref: u32,
    max_key_ref: u32,

    key_block_section_id: u32,
    entry_block_section_id: u32,
    postings_block_section_id: u32,
    aggregate_block_section_id: u32,
    coverage_set_ref: u32,
    capability_ref: u32,
    snapshot_validity_ref: u32,
    checksum: u32,
}
```

### 33.1.0 COVE-I Block Containers and Reference Spaces

COVE-I uses local block containers so entries, postings, coverage references, and aggregate answers can be validated independently and resolved without JSON metadata. A `.covi` reader MUST resolve references through the binary block headers and arrays described here.

```rust
enum CoviBlockKindV2 {
    KeyBlock = 0,
    EntryBlock = 1,
    PostingsBlock = 2,
    RowOrdinalSetBlock = 3,
    AggregateAnswerBlock = 4,
    CoverageSetBlock = 5,
    Extension = 255,
}

struct CoviEntryBlockHeaderV2 {
    magic: [u8; 4],              // "CIE2"
    version_major: u16,
    version_minor: u16,
    header_len: u16,
    entry_len: u16,

    entry_block_id: u32,
    index_root_id: u32,
    entry_count: u32,            // maximum u32::MAX entries per block
    key_block_id: u32,
    postings_block_id: u32,      // u32::MAX when no postings block
    aggregate_block_id: u32,     // u32::MAX when no aggregate block

    entries_offset: u64,
    entries_length: u64,
    flags: u32,
    checksum: u32,
}

struct CoviPostingsBlockHeaderV2 {
    magic: [u8; 4],              // "CIP2"
    version_major: u16,
    version_minor: u16,
    header_len: u16,
    postings_header_len: u16,

    postings_block_id: u32,
    index_root_id: u32,
    postings_count: u32,
    row_ordinal_set_count: u32,

    postings_headers_offset: u64,
    row_ordinal_headers_offset: u64, // 0 when absent
    postings_payload_offset: u64,
    postings_payload_length: u64,

    flags: u32,
    checksum: u32,
}

struct CoviAggregateAnswerBlockHeaderV2 {
    magic: [u8; 4],              // "CIA2"
    version_major: u16,
    version_minor: u16,
    header_len: u16,
    aggregate_answer_len: u16,

    aggregate_block_id: u32,
    index_root_id: u32,
    aggregate_answer_count: u32,
    aggregate_answers_offset: u64,
    aggregate_payload_offset: u64,
    aggregate_payload_length: u64,

    flags: u32,
    checksum: u32,
}
```

**COVE-I reference spaces:**
- `key_block_id`, `entry_block_id`, `postings_block_id`, and `aggregate_block_id` are local to one `.covi` artifact and MUST identify blocks referenced by the owning `CoviIndexRootV2`.
- `CoviIndexEntryV2.entry_ref` is the dense local index of the entry within its `CoviEntryBlockHeaderV2` entry array. `entry_ref` MUST equal the array position of the entry.
- `postings_ref` is a dense local index into the `CoviPostingsHeaderV2` array of the postings block named by the owning index root. `u32::MAX` means absent.
- `aggregate_answer_ref` is a dense local index into the `CoviAggregateAnswerV2` array of the aggregate block named by the owning index root. `u32::MAX` means absent.
- `coverage_set_ref` references a `coverage_set_id` in the same `.covi` artifact's COVE-COVERAGE `COVERAGE_SET` section. External coverage sets MUST be copied into the `.covi` artifact or referenced through a registered extension payload with digest-verified validity.
- `next_duplicate_ref` is a dense local `entry_ref` in the same entry block. `u32::MAX` means absent. Duplicate chains MUST terminate and MUST NOT contain cycles.
- Payload offsets inside COVE-I block headers are relative to the start of that block payload, not the start of the `.covi` file.

**Block validation rules:**
- Block magic, version, lengths, counts, offsets, and checksums MUST validate before any local reference is resolved.
- Entry blocks MUST be sorted according to the owning root's comparator unless the root declares hash/minimal-perfect-hash ordering.
- Postings blocks MUST contain exactly `postings_count` posting headers; each `postings_ref` MUST identify exactly one header.
- Aggregate blocks MUST contain exactly `aggregate_answer_count` aggregate answer descriptors; exact answers MUST be snapshot-, overlay-, schema-, redaction-, and mapping-valid before use.
- A malformed local reference invalidates the entry or block. If the index is optional, readers MUST ignore the index and fall back.

### 33.1.1 COVE-I Key, Comparator, and Entry Grammar

Keys in COVE-I are deterministic byte strings or fixed-width scalar encodings. The comparator declared by the root determines equality and ordering. A COVE-I reader MUST NOT compare display bytes, source bytes, raw FileCodes from another file, or engine-local ExecutionCodes as a substitute for the declared key semantics.

```rust
enum CoviKeyEncodingKindV2 {
    FileCode = 0,             // only within the referenced file scope declared by postings
    NumCode = 1,
    CanonicalValueBytes = 2,
    CanonicalHash64 = 3,
    CanonicalHash128 = 4,
    FixedBytes = 5,
    Utf8BytewisePrefix = 6,
    IntervalTuple = 7,
    DimensionalTuple = 8,
    ObjectPathTuple = 9,
    Extension = 255,
}

enum CoviComparatorKindV2 {
    CanonicalEquality = 0,
    CanonicalOrdering = 1,
    DomainRankOrdering = 2,
    NumCodeLogicalOrdering = 3,
    Utf8BytewisePrefix = 4,
    IntervalOverlap = 5,
    DimensionalTupleLexicographic = 6,
    ObjectPathLexicographic = 7,
    ExtensionRequired = 255,
}

struct CoviKeyBlockHeaderV2 {
    magic: [u8; 4],              // "CIK2"
    version_major: u16,
    version_minor: u16,
    header_len: u16,
    reserved0: u16,

    key_block_id: u32,
    index_root_id: u32,
    key_count: u64,
    encoding_kind: u8,
    comparator_kind: u16,
    flags: u8,
    key_data_offset: u64,
    key_data_length: u64,
    checksum: u32,
}

struct CoviIndexEntryV2 {
    entry_ref: u32,             // dense local index in the owning entry block
    index_root_id: u32,
    entry_id: u64,              // stable/debug identifier; not the reference space
    key_kind: u8,
    comparator_kind: u16,
    flags: u8,
    key_offset: u64,          // into root key block
    key_length: u32,
    key_hash64: u64,          // hint only unless hash index declares collision policy
    postings_ref: u32,
    coverage_set_ref: u32,
    aggregate_answer_ref: u32,
    next_duplicate_ref: u32,  // u32::MAX when absent
    checksum: u32,
}
```

**Key-block rules:**
- `CoviKeyBlockHeaderV2.magic` MUST be `"CIK2"`; unsupported major versions make the key block unsupported.
- `header_len` MUST cover the fixed header fields and `reserved0` MUST be zero.
- `key_data_offset` and `key_data_length` are relative to the start of the key-block payload and MUST lie within the block.
- The key block checksum covers the key-block header with the checksum field zeroed plus the key data bytes.

**Key rules:**
- Canonical value keys are `[value_tag: varint][canonical_value_payload]` or a length-delimited sequence of those components for tuple keys.
- `FileCode` keys are valid only for postings that are scoped to exactly one referenced COVE file and dictionary digest. Cross-file equality MUST use canonical value bytes or canonical hashes with collision resolution.
- `NumCode` keys are compared using the declared logical type and NumCode descriptor. Raw numeric bit comparison is allowed only when the descriptor declares it safe.
- `CanonicalHash64` and `CanonicalHash128` are lookup accelerators. A hash match is not equality unless the root declares collision-free construction or the entry stores canonical bytes for verification.
- Sorted indexes MUST sort entries by the declared comparator and then by canonical key bytes as a deterministic tie-breaker.
- Duplicate keys are allowed only when `next_duplicate_ref` chains or postings lists express all duplicate locations. `next_duplicate_ref` is a local entry-block reference and MUST NOT be interpreted as a file offset or global ID. Silent duplicate collapse is invalid unless the root declares aggregate-only semantics.

### 33.1.2 COVE-I Postings, Row Ranges, and Ordinal Sets

A posting maps one key to one or more candidate fragments. Postings may over-include candidates but MUST NOT under-include when the index advertises conservative coverage or exact-answer semantics.

```rust
enum CoviPostingRepresentationV2 {
    SortedFileRefs = 0,
    SortedSegmentRefs = 1,
    SortedPageRefs = 2,
    SortedMorselRefs = 3,
    RowRangeList = 4,
    RowOrdinalBitmap = 5,
    RowOrdinalDeltaVarint = 6,
    ByteRangeList = 7,
    ObjectPathRefs = 8,
    DimensionalBucketRefs = 9,
    CoverageSetRef = 10,
    Extension = 255,
}

struct CoviPostingsHeaderV2 {
    postings_ref: u32,
    index_root_id: u32,
    representation: u8,          // CoviPostingRepresentationV2
    target_granularity: u8,      // CoverageGranularityV2
    flags: u16,
    item_count: u64,
    payload_offset: u64,
    payload_length: u64,
    coverage_set_ref: u32,
    checksum: u32,
}

struct CoviFragmentRefV2 {
    file_ref: u32,
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,
    page_ref: u32,
    object_type_id: u32,
    path_ref: u32,
    dimensional_bucket_ref: u32,
    flags: u32,
    checksum: u32,
}

struct CoviRowRangePostingV2 {
    file_ref: u32,
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,          // u32::MAX when segment/global row range
    row_start: u64,
    row_count: u64,
    flags: u32,
    checksum: u32,
}

struct CoviFileRefPostingV2 {
    file_ref: u32,
    flags: u32,
    checksum: u32,
}

struct CoviSegmentRefPostingV2 {
    file_ref: u32,
    table_id: u32,
    segment_id: u32,
    flags: u32,
    checksum: u32,
}

struct CoviMorselRefPostingV2 {
    file_ref: u32,
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,
    flags: u32,
    checksum: u32,
}

struct CoviPageRefPostingV2 {
    file_ref: u32,
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,
    page_ref: u32,
    flags: u32,
    checksum: u32,
}

struct CoviByteRangePostingV2 {
    file_ref: u32,
    section_id: u32,
    offset: u64,
    length: u64,
    flags: u32,
    checksum: u32,
}

struct CoviObjectPathPostingV2 {
    file_ref: u32,
    object_type_id: u32,
    path_ref: u32,
    segment_id: u32,
    row_start: u64,
    row_count: u64,
    flags: u32,
    checksum: u32,
}

struct CoviDimensionalBucketPostingV2 {
    file_ref: u32,
    table_id: u32,
    segment_id: u32,
    morsel_id: u32,
    dimensional_bucket_ref: u32,
    flags: u32,
    checksum: u32,
}
```

```rust
enum CoviBitmapKindV2 {
    DenseBitsetLsb0 = 0,
    SortedU32 = 1,
    SortedU64 = 2,
    DeltaVarintU32 = 3,
    RangeList = 4,
    RegisteredRoaring32 = 5,
    RegisteredRoaring64 = 6,
    Extension = 255,
}

struct CoviRowOrdinalSetHeaderV2 {
    row_ordinal_set_ref: u32,
    file_ref: u32,
    table_id: u32,
    segment_id: u32,          // u32::MAX when file/table scoped
    morsel_id: u32,           // u32::MAX when not morsel scoped
    bitmap_kind: u8,          // CoviBitmapKindV2
    flags: u8,
    reserved: u16,
    universe_row_count: u64,
    set_row_count: u64,
    payload_offset: u64,
    payload_length: u64,
    checksum: u32,
}
```

**Posting payload layouts:**

`CoviPostingsHeaderV2.payload_offset` and `CoviPostingsHeaderV2.payload_length` are relative to the `postings_payload_offset` base of the owning `CoviPostingsBlockHeaderV2`. `CoviRowOrdinalSetHeaderV2.payload_offset` and `payload_length` use the same base. The following representation payloads are normative for v2:

| Representation | Payload at `payload_offset` | Length and count rules |
| --- | --- | --- |
| `SortedFileRefs` | `CoviFileRefPostingV2[item_count]` | `payload_length == item_count * encoded_len(CoviFileRefPostingV2)`. Entries sorted by `file_ref`. |
| `SortedSegmentRefs` | `CoviSegmentRefPostingV2[item_count]` | Entries sorted by `(file_ref, table_id, segment_id)`. |
| `SortedMorselRefs` | `CoviMorselRefPostingV2[item_count]` | Entries sorted by `(file_ref, table_id, segment_id, morsel_id)`. |
| `SortedPageRefs` | `CoviPageRefPostingV2[item_count]` | Entries sorted by `(file_ref, table_id, segment_id, morsel_id, page_ref)`. |
| `RowRangeList` | `CoviRowRangePostingV2[item_count]` | Ranges sorted by `(file_ref, table_id, segment_id, morsel_id, row_start)`, non-overlapping, and coalesced where adjacent. |
| `RowOrdinalBitmap` | `u32 row_ordinal_set_ref[item_count]` | Each ref MUST identify a `CoviRowOrdinalSetHeaderV2` in the owning postings block with bitmap-compatible `bitmap_kind`. |
| `RowOrdinalDeltaVarint` | `u32 row_ordinal_set_ref[item_count]` | Each ref MUST identify a `CoviRowOrdinalSetHeaderV2` whose `bitmap_kind` is `DeltaVarintU32` or a compatible required extension. |
| `ByteRangeList` | `CoviByteRangePostingV2[item_count]` | Byte ranges sorted by `(file_ref, section_id, offset)`, non-overlapping, and within the validated referenced file/section. |
| `ObjectPathRefs` | `CoviObjectPathPostingV2[item_count]` | Entries sorted by `(file_ref, object_type_id, path_ref, segment_id, row_start)`. |
| `DimensionalBucketRefs` | `CoviDimensionalBucketPostingV2[item_count]` | Entries sorted by `(dimensional_bucket_ref, file_ref, table_id, segment_id, morsel_id)`. |
| `CoverageSetRef` | no payload bytes | `payload_length == 0`, `item_count == 1`, and `coverage_set_ref` MUST identify a validated coverage set. |
| `Extension` | extension-defined | Required extension defines payload layout, sorting, duplicate, false-negative, and validation rules. |

For every fixed-structure array listed in the table, `payload_length` MUST equal `item_count * encoded_len(payload_struct)` unless the representation explicitly states otherwise. `encoded_len(T)` means the fixed wire length of the named COVE-I posting structure as emitted field-by-field in little-endian order, including its checksum field. Readers MUST NOT infer payload layout from native struct size or padding.

**Posting rules:**
- Posting items MUST be sorted in deterministic target order and duplicates MUST be removed unless a required extension defines multiset postings.
- Row ranges MUST be sorted, non-overlapping, and coalesced when adjacent.
- `CoviPostingsHeaderV2.payload_length` MUST match the representation's fixed layout or registered extension layout exactly. Trailing bytes are invalid.
- `DenseBitsetLsb0` uses the same bit order as COVE null bitmaps: row ordinal `i` uses bit `(i & 7)` of byte `(i >> 3)`. Unused high bits in the final byte MUST be zero.
- `SortedU32`, `SortedU64`, and `DeltaVarintU32` payloads are exact lists of row ordinals in ascending order.
- `RegisteredRoaring32` and `RegisteredRoaring64` are reserved names until a companion COVE-I bitmap specification defines exact bytes and vectors. They MUST NOT be required for broad COVE-I conformance before that companion spec exists.
- A posting with `CoverageSetRef` MUST reference a validated COVE-COVERAGE set that obeys the same snapshot and overlay validity rules as the index root.

### 33.1.3 COVE-I Aggregate and Index-Only Payloads

```rust
struct CoviAggregateAnswerV2 {
    aggregate_answer_ref: u32,
    index_root_id: u32,
    aggregate_kind: u16,       // count, min, max, sum, avg, distinct_count, exists, membership
    exactness: u8,
    null_semantics: u8,
    flags: u16,
    row_count: u64,
    null_count: u64,
    non_null_count: u64,
    value_ref: u32,            // canonical scalar/list payload or extension payload
    predicate_form_ref: u32,   // u32::MAX when unfiltered
    snapshot_validity_ref: u32,
    checksum: u32,
}
```

**Aggregate/index-only rules:**
- Exact aggregate answers MUST be computed over the selected snapshot, schema, external visibility overlay, redaction policy, and COVE-MAP projection semantics when applicable.
- Approximate aggregate answers MUST carry approximate exactness and MUST NOT answer exact queries without explicit approximate query semantics.
- `sum` and `avg` payloads MUST declare decimal scale, overflow policy, NaN handling, and redaction policy through `value_ref` or a required extension payload.
- A COVE-I index-only answer MUST be rejected when the required visibility overlay, source projection version, or semantic-map fingerprint does not match the selected dataset state.

**Supported index mappings include:**

| Mapping | Meaning |
| --- | --- |
| `value -> file_id` | Candidate files for equality, membership, or range predicates. |
| `value -> segment_id` | Candidate table segments or temporal segments. |
| `value -> page_id` | Candidate pages. |
| `value -> morsel_id` | Candidate morsels. |
| `value -> row_range` | Candidate physical row ranges. |
| `value -> row_ordinal_set` | Candidate row ordinal bitmap or compressed set. |
| `path -> object_path` | Candidate object/path fragments for COVE-O/COVE-MAP. |
| `dimension_tuple -> dimensional_bucket` | Candidate dimensional buckets for spatial, genomic, temporal, or object-dimensional layouts. |
| `association_endpoint -> association fragment` | Candidate association/link object records. |

**Rules:**
- COVE-I is optional. A conforming COVE-Core/COVE-T reader MUST NOT require `.covi` artifacts for ordinary logical decode.
- A COVE-I artifact MUST declare dataset, snapshot, file, schema, semantic-map, and digest validity sufficient for the requested operation.
- A stale, corrupt, unsupported, or mismatched COVE-I artifact MUST be ignored or cause rejection only when the requested operation explicitly requires that index.
- A COVE-I artifact MUST NOT change COVE logical values, COVE-O reconstruction, COVE-MAP identity, external visibility overlays, or table/catalog semantics.
- COVE-I index roots may advertise conservative coverage, exact answer, approximate answer, or advisory capabilities. Readers MUST interpret each capability under its declared proof strength and exactness.
- COVE-I global indexes SHOULD be referenced from COVM or an external catalog by digest and snapshot ID.

### 33.2 Secondary Index Capabilities and Index-Only Access

A COVE-I or COVX index may declare operations it can answer or accelerate. Capability declarations are not enough for correctness; the index must also validate against the selected snapshot and proof semantics.

```rust
struct IndexCapabilityV2 {
    capability_id: u32,
    index_root_id: u32,
    flags: u32,

    supports_eq: u8,
    supports_range: u8,
    supports_membership: u8,
    supports_prefix: u8,
    supports_contains: u8,
    supports_count: u8,
    supports_min: u8,
    supports_max: u8,
    supports_sum: u8,
    supports_distinct_count: u8,
    supports_join_coverage: u8,
    supports_index_only: u8,

    exactness: u8,             // exact, approximate, advisory
    proof_strength: u8,
    null_semantics: u8,
    reserved: u8,

    snapshot_validity_ref: u32,
    coverage_provider_ref: u32,
    checksum: u32,
}

struct IndexOnlyCapabilityV2 {
    capability_id: u32,
    aggregate_kind: u16,       // count, min, max, sum, avg, distinct_count, exists, membership
    predicate_supported: u8,
    exactness: u8,
    null_semantics: u8,
    flags: u8,
    snapshot_validity_ref: u32,
    required_visibility_overlay_ref: u32,
    checksum: u32,
}
```

**Rules:**
- Exact index-only capabilities MAY be used for exact query answers only when the index, snapshot validity, null semantics, predicate form, collation, and external visibility overlay rules all validate.
- Approximate index-only capabilities MUST be surfaced as approximate and MUST NOT answer exact SQL queries unless the query explicitly requests approximate semantics.
- Index-only counts, min/max, distinct counts, and existence checks MUST account for nulls, redactions, external overlays, and COVE-MAP projection semantics according to declared policy.
- If a non-empty external delete or visibility overlay is active, physical-file index-only aggregate answers are invalid unless an overlay-aware correction or proof is declared and validated.
- Readers MAY use index-only capabilities to avoid opening `.cove` files only when the manifest or index artifact provides sufficient digest and snapshot validation.

---

## 34. Aggregate Synopsis Indexes

Aggregate synopses allow metadata-answerable queries and faster aggregation.

```rust
struct AggregateSynopsisEntryV2 {
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
struct CompositeZoneIndexHeaderV2 {
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
struct TopNZoneSummaryV2 {
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


### 37.6 Encoded Predicate Evaluation and Late Materialisation

COVE readers SHOULD avoid logical materialisation until it is necessary for projection, export, or an unsupported predicate. This is a performance rule only; it MUST NOT change logical results.

**Recommended scan shape:**
1. use COVM, COVE-COVERAGE, COVE-I, COVX, zone stats, exact sets, blooms, and ColumnDomains to derive a conservative candidate fragment set;
2. evaluate safe predicates against encoded FileCode, NumCode, local-codebook, RLE, bit-packed, or registered codec streams when an equivalent encoded predicate kernel is declared;
3. produce a selection bitmap or row-id vector;
4. decode only selected rows for projected columns;
5. preserve dictionary/code vectors where the output engine can use them safely;
6. materialise Arrow-owned arrays, Arrow-view arrays, COVE-native views, or engine-native vectors according to export capability.

```rust
enum ExportCapabilityKindV2 {
    ArrowOwnedArray = 0,
    ArrowViewArray = 1,
    ArrowDictionaryArray = 2,
    CoveNativeView = 3,
    SelectionBitmap = 4,
    RowIdVector = 5,
    EngineNativeVector = 6,
}

struct ExportCapabilityV2 {
    capability_id: u32,
    target_kind: u16,
    table_id: u32,
    column_id: u32,
    logical_type: u16,
    physical_kind: u8,
    flags: u8,
    requires_owned_buffers: u8,
    supports_zero_copy: u8,
    supports_late_materialisation: u8,
    supports_dictionary_preservation: u8,
    null_bitmap_polarity: u8,
    dictionary_key_width_bits: u16,
    lifetime_policy: u8,
    reserved: [u8; 3],
    checksum: u32,
}
```

**Rules:**
- Encoded predicate evaluation is allowed only when the physical encoding, NumCode descriptor, codec descriptor, null semantics, collation, NaN/signed-zero rules, and predicate form declare equivalence to baseline logical evaluation.
- A reader MUST fall back to logical decode for unsupported or unsafe encoded predicates.
- Selection bitmaps and row-id vectors are intermediate execution artifacts. They MUST NOT be persisted as canonical COVE truth unless a future required extension defines such a section.
- Late materialisation MUST preserve row order, null positions, redaction policy, dictionary semantics, and projection semantics.
- A reader MAY expose zero-copy views only when the target format's alignment, lifetime, null polarity, key width, offset width, dictionary semantics, and ownership rules are satisfied.

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
struct EngineProfileRegistryHeaderV2 {
    profile_count: u32,
    flags: u32,
}
```

```rust
struct EngineProfileEntryV2 {
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
struct ExecutionCodeDescriptorV2 {
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
struct ExecutionScopeDescriptorV2 {
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
struct CodeSpaceDescriptorV2 {
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
struct EngineMountPolicyV2 {
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
struct HarborMountHintsV2 {
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
struct ExtensionRegistryHeaderV2 {
    extension_count: u32;
    flags: u32;
}
```

```rust
struct ExtensionEntryV2 {
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


### 45.1 V2 Registry Discipline

COVE v2 strengthens the extension registry so that extension identifiers are portable and conformance-testable rather than purely runtime-local.

**Rules:**
- Extension namespace, name, version, and kind MUST identify a stable public contract, not merely an implementation class name.
- Required extensions MUST declare exact fallback and failure behaviour.
- Extension payloads MUST be length-delimited, checksummed, and bound to a feature bit or requested operation.
- Runtime registry/session identifiers MAY be used to instantiate implementation code, but they MUST NOT be the only authority for on-disk semantics.
- A vendor extension MAY be optional and ignorable; if it is required to decode projected data or preserve semantics, it MUST be a required extension and MUST provide conformance vectors.

### 45.2 Runtime Registry Name Binding

```rust
struct RuntimeRegistryBindingV2 {
    extension_id: u32,

    registry_kind: u16,
    // 0=codec
    // 1=layout
    // 2=index
    // 3=synopsis
    // 4=predicate_kernel
    // 5=engine_profile
    // 6=mapping_function
    // 7=ffi_adapter

    runtime_namespace_len: u16,
    runtime_namespace: [u8],

    runtime_name_len: u16,
    runtime_name: [u8],

    runtime_version_major: u16,
    runtime_version_minor: u16,

    required: u8,
    flags: u8,
    reserved: u16,

    checksum: u32,
}
```

`RuntimeRegistryBindingV2` is optional COVE-R metadata. It helps implementations map portable extension definitions to local registries, but it is not a substitute for the extension definition itself.

**Rules:**
- Unknown optional runtime bindings MUST be ignored.
- Unknown required runtime bindings cause rejection only for operations that explicitly request that runtime integration.
- Runtime bindings MUST NOT change canonical decode, predicate-proof semantics, COVE-MAP identity, or COVE-O reconstruction.

---

## 46. Custom Logical Types

```rust
struct ExtensionLogicalTypeV2 {
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
struct ExtensionIndexDescriptorV2 {
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


### 49.2 Arrow Export Profiles

COVE is Arrow-friendly but not Arrow-dependent. Arrow export profiles describe how a reader may expose COVE values to Arrow runtimes without making Arrow IPC or Arrow memory layout the canonical COVE representation.

| Profile | Meaning |
| --- | --- |
| COVE-Arrow-Owned | Reader materialises Arrow-compatible owned buffers. This is the safest universal export path. |
| COVE-Arrow-View | Reader exposes COVE buffers as Arrow-compatible views when lifetime, alignment, offset, null, and ownership rules are satisfied. |
| COVE-Arrow-Dictionary | Reader exposes FileCode or remapped dictionary data as Arrow dictionary arrays where key width and dictionary values are compatible. |

**Rules:**
- Arrow export MUST preserve COVE logical values, null positions, redaction policy, nested structure, and extension-type reporting.
- A reader MUST materialise owned Arrow buffers when zero-copy view requirements are not met.
- Arrow view export MUST NOT expose COVE null bitmaps as Arrow validity bitmaps unless the polarity is compatible or the target explicitly accepts COVE polarity.
- FileCode values MAY be reused as Arrow dictionary keys only when the key width, dictionary ordering, null representation, and lifetime are compatible.
- Arrow export profiles are interoperability surfaces. They MUST NOT become COVE schema authority.

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
- Visibility/delete overlays are external in v2.
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

COVE v2 intentionally does not define a COVE Table Layer with ACID commits, catalog state, snapshot isolation, schema evolution, partition evolution, or transaction logs. Those responsibilities belong to an external table format or catalog.

Official integration specifications MAY define how COVE-T files are used as data files inside Iceberg, Delta, Hudi, Hive-style catalogs, Unity-style catalogs, or engine-specific catalogs. Such adapter specifications MUST:
- keep .cove files immutable,
- identify data files by URI plus stable file_id, file_len, footer_crc32c, and digest where available,
- map external table schema fields to COVE table_id/column_id without changing the COVE file schema,
- apply the external catalog's snapshot, partition, delete, visibility, time-travel, and schema-evolution rules before returning rows,
- treat LAKEHOUSE_HINTS, COVM entries, and metadata JSON as hints unless the external catalog explicitly accepts them,
- reject or ignore any COVE hint that conflicts with the selected external snapshot.

A future COVE-native table protocol, if one is ever standardised, MUST be a separate companion specification with its own conformance level, commit protocol, and feature gates. It MUST NOT weaken the immutability or standalone readability of COVE data files.

### 50.3 External Delete and Visibility Overlay Semantics

External row-level deletes, deletion vectors, equality deletes, access filters, and visibility overlays are outside COVE-Core and COVE-T v2. They MAY be referenced by lakehouse hints or manifests, but their semantics are defined by the external table format, catalog, or application protocol.

COVE predicate metadata and indexes describe the physical rows present in the immutable COVE file before external visibility filtering. When an external overlay is active:
- PredicateZoneOutcome::DefinitelyNo remains safe for pruning because no physical row in the zone satisfies the predicate.
- PredicateZoneOutcome::DefinitelyYes remains safe only as a claim that every remaining visible row from that physical zone satisfies the predicate; it does not prove that any visible row remains.
- Unknown remains Unknown.
- Exact sets, blooms, ColumnDomain ranges, and zone stats MAY be used to reject impossible predicates over the physical file, but they MUST NOT be interpreted as exact visible-table domains unless the overlay is proven empty or overlay-aware metadata is available.
- Lookup indexes and inverted morsel indexes return physical row candidates. Readers MUST apply the external visibility/delete overlay before returning rows.
- Aggregate synopses over a COVE file are exact only for the physical COVE rows. They MUST NOT answer visible-table aggregate queries when a non-empty external overlay is active unless an overlay-aware correction or proof is applied.

External overlays that reference physical positions SHOULD identify the target COVE file by file_id plus file length, footer CRC, and cryptographic digest where available. Rewritten or compacted COVE files receive new physical row references; overlays for old files MUST NOT be silently applied to rewritten files.

In v2 `LAKEHOUSE_HINTS` may reference an external visibility overlay by setting
hint flag bit 2. The overlay reference is encoded after `conversion_digest` as
`overlay_kind: u8`, `fingerprint_flags: u8`, optional fingerprint fields in
flag order (`file_id`, `file_len`, `footer_crc32c`, `digest`), then
`reference_len: u16` and UTF-8 `reference` bytes. This reference is descriptive;
the external table format or catalog remains authoritative for overlay
semantics.

### 50.4 Append, Streaming, CDC, and Compaction Boundary

**The accepted mutable-data pattern for COVE v2 is immutable-file publication:**
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

COVE-Core v2 does not define Vector, Tensor, or Embedding as additional scalar logical types. Dense vectors SHOULD be represented by existing nested or extension mechanisms rather than by adding ad hoc core scalar types.

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
struct SortKeyEntryV2 {
    column_id: u32,
    direction: u8,       // 0=asc, 1=desc
    null_order: u8,      // 0=nulls first, 1=nulls last
    collation_id: u16,
}
```

```rust
struct ClusteringKeyEntryV2 {
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
struct CoveTableRowRefV2 {
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
- CoveTableRowRefV2 identifies a physical row position inside one immutable COVE file.
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
struct ObjectTypeCatalogV2 {
    type_count: u32,
    flags: u32,

    types: [ObjectTypeEntryV2],
}
```

```rust
struct ObjectTypeEntryV2 {
    object_type_id: u32,

    type_name_len: u16,
    type_name: [u8],

    flags: u32,

    property_count: u16,

    properties: [PropertyEntryV2],
}
```

```rust
struct PropertyEntryV2 {
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
- Readers SHOULD use ObjectTypeEntryV2.flags and PropertyEntryV2.flags, not property names alone, as the authoritative cues for association, evidence, and projection readback. Property names such as `from_goid`, `to_goid`, `association_type`, `source_evidence_id`, and `mapping_rule_id` remain recommended conventions only.
- An object type flagged OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT SHOULD expose exactly one PROPERTY_FLAG_ASSOCIATION_FROM_GOID property and exactly one PROPERTY_FLAG_ASSOCIATION_TO_GOID property unless a required extension defines a multi-endpoint association form.
- OBJECT_TYPE_FLAG_LINK_OBJECT and OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT MAY be set together when a type is both a first-class object and an association carrier. Other combinations that materially change readback semantics SHOULD be documented by the profile or extension that emits them.

---

## 57. COVE-O Temporal Segment Index

```rust
struct TemporalSegmentIndexEntryV2 {
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
struct TemporalSegmentHeaderV2 {
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
struct CoveRecordRefV2 {
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

COVE-O v2 files MUST be reconstruction self-contained.
**For every represented object chain, the file MUST contain either:**
- the full chain back to the first record, or
- a Baseline/Snapshot sufficient to reconstruct state before dependent Delta records.
If a chain continues from outside the file, the writer MUST emit a Baseline or Snapshot anchor inside the file.
Mandatory cross-file prev_ref is not supported in v2.

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

COVE-O v2 does not require a dedicated native edge section. When COVE-MAP produces association assertions and the destination is object-based COVE, a writer MUST materialise those associations using declared COVE-O object types unless a future association-capable COVE-O extension is explicitly required.

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

The property names above are recommended conventions, not the only interoperable spelling. When association readback is claimed, ObjectTypeEntryV2.flags and PropertyEntryV2.flags are authoritative for identifying association objects, endpoint properties, validity fields, evidence references, and mapping-rule references.

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
struct RedactionManifestEntryV2 {
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

COVE v2 provides corruption detection, optional cryptographic digests, redaction markers, and trust metadata. It does not define a complete access-control, key-management, or encrypted-storage protocol.

**Rules:**
- The encryption fields in v2 section specs and postscript specs MUST be 0. Encrypted sections, encrypted columns, authenticated encryption modes, key identifiers, key rotation, and associated-data rules require a future required extension or profile.
- Redaction is a logical/audit marker, not access control. If sensitive bytes are present unencrypted in a COVE file, COVE redaction metadata alone does not prevent disclosure.
- Column-level or row-level access control is external to COVE v2. Engines enforcing access policy MUST apply that policy before exposing decoded values, indexes, synopses, dictionaries, or metadata that could reveal protected data.
- Indexes, dictionaries, exact sets, blooms, histograms, Top-N summaries, and aggregate synopses may reveal value distributions. Writers handling sensitive datasets SHOULD omit or coarsen acceleration metadata according to policy.
- Differentially private, sampled, masked, or otherwise privacy-preserving statistics MUST be marked as approximate or policy-protected. They MUST NOT be used as exact aggregate synopses, exact value sets, or predicate-proof metadata unless the proof remains valid under the declared privacy transformation.

---

## 65. Digest Manifest

The digest manifest provides cryptographic integrity.

```rust
struct DigestManifestHeaderV2 {
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
struct DigestEntryV2 {
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
struct CoveIoHintV2 {
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


### 67.2 Fast Metadata Index

COVE v2 MAY include a `FAST_METADATA_INDEX` section to make very wide schemas, large page indexes, and object-store planning cheaper to access. This index is an acceleration mirror over authoritative sections.

```rust
struct FastMetadataIndexHeaderV2 {
    entry_count: u32,
    entry_len: u16,
    index_kind: u8,
    flags: u8,
    entries_offset: u64,
    entries_length: u64,
    checksum: u32,
}

struct FastMetadataIndexEntryV2 {
    target_kind: u16,
    // 0=table
    // 1=column
    // 2=segment
    // 3=morsel
    // 4=page
    // 5=stats
    // 6=section
    // 7=layout_node

    flags: u16,

    table_id: u32,
    column_id: u32,
    segment_id: u32,
    morsel_id: u32,

    section_id: u32,
    local_id: u32,

    offset: u64,
    length: u64,

    checksum_or_crc32c: u32,
    reserved: u32,
}
```

**Rules:**
- Fast metadata entries MUST reference existing authoritative metadata.
- A mismatch between a fast metadata entry and the authoritative section invalidates the fast metadata entry.
- If the section is optional, corrupt fast metadata MUST be ignored and readers MUST fall back to the footer and profile sections.
- A writer MUST NOT rely on `FAST_METADATA_INDEX` as the only location for schema, page, or statistics metadata.

### 67.3 Page Cluster Directory

A page cluster groups nearby page payloads for efficient range reads and coalescing. It is a physical I/O hint, not a logical page boundary.

```rust
struct PageClusterDirectoryHeaderV2 {
    cluster_count: u32,
    flags: u32,
    checksum: u32,
}

struct PageClusterEntryV2 {
    cluster_id: u32,
    section_id: u32,

    offset: u64,
    length: u64,

    table_id: u32,
    segment_id: u32,
    first_morsel_id: u32,
    morsel_count: u32,

    first_page_ref: u32,
    page_count: u32,

    preferred_read_alignment: u32,
    preferred_coalesce_distance: u32,

    flags: u32,
    checksum: u32,
}
```

**Rules:**
- Page clusters MAY contain pages from multiple columns and morsels only when every page remains independently addressable by its `ColumnPageIndexEntryV2`.
- Page cluster checksums MAY provide an enclosing integrity check, but each page checksum remains authoritative for page bytes.
- Page clusters MUST NOT change row order, page row_count, null counts, or page reconstruction rules.

### 67.4 Zero-Copy Buffer Map

A zero-copy buffer map describes when COVE page buffers can be exposed directly to an output format or engine runtime. It is optional compatibility metadata.

```rust
struct ZeroCopyBufferMapHeaderV2 {
    map_count: u32,
    target_count: u32,
    flags: u32,
    checksum: u32,
}

struct ZeroCopyTargetV2 {
    target_id: u32,

    namespace_len: u16,
    namespace: [u8],

    target_name_len: u16,
    target_name: [u8],

    version_major: u16,
    version_minor: u16,

    flags: u32,
}

enum ZeroCopyNullBitmapPolarityV2 {
    OneMeansNull = 0,
    OneMeansValid = 1,
    NoNullBitmap = 2,
    TargetDefines = 255,
}

enum ZeroCopyLifetimeScopeV2 {
    Page = 0,
    Segment = 1,
    FileMapping = 2,
    ReaderSession = 3,
    ExternalOwner = 4,
    InvalidAfterDecode = 5,
}

enum ZeroCopyDictionarySemanticsV2 {
    NoDictionary = 0,
    FileCodeDictionary = 1,
    ArrowDictionaryValues = 2,
    EngineDictionary = 3,
    RequiresRemap = 4,
    Incompatible = 255,
}

enum ZeroCopyNestedLayoutKindV2 {
    NotNested = 0,
    ArrowListOffsets32 = 1,
    ArrowLargeListOffsets64 = 2,
    ArrowStructChildren = 3,
    ArrowMapOffsets32 = 4,
    CoveNativeNested = 5,
    Extension = 255,
}

enum ZeroCopyTargetBufferRoleV2 {
    Values = 0,
    ValidityBitmap = 1,
    NullBitmap = 2,
    Offsets32 = 3,
    Offsets64 = 4,
    TypeIds = 5,
    DictionaryKeys = 6,
    DictionaryValues = 7,
    ChildData = 8,
    SelectionBitmap = 9,
    RunEnds = 10,
    Extension = 255,
}

enum ZeroCopySourceBufferRoleV2 {
    CoveValues = 0,
    CoveNullBitmap = 1,
    CoveOffsets = 2,
    CoveChildLayout = 3,
    CoveDictionaryCodes = 4,
    CoveDictionaryPayload = 5,
    CoveEncodedPayload = 6,
    CoveSelectionBitmap = 7,
    CoveRunEnds = 8,
    Extension = 255,
}

struct ZeroCopyBufferMapEntryV2 {
    target_id: u32,
    table_id: u32,
    column_id: u32,
    segment_id: u32,
    morsel_id: u32,

    page_ref: u32,
    buffer_id: u16,
    buffer_kind: u16,

    logical_type: u16,
    physical_kind: u8,
    source_endianness: u8,

    required_alignment_log2: u8,
    null_bitmap_polarity: u8,      // ZeroCopyNullBitmapPolarityV2
    source_offset_width_bits: u16,
    target_offset_width_bits: u16,
    dictionary_key_width_bits: u16,

    dictionary_semantics: u8,      // ZeroCopyDictionarySemanticsV2
    lifetime_scope: u8,            // ZeroCopyLifetimeScopeV2
    nested_layout_kind: u8,        // ZeroCopyNestedLayoutKindV2
    compression_required_none: u8,

    target_buffer_role: u16,       // ZeroCopyTargetBufferRoleV2
    source_buffer_role: u16,       // ZeroCopySourceBufferRoleV2
    target_type_ref: u32,
    dictionary_values_ref: u32,
    child_layout_ref: u32,
    owner_lifetime_ref: u32,

    flags: u32,
    checksum: u32,
}
```

**Rules:**
- Zero-copy maps MUST be ignored when the target format, alignment, null bitmap polarity, key width, offset width, endianness, lifetime, dictionary semantics, nested layout, target/source buffer role, compression state, redaction state, or external visibility policy is incompatible.
- `target_buffer_role` and `source_buffer_role` MUST use `ZeroCopyTargetBufferRoleV2` and `ZeroCopySourceBufferRoleV2`. Unknown role values make the map entry unsupported unless a required extension defines the role and the reader supports that extension.
- COVE null bitmap polarity remains `1 = null`. A target requiring `1 = valid` needs inversion unless the target explicitly accepts COVE polarity. A map entry with `OneMeansValid` cannot directly expose a COVE null bitmap; it can only describe a target-native buffer already materialised or supplied by an extension.
- Direct exposure is permitted only after the page, buffer descriptor, section CRC, and any required digest have validated.
- Compressed, encrypted, encoded, transformed, or value-stream-elided buffers MUST NOT be exposed as target logical buffers unless the target explicitly expects that encoded representation and the export profile declares it as a native encoded view.
- Dictionary buffers may be exposed directly only when dictionary values, key width, null policy, ordering expectations, and dictionary lifetime match the target. FileCode values MUST NOT be exposed as Arrow dictionary keys when the key width or dictionary value order is incompatible.
- Nested offsets may be exposed only when offset width, offset origin, monotonicity, final offset, child length, parent null semantics, and child layout match the target.
- If an external delete/visibility overlay or selection bitmap is active, a zero-copy value buffer may be exposed only together with a target-compatible selection/filter representation; otherwise the reader MUST materialise the visible rows.
- `lifetime_scope` MUST be at least as long as the target consumer's access. Readers MUST materialise owned buffers when memory mapping, ref-counting, or external owner lifetime cannot be guaranteed.
- A reader MUST materialise compatible buffers rather than exposing incompatible COVE bytes.
- Zero-copy compatibility MUST NOT influence writer choices if doing so would weaken COVE encoding, predicate-proof metadata, digest coverage, or logical type fidelity.

### 67.4.1 Arrow Zero-Copy Compatibility Checklist

For Arrow export, a reader MAY expose a COVE buffer as an Arrow buffer without copying only when all of the following hold:

1. the COVE buffer contains the exact Arrow physical buffer role being requested;
2. endianness is little-endian and matches Arrow's physical layout;
3. the buffer is uncompressed and not wrapped by a block codec;
4. the buffer is not an encoded stream unless the Arrow output is an explicitly encoded extension view;
5. null bitmap polarity is compatible or no null bitmap is required;
6. offset buffers are 32-bit or 64-bit as required by the Arrow type;
7. dictionary keys and dictionary values match Arrow dictionary semantics;
8. nested child buffers and parent offsets satisfy Arrow layout invariants;
9. the selected row set is contiguous or represented by a target-compatible selection vector;
10. memory lifetime extends beyond the Arrow array consumer's use;
11. redaction, privacy, and external visibility policies permit exposing the bytes;
12. COVE checksums and required digests validate before exposure.

Failure of any checklist item requires materialised Arrow-owned output.

### 67.5 COVE-L Layout Plan Profile

COVE-L is the v2 layout-plan and scan-split profile. It borrows the useful idea of hierarchical lazy read planning but keeps COVE's explicit catalog, segment, morsel, page, and proof metadata authoritative.

```rust
struct LayoutPlanHeaderV2 {
    layout_id: u32,
    node_count: u32,
    root_node_id: u32,
    flags: u32,
    checksum: u32,
}

struct LayoutPlanNodeV2 {
    node_id: u32,
    parent_node_id: u32,       // u32::MAX for root

    node_kind: u16,
    // 0=root
    // 1=table
    // 2=segment_group
    // 3=segment
    // 4=morsel_range
    // 5=column_group
    // 6=page_cluster
    // 7=section_range
    // 255=vendor_hint

    flags: u16,

    table_id: u32,
    column_id: u32,            // u32::MAX when not column-specific
    segment_id: u32,           // u32::MAX when not segment-specific
    first_morsel_id: u32,
    morsel_count: u32,

    row_start: u64,
    row_count: u64,

    section_id: u32,
    cluster_id: u32,

    first_child_index: u32,
    child_count: u32,

    stats_ref: u32,
    split_ref: u32,

    checksum: u32,
}
```

**Rules:**
- COVE-L is optional. A COVE-T reader MUST be able to scan a valid COVE-T file without COVE-L.
- Layout nodes MUST reference existing COVE-T/COVE-O sections, segments, morsels, pages, statistics, or page clusters.
- A layout node MUST NOT introduce a new table, column, row order, predicate proof, or logical value.
- If a COVE-L layout node disagrees with authoritative COVE metadata, the node is invalid and MUST be ignored or rejected according to whether the layout profile is optional or required for the requested operation.
- Predicate pruning through COVE-L is valid only when the node references validated COVE proof metadata. The layout node itself is not proof.
- A writer SHOULD use COVE-L to describe large-scale read planning, not to smuggle arbitrary runtime-specific layouts into the file.


### 67.5.1 Layout Unit Taxonomy

COVE-L distinguishes logical, physical, predicate, decode, compression, I/O, and scheduling units. A writer SHOULD NOT force these units to be identical unless doing so is beneficial for the workload and does not weaken validation.

```rust
enum LayoutUnitKindV2 {
    LogicalPage = 0,
    PhysicalPage = 1,
    PredicateStatsUnit = 2,
    DecodeUnit = 3,
    CompressionUnit = 4,
    IoRangeUnit = 5,
    ObjectStoreSplitUnit = 6,
    Morsel = 7,
    PageCluster = 8,
    DimensionalBucket = 9,
    ObjectPathFragment = 10,
    VendorDefined = 255,
}

struct LayoutUnitDescriptorV2 {
    unit_id: u32,
    unit_kind: u16,
    flags: u16,
    table_id: u32,
    column_id: u32,
    segment_id: u32,
    first_morsel_id: u32,
    morsel_count: u32,
    row_start: u64,
    row_count: u64,
    section_id: u32,
    byte_offset: u64,
    byte_length: u64,
    decode_dependency_ref: u32,
    compression_dependency_ref: u32,
    predicate_stats_ref: u32,
    coverage_set_ref: u32,
    preferred_read_size: u32,
    object_store_alignment: u32,
    checksum: u32,
}
```

**Rules:**
- A logical page is the row/value unit described by COVE page reconstruction rules.
- A physical page is the byte range containing one page payload after any page-level compression rules.
- A predicate statistics unit is the granularity at which proof metadata is valid.
- A decode unit is the smallest independently decodable value unit.
- A compression unit is the smallest independently decompressible byte unit.
- An I/O range unit is a suggested byte range for object-store or file-system reads.
- An object-store split unit is a scheduling unit for distributed readers.
- A layout unit MUST reference authoritative COVE metadata and MUST NOT introduce a new schema, row order, logical value, or predicate proof.

### 67.6 Scan Split Index

A scan split is a planner-ready unit of work that may group one or more table segments, morsel ranges, column groups, and page clusters.

```rust
struct ScanSplitIndexHeaderV2 {
    split_count: u32,
    flags: u32,
    checksum: u32,
}

struct ScanSplitEntryV2 {
    split_id: u32,
    table_id: u32,

    row_start: u64,
    row_count: u64,

    first_segment_id: u32,
    segment_count: u32,

    first_morsel_id: u32,
    morsel_count: u32,

    first_cluster_id: u32,
    cluster_count: u32,

    stats_ref: u32,
    estimated_uncompressed_bytes: u64,
    estimated_encoded_bytes: u64,

    flags: u32,
    checksum: u32,
}
```

**Rules:**
- Scan splits are scheduling hints. They MUST NOT change logical row order or returned rows.
- A reader MAY ignore scan splits and derive splits from table segment and morsel metadata.
- A corrupt optional split index MUST be ignored.
- Split estimates are advisory and MUST NOT be used as predicate proof.


### 67.6.1 COVE-R Standards Boundary

COVE-R is an implementation-guidance standard plus a small optional metadata surface. It SHOULD NOT be treated as a prerequisite for ordinary COVE-Core or COVE-T interoperability.

**Rules:**
- A COVE-Core/COVE-T reader MAY ignore all COVE-R metadata and remain conforming.
- Runtime registries, sessions, FFI adapters, language bindings, and engine adapters are not COVE logical data.
- Only explicitly encoded `RuntimeCompatibilityHintV2` and `RuntimeRegistryBindingV2` records are wire artifacts, and those records are advisory unless required by a requested runtime operation.
- A file MUST NOT depend on an unversioned process-global registry to define decode semantics.
- A required registered codec, mapping function, extension type, or engine profile MUST have a portable descriptor and conformance contract in the file, companion artifact, or registry specification; a runtime binding alone is insufficient.

### 67.7 COVE-R Runtime Registry and Session Model

COVE-R is primarily implementation guidance. It describes how readers SHOULD organise extensible runtime state without making that state part of file semantics.

**Recommended implementation model:**

```rust
struct CoveReaderSession {
    codec_registry;
    layout_registry;
    extension_type_registry;
    predicate_kernel_registry;
    synopsis_registry;
    engine_profile_registry;
    mapping_function_registry;
    ffi_adapter_registry;
    memory_and_io_policy;
}
```

**Rules:**
- A reader SHOULD instantiate codecs, kernels, mapping functions, and engine adapters through an explicit session or equivalent context rather than process-global mutable state.
- A COVE file MUST NOT depend on an unversioned global runtime registry to define required semantics.
- Runtime compatibility hints MAY help select adapters, but COVE-Core/COVE-T logical decode MUST remain possible without COVE-R unless a required codec/profile is explicitly needed.
- FFI and language bindings are ecosystem surfaces, not COVE logical data.
- Session caches MAY store decoded dictionaries, FileCode-to-ExecutionCode maps, layout plans, or range-read plans, but caches are not authoritative and MUST be rebuildable.

### 67.8 Runtime Compatibility Hints

```rust
struct RuntimeCompatibilityHintV2 {
    hint_id: u32,
    hint_kind: u16,
    // 0=codec_registry
    // 1=layout_registry
    // 2=predicate_kernel
    // 3=engine_adapter
    // 4=ffi_surface
    // 5=language_binding
    // 6=wasm_or_external_kernel_package

    required: u8,
    flags: u8,

    namespace_len: u16,
    namespace: [u8],

    name_len: u16,
    name: [u8],

    version_major: u16,
    version_minor: u16,

    payload_ref: u32,
    checksum: u32,
}
```

**Rules:**
- Runtime compatibility hints are optional unless the requested operation explicitly requires the hinted runtime surface.
- External kernel packages MUST NOT be required for baseline COVE-Core/COVE-T decode unless a required feature bit and extension contract explicitly say so.
- A runtime hint MUST NOT override a codec, extension, table schema, COVE-MAP function, or engine profile definition.

### 67.9 Non-Normative Vortex Interoperability Boundary

COVE v2 may be implemented using Vortex-inspired or Vortex-backed libraries, adapters, encodings, or benchmarks. Such implementation choices are non-normative.

**Rules:**
- A valid COVE v2 file is not a Vortex file and does not contain a Vortex layout tree as its authoritative data model.
- A COVE reader MAY map COVE pages into Vortex arrays or layouts internally, but that mapping MUST be derived from validated COVE metadata.
- A COVE writer MUST NOT make Vortex dtype/schema identity, runtime layout identifiers, or plugin registry IDs the only way to decode COVE logical values.
- A Vortex-backed adapter MUST preserve COVE table catalog authority, FileCode semantics, null bitmap polarity, predicate-proof safety, COVE-O reconstruction, and COVE-MAP provenance rules.

---

## 68. COVX Accelerator Sidecar

COVX is an optional sidecar containing rebuildable acceleration metadata.
**COVX final bytes:**
[postscript bytes]
[postscript_version: u16]
[postscript_len: u16]
[magic: "CVX2"]

### 68.1 COVX Header

```rust
struct CovxHeaderV2 {
    magic: [u8; 4],          // "CVX2"

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
struct CovxReferencedFileV2 {
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


### 68.3 COVX Kernel Capability Vocabulary

COVX may describe optional accelerated kernels. These kernels are implementation dispatch hints, not mandatory execution plans.

```rust
enum CoveKernelKindV2 {
    ScanEncodedEq = 0,
    ScanEncodedRange = 1,
    ScanEncodedInSet = 2,
    ScanEncodedNotNull = 3,
    SelectByBitmap = 4,
    ExtractRowRange = 5,
    ExpandByBitmap = 6,
    DecodeSelected = 7,
    DecompressBlock = 8,
    MaterialiseArrowView = 9,
    MaterialiseArrowOwned = 10,
    BuildCoverageSet = 11,
    IntersectCoverageSets = 12,
    UnionCoverageSets = 13,
    VendorDefined = 255,
}

struct CovxKernelDescriptorV2 {
    kernel_id: u32,
    kernel_kind: u16,
    input_encoding_kind: u16,
    input_codec_id: u32,
    output_form: u16,
    null_semantics: u8,
    comparison_semantics: u8,
    deterministic_equivalence_ref: u32,
    requires_alignment_log2: u8,
    optional_hardware: u8,       // 0=none, 1=simd, 2=gpu, 3=iaa_qpl, 4=arm_extension, 255=vendor
    reserved: u16,
    checksum: u32,
}
```

**Rules:**
- A COVX kernel descriptor MUST declare equivalence to baseline logical semantics or be marked advisory/non-semantic.
- Hardware acceleration is optional. A reader MUST NOT require a specific hardware accelerator to read ordinary COVE-Core/COVE-T values.
- A reader MAY ignore COVX kernels and use baseline decode.
- A kernel descriptor MUST NOT be used as predicate proof. Proof still comes from validated COVE predicate or coverage metadata.

### 68.4 Sidecar Validity

Every COVX sidecar MUST describe exactly which data snapshot, file set, schema, semantic map, and digest root it applies to.

```rust
struct SidecarValidityV2 {
    dataset_id: [u8; 16],
    snapshot_id: [u8; 16],
    file_id: [u8; 16],          // zero UUID when dataset-scoped
    schema_fingerprint_ref: u32,
    semantic_map_fingerprint_ref: u32,
    data_checksum_root_ref: u32,
    external_visibility_ref: u32,
    created_at_us: i64,
    producer_ref: u32,
    flags: u32,
    checksum: u32,
}
```

**Rules:**
- A reader MUST NOT use a sidecar whose declared validity does not match the selected data snapshot and requested operation.
- If `semantic_map_fingerprint_ref` is non-zero, the sidecar is valid only for that mapping/projection version unless a required extension proves compatibility.
- If an external visibility/delete overlay is active, a sidecar that is not overlay-aware may be used for conservative physical-file pruning but MUST NOT provide exact visible-table aggregate answers.
- Sidecar validity applies to COVX, COVE-I, COVM references, COVE-MAP references, runtime mapping artifacts, and coverage artifacts.

---

## 69. COVM Dataset Manifest

COVM is an optional multi-file dataset manifest.
**COVM final bytes:**
[postscript bytes]
[postscript_version: u16]
[postscript_len: u16]
[magic: "CVM2"]

### 69.1 COVM Header

```rust
struct CovmHeaderV2 {
    magic: [u8; 4],          // "CVM2"

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
struct CovmFileEntryV2 {
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
- COVE-I secondary index references, including root indexes and index validity,
- COVE-COVERAGE provider references and coverage set summaries,
- COVE-CACHE compatibility and invalidation hints when a runtime cache is permitted by policy,
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




### 69.5 COVE-CACHE Runtime Coverage Cache

COVE-CACHE is an optional mutable runtime/local cache for predicate coverage sets. It is intentionally not part of immutable `.cove` logical truth and SHOULD NOT be stored inside a `.cove` file.

**Local persistence boundary:**
COVE-CACHE does not define a normative `.cove`-family artifact, magic value, or durable publication protocol in v2. An implementation MAY persist cache entries in a local store, but that store is engine-owned, mutable, revocable, and outside COVE logical truth. The structure below is a recommended diagnostic/interop record shape for implementations that expose cache state; it is not a canonical artifact header.

```rust
struct CoveCoverageCacheHeaderV2 {
    cache_format_namespace_ref: u32,
    cache_format_version_major: u16,
    cache_format_version_minor: u16,
    flags: u32,
    cache_id: [u8; 16],
    dataset_id: [u8; 16],
    snapshot_id: [u8; 16],
    entry_count: u32,
    created_at_us: i64,
    producer_engine_ref: u32,
    reserved: [u8; 32],
    checksum: u32,
}

struct CoverageCacheEntryV2 {
    entry_id: u64,
    dataset_id: [u8; 16],
    snapshot_id: [u8; 16],
    predicate_normal_form_ref: u32,
    interval_normal_form_ref: u32,
    coverage_set_ref: u32,
    coverage_granularity: u8,
    proof_strength: u8,
    exactness: u8,
    flags: u8,
    actual_coverage_size_bytes: u64,
    actual_read_cost_ns: u64,
    created_at_us: i64,
    valid_until_snapshot_ref: u32,
    producer_engine_ref: u32,
    checksum: u32,
}
```

**Invalidation triggers:**

A COVE-CACHE entry MUST be invalidated when any of the following changes unless a required extension proves the cached coverage remains conservative:

- selected dataset snapshot;
- COVM publication state;
- referenced `.cove` file list, file length, footer CRC, or digest;
- schema fingerprint;
- external delete or visibility overlay;
- COVE-MAP mapping/projection version;
- COVE-I or COVX sidecar version used to build the entry;
- semantic dimension definition;
- collation or deterministic function version;
- policy governing redaction or protected metadata.

**Rules:**
- COVE-CACHE may improve planning, but it is never canonical truth.
- A reader MUST be able to ignore COVE-CACHE and still read correct logical values from `.cove` files and validated required artifacts.
- COVE-CACHE entries MAY be used only for the dataset snapshot and predicate context for which they are valid.
- COVE-CACHE entries that are stale, corrupt, unsupported, engine-local to another incompatible engine, or approximate-may-under-include MUST NOT be used for pruning.
- COVE-CACHE may store predicate containment relationships, interval normal forms, and actual observed coverage costs, but these are planning hints unless the stored coverage set itself is validated as conservative.

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

COVE-MAP is an optional v2 profile with a stable conceptual and conformance boundary. Artifact identifiers, artifact framing, validation boundary, identity rules, projection/evidence rules, standard `MAP_*` payload schemas, and operation-level fallback/rejection behaviour in this section are normative for v2. Non-normative implementation documents MAY provide examples, generated JSON Schema files, implementation notes, or extension payload schemas, but they are not required to implement the standard COVE-MAP v2 mapping model defined here. Registered required extensions MAY add new section kinds, encodings, functions, or expression operators, but they MUST NOT redefine the standard v2 semantics in this section.

A reusable mapping definition SHOULD be stored in a separate `.covemap` artifact. Embedded `MAP_*` sections inside a `.cove` file are typically file-local evidence, projection catalogs, conversion reports, identity-equivalence indexes, or embedded mapping snapshots tied to that file or dataset state. Unless a required profile or extension explicitly says otherwise, the `.covemap` artifact is the authoritative reusable mapping definition.

**COVEMAP final bytes:**
[postscript bytes]
[postscript_version: u16]
[postscript_len: u16]
[magic: "CMP2"]

`.covemap` uses the same tail-discovery pattern as COVE files. The postscript points to the CovemapHeaderV2 region rather than to a COVE footer.

```rust
struct CovemapPostscriptV2 {
  required_features: u64,
  optional_features: u64,
  file_len: u64,
  header_offset: u64,
  header_length: u64,
  checksum: u32,
}
```

```rust
struct CovemapHeaderV2 {
  magic: [u8; 4],          // "CMP2"

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
//   CovemapSectionEntryV2[section_count]
```

```rust
enum CovemapPayloadEncodingV2 {
  CoveMapJsonV2 = 1,       // UTF-8 canonical JSON payload using Section 70 schema
  CoveMapCborV2 = 2,       // deterministic CBOR representation of the same Section 70 schema
  Extension = 255,
}

struct CovemapSectionEntryV2 {
  section_id: u32,         // MAP_* or VENDOR_EXTENSION
  offset: u64,
  length: u64,
  uncompressed_length: u64,
  compression: u8,
  payload_encoding: u8,   // CovemapPayloadEncodingV2
  required: u8,
  reserved: u8,
  checksum: u32,
}
```

**Assigned mapping artifact identifiers:**

| Field | Value |
| --- | --- |
| Artifact magic | `CMP2` |
| Extension | `.covemap` |
| Primary role | Deterministic source-row to semantic-assertion mapping |
| Output role | Produce COVE-O object/association output and optional COVE-T/COVM/COVX/projection artifacts |

A `.covemap` artifact may be referenced by COVM or by output COVE metadata using digest-verified references. COVM may reference mappings for lineage and replay, but COVM MUST NOT be the sole authority for semantic interpretation unless a future required profile defines that behaviour.

COVE-MAP artifacts MUST be immutable for a declared mapping version. A new mapping version may produce different output, but the mapping version, source snapshot/load identity, deterministic functions, and conflict rules must make the difference explainable.

Generated JSON Schema files, reference-tool schema documents, and examples MAY be published for implementer convenience, but they are derived artifacts. The authority for standard COVE-MAP v2 `MAP_*` payload content is this section.

**Rules:**
- `magic` MUST be `CMP2`.
- `mapping_version` identifies the reusable mapping-definition version; a new version MUST produce a new immutable artifact.
- `.covemap` postscript discovery MUST use absolute byte offsets from the start of the artifact. `header_offset` and `header_length` in CovemapPostscriptV2 MUST be within `file_len` and MUST locate the CovemapHeaderV2 region for the artifact version being read.
- `section_id` SHOULD reference `MAP_*` section kinds or `VENDOR_EXTENSION`.
- `offset` and `length` in CovemapSectionEntryV2 are absolute byte offsets from the start of the `.covemap` artifact unless a future required extension defines otherwise.
- `compression` in CovemapSectionEntryV2 uses the Section 66 `CompressionCodec` registry.
- `payload_encoding` MUST identify the encoding of the uncompressed payload bytes. A COVE-MAP v2 artifact validator MUST support `CoveMapJsonV2` for all standard section kinds it claims. `CoveMapCborV2` and `Extension` are optional unless advertised by a required feature bit or claimed profile.
- If `compression` is `None`, `length` MUST equal `uncompressed_length`.
- If `compression` is not `None`, `uncompressed_length` MUST be the exact decoded byte length.
- If `length == 0`, `uncompressed_length` MUST also be zero.
- A `.covemap` artifact MUST be discoverable and integrity-checkable without consulting a COVE data file.
- The artifact framing and standard payload schema defined here are stable for v2. Payload bodies MUST conform to the Section 70 schema for their `section_id`, mapping version, and `payload_encoding`, or to a registered required extension when `section_id` or `payload_encoding` is extension-defined.
- An implementation MAY claim support for a subset of standard COVE-MAP section kinds, but it MUST NOT claim full COVE-MAP artifact validation unless it validates every standard section kind present in a required `.covemap` artifact.

#### 70.1.1 Standard COVE-MAP Payload Schema

COVE-MAP v2 defines a standard logical payload schema for the `MAP_*` section kinds listed in Section 14. This schema is normative in this document. Encodings such as `CoveMapJsonV2` and `CoveMapCborV2` are byte representations of the same logical schema; they are not separate mapping languages.

**Standard section payload model:**

| Section kind | Standard payload body |
| --- | --- |
| `MAP_SOURCE_CATALOG` | Mapping metadata plus an ordered array of source declarations described by Section 70.2. |
| `MAP_FUNCTION_REGISTRY` | Ordered function declarations described by Section 70.13. |
| `MAP_IDENTITY_RULE_CATALOG` | Identity rule declarations, semantic join-key components, confidence classes, merge policy, do-not-merge policy, and tie-breakers described by Section 70.5. |
| `MAP_ROW_SEMANTICS_CATALOG` | Source row semantics, operation semantics, dispatch/composite/key-value rules, temporal roles, and assertion kinds described by Sections 70.3 and 70.4. |
| `MAP_ASSERTION_LOG` | Optional ordered semantic assertion records described by Section 70.4, including deterministic assertion identity, source row identity, rule identity, payload, and conflict/candidate status where applicable. |
| `MAP_IDENTITY_EQUIVALENCE_INDEX` | Deterministic identity-key to GOID/equivalence-set records described by Sections 70.5 through 70.7. |
| `MAP_EVIDENCE_INDEX` | Evidence records described by Section 70.12. |
| `MAP_CONVERSION_REPORT` | Conversion diagnostics, rejected rows, unresolved conflicts, candidate matches, fidelity metrics, and policy outcomes described by Sections 70.8, 70.14, and 70.15. |
| `MAP_PROJECTION_CATALOG` | Object/association projection declarations and expression records described by Section 70.10. |

**Common payload rules:**
- A standard payload MUST identify `schema_id = "org.coveformat.covemap.v2"`, `section_id`, `mapping_id`, and `mapping_version`.
- All field names in standard JSON payloads are lowercase snake_case ASCII names from this specification. Duplicate object keys are invalid. Unknown fields are invalid unless they are nested under an `extensions` object whose namespace is declared by a required extension or under a vendor-specific optional extension that the reader is allowed to ignore.
- `CoveMapJsonV2` payloads MUST be UTF-8 JSON text with a top-level object, no duplicate object keys, no non-finite numbers, and no semantic dependence on object member order. When a writer advertises canonical-byte reproducibility for JSON payloads, object members MUST be emitted in lexicographic order by UTF-8 member-name bytes, insignificant whitespace MUST be omitted, and numbers MUST use the COVE canonical textual form for their logical type.
- `CoveMapCborV2` payloads MUST be a deterministic CBOR representation of the same logical schema: definite lengths, shortest integer encodings, deterministic map ordering, and COVE canonical logical-value semantics for typed values.
- Arrays whose order affects identity, conflict resolution, function dispatch, projection output, or evidence replay MUST be emitted in declared deterministic order and MUST be validated in that order.
- Logical values embedded in COVE-MAP payloads MUST use COVE canonical logical value semantics. Identity keys, hashes, and digests MUST be computed from canonical logical values or from the canonical tuple bytes defined in Section 70.5.
- IDs used for sources, functions, rules, projections, object types, association types, semantic roles, and dimensions MUST be stable within the mapping version. A payload MUST NOT rely on source file order, map-object iteration order, locale defaults, or runtime-generated names for semantic identity.
- A payload that omits a field marked as required by the relevant Section 70 rule is malformed. A payload that uses an undeclared function, source, identity rule, projection, object type, association type, semantic role, or extension namespace is malformed.

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
| AssociationOnly | Row creates an association assertion without a separate object, unless materialised as a link object for COVE-O v2. |
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

#### 70.3.1 Harbor Row Semantics Translation Boundary

COVE-MAP adopts the useful two-axis structure from Harbor Row Semantics in an engine-neutral, offline form.

**Axis 1: core row meaning** describes what the row fundamentally contributes:

| Harbor core semantics | COVE-MAP row semantics | COVE-MAP meaning |
| --- | --- | --- |
| Object | Object | Row contributes to one independent destination object. |
| Transaction | EventObject | Row creates a point-in-time event or transaction object, often with frozen/stamped property values. |
| Link | LinkObject | Row creates a first-class connector object between endpoint objects and may also create association assertions. |
| Association | AssociationOnly | Row creates an association assertion without requiring a separate source object, although COVE-O v2 materialises it as a link/association object when object output is requested. |
| View | ProjectionOnly | Row is a read surface and does not create canonical object truth unless an explicit mapping rule says so. |

**Axis 2: derived meaning wrappers** describe how row meaning is selected or expanded:

| Harbor wrapper | COVE-MAP equivalent | COVE-MAP meaning |
| --- | --- | --- |
| Dispatched | Dispatched | A discriminator value selects one of several deterministic row-semantics rules. |
| Composite | Composite | One source row emits multiple object, property, association, temporal, tombstone, or evidence assertions. |
| KeyValueFragment | KeyValueFragment | Row is an entity-attribute-value or sparse-property fragment. |
| DerivedTable | ProjectionOnly / Projection rule | Row/table is a derived read surface, aggregate, or debug/export projection rather than canonical truth unless explicitly materialised with lineage. |

**Rules:**
- COVE-MAP MUST describe source-row meaning without requiring Harbor runtime mutation semantics.
- COVE-MAP row semantics are applied to immutable source snapshots, source files, source streams, or declared source loads.
- A COVE-MAP converter MAY materialise results as COVE-O object/association history, COVE-T projections, evidence indexes, conversion reports, or future profile outputs.
- Harbor-specific concepts such as SQL DML side effects, Harbor object graph mutation, Harbor tenant state, and Harbor leased codes remain COVE-H or implementation concerns.

#### 70.3.2 Source Operation Semantics

Some source rows describe operational changes rather than complete facts. COVE-MAP supports deterministic operation interpretation without making COVE files mutable.

```rust
enum SourceOperationKind {
    Fact = 0,                 // row asserts a complete or partial fact
    Insert = 1,               // row represents creation in the source
    Upsert = 2,               // row creates or updates according to source identity
    PatchProperty = 3,        // row modifies one or more properties
    ReplaceObjectState = 4,   // row replaces the mapped state for an object or association
    CloseAssociation = 5,     // row ends an association validity interval
    ExpireAndCreate = 6,      // row closes an old association/state and creates a replacement
    TombstoneObject = 7,      // row tombstones an object
    TombstoneProperty = 8,    // row clears/tombstones a property
    TombstoneAssociation = 9, // row tombstones or closes an association
    RedactEvidence = 10,      // row redacts evidence or protected payload
    EvidenceOnly = 11,        // row is retained for provenance but does not alter canonical truth
    Correction = 12,          // row corrects previous source evidence according to declared policy
}
```

**Rules:**
- Source operation semantics MUST be declared by mapping rule, source stream, source table, or row discriminator.
- Operation rows MUST still produce deterministic semantic assertions.
- Operation rows MUST NOT imply in-place mutation of an existing COVE file.
- When materialised as COVE-O, operation rows SHOULD produce deltas, snapshots, baselines, tombstones, association validity changes, or evidence records according to COVE-O rules.
- `PatchProperty` MUST declare null/missing semantics and whether null means unknown, no-op, clear, tombstone, or redacted.
- `ReplaceObjectState` MUST declare whether omitted properties are unchanged, cleared, tombstoned, or unknown.
- `CloseAssociation` and `ExpireAndCreate` MUST declare the temporal axis used: valid time, observed time, source transaction time, mapping execution time, or COVE-O commit/file-ordering time.
- `Correction` MUST declare whether the correction rewrites interpretation for a replayed mapping version, emits a new temporal correction fact, or records conflict evidence only.
- Operation interpretation MUST be replayable from the declared source snapshot/load, mapping version, function versions, and conflict policy.

#### 70.3.3 Stamped and Frozen Value Semantics

A source row may intentionally freeze a value copied or derived from another object at the time the source event occurred. This is common for order/customer, payment/account, admission/patient, and audit/event records.

**A stamped value rule SHOULD declare:**
- destination object or association type;
- stamped property ID/name;
- source or referenced object/property expression;
- temporal role of the stamp;
- whether the stamped value is immutable after creation;
- evidence source and rule ID;
- conflict behaviour if replay finds a different referenced current value.

**Rules:**
- A stamped value is a canonical property assertion of the event/link/association object that receives it.
- A stamped value MUST NOT be silently recomputed from current object state during readback unless the projection rule explicitly requests a derived current-state value.
- A stamped value SHOULD retain evidence identifying the source row and mapping rule that produced the frozen value.
- When materialised as COVE-O, stamped values SHOULD appear as ordinary properties of the event or link object with evidence references, not as hidden projection-only metadata.


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
  - id: customer.name_email.v2
    object_type: Customer
    class: strong_deterministic
    auto_merge: true
    null_policy: all_components_required
    components:
      - role: Customer.Name
        logical_type: Utf8
        normalise: cove.fn.person_name.v2
        bindings:
          crm.customers: name
          support.tickets: requester_name
      - role: Customer.Email
        logical_type: Utf8
        normalise: cove.fn.email.v2
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

For COVE-O v2 destinations, association assertions SHOULD be materialised as link/association object types as described in Section 61.1 unless a future association-specific extension is required. A reader that exposes COVE-O as an object-association surface SHOULD present these materialised records as associations even though their v2 storage form is object records.

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

COVE-MAP v2 standard projection expressions use the following normative expression model. Surface syntaxes such as JSON strings, JSON expression objects, or deterministic CBOR expression objects MUST map exactly to this model.

```text
projection_expr =
    path_ref
  | literal
  | function_call
  | aggregate_call
  | association_traversal
  | conditional_expr

path_ref =
  identifier("." identifier)*

association_traversal =
  "association(" association_type_ref ["," endpoint_role] ")" ["." path_ref]

aggregate_call =
  ("count" | "min" | "max" | "sum" | "avg" | "exists" | "distinct_count")
  "(" [projection_expr] ")"

function_call =
  function_id "(" [projection_expr ("," projection_expr)*] ")"

conditional_expr =
  "if" "(" predicate_ref "," projection_expr "," projection_expr ")"
```

**Projection expression rules:**
- `identifier`, `association_type_ref`, `endpoint_role`, `function_id`, and `predicate_ref` MUST resolve through the mapping artifact's source, object, association, function, predicate, or projection catalogs.
- A path reference MUST resolve to exactly one declared object property, association property, endpoint role, evidence field, temporal role, or projection-local binding.
- Function calls MUST reference declared deterministic functions from `MAP_FUNCTION_REGISTRY`.
- Aggregate calls MUST declare their null policy, empty-set policy, cardinality policy, and temporal cut when those policies are not implied by the projection rule.
- Association traversals MUST declare how zero, one, and many matching associations affect the output row: null, empty list, row explosion, aggregation, rejection, or deterministic first/last according to a declared ordering.
- An implementation MAY reject an expression operator it does not support unless the projection is optional and can be ignored for the requested operation.

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
  - id: customer_summary.v2
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
  - id: customer_order_edges.v2
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


### 70.10.1 Semantic Dimensions and Object/Dimensional Coverage Maps

COVE-MAP may describe logical dimensions over object, association, nested, or projected data. A semantic dimension is a named logical axis that can be mapped to physical fragments, layout buckets, COVE-I index entries, COVX acceleration structures, or COVE-COVERAGE coverage sets.

```rust
struct SemanticDimensionV2 {
    dimension_id: u32,
    name_ref: u32,
    dimension_kind: u16,       // categorical, integer, decimal, timestamp, spatial, genomic, object_path, association_role, extension
    logical_type: u16,
    collation_id: u16,
    path_ref: u32,
    object_type_id: u32,
    association_type_ref: u32,
    bucket_policy_ref: u32,
    flags: u32,
    checksum: u32,
}

struct DimensionalCoverageLayoutV2 {
    layout_id: u32,
    dimension_count: u16,
    coverage_function_kind: u16, // tuple, range_bucket, z_order, hilbert, semantic_path, extension
    flags: u32,
    dimensions_ref: u32,
    maps_to_granularity: u8,     // file, segment, morsel, page, object, projection fragment, etc.
    complete_coverage: u8,
    tight_when_predicate_matches_layout: u8,
    reserved: u8,
    coverage_provider_ref: u32,
    checksum: u32,
}
```

**Example dimensions:**

```text
semantic_dimension chromosome:
  kind: categorical
  path: /variant/chromosome

semantic_dimension position:
  kind: integer
  path: /variant/position
  bucket_width: 100000
```

**Rules:**
- Semantic dimensions MUST be derived from canonical logical values and declared COVE-MAP functions, not source display bytes or engine-local codes.
- A dimensional coverage layout MUST declare whether it provides complete conservative coverage, tight coverage for matching predicates, or advisory layout hints only.
- A dimensional coverage layout MUST NOT redefine object identity, association identity, temporal truth, or projected-table semantics.
- Dimensional bucket maps may be used for object/dimensional query planning only when their coverage proof and snapshot validity are validated.
- Unknown semantic dimensions MUST be ignored for ordinary object/table reads. Operations requesting dimensional coverage or projection planning MAY reject if required dimensions are unsupported.

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

COVE-MAP v2 deliberately does not define:
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
| COVE-CX codec registry | Optional | Optional unless required codec pages are projected | Optional | Optional | Optional | Optional |
| Registered codec decode | Not required unless feature is required | Required when projected pages use required registered codecs | Required when projected pages use required registered codecs | Optional | Optional | Optional |
| COVE-L layout plan | Not required | Optional | Recommended for large archive planning | Optional | Optional | Optional |
| Scan split index | Not required | Optional | Recommended | Optional | Optional | Optional |
| Zero-copy buffer map | Optional | Optional | Optional | Optional | Optional | Optional |
| COVE-R runtime registry hints | Optional | Optional | Optional | Optional | Optional | Optional |
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

This subset is an implementation rollout tier, not a reduced COVE-T specification. A first public reader/writer SHOULD target this subset before claiming broader COVE ecosystem readiness, while the full COVE-T standard remains defined by all applicable COVE-T sections:
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


### 72.9 COVE-CX Registered Codec Profile

**Recommended for writers that want v2 specialised encodings:**
- emit `CODEC_EXTENSION_REGISTRY` with exact codec descriptors;
- set `FEATURE_CODEC_EXTENSION_REGISTRY`;
- set `FEATURE_REGISTERED_ENCODINGS` as required when projected pages need a registered codec and no valid fallback is present;
- provide fallback payloads or reject unsupported readers safely;
- include codec conformance vector references where available;
- preserve exact logical values, null positions, FileCode/NumCode semantics, collation, and trust inputs.

**Recommended first codecs:**
- FSST-style Utf8/VarBytes codec only where FileCode dictionary encoding is not better;
- ALP-style Float32/Float64 NumCode codec only when exact IEEE semantics are preserved;
- FastLanes-style integer/date/timestamp/decimal NumCode codec where frame-of-reference, delta, patched-base, or bit-packing improves scan or storage cost.

### 72.10 COVE-L Layout/Split Planning Profile

**Recommended for object-store and large archive datasets:**
- emit page cluster directory for range-read coalescing;
- emit scan split index for scheduling;
- emit layout plan nodes that reference existing tables, segments, morsels, columns, pages, statistics, and clusters;
- emit fast metadata index for very wide schemas or very large page directories;
- never rely on layout plans as the only schema, page, or predicate-proof authority.

### 72.11 COVE-R Runtime Registry Profile

**Recommended for reference implementations and engine adapters:**
- use explicit sessions/registries for codecs, layout plans, kernels, mapping functions, engine profiles, and FFI adapters;
- expose capability discovery without requiring global mutable state;
- keep runtime compatibility hints optional and rebuildable;
- make engine adapters read-only until COVE-Core/COVE-T/COVE-CX/COVE-L vectors pass.

### 72.12 COVE-COVERAGE Coverage Metadata Profile

**Recommended for writers that expose conservative coverage:**
- emit coverage provider descriptors for proof-carrying stats, indexes, maps, dimensions, or sidecars;
- emit predicate normal forms when a coverage provider depends on a normalised predicate representation;
- declare coverage granularity, proof kind, proof strength, exactness, snapshot validity, collation, logical type context, and null semantics;
- emit coverage degree and tightness degree as planning metrics only;
- emit coverage plan candidates and fallback policy when lookup cost matters;
- never use approximate-may-under-include artifacts for correctness-sensitive pruning.

### 72.13 COVE-I Secondary Index Profile

**Recommended for cross-file and high-selectivity archive workloads:**
- emit `.covi` artifacts with `CVI2` magic;
- reference COVE files by file_id, file length, footer CRC, and digest;
- emit index roots for indexed columns, object paths, associations, projection fragments, or semantic dimensions;
- declare exactness, coverage granularity, null semantics, collation, and index-only capabilities;
- reference `.covi` artifacts from COVM or an external catalog;
- ensure indexes are rebuildable and optional for ordinary reads.

### 72.14 COVE-CACHE Runtime Coverage Cache Profile

**Recommended for engines that repeatedly query the same immutable snapshot:**
- store cache entries outside `.cove` files;
- bind entries to dataset_id, snapshot_id, schema fingerprint, semantic-map version, visibility overlay, and sidecar versions;
- cache predicate normal forms, interval forms, conservative coverage sets, and observed costs;
- invalidate entries on snapshot, manifest, schema, semantic-map, index, sidecar, visibility, or policy changes;
- never treat cache entries as canonical truth.

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

### 73.6 COVE-CX Codec Validation

A COVE-CX-aware validator MUST validate codec descriptors before using registered codec pages.

**Validation requirements:**
- codec IDs unique within the file or artifact;
- namespace/name/version identifies a known exact codec contract or a supported required extension;
- `specification_status` is compatible with the claimed conformance level; candidate/provisional codecs are not required for broad COVE-T conformance;
- feature bits match required/optional codec usage;
- codec envelope row counts match page index row counts;
- params and payload ranges are within the page payload;
- fallback payloads, when present, decode to the same logical sequence as the registered payload;
- float codecs preserve exact declared IEEE semantics;
- string codecs preserve exact UTF-8 or Binary bytes;
- unsupported required codecs reject safely.

### 73.7 COVE-L Layout and Split Validation

A COVE-L-aware validator MUST validate that layout, cluster, split, zero-copy, and fast metadata sections reference existing authoritative metadata.

**Validation requirements:**
- layout node IDs unique;
- parent/child ranges valid and acyclic;
- referenced table, column, segment, morsel, page, section, stats, cluster, and split IDs exist;
- row ranges agree with table segment and morsel metadata;
- page cluster byte ranges do not contradict page index ranges;
- scan split row ranges are contiguous or explicitly declared non-contiguous according to profile rules;
- zero-copy targets do not claim incompatible null polarity, key width, alignment, offset width, endianness, compression state, dictionary semantics, nested layout, visibility overlay compatibility, or lifetime;
- corrupt optional COVE-L sections are ignored.

### 73.8 COVE-R Runtime Compatibility Validation

Runtime compatibility hints MUST be validated only when the requested operation uses them. Unknown optional runtime hints are ignored. Required runtime hints cause rejection only for the runtime operation that requires them. Runtime hints MUST NOT affect baseline logical decode.

### 73.8.1 COVE-COVERAGE Validation

A COVE-COVERAGE-aware validator MUST validate coverage providers, predicate normal forms, coverage sets, and plan candidates before using them for pruning, metadata-only answers, or index routing.

**Validation requirements:**
- provider IDs unique within the file/artifact;
- granularity, proof kind, proof strength, and exactness are known or registered;
- predicate normal forms parse under the declared AST, CNF/DNF, interval, or encoded-predicate grammar and reference declared columns, object paths, dimensions, logical types, collations, and null semantics;
- coverage entries obey per-granularity required fields, absent sentinels, ordering, duplicate, row-range, and row-ordinal-set invariants;
- coverage entries reference existing files, segments, pages, morsels, row ranges, objects, paths, projection fragments, or external fragments;
- snapshot validity matches the selected dataset state, semantic-map version, sidecar versions, and external visibility overlay;
- coverage metrics and costs are not used as proof;
- advisory, approximate-may-under-include, stale, corrupt, or unsupported coverage artifacts are not used for skipping.

### 73.8.2 COVE-I and COVE-CACHE Validation

COVE-I artifacts and COVE-CACHE entries MUST be validated only when the requested operation uses them.

**Validation requirements:**
- referenced file IDs, file lengths, footer CRCs, and digests match the selected files;
- COVE-I postscript, header, section directory, referenced-file table, snapshot validity records, index roots, key blocks, entry blocks, postings blocks, row ranges, row ordinal sets, and aggregate-answer blocks validate before use;
- index root logical types, physical kinds, key encoding, comparators, collations, null semantics, and path/dimension references match the query context;
- sorted keys, duplicate chains, hash-collision policy, postings ordering, row-range coalescing, and row-ordinal bitmap bit order validate;
- index-only capabilities declare exactness and overlay-awareness before answering exact queries;
- cache entries match dataset snapshot, predicate form, semantic-map version, schema fingerprint, and sidecar versions;
- stale or corrupt indexes/caches fail open to a wider conservative plan or full scan.

### 73.9 COVE-MAP Semantic Validation

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
| COVE-COVERAGE corrupt/stale/unsupported | Ignore coverage artifact and use wider conservative plan or full scan unless operation requires it |
| COVE-I stale/corrupt | Ignore index and scan unless operation explicitly requires the index |
| COVE-CACHE stale/corrupt | Ignore cache and plan from validated metadata or full scan |
| Index-only exactness unsupported | Do not answer from index; scan or reject if index-only was explicitly required |
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
| COVE_E_BAD_CODEC_EXTENSION | COVE-CX codec descriptor, envelope, payload, fallback, or feature-bit contract is invalid. |
| COVE_E_CODEC_UNSUPPORTED | A required registered codec is not supported and no valid fallback exists. |
| COVE_E_BAD_LAYOUT_PLAN | COVE-L layout plan, split index, page cluster, zero-copy map, or fast metadata index is invalid. |
| COVE_E_RUNTIME_HINT_UNSUPPORTED | A required COVE-R runtime compatibility hint is unsupported for the requested runtime operation. |
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
| COVE_E_BAD_COVERAGE | Coverage provider, predicate form, coverage set, proof strength, or coverage entry is invalid. |
| COVE_E_COVERAGE_STALE | Coverage artifact does not match the selected snapshot, schema, semantic map, file digest, or visibility overlay. |
| COVE_E_BAD_COVI | COVE-I secondary index artifact is malformed, stale, corrupt, or unsupported for the requested operation. |
| COVE_E_INDEX_ONLY_UNSAFE | Requested metadata/index-only answer is not exact or not valid for the selected snapshot/overlay. |
| COVE_E_CACHE_STALE | COVE-CACHE entry is stale, corrupt, approximate-may-under-include, or incompatible with the current runtime operation. |

---

## 77. Compatibility

### 77.1 Versioning

**COVE v2 readers support:**
version_major = 2

**Compatibility note:**
- A COVE v2 reader MAY implement a separate COVE v1 reader.
- A COVE v1 reader MUST reject COVE v2 files because the primary magic and major version differ.
- A COVE v2 writer SHOULD NOT emit COVE v2 magic for a file that uses only v1 semantics unless it intentionally opts into v2 conformance and validation rules.

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

Conformance levels are cumulative implementation claims, not reductions in specification detail. A narrower conformance claim lets an implementation honestly state what it supports, but it does not make unsupported profiles underspecified or optional in the standards-suite sense.

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
**A COVE-CX-aware reader MUST additionally:**
- parse and validate codec extension descriptors when required,
- reject unsupported required registered codecs without valid fallback,
- use fallback payloads only after checksum and semantic validation,
- preserve exact logical values and null positions when decoding registered codecs,
- never use codec capability metadata as predicate proof.
**A COVE-L-aware reader MUST additionally:**
- validate layout nodes, scan splits, page clusters, zero-copy maps, and fast metadata references before use,
- ignore corrupt optional COVE-L sections,
- treat layout plans and split indexes as scheduling metadata, not logical truth,
- derive predicate pruning only from validated COVE proof metadata.
**A COVE-R-aware implementation MUST additionally:**
- keep runtime registries/session state outside COVE logical truth,
- ignore unknown optional runtime hints,
- reject unknown required runtime hints only for operations that explicitly require them.
**A COVE-COVERAGE-aware reader MUST additionally:**
- validate provider descriptors, predicate forms, coverage sets, proof strength, exactness, and snapshot validity before use,
- use only conservative coverage for pruning or index routing,
- fail open to wider coverage or full scan when coverage is unsupported, stale, corrupt, or advisory,
- never use coverage metrics or cost estimates as proof.
**A COVE-I-aware reader MUST additionally:**
- validate `.covi` header, referenced file fingerprints, index roots, capabilities, and snapshot validity before use,
- distinguish exact, approximate, and advisory index-only capabilities,
- apply external visibility overlays before returning rows or exact aggregates,
- ignore stale or corrupt secondary indexes for ordinary reads.
**A COVE-CACHE-aware implementation MUST additionally:**
- keep cache entries outside COVE logical truth,
- bind cache entries to snapshot, schema, mapping, visibility, and sidecar versions,
- invalidate stale entries,
- never use cache entries that may under-include data for pruning.
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

A public interoperability release of COVE SHOULD NOT claim broad v2 readiness without a working reference reader, reference writer, and binary conformance suite. The wire format is defined by this specification, but adoption depends on reproducible test artifacts. An implementation SHOULD NOT claim COVE-Core or COVE-T conformance until it passes the applicable public vectors for that level.

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
- COVE-MAP source-to-object conversion cost and identity-resolution cost when COVE-MAP tooling is claimed,
- registered codec decode and predicate-kernel cost,
- fallback payload overhead,
- layout-plan and scan-split planning overhead,
- page-cluster range-read coalescing benefit,
- zero-copy export success/fallback rate,
- coverage degree and tightness degree for representative predicates,
- coverage-provider lookup cost versus scan cost,
- COVE-I index lookup and index-only answer latency,
- COVE-CACHE hit/miss/invalidation behaviour.

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
- COVE-MAP source catalog validation, deterministic function registry validation, multi-column join-key canonicalisation, candidate-vs-canonical identity separation, do-not-merge enforcement, source evidence traceability, object-and-association-based COVE-O output validation, association readback validation, and projection-rule validation,
- COVE-COVERAGE provider validation, predicate normal-form validation, interval predicate canonicalisation, conservative coverage proof validation, coverage/tightness metric reporting, and stale coverage rejection,
- COVE-I secondary index root validation, value-to-fragment lookup, path/dimensional-bucket lookup, exact index-only count/min/max/existence vectors, approximate answer rejection for exact queries, and stale index rejection,
- COVE-CACHE predicate containment, snapshot-bound cache reuse, invalidation triggers, and full-scan fallback,
- feature-scope rejection vectors: unknown header `FileRequired` feature rejects, unknown section `SectionRequired` feature rejects only when used, unknown `OperationRequired` feature rejects only the requested operation, global extended feature-word numbering is honoured in `SECTION_FEATURE_BINDING`, and ordinary COVE-T scan succeeds when unsupported optional COVE-MAP/COVE-I/COVE-L/COVE-R metadata is present,
- registered codec vectors: unsupported required codec without fallback rejects selected page decode, unsupported codec with valid fallback decodes identically, malformed fallback rejects, and candidate/provisional codecs cannot be required for broad COVE-T conformance,
- coverage false-negative prevention vectors: corrupt, stale, under-inclusive, mismatched-snapshot, mismatched-overlay, and unsupported coverage proofs are ignored or rejected rather than used for skipping,
- COVE-I binary grammar vectors: block-container header validation, `CIK2` key-block validation, local reference-space validation, exact posting payload-layout validation for every v2 representation, sorted-key validation, duplicate-key handling, postings ordering, row-range coalescing, row-ordinal bitset bit order, hash-collision verification, aggregate-answer reference validation, coverage-set reference validation, and stale referenced-file digest rejection,
- predicate grammar vectors: canonical operand-ref validation, mirror-field mismatch rejection, operator arity validation for `LiteralValue`, `ColumnRef`, `Between`, `InSet`, n-ary `And`/`Or`, `Not`, `FunctionCall`, malformed operand ordering, literal-list sorting, and unsupported extension operators evaluating to `Unknown` for pruning,
- zero-copy incompatibility vectors: unknown target/source buffer roles, null-polarity mismatch, offset-width mismatch, dictionary-key mismatch, compressed-buffer mismatch, insufficient lifetime, nested-layout mismatch, and active visibility overlay all force materialised output,
- external visibility overlay vectors for coverage, COVE-I index-only answers, lookup indexes, aggregate synopses, and zero-copy export.

### 79.2 New v2 Surface Hardening Requirements

Before a public release claims broad v2 readiness, every newly introduced v2 surface MUST have both positive and negative vectors at the same mechanical precision as COVE-Core/COVE-T.

**Required hardening vector families:**
- `feature-scope/`: header, section, page, profile, and operation requiredness combinations, including global extended feature-word numbering and invalid local word-bank bindings;
- `coverage/`: predicate AST/CNF/interval payload parsing, canonical operand references, mirror-field validation, operator arity, set algebra, false-negative prevention, and snapshot/overlay invalidation;
- `covi/`: `.covi` postscript/header/section validation, block-container validation, `CIK2` key-block validation, local reference-space validation, key grammar, comparator semantics, exact posting payload layouts, row ordinal sets, index-only answers, and stale file references;
- `codecs/`: descriptor status, exact bitstream versions, fallback equivalence, negative malformed payloads, and unsupported required codecs;
- `zerocopy/`: compatible Arrow-view exports and every mandatory materialisation case;
- `sidecars/`: COVX/COVM/COVE-I digest mismatch, schema mismatch, semantic-map mismatch, and sidecar freshness;
- `visibility/`: delete/visibility overlay interactions with pruning, metadata-only answers, indexes, coverage, and zero-copy.

A profile whose new binary grammar lacks these vectors SHOULD remain provisional or implementation-specific rather than broad-conformance-ready.

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
- **cove-codec:** Optional COVE-CX library for registered codec descriptors, codec dispatch, fallback validation, and codec conformance tests.
- **cove-layout:** Optional COVE-L library for layout plans, scan splits, page cluster planning, fast metadata indexes, and zero-copy buffer maps.
- **cove-coverage:** Optional COVE-COVERAGE library for predicate normal forms, coverage providers, coverage sets, plan candidates, and proof validation.
- **cove-index:** Optional COVE-I library for building, validating, and querying `.covi` secondary indexes.
- **cove-cache:** Optional COVE-CACHE helpers for runtime/local predicate coverage caches and invalidation.
- **cove-runtime:** Optional COVE-R helpers for explicit reader sessions, registries, FFI adapter discovery, and engine compatibility hints.

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
- **cove-codec-validate:** Validate registered codec descriptors, codec envelopes, fallback payloads, and codec conformance vectors.
- **cove-layout-inspect:** Inspect COVE-L layout plans, scan splits, page clusters, zero-copy maps, and fast metadata indexes.
- **cove-runtime-inspect:** Inspect runtime compatibility hints and registry bindings without treating them as logical file authority.
- **cove-coverage-inspect:** Inspect coverage providers, coverage sets, predicate forms, proof strength, coverage degree, tightness degree, and fallback policy.
- **cove-build-covi:** Build or refresh COVE-I secondary index artifacts for selected columns, object paths, semantic dimensions, or projection fragments.
- **cove-index-inspect:** Inspect `.covi` index roots, capabilities, validity, index-only answers, and coverage mappings.
- **cove-cache-inspect:** Inspect local COVE-CACHE entries, invalidation state, predicate containment, and observed coverage costs.

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

### 80.3A Coverage-Centred Benchmark Reporting

Coverage-aware benchmark reports SHOULD include metrics that show *why* work was avoided, not only final wall-clock time.

**Coverage-level benchmark groups SHOULD include:**
- min/max coverage pruning;
- dictionary/FileCode coverage pruning;
- Bloom/no-false-negative exclusion;
- exact set and inverted-morsel coverage;
- COVE-I global index lookup;
- COVE-CACHE coverage-cache hit and miss planning;
- semantic/dimensional bucket coverage;
- index-only count/min/max/distinct/existence answers;
- object-store many-file coverage planning.

**Coverage metrics SHOULD include:**
- bytes read;
- object-store requests;
- fragments considered;
- fragments in coverage set;
- rows decoded;
- rows materialised;
- coverage degree;
- tightness degree;
- index lookup cost;
- cache hit rate;
- decode time;
- materialisation time;
- full-scan fallback frequency.

A benchmark MUST NOT claim format-level superiority when the result depends on optional COVE-I, COVX, COVE-CACHE, engine-native kernels, hardware acceleration, or zero-copy export unless that dependency is explicitly disclosed and separately measured.

### 80.4 Dataset and Benchmark Corpus

**Recommended corpora:**

- **synthetic-numeric:** numeric full scan and range predicates.
- **synthetic-categorical:** low/medium-cardinality FileCode workloads.
- **synthetic-wide:** hundreds/thousands of columns with small projections.
- **synthetic-point:** lookup-heavy high-cardinality IDs.
- **synthetic-composite:** multi-column predicates and composite pruning.
- **synthetic-coverage:** coverage-set, tightness, coverage-degree, and do-no-harm planning workloads.
- **synthetic-index-only:** exact metadata/index-only count, min, max, distinct-count, and existence checks.
- **synthetic-cache:** repeated predicate-containment workloads with cache hit, miss, and invalidation cases.
- **synthetic-dimensional:** object/dimensional path and bucket queries over sparse or nested data.
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
- Benchmarks SHOULD separate file-size, conversion cost, cold planning latency, warm planning latency, scan CPU, decompression CPU, materialisation time, object requests, bytes read, rows decoded, rows materialised, coverage degree, tightness degree, coverage-provider lookup cost, index build cost, cache hit rate, and end-to-end query latency.
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
- COVE-COVERAGE proof-kind, proof-strength, coverage-granularity, and predicate-form registries,
- COVE-I index-kind and index-capability registries,
- COVE-CACHE compatibility and invalidation registry,
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

## 80.6 Preservation Checklist for Reviewers

This combined specification was produced by retaining the original COVE v1 value and layering the v2 recommendations without replacing the core concept. Reviewers SHOULD verify that no future edit removes these preserved capabilities unless an explicit design decision records the replacement.

| Original value | Preserved in this draft |
| --- | --- |
| Immutable `.cove` files | COVE-Core invariants and durable replace rules. |
| Engine-neutral table scans | COVE-T baseline and starter interoperability subset. |
| File-local FileCodes | Core concepts, dictionary rules, and trust rules. |
| Engine-local ExecutionCodes | COVE-E and COVE-H, explicitly non-authoritative. |
| Null bitmap polarity `1 = null` | Core null semantics and Arrow conversion rules. |
| Morsel-aligned scans | COVE-T segments, morsels, predicate bitmaps, and late materialisation. |
| Predicate proof metadata | Zone stats, exact sets, blooms, composition rules, and conservative pushdown. |
| Archive acceleration | COVE-A, COVX, COVM, lookup indexes, synopses, composite zones, Top-N. |
| Object-temporal profile | COVE-O object catalogs, temporal segments, deltas, baselines, snapshots, tombstones, trust chains. |
| Semantic mapping | COVE-MAP artifact framing, source catalogs, row semantics, identity rules, associations, evidence, projections. |
| Harbor execution value | COVE-H named registration, separate from COVE-Core. |
| Arrow/lakehouse compatibility | COVE-Interop sections without making Arrow IPC or a table protocol authoritative. |
| Security/privacy boundaries | Redaction, digest, trust, privacy, sensitive index guidance, no v2 encryption claims. |
| Public conformance | Conformance levels, vectors, utilities, benchmark corpus, governance artifacts. |
| New v2 codec/layout/runtime value | COVE-CX, COVE-L, COVE-R added as subordinate optional standards. |
| Full-detail preservation | Split documents and starter subsets are implementation/conformance boundaries, not micro-spec replacements. |

## 81. Summary of v2 Design Decisions

**COVE v2 chooses:**

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
- **COVE-COVERAGE:** over vague pruning hints, so conservative coverage sets and proof strength are explicit.
- **COVE-I secondary indexes:** over mandatory global indexes, so value/path/dimensional indexes remain optional and snapshot-bound.
- **COVE-CACHE runtime coverage caches:** over persisted mutable file state, so predicate containment and coverage reuse remain engine-local and non-authoritative.
- **COVM manifests:** over opening every archive file for planning.
- **Extension registry:** so custom logical types, indexes, synopses, encodings, and engine profiles are safe, discoverable, and either ignorable or required.
- **Arrow interop:** so COVE-T is useful without Harbor.
- **Lakehouse compatibility:** so COVE files can live inside existing catalog/table ecosystems.
- **No COVE table protocol in v2:** over duplicating Iceberg/Delta/Hudi-style ACID catalog responsibilities inside the file spec.
- **External visibility overlays:** so delete vectors and table snapshots can be applied safely without changing immutable COVE file semantics.
- **Binary section directories:** over JSON-authoritative metadata.
- **Digest manifests:** over CRC-only archive integrity.
- **Self-contained object reconstruction:** over mandatory cross-file prev_ref.
- **WORM durable replace:** over in-place mutation.
- **Extension-gated vectors, tensors, semantic JSON, encryption, and advanced indexes:** over adding immature workload-specific semantics to COVE-Core v2.
- **COVE-MAP as an optional conversion/projection profile:** over embedding multi-source identity resolution, business-object mapping, source reconciliation, association readback, or object-to-table projection semantics into COVE-Core or COVE-T.
- **Deterministic multi-column semantic join keys:** over probabilistic or hidden matching for canonical object identity.

**The final shape is:**

- **COVE-Core:** immutable binary foundation.
- **COVE-T:** engine-neutral table scan format.
- **COVE-A:** queryable archive acceleration profile.
- **COVE-COVERAGE:** conservative coverage semantics and proof vocabulary.
- **COVE-I:** optional secondary index artifact profile.
- **COVE-CACHE:** optional runtime/local predicate coverage cache guidance, not canonical artifact truth.
- **COVE-E:** universal engine execution/mount profile.
- **COVE-H:** optional Harbor leased-code implementation of COVE-E.
- **COVE-O:** optional object-temporal extension profile.
- **COVE-MAP:** optional deterministic semantic mapping and multi-source object-conversion profile.
- **COVX:** optional rebuildable accelerator sidecar.
- **COVM:** optional multi-file dataset manifest.
This gives Cove Format a neutral public identity, a strict portable decode path, rich queryable archive acceleration, a universal execution-profile mechanism, and an optional path from fragmented source data into object-based COVE while allowing named engine fast paths such as COVE-H without making them dependencies of the core format.



### 81.1 Coverage-Centred v2 Philosophy

COVE v2 makes coverage the conceptual centre of acceleration. Compression, codecs, indexes, sidecars, layout plans, zero-copy export, late materialisation, runtime registries, semantic maps, and hardware-neutral kernels are all valuable, but they cohere only when the reader can tell which fragments are sufficient for a predicate and why.

**Final design principle:**

COVE stores immutable logical values in explicitly declared physical encodings. Optional metadata and sidecars may conservatively prove which fragments are sufficient for a predicate, describe how to evaluate predicates against encoded data, and expose acceleration paths. All such acceleration is advisory unless explicitly required by an extension or requested operation, and ignoring it must preserve logical correctness.

**Public positioning rule:**
COVE MUST NOT claim universal superiority over Parquet, ORC, Arrow IPC, Iceberg, Delta, Hudi, DuckLake, or any particular engine. The credible claim is narrower and stronger: COVE makes acceleration explicit, portable, optional, coverage-aware, and safe for selective, dimensional, indexed, cached, late-materialised, or object/archive workloads.

## 82. Summary of v2 Additions over v1

COVE v2 keeps the v1 identity and adds only the mechanisms that improve performance and implementation maturity without turning COVE into a layout-tree clone.

**COVE v2 adds:**
- `COV2`, `CV2F`, `CVX2`, `CVM2`, and `CMP2` major-version artifact identifiers;
- widened header with bootstrap pointers for extended features, profile capabilities, and fast metadata;
- extended feature set section;
- COVE-CX registered codec-extension profile;
- registered encoding page envelope;
- codec/kernel capability binding;
- COVE-L layout-plan profile;
- scan split index;
- page cluster directory;
- zero-copy buffer map;
- fast metadata index;
- COVE-R explicit runtime registry/session guidance;
- COVE-COVERAGE coverage provider, coverage set, predicate normal-form, and plan-candidate structures;
- COVE-I `.covi` secondary index artifact framing, index roots, and index-only capabilities;
- COVE-CACHE runtime/local cache guidance and invalidation rules;
- stronger registry discipline for extensions and runtime bindings;
- conformance vectors for registered codecs, fallback payloads, layout plans, split indexes, zero-copy maps, fast metadata, and runtime hint behaviour.

**COVE v2 preserves:**
- immutable write-once-read-many files;
- explicit table/object catalog authority;
- file-local FileCode semantics;
- portable canonical logical values;
- structural null bitmaps;
- predicate-proof pruning rules;
- COVE-A acceleration as optional and semantics-preserving;
- COVE-E and COVE-H as execution mappings, not logical truth;
- COVE-O self-contained reconstruction;
- COVE-MAP deterministic object/association/provenance semantics;
- COVX and COVM as optional sidecar/manifest artifacts;
- ignorable-or-required extension discipline.

**COVE v2 explicitly does not adopt:**
- dtype-only schema authority;
- generic runtime layout trees as the file's logical data model;
- unchecked plugin IDs as wire-format semantics;
- advisory statistics as proof;
- mandatory dependency on any single engine, library, runtime, or vendor;
- lossy specialised encodings as core COVE semantics.

The v2 design therefore lets COVE become more modern—registered codecs, lazy planning, zero-copy metadata, and implementation registries—while remaining recognisably COVE: a canonical, immutable, queryable archive format with portable values, explicit schema, proof-carrying metadata, and optional engine-local execution mappings.
