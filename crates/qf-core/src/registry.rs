//! Quay Format (QF) v1.0 — spec registries used by validators and tools.
//!
//! This module is the first piece of the implementation ledger described by the
//! reference plan: feature bits (Spec §11), section kinds (Spec §14), writer
//! profiles (Spec §71), recovery behavior (Spec §73), compatibility rules
//! (Spec §76), and error codes (Spec §75) are available as structured data
//! instead of duplicated strings in CLI tools.

use crate::constants::*;

/// A registered v1 feature bit from Spec §11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureInfo {
    pub bit: u64,
    pub name: &'static str,
    pub spec_section: &'static str,
    pub description: &'static str,
}

/// A registered v1 section kind from Spec §14.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionInfo {
    pub kind: SectionKind,
    pub id: u16,
    pub wire_name: &'static str,
    pub profiles: &'static [PrimaryProfile],
    pub required_feature: Option<u64>,
    pub spec_section: &'static str,
    pub description: &'static str,
}

/// A registered v1 error code from Spec §75.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCodeInfo {
    pub code: &'static str,
    pub spec_section: &'static str,
    pub meaning: &'static str,
}

/// A registered writer profile tier from Spec §71.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriterProfileInfo {
    pub name: &'static str,
    pub spec_section: &'static str,
    pub summary: &'static str,
    pub requirements: &'static [&'static str],
}

/// A recovery/failure behavior row from Spec §73.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryBehaviorInfo {
    pub condition: &'static str,
    pub spec_section: &'static str,
    pub default_behavior: &'static str,
}

/// A compatibility rule from Spec §76.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatibilityRuleInfo {
    pub key: &'static str,
    pub spec_section: &'static str,
    pub rule: &'static str,
    pub examples: &'static [&'static str],
}

/// All feature bits assigned by Spec §11.
pub const FEATURE_REGISTRY: &[FeatureInfo] = &[
    FeatureInfo {
        bit: FEATURE_OBJECT_PROFILE,
        name: "FEATURE_OBJECT_PROFILE",
        spec_section: "Spec §11",
        description: "File contains QF-O sections.",
    },
    FeatureInfo {
        bit: FEATURE_TABLE_PROFILE,
        name: "FEATURE_TABLE_PROFILE",
        spec_section: "Spec §11",
        description: "File contains QF-T sections.",
    },
    FeatureInfo {
        bit: FEATURE_ARCHIVE_PROFILE,
        name: "FEATURE_ARCHIVE_PROFILE",
        spec_section: "Spec §11",
        description: "File contains QF-A sections.",
    },
    FeatureInfo {
        bit: FEATURE_ENGINE_PROFILE,
        name: "FEATURE_ENGINE_PROFILE",
        spec_section: "Spec §11",
        description: "File contains QF-E sections.",
    },
    FeatureInfo {
        bit: FEATURE_HARBOR_PROFILE,
        name: "FEATURE_HARBOR_PROFILE",
        spec_section: "Spec §11",
        description: "File contains QF-H Harbor-specific metadata.",
    },
    FeatureInfo {
        bit: FEATURE_FILE_DICTIONARY,
        name: "FEATURE_FILE_DICTIONARY",
        spec_section: "Spec §11",
        description: "File uses FileCode dictionary.",
    },
    FeatureInfo {
        bit: FEATURE_NUMCODES,
        name: "FEATURE_NUMCODES",
        spec_section: "Spec §11",
        description: "File contains NumCode columns.",
    },
    FeatureInfo {
        bit: FEATURE_COLUMN_DOMAINS,
        name: "FEATURE_COLUMN_DOMAINS",
        spec_section: "Spec §11",
        description: "File contains ColumnDomain sections.",
    },
    FeatureInfo {
        bit: FEATURE_EXACT_SETS,
        name: "FEATURE_EXACT_SETS",
        spec_section: "Spec §11",
        description: "File contains exact set indexes.",
    },
    FeatureInfo {
        bit: FEATURE_BLOOM_FILTERS,
        name: "FEATURE_BLOOM_FILTERS",
        spec_section: "Spec §11",
        description: "File contains bloom indexes.",
    },
    FeatureInfo {
        bit: FEATURE_INVERTED_INDEXES,
        name: "FEATURE_INVERTED_INDEXES",
        spec_section: "Spec §11",
        description: "File contains inverted morsel indexes.",
    },
    FeatureInfo {
        bit: FEATURE_LOOKUP_INDEXES,
        name: "FEATURE_LOOKUP_INDEXES",
        spec_section: "Spec §11",
        description: "File contains point lookup indexes.",
    },
    FeatureInfo {
        bit: FEATURE_AGGREGATE_SYNOPSES,
        name: "FEATURE_AGGREGATE_SYNOPSES",
        spec_section: "Spec §11",
        description: "File contains aggregate synopsis sections.",
    },
    FeatureInfo {
        bit: FEATURE_COMPOSITE_ZONES,
        name: "FEATURE_COMPOSITE_ZONES",
        spec_section: "Spec §11",
        description: "File contains composite zone indexes.",
    },
    FeatureInfo {
        bit: FEATURE_TOPN_SUMMARIES,
        name: "FEATURE_TOPN_SUMMARIES",
        spec_section: "Spec §11",
        description: "File contains Top-N zone summaries.",
    },
    FeatureInfo {
        bit: FEATURE_TRUST_CHAIN,
        name: "FEATURE_TRUST_CHAIN",
        spec_section: "Spec §11",
        description: "File contains trust-chain data.",
    },
    FeatureInfo {
        bit: FEATURE_REDACTIONS,
        name: "FEATURE_REDACTIONS",
        spec_section: "Spec §11",
        description: "File contains redacted values or audit references.",
    },
    FeatureInfo {
        bit: FEATURE_NESTED_COLUMNS,
        name: "FEATURE_NESTED_COLUMNS",
        spec_section: "Spec §11",
        description: "File contains list/struct/map columns.",
    },
    FeatureInfo {
        bit: FEATURE_DIGEST_MANIFEST,
        name: "FEATURE_DIGEST_MANIFEST",
        spec_section: "Spec §11",
        description: "File contains cryptographic digest manifest.",
    },
    FeatureInfo {
        bit: FEATURE_ARROW_INTEROP_HINTS,
        name: "FEATURE_ARROW_INTEROP_HINTS",
        spec_section: "Spec §11",
        description: "File contains Arrow mapping hints.",
    },
    FeatureInfo {
        bit: FEATURE_LAKEHOUSE_HINTS,
        name: "FEATURE_LAKEHOUSE_HINTS",
        spec_section: "Spec §11",
        description: "File contains lakehouse integration hints.",
    },
    FeatureInfo {
        bit: FEATURE_EXTENSION_REGISTRY,
        name: "FEATURE_EXTENSION_REGISTRY",
        spec_section: "Spec §11",
        description: "File contains extension registry.",
    },
    FeatureInfo {
        bit: FEATURE_CODEC_LZ4,
        name: "FEATURE_CODEC_LZ4",
        spec_section: "Spec §11",
        description: "File uses LZ4-compressed payloads.",
    },
    FeatureInfo {
        bit: FEATURE_CODEC_ZSTD,
        name: "FEATURE_CODEC_ZSTD",
        spec_section: "Spec §11",
        description: "File uses Zstd-compressed payloads.",
    },
];

/// All section kinds assigned by Spec §14.
pub const SECTION_REGISTRY: &[SectionInfo] = &[
    SectionInfo {
        kind: SectionKind::FileDictionaryIndex,
        id: 1,
        wire_name: "FILE_DICTIONARY_INDEX",
        profiles: &[PrimaryProfile::Mixed],
        required_feature: Some(FEATURE_FILE_DICTIONARY),
        spec_section: "Spec §14",
        description: "Fixed dictionary index entries.",
    },
    SectionInfo {
        kind: SectionKind::FileDictionaryPayload,
        id: 2,
        wire_name: "FILE_DICTIONARY_PAYLOAD",
        profiles: &[PrimaryProfile::Mixed],
        required_feature: Some(FEATURE_FILE_DICTIONARY),
        spec_section: "Spec §14",
        description: "Variable or large value payloads.",
    },
    SectionInfo {
        kind: SectionKind::CollationRegistry,
        id: 3,
        wire_name: "COLLATION_REGISTRY",
        profiles: &[PrimaryProfile::Mixed],
        required_feature: None,
        spec_section: "Spec §14",
        description: "Collation and canonicalisation registry.",
    },
    SectionInfo {
        kind: SectionKind::DigestManifest,
        id: 4,
        wire_name: "DIGEST_MANIFEST",
        profiles: &[PrimaryProfile::Mixed],
        required_feature: Some(FEATURE_DIGEST_MANIFEST),
        spec_section: "Spec §14",
        description: "Cryptographic digests.",
    },
    SectionInfo {
        kind: SectionKind::RedactionManifest,
        id: 5,
        wire_name: "REDACTION_MANIFEST",
        profiles: &[PrimaryProfile::Mixed],
        required_feature: Some(FEATURE_REDACTIONS),
        spec_section: "Spec §14",
        description: "Redaction audit metadata.",
    },
    SectionInfo {
        kind: SectionKind::ArrowInteropHints,
        id: 6,
        wire_name: "ARROW_INTEROP_HINTS",
        profiles: &[PrimaryProfile::Mixed],
        required_feature: Some(FEATURE_ARROW_INTEROP_HINTS),
        spec_section: "Spec §14",
        description: "Arrow mapping hints.",
    },
    SectionInfo {
        kind: SectionKind::LakehouseHints,
        id: 7,
        wire_name: "LAKEHOUSE_HINTS",
        profiles: &[PrimaryProfile::Mixed],
        required_feature: Some(FEATURE_LAKEHOUSE_HINTS),
        spec_section: "Spec §14",
        description: "Iceberg, Delta, Hudi, or catalog hints.",
    },
    SectionInfo {
        kind: SectionKind::ExtensionRegistry,
        id: 8,
        wire_name: "EXTENSION_REGISTRY",
        profiles: &[PrimaryProfile::Mixed],
        required_feature: Some(FEATURE_EXTENSION_REGISTRY),
        spec_section: "Spec §14",
        description: "Registered custom extensions.",
    },
    SectionInfo {
        kind: SectionKind::ProfileCapabilityMatrix,
        id: 9,
        wire_name: "PROFILE_CAPABILITY_MATRIX",
        profiles: &[PrimaryProfile::Mixed],
        required_feature: None,
        spec_section: "Spec §14",
        description: "Declared profile support.",
    },
    SectionInfo {
        kind: SectionKind::TableCatalog,
        id: 10,
        wire_name: "TABLE_CATALOG",
        profiles: &[PrimaryProfile::TableScan],
        required_feature: Some(FEATURE_TABLE_PROFILE),
        spec_section: "Spec §14",
        description: "Table schemas.",
    },
    SectionInfo {
        kind: SectionKind::TableSegmentIndex,
        id: 11,
        wire_name: "TABLE_SEGMENT_INDEX",
        profiles: &[PrimaryProfile::TableScan],
        required_feature: Some(FEATURE_TABLE_PROFILE),
        spec_section: "Spec §14",
        description: "Segment locators and row ranges.",
    },
    SectionInfo {
        kind: SectionKind::TableSegmentData,
        id: 12,
        wire_name: "TABLE_SEGMENT_DATA",
        profiles: &[PrimaryProfile::TableScan],
        required_feature: Some(FEATURE_TABLE_PROFILE),
        spec_section: "Spec §14",
        description: "Table segment payloads.",
    },
    SectionInfo {
        kind: SectionKind::ColumnDomain,
        id: 13,
        wire_name: "COLUMN_DOMAIN",
        profiles: &[PrimaryProfile::TableScan],
        required_feature: Some(FEATURE_COLUMN_DOMAINS),
        spec_section: "Spec §14",
        description: "Logical ordering for FileCodes.",
    },
    SectionInfo {
        kind: SectionKind::ZoneStats,
        id: 14,
        wire_name: "ZONE_STATS",
        profiles: &[PrimaryProfile::TableScan],
        required_feature: Some(FEATURE_TABLE_PROFILE),
        spec_section: "Spec §14",
        description: "Segment, morsel, and page statistics.",
    },
    SectionInfo {
        kind: SectionKind::ExactSetIndex,
        id: 15,
        wire_name: "EXACT_SET_INDEX",
        profiles: &[
            PrimaryProfile::TableScan,
            PrimaryProfile::ArchiveAcceleration,
        ],
        required_feature: Some(FEATURE_EXACT_SETS),
        spec_section: "Spec §14",
        description: "Exact value-set indexes.",
    },
    SectionInfo {
        kind: SectionKind::BloomIndex,
        id: 16,
        wire_name: "BLOOM_INDEX",
        profiles: &[
            PrimaryProfile::TableScan,
            PrimaryProfile::ArchiveAcceleration,
        ],
        required_feature: Some(FEATURE_BLOOM_FILTERS),
        spec_section: "Spec §14",
        description: "Bloom filters.",
    },
    SectionInfo {
        kind: SectionKind::InvertedMorselIndex,
        id: 17,
        wire_name: "INVERTED_MORSEL_INDEX",
        profiles: &[
            PrimaryProfile::TableScan,
            PrimaryProfile::ArchiveAcceleration,
        ],
        required_feature: Some(FEATURE_INVERTED_INDEXES),
        spec_section: "Spec §14",
        description: "Value-to-morsel indexes.",
    },
    SectionInfo {
        kind: SectionKind::LookupIndex,
        id: 18,
        wire_name: "LOOKUP_INDEX",
        profiles: &[PrimaryProfile::ArchiveAcceleration],
        required_feature: Some(FEATURE_LOOKUP_INDEXES),
        spec_section: "Spec §14",
        description: "Point lookup indexes.",
    },
    SectionInfo {
        kind: SectionKind::AggregateSynopsis,
        id: 19,
        wire_name: "AGGREGATE_SYNOPSIS",
        profiles: &[PrimaryProfile::ArchiveAcceleration],
        required_feature: Some(FEATURE_AGGREGATE_SYNOPSES),
        spec_section: "Spec §14",
        description: "Counts, histograms, sketches.",
    },
    SectionInfo {
        kind: SectionKind::CompositeZoneIndex,
        id: 20,
        wire_name: "COMPOSITE_ZONE_INDEX",
        profiles: &[PrimaryProfile::ArchiveAcceleration],
        required_feature: Some(FEATURE_COMPOSITE_ZONES),
        spec_section: "Spec §14",
        description: "Multi-column pruning metadata.",
    },
    SectionInfo {
        kind: SectionKind::TopNZoneSummary,
        id: 21,
        wire_name: "TOPN_ZONE_SUMMARY",
        profiles: &[PrimaryProfile::ArchiveAcceleration],
        required_feature: Some(FEATURE_TOPN_SUMMARIES),
        spec_section: "Spec §14",
        description: "Top/bottom zone summaries.",
    },
    SectionInfo {
        kind: SectionKind::KernelCapabilities,
        id: 22,
        wire_name: "KERNEL_CAPABILITIES",
        profiles: &[
            PrimaryProfile::TableScan,
            PrimaryProfile::ArchiveAcceleration,
        ],
        required_feature: None,
        spec_section: "Spec §14",
        description: "Encoded-kernel capability metadata.",
    },
    SectionInfo {
        kind: SectionKind::EngineProfileRegistry,
        id: 30,
        wire_name: "ENGINE_PROFILE_REGISTRY",
        profiles: &[PrimaryProfile::EngineExecution],
        required_feature: Some(FEATURE_ENGINE_PROFILE),
        spec_section: "Spec §14",
        description: "Registered engine execution profiles.",
    },
    SectionInfo {
        kind: SectionKind::ExecutionCodeDescriptor,
        id: 31,
        wire_name: "EXECUTION_CODE_DESCRIPTOR",
        profiles: &[PrimaryProfile::EngineExecution],
        required_feature: Some(FEATURE_ENGINE_PROFILE),
        spec_section: "Spec §14",
        description: "ExecutionCode description.",
    },
    SectionInfo {
        kind: SectionKind::ExecutionScopeDescriptor,
        id: 32,
        wire_name: "EXECUTION_SCOPE_DESCRIPTOR",
        profiles: &[PrimaryProfile::EngineExecution],
        required_feature: Some(FEATURE_ENGINE_PROFILE),
        spec_section: "Spec §14",
        description: "Execution scope metadata.",
    },
    SectionInfo {
        kind: SectionKind::CodeSpaceDescriptor,
        id: 33,
        wire_name: "CODE_SPACE_DESCRIPTOR",
        profiles: &[PrimaryProfile::EngineExecution],
        required_feature: Some(FEATURE_ENGINE_PROFILE),
        spec_section: "Spec §14",
        description: "Code-space metadata.",
    },
    SectionInfo {
        kind: SectionKind::EngineMountPolicy,
        id: 34,
        wire_name: "ENGINE_MOUNT_POLICY",
        profiles: &[PrimaryProfile::EngineExecution],
        required_feature: Some(FEATURE_ENGINE_PROFILE),
        spec_section: "Spec §14",
        description: "Generic mount/execution mapping policy.",
    },
    SectionInfo {
        kind: SectionKind::ObjectTypeCatalog,
        id: 40,
        wire_name: "OBJECT_TYPE_CATALOG",
        profiles: &[PrimaryProfile::ObjectTemporal],
        required_feature: Some(FEATURE_OBJECT_PROFILE),
        spec_section: "Spec §14",
        description: "Object/property catalog.",
    },
    SectionInfo {
        kind: SectionKind::TemporalSegmentIndex,
        id: 41,
        wire_name: "TEMPORAL_SEGMENT_INDEX",
        profiles: &[PrimaryProfile::ObjectTemporal],
        required_feature: Some(FEATURE_OBJECT_PROFILE),
        spec_section: "Spec §14",
        description: "Temporal segment locators.",
    },
    SectionInfo {
        kind: SectionKind::TemporalSegmentData,
        id: 42,
        wire_name: "TEMPORAL_SEGMENT_DATA",
        profiles: &[PrimaryProfile::ObjectTemporal],
        required_feature: Some(FEATURE_OBJECT_PROFILE),
        spec_section: "Spec §14",
        description: "Temporal segment payloads.",
    },
    SectionInfo {
        kind: SectionKind::TemporalBloomIndex,
        id: 43,
        wire_name: "TEMPORAL_BLOOM_INDEX",
        profiles: &[PrimaryProfile::ObjectTemporal],
        required_feature: Some(FEATURE_OBJECT_PROFILE),
        spec_section: "Spec §14",
        description: "Scope, branch, GOID, and time bloom filters.",
    },
    SectionInfo {
        kind: SectionKind::TrustManifest,
        id: 44,
        wire_name: "TRUST_MANIFEST",
        profiles: &[PrimaryProfile::ObjectTemporal],
        required_feature: Some(FEATURE_TRUST_CHAIN),
        spec_section: "Spec §14",
        description: "Trust-chain metadata.",
    },
    SectionInfo {
        kind: SectionKind::HarborMountHints,
        id: 50,
        wire_name: "HARBOR_MOUNT_HINTS",
        profiles: &[PrimaryProfile::HarborExecution],
        required_feature: Some(FEATURE_HARBOR_PROFILE),
        spec_section: "Spec §14",
        description: "Harbor-specific lease/mount hints.",
    },
    SectionInfo {
        kind: SectionKind::VendorExtension,
        id: 255,
        wire_name: "VENDOR_EXTENSION",
        profiles: &[PrimaryProfile::Mixed],
        required_feature: None,
        spec_section: "Spec §14",
        description: "Reserved extension section.",
    },
];

/// All error codes assigned by Spec §75.
pub const ERROR_CODE_REGISTRY: &[ErrorCodeInfo] = &[
    ErrorCodeInfo {
        code: "QF_E_BAD_MAGIC",
        spec_section: "Spec §75",
        meaning: "Missing or invalid magic.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_VERSION",
        spec_section: "Spec §75",
        meaning: "Unsupported QF version.",
    },
    ErrorCodeInfo {
        code: "QF_E_UNKNOWN_REQUIRED_FEATURE",
        spec_section: "Spec §75",
        meaning: "Unknown required feature bit set.",
    },
    ErrorCodeInfo {
        code: "QF_E_CHECKSUM_MISMATCH",
        spec_section: "Spec §75",
        meaning: "CRC32C mismatch.",
    },
    ErrorCodeInfo {
        code: "QF_E_DIGEST_MISMATCH",
        spec_section: "Spec §75",
        meaning: "Cryptographic digest mismatch.",
    },
    ErrorCodeInfo {
        code: "QF_E_OFFSET_RANGE",
        spec_section: "Spec §75",
        meaning: "Offset, length, or count exceeds file bounds.",
    },
    ErrorCodeInfo {
        code: "QF_E_ARITH_OVERFLOW",
        spec_section: "Spec §75",
        meaning: "Offset/count/size arithmetic overflow.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_SECTION",
        spec_section: "Spec §75",
        meaning: "Section malformed or invalid.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_SCHEMA",
        spec_section: "Spec §75",
        meaning: "Catalog/schema malformed.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_LOGICAL_PHYSICAL_PAIR",
        spec_section: "Spec §75",
        meaning: "Logical type incompatible with physical kind.",
    },
    ErrorCodeInfo {
        code: "QF_E_DICT_MISS",
        spec_section: "Spec §75",
        meaning: "FileCode missing from dictionary.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_FILECODE",
        spec_section: "Spec §75",
        meaning: "FileCode outside dictionary range.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_NUMCODE",
        spec_section: "Spec §75",
        meaning: "NumCode invalid for declared logical type.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_DOMAIN",
        spec_section: "Spec §75",
        meaning: "ColumnDomain invalid.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_STATS",
        spec_section: "Spec §75",
        meaning: "Statistics invalid or unsafe.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_INDEX",
        spec_section: "Spec §75",
        meaning: "Optional index invalid or corrupt.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_EXTENSION",
        spec_section: "Spec §75",
        meaning: "Extension invalid or required extension unsupported.",
    },
    ErrorCodeInfo {
        code: "QF_E_BAD_ENGINE_PROFILE",
        spec_section: "Spec §75",
        meaning: "Engine profile invalid or unsupported when required.",
    },
    ErrorCodeInfo {
        code: "QF_E_EXECUTION_CODE_MAP",
        spec_section: "Spec §75",
        meaning: "Engine-local code mapping failed.",
    },
    ErrorCodeInfo {
        code: "QF_E_HARBOR_MOUNT_LEASE",
        spec_section: "Spec §75",
        meaning: "Harbor code lease resolution failed.",
    },
    ErrorCodeInfo {
        code: "QF_E_REF_INVALID",
        spec_section: "Spec §75",
        meaning: "QF-O prev_ref invalid.",
    },
    ErrorCodeInfo {
        code: "QF_E_NOT_SELF_CONTAINED",
        spec_section: "Spec §75",
        meaning: "QF-O chain lacks baseline/snapshot/full chain.",
    },
    ErrorCodeInfo {
        code: "QF_E_SEGMENT_CORRUPT",
        spec_section: "Spec §75",
        meaning: "Segment structure invalid.",
    },
    ErrorCodeInfo {
        code: "QF_E_PAGE_CORRUPT",
        spec_section: "Spec §75",
        meaning: "Page structure invalid.",
    },
    ErrorCodeInfo {
        code: "QF_E_REDACTION_POLICY",
        spec_section: "Spec §75",
        meaning: "Redacted value cannot be surfaced under current policy.",
    },
    ErrorCodeInfo {
        code: "QF_E_SIDECAR_STALE",
        spec_section: "Spec §75",
        meaning: "QFX/QFM sidecar does not match referenced QF.",
    },
];

const WRITER_PROFILE_CORE_MINIMAL: &[&str] = &[
    "valid header",
    "valid postscript",
    "valid footer",
    "section directory",
    "file dictionary if FileCode columns exist",
    "valid checksums",
    "valid logical/physical typing",
    "valid null bitmaps",
];

const WRITER_PROFILE_TABLE_MINIMAL: &[&str] = &[
    "all QF-Core Minimal requirements",
    "table catalog",
    "table segment index",
    "table segment data",
    "column page indexes",
    "page checksums",
    "null counts",
    "segment/morsel row counts",
];

const WRITER_PROFILE_TABLE_SCAN: &[&str] = &[
    "all QF-T Minimal requirements",
    "FileCode columns for repeated strings/categories",
    "NumCode columns for numeric/timestamp data",
    "morsel_row_count = 4096",
    "ColumnDomain for comparable FileCode columns",
    "morsel-level zone stats",
    "predicate proof support",
    "exact sets for low/medium-cardinality columns",
    "bloom filters for high-cardinality equality columns",
    "local codebook encoding for FileCode pages",
    "frame-of-reference or delta encoding for NumCode pages",
    "LZ4 for hot scan pages",
];

const WRITER_PROFILE_ARCHIVE: &[&str] = &[
    "all QF-T Scan Profile features",
    "QFM manifest",
    "digest manifest",
    "FileCode histograms",
    "lookup indexes",
    "composite zone indexes",
    "Top-N summaries for ordered hot columns",
    "optional QFX sidecar",
    "Zstd for cold page payloads where scan latency permits",
];

const WRITER_PROFILE_ENGINE: &[&str] = &[
    "engine profile registry",
    "execution code descriptor",
    "execution scope descriptor",
    "code-space descriptor",
    "engine mount policy",
    "FileCode -> ExecutionCode mapping strategy",
    "optional execution-code cache metadata",
    "reverse lookup policy",
];

const WRITER_PROFILE_HARBOR: &[&str] = &[
    "all QF-T Scan Profile features",
    "QF-E engine execution profile",
    "FileCode -> Harbor EngineCode mount map",
    "Harbor lease epoch tracking",
    "Harbor code-space descriptor",
    "Harbor mount cache key",
    "direct Harbor vector materialisation",
    "optional QF-O object-temporal support",
];

const WRITER_PROFILE_OBJECT_CHECKPOINT: &[&str] = &[
    "object type catalog",
    "temporal segment index",
    "self-contained baselines/snapshots",
    "FileCode/NumCode property columns",
    "temporal blooms",
    "trust chain if compliance requires",
    "redaction manifest if redactions are present",
];

/// All writer profile tiers from Spec §71.
pub const WRITER_PROFILE_REGISTRY: &[WriterProfileInfo] = &[
    WriterProfileInfo {
        name: "QF-Core Minimal Profile",
        spec_section: "Spec §71.1",
        summary: "Minimum valid QF-Core writer output.",
        requirements: WRITER_PROFILE_CORE_MINIMAL,
    },
    WriterProfileInfo {
        name: "QF-T Minimal Table Profile",
        spec_section: "Spec §71.2",
        summary: "Minimum valid table-scan writer output.",
        requirements: WRITER_PROFILE_TABLE_MINIMAL,
    },
    WriterProfileInfo {
        name: "QF-T Scan Profile",
        spec_section: "Spec §71.3",
        summary: "Recommended default table-scan writer profile.",
        requirements: WRITER_PROFILE_TABLE_SCAN,
    },
    WriterProfileInfo {
        name: "QF-A Archive Acceleration Profile",
        spec_section: "Spec §71.4",
        summary: "Recommended archive acceleration writer profile.",
        requirements: WRITER_PROFILE_ARCHIVE,
    },
    WriterProfileInfo {
        name: "QF-E Engine Execution Profile",
        spec_section: "Spec §71.5",
        summary: "Recommended engine execution profile writer output.",
        requirements: WRITER_PROFILE_ENGINE,
    },
    WriterProfileInfo {
        name: "QF-H Harbor Profile",
        spec_section: "Spec §71.6",
        summary: "Recommended Harbor-oriented writer profile.",
        requirements: WRITER_PROFILE_HARBOR,
    },
    WriterProfileInfo {
        name: "QF-O Object Checkpoint Profile",
        spec_section: "Spec §71.7",
        summary: "Recommended object state checkpoint writer profile.",
        requirements: WRITER_PROFILE_OBJECT_CHECKPOINT,
    },
];

/// Recovery and failure behavior rows from Spec §73.
pub const RECOVERY_BEHAVIOR_REGISTRY: &[RecoveryBehaviorInfo] = &[
    RecoveryBehaviorInfo {
        condition: "Bad header magic",
        spec_section: "Spec §73",
        default_behavior: "Reject file",
    },
    RecoveryBehaviorInfo {
        condition: "Bad trailing magic",
        spec_section: "Spec §73",
        default_behavior: "Reject file",
    },
    RecoveryBehaviorInfo {
        condition: "Unsupported version",
        spec_section: "Spec §73",
        default_behavior: "Reject file",
    },
    RecoveryBehaviorInfo {
        condition: "Unknown required feature",
        spec_section: "Spec §73",
        default_behavior: "Reject file",
    },
    RecoveryBehaviorInfo {
        condition: "Unknown optional feature",
        spec_section: "Spec §73",
        default_behavior: "Ignore if not needed",
    },
    RecoveryBehaviorInfo {
        condition: "Header checksum mismatch",
        spec_section: "Spec §73",
        default_behavior: "Reject file",
    },
    RecoveryBehaviorInfo {
        condition: "Postscript checksum mismatch",
        spec_section: "Spec §73",
        default_behavior: "Reject file",
    },
    RecoveryBehaviorInfo {
        condition: "Footer CRC mismatch",
        spec_section: "Spec §73",
        default_behavior: "Reject file",
    },
    RecoveryBehaviorInfo {
        condition: "Required section CRC mismatch",
        spec_section: "Spec §73",
        default_behavior: "Reject file",
    },
    RecoveryBehaviorInfo {
        condition: "Optional index CRC mismatch",
        spec_section: "Spec §73",
        default_behavior: "Ignore index and scan",
    },
    RecoveryBehaviorInfo {
        condition: "Bloom corruption",
        spec_section: "Spec §73",
        default_behavior: "Ignore bloom and scan",
    },
    RecoveryBehaviorInfo {
        condition: "Exact set corruption",
        spec_section: "Spec §73",
        default_behavior: "Ignore exact set and scan",
    },
    RecoveryBehaviorInfo {
        condition: "Inverted index corruption",
        spec_section: "Spec §73",
        default_behavior: "Ignore index and scan",
    },
    RecoveryBehaviorInfo {
        condition: "Lookup index corruption",
        spec_section: "Spec §73",
        default_behavior: "Ignore index and scan",
    },
    RecoveryBehaviorInfo {
        condition: "Aggregate synopsis corruption",
        spec_section: "Spec §73",
        default_behavior: "Ignore synopsis unless required by query-only plan",
    },
    RecoveryBehaviorInfo {
        condition: "Composite zone corruption",
        spec_section: "Spec §73",
        default_behavior: "Ignore composite zone and scan",
    },
    RecoveryBehaviorInfo {
        condition: "Top-N summary corruption",
        spec_section: "Spec §73",
        default_behavior: "Ignore summary and scan",
    },
    RecoveryBehaviorInfo {
        condition: "QF-E optional profile corrupt",
        spec_section: "Spec §73",
        default_behavior: "Ignore profile",
    },
    RecoveryBehaviorInfo {
        condition: "QF-E required profile corrupt",
        spec_section: "Spec §73",
        default_behavior: "Reject if needed",
    },
    RecoveryBehaviorInfo {
        condition: "QFX stale/corrupt",
        spec_section: "Spec §73",
        default_behavior: "Ignore QFX",
    },
    RecoveryBehaviorInfo {
        condition: "QFM stale/corrupt",
        spec_section: "Spec §73",
        default_behavior: "Ignore QFM",
    },
    RecoveryBehaviorInfo {
        condition: "Segment checksum mismatch",
        spec_section: "Spec §73",
        default_behavior: "Reject segment; fail read unless explicit best-effort mode",
    },
    RecoveryBehaviorInfo {
        condition: "Page checksum mismatch",
        spec_section: "Spec §73",
        default_behavior: "Reject page; fail read unless explicit best-effort mode",
    },
    RecoveryBehaviorInfo {
        condition: "Invalid FileCode",
        spec_section: "Spec §73",
        default_behavior: "Treat as corruption",
    },
    RecoveryBehaviorInfo {
        condition: "Invalid NumCode/logical type pairing",
        spec_section: "Spec §73",
        default_behavior: "Schema error",
    },
    RecoveryBehaviorInfo {
        condition: "Invalid prev_ref",
        spec_section: "Spec §73",
        default_behavior: "Reject QF-O file",
    },
    RecoveryBehaviorInfo {
        condition: "Unsafe min/max",
        spec_section: "Spec §73",
        default_behavior: "Do not use for skipping",
    },
];

const COMPAT_REQUIRED_FEATURE_EXAMPLES: &[&str] = &[
    "codec needed to decode projected data",
    "nested column support when projected",
    "trust-chain support when verification is requested",
    "engine profile required by requested output mode",
];

const COMPAT_OPTIONAL_FEATURE_EXAMPLES: &[&str] = &[
    "bloom filters",
    "exact sets",
    "lookup indexes",
    "aggregate synopses",
    "Top-N summaries",
    "QFX sidecars",
    "QFM manifests",
    "optional engine profile mappings",
];

/// Compatibility rules from Spec §76.
pub const COMPATIBILITY_REGISTRY: &[CompatibilityRuleInfo] = &[
    CompatibilityRuleInfo {
        key: "supported_major_version",
        spec_section: "Spec §76.1",
        rule: "QF v1 readers support version_major = 1.",
        examples: &[],
    },
    CompatibilityRuleInfo {
        key: "reject_unsupported_major",
        spec_section: "Spec §76.1",
        rule: "Readers MUST reject unsupported major versions.",
        examples: &[],
    },
    CompatibilityRuleInfo {
        key: "accept_safe_newer_minor",
        spec_section: "Spec §76.1",
        rule: "Readers MAY accept newer minor versions if no unknown required features are set.",
        examples: &[],
    },
    CompatibilityRuleInfo {
        key: "required_features",
        spec_section: "Spec §76.2",
        rule: "Required features are needed for correctness.",
        examples: COMPAT_REQUIRED_FEATURE_EXAMPLES,
    },
    CompatibilityRuleInfo {
        key: "optional_features",
        spec_section: "Spec §76.2",
        rule: "Optional features are accelerators or metadata.",
        examples: COMPAT_OPTIONAL_FEATURE_EXAMPLES,
    },
];

/// Return metadata for a feature bit, if it is defined by Spec §11.
pub fn feature_info(bit: u64) -> Option<&'static FeatureInfo> {
    FEATURE_REGISTRY.iter().find(|info| info.bit == bit)
}

/// Return the names of all registered feature bits present in `bits`.
pub fn feature_names(bits: u64) -> Vec<&'static str> {
    FEATURE_REGISTRY
        .iter()
        .filter(|info| bits & info.bit != 0)
        .map(|info| info.name)
        .collect()
}

/// Return metadata for a section kind, if it is defined by Spec §14.
pub fn section_info(kind: SectionKind) -> Option<&'static SectionInfo> {
    SECTION_REGISTRY.iter().find(|info| info.kind == kind)
}

/// Return metadata for an error code, if it is defined by Spec §75.
pub fn error_code_info(code: &str) -> Option<&'static ErrorCodeInfo> {
    ERROR_CODE_REGISTRY.iter().find(|info| info.code == code)
}

/// Return metadata for a writer profile tier from Spec §71.
pub fn writer_profile_info(name: &str) -> Option<&'static WriterProfileInfo> {
    WRITER_PROFILE_REGISTRY
        .iter()
        .find(|info| info.name == name)
}

/// Return the default recovery behavior for a Spec §73 condition.
pub fn recovery_behavior_info(condition: &str) -> Option<&'static RecoveryBehaviorInfo> {
    RECOVERY_BEHAVIOR_REGISTRY
        .iter()
        .find(|info| info.condition == condition)
}

/// Return a compatibility rule from Spec §76.
pub fn compatibility_rule_info(key: &str) -> Option<&'static CompatibilityRuleInfo> {
    COMPATIBILITY_REGISTRY.iter().find(|info| info.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_11_feature_registry_covers_known_feature_mask() {
        let from_registry = FEATURE_REGISTRY
            .iter()
            .fold(0u64, |bits, info| bits | info.bit);
        assert_eq!(from_registry, KNOWN_FEATURE_BITS_MASK);
    }

    #[test]
    fn spec_14_section_registry_covers_every_known_section_kind() {
        for info in SECTION_REGISTRY {
            assert_eq!(SectionKind::from_u16(info.id), Some(info.kind));
            assert_eq!(info.kind as u16, info.id);
        }
        assert_eq!(SECTION_REGISTRY.len(), 34);
        let exact_set = section_info(SectionKind::ExactSetIndex).unwrap();
        assert!(exact_set.profiles.contains(&PrimaryProfile::TableScan));
        assert!(exact_set
            .profiles
            .contains(&PrimaryProfile::ArchiveAcceleration));
    }

    #[test]
    fn spec_75_error_registry_contains_all_v1_codes() {
        assert_eq!(ERROR_CODE_REGISTRY.len(), 26);
        assert!(error_code_info("QF_E_BAD_MAGIC").is_some());
        assert!(error_code_info("QF_E_SIDECAR_STALE").is_some());
    }

    #[test]
    fn spec_71_writer_profile_registry_lists_all_v1_tiers() {
        assert_eq!(WRITER_PROFILE_REGISTRY.len(), 7);
        assert!(writer_profile_info("QF-Core Minimal Profile").is_some());
        assert!(writer_profile_info("QF-O Object Checkpoint Profile").is_some());
        assert!(writer_profile_info("QF-T Scan Profile")
            .unwrap()
            .requirements
            .contains(&"morsel_row_count = 4096"));
    }

    #[test]
    fn spec_73_recovery_registry_covers_recovery_table() {
        assert_eq!(RECOVERY_BEHAVIOR_REGISTRY.len(), 27);
        assert_eq!(
            recovery_behavior_info("Bad header magic")
                .unwrap()
                .default_behavior,
            "Reject file"
        );
        assert_eq!(
            recovery_behavior_info("QFX stale/corrupt")
                .unwrap()
                .default_behavior,
            "Ignore QFX"
        );
    }

    #[test]
    fn spec_76_compatibility_registry_contains_version_and_feature_rules() {
        assert_eq!(COMPATIBILITY_REGISTRY.len(), 5);
        assert_eq!(
            compatibility_rule_info("reject_unsupported_major")
                .unwrap()
                .rule,
            "Readers MUST reject unsupported major versions."
        );
        assert!(compatibility_rule_info("required_features")
            .unwrap()
            .examples
            .contains(&"codec needed to decode projected data"));
        assert!(compatibility_rule_info("optional_features")
            .unwrap()
            .examples
            .contains(&"QFX sidecars"));
    }
}
