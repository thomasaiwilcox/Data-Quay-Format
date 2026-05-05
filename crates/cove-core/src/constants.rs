//! Cove Format (COVE) v1.0 — Constants, magic bytes, feature bits, and enumerations.
//!
//! References:
//! - Section 6 (Core Concepts)
//! - Section 10 (Header)
//! - Section 11 (Feature Bits)
//! - Section 14 (Section Kinds)
//! - Section 16 (File Dictionary)
//! - Section 18 (Logical Types)
//! - Section 19 (Physical Kinds)
//! - Section 20 (Encoded Arrays)

// ── Magic bytes ──────────────────────────────────────────────────────────────

/// Magic bytes at the start of every COVE v1 data file (`b"COV1"`).
pub const MAGIC_COVE: [u8; 4] = *b"COV1";

/// Magic bytes at the start of a COVE footer (`b"COVF"`).
pub const MAGIC_COVE_FOOTER: [u8; 4] = *b"COVF";

/// Magic bytes at the end of a COVX accelerator sidecar (`b"CVX1"`).
pub const MAGIC_COVX: [u8; 4] = *b"CVX1";

/// Magic bytes at the end of a COVM dataset manifest (`b"CVM1"`).
pub const MAGIC_COVM: [u8; 4] = *b"CVM1";

/// Magic bytes at the end of a COVE-MAP reusable mapping artifact (`b"CMP1"`).
pub const MAGIC_COVEMAP: [u8; 4] = *b"CMP1";

// ── File-level constants ──────────────────────────────────────────────────────

/// Required value of `header_len` for COVE v1.
pub const HEADER_LEN_V1: u16 = 128;

/// Required value of `version_major` for COVE v1.
pub const VERSION_MAJOR_V1: u16 = 1;

/// Required value of `version_minor` for COVE v1.
pub const VERSION_MINOR_V1: u16 = 0;

/// Required `endianness` value: 1 = little-endian.
pub const ENDIANNESS_LITTLE: u8 = 1;

/// Default `morsel_row_count` as recommended by the spec.
pub const DEFAULT_MORSEL_ROW_COUNT: u32 = 4096;

/// Required `footer_version` for COVE v1.
pub const FOOTER_VERSION_V1: u16 = 1;

/// Serialised size (bytes) of [`CoveFooterHeaderV1`](crate::footer::CoveFooterHeaderV1).
pub const FOOTER_HEADER_LEN: usize = 44;

/// Serialised size (bytes) of [`CoveSectionEntryV1`](crate::footer::CoveSectionEntryV1).
pub const SECTION_ENTRY_LEN: u16 = 76;

/// Serialised size (bytes) of [`CoveSectionSpecV1`](crate::postscript::CoveSectionSpecV1).
pub const SECTION_SPEC_LEN: usize = 36;

/// Serialised size (bytes) of [`CovePostscriptV1`](crate::postscript::CovePostscriptV1).
pub const POSTSCRIPT_LEN: usize = 64;

/// Postscript version field value for COVE v1.
pub const POSTSCRIPT_VERSION_V1: u16 = 1;

/// Minimum metadata JSON length limit (0 bytes).
pub const METADATA_LEN_MIN: u32 = 0;

/// Maximum metadata JSON length as required by the spec (1 MiB).
pub const METADATA_LEN_MAX: u32 = 1 << 20; // 1 MiB

/// Maximum `postscript_len` value (u16::MAX).
pub const POSTSCRIPT_LEN_MAX: u16 = u16::MAX;

// ── Feature bits (Section 11) ─────────────────────────────────────────────────

pub const FEATURE_OBJECT_PROFILE: u64 = 0x0000_0000_0000_0001;
pub const FEATURE_TABLE_PROFILE: u64 = 0x0000_0000_0000_0002;
pub const FEATURE_ARCHIVE_PROFILE: u64 = 0x0000_0000_0000_0004;
pub const FEATURE_ENGINE_PROFILE: u64 = 0x0000_0000_0000_0008;
pub const FEATURE_HARBOR_PROFILE: u64 = 0x0000_0000_0000_0010;
pub const FEATURE_FILE_DICTIONARY: u64 = 0x0000_0000_0000_0020;
pub const FEATURE_NUMCODES: u64 = 0x0000_0000_0000_0040;
pub const FEATURE_COLUMN_DOMAINS: u64 = 0x0000_0000_0000_0080;
pub const FEATURE_EXACT_SETS: u64 = 0x0000_0000_0000_0100;
pub const FEATURE_BLOOM_FILTERS: u64 = 0x0000_0000_0000_0200;
pub const FEATURE_INVERTED_INDEXES: u64 = 0x0000_0000_0000_0400;
pub const FEATURE_LOOKUP_INDEXES: u64 = 0x0000_0000_0000_0800;
pub const FEATURE_AGGREGATE_SYNOPSES: u64 = 0x0000_0000_0000_1000;
pub const FEATURE_COMPOSITE_ZONES: u64 = 0x0000_0000_0000_2000;
pub const FEATURE_TOPN_SUMMARIES: u64 = 0x0000_0000_0000_4000;
pub const FEATURE_TRUST_CHAIN: u64 = 0x0000_0000_0000_8000;
pub const FEATURE_REDACTIONS: u64 = 0x0000_0000_0001_0000;
pub const FEATURE_NESTED_COLUMNS: u64 = 0x0000_0000_0002_0000;
pub const FEATURE_DIGEST_MANIFEST: u64 = 0x0000_0000_0004_0000;
pub const FEATURE_ARROW_INTEROP_HINTS: u64 = 0x0000_0000_0008_0000;
pub const FEATURE_LAKEHOUSE_HINTS: u64 = 0x0000_0000_0010_0000;
pub const FEATURE_EXTENSION_REGISTRY: u64 = 0x0000_0000_0020_0000;
pub const FEATURE_CODEC_LZ4: u64 = 0x0000_0000_0040_0000;
pub const FEATURE_CODEC_ZSTD: u64 = 0x0000_0000_0080_0000;
pub const FEATURE_SEMANTIC_MAP: u64 = 0x0000_0000_0100_0000;
pub const FEATURE_PAGE_PAYLOAD_ELISION: u64 = 0x0000_0000_0200_0000;

/// Bitmask of all feature bits defined in COVE v1.0 (Section 11).
///
/// Any required feature bit that is NOT in this mask is unknown to this
/// implementation.  Readers MUST reject files whose `required_features`
/// field contains unknown bits (Section 11 rule).
pub const KNOWN_FEATURE_BITS_MASK: u64 = FEATURE_OBJECT_PROFILE
    | FEATURE_TABLE_PROFILE
    | FEATURE_ARCHIVE_PROFILE
    | FEATURE_ENGINE_PROFILE
    | FEATURE_HARBOR_PROFILE
    | FEATURE_FILE_DICTIONARY
    | FEATURE_NUMCODES
    | FEATURE_COLUMN_DOMAINS
    | FEATURE_EXACT_SETS
    | FEATURE_BLOOM_FILTERS
    | FEATURE_INVERTED_INDEXES
    | FEATURE_LOOKUP_INDEXES
    | FEATURE_AGGREGATE_SYNOPSES
    | FEATURE_COMPOSITE_ZONES
    | FEATURE_TOPN_SUMMARIES
    | FEATURE_TRUST_CHAIN
    | FEATURE_REDACTIONS
    | FEATURE_NESTED_COLUMNS
    | FEATURE_DIGEST_MANIFEST
    | FEATURE_ARROW_INTEROP_HINTS
    | FEATURE_LAKEHOUSE_HINTS
    | FEATURE_EXTENSION_REGISTRY
    | FEATURE_CODEC_LZ4
    | FEATURE_CODEC_ZSTD
    | FEATURE_SEMANTIC_MAP
    | FEATURE_PAGE_PAYLOAD_ELISION;

// ── Primary profile (header field) ──────────────────────────────────────────

/// Profile identifier used in the header `primary_profile` field and section `profile` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PrimaryProfile {
    /// Mixed / unknown profile.
    Mixed = 0,
    /// COVE-O object temporal profile.
    ObjectTemporal = 1,
    /// COVE-T table scan profile.
    TableScan = 2,
    /// COVE-A archive acceleration profile.
    ArchiveAcceleration = 3,
    /// COVE-E engine execution profile.
    EngineExecution = 4,
    /// COVE-H Harbor registered execution profile.
    HarborExecution = 5,
    /// COVE-MAP semantic mapping profile.
    SemanticMapping = 6,
}

impl PrimaryProfile {
    /// Convert a raw `u8` to a [`PrimaryProfile`].
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Mixed),
            1 => Some(Self::ObjectTemporal),
            2 => Some(Self::TableScan),
            3 => Some(Self::ArchiveAcceleration),
            4 => Some(Self::EngineExecution),
            5 => Some(Self::HarborExecution),
            6 => Some(Self::SemanticMapping),
            _ => None,
        }
    }
}

// ── Producer scope kinds (Section 6.5 / 10) ──────────────────────────────────

/// Scope kind stored in the header `producer_scope_kind` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ProducerScopeKind {
    None = 0,
    Tenant = 1,
    Account = 2,
    Organisation = 3,
    Workspace = 4,
    Catalog = 5,
    Dataset = 6,
    EngineSpecific = 255,
}

impl ProducerScopeKind {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Tenant),
            2 => Some(Self::Account),
            3 => Some(Self::Organisation),
            4 => Some(Self::Workspace),
            5 => Some(Self::Catalog),
            6 => Some(Self::Dataset),
            255 => Some(Self::EngineSpecific),
            _ => None,
        }
    }
}

// ── Section kinds (Section 14) ────────────────────────────────────────────────

/// Identifies the logical kind of a section listed in the footer directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum SectionKind {
    // Shared sections (profile = 0)
    FileDictionaryIndex = 1,
    FileDictionaryPayload = 2,
    CollationRegistry = 3,
    DigestManifest = 4,
    RedactionManifest = 5,
    ArrowInteropHints = 6,
    LakehouseHints = 7,
    ExtensionRegistry = 8,
    ProfileCapabilityMatrix = 9,

    // COVE-T sections (profile = 2)
    TableCatalog = 10,
    TableSegmentIndex = 11,
    TableSegmentData = 12,
    ColumnDomain = 13,
    ZoneStats = 14,
    ExactSetIndex = 15,
    BloomIndex = 16,
    InvertedMorselIndex = 17,
    LookupIndex = 18,
    AggregateSynopsis = 19,
    CompositeZoneIndex = 20,
    TopNZoneSummary = 21,
    KernelCapabilities = 22,

    // COVE-E sections (profile = 4)
    EngineProfileRegistry = 30,
    ExecutionCodeDescriptor = 31,
    ExecutionScopeDescriptor = 32,
    CodeSpaceDescriptor = 33,
    EngineMountPolicy = 34,

    // COVE-O sections (profile = 1)
    ObjectTypeCatalog = 40,
    TemporalSegmentIndex = 41,
    TemporalSegmentData = 42,
    TemporalBloomIndex = 43,
    TrustManifest = 44,

    // COVE-H sections (profile = 5)
    HarborMountHints = 50,

    // COVE-MAP sections (profile = 6)
    MapSourceCatalog = 60,
    MapFunctionRegistry = 61,
    MapIdentityRuleCatalog = 62,
    MapRowSemanticsCatalog = 63,
    MapAssertionLog = 64,
    MapIdentityEquivalenceIndex = 65,
    MapEvidenceIndex = 66,
    MapConversionReport = 67,
    MapProjectionCatalog = 68,

    // Vendor extension (shared)
    VendorExtension = 255,
}

impl SectionKind {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            1 => Some(Self::FileDictionaryIndex),
            2 => Some(Self::FileDictionaryPayload),
            3 => Some(Self::CollationRegistry),
            4 => Some(Self::DigestManifest),
            5 => Some(Self::RedactionManifest),
            6 => Some(Self::ArrowInteropHints),
            7 => Some(Self::LakehouseHints),
            8 => Some(Self::ExtensionRegistry),
            9 => Some(Self::ProfileCapabilityMatrix),
            10 => Some(Self::TableCatalog),
            11 => Some(Self::TableSegmentIndex),
            12 => Some(Self::TableSegmentData),
            13 => Some(Self::ColumnDomain),
            14 => Some(Self::ZoneStats),
            15 => Some(Self::ExactSetIndex),
            16 => Some(Self::BloomIndex),
            17 => Some(Self::InvertedMorselIndex),
            18 => Some(Self::LookupIndex),
            19 => Some(Self::AggregateSynopsis),
            20 => Some(Self::CompositeZoneIndex),
            21 => Some(Self::TopNZoneSummary),
            22 => Some(Self::KernelCapabilities),
            30 => Some(Self::EngineProfileRegistry),
            31 => Some(Self::ExecutionCodeDescriptor),
            32 => Some(Self::ExecutionScopeDescriptor),
            33 => Some(Self::CodeSpaceDescriptor),
            34 => Some(Self::EngineMountPolicy),
            40 => Some(Self::ObjectTypeCatalog),
            41 => Some(Self::TemporalSegmentIndex),
            42 => Some(Self::TemporalSegmentData),
            43 => Some(Self::TemporalBloomIndex),
            44 => Some(Self::TrustManifest),
            50 => Some(Self::HarborMountHints),
            60 => Some(Self::MapSourceCatalog),
            61 => Some(Self::MapFunctionRegistry),
            62 => Some(Self::MapIdentityRuleCatalog),
            63 => Some(Self::MapRowSemanticsCatalog),
            64 => Some(Self::MapAssertionLog),
            65 => Some(Self::MapIdentityEquivalenceIndex),
            66 => Some(Self::MapEvidenceIndex),
            67 => Some(Self::MapConversionReport),
            68 => Some(Self::MapProjectionCatalog),
            255 => Some(Self::VendorExtension),
            _ => None,
        }
    }
}

// ── Compression codecs (Section 66) ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompressionCodec {
    None = 0,
    Lz4 = 1,
    Zstd = 2,
}

impl CompressionCodec {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Lz4),
            2 => Some(Self::Zstd),
            _ => None,
        }
    }
}

// ── Logical types (Section 18) ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CoveLogicalType {
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

impl CoveLogicalType {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(Self::Null),
            1 => Some(Self::Bool),
            2 => Some(Self::Int8),
            3 => Some(Self::Int16),
            4 => Some(Self::Int32),
            5 => Some(Self::Int64),
            6 => Some(Self::UInt8),
            7 => Some(Self::UInt16),
            8 => Some(Self::UInt32),
            9 => Some(Self::UInt64),
            10 => Some(Self::Float32),
            11 => Some(Self::Float64),
            12 => Some(Self::Decimal64),
            13 => Some(Self::Decimal128),
            14 => Some(Self::DateDays),
            15 => Some(Self::TimestampMicros),
            16 => Some(Self::TimestampNanos),
            17 => Some(Self::Utf8),
            18 => Some(Self::Binary),
            19 => Some(Self::Uuid),
            20 => Some(Self::Json),
            21 => Some(Self::List),
            22 => Some(Self::Struct),
            23 => Some(Self::Map),
            _ => None,
        }
    }
}

// ── Physical kinds (Section 19) ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CovePhysicalKind {
    FileCode = 0,
    NumCode = 1,
    Boolean = 2,
    FixedBytes = 3,
    VarBytes = 4,
    List = 5,
    Struct = 6,
    Map = 7,
}

impl CovePhysicalKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::FileCode),
            1 => Some(Self::NumCode),
            2 => Some(Self::Boolean),
            3 => Some(Self::FixedBytes),
            4 => Some(Self::VarBytes),
            5 => Some(Self::List),
            6 => Some(Self::Struct),
            7 => Some(Self::Map),
            _ => None,
        }
    }
}

// ── Value tags (Section 16.3) ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ValueTag {
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

impl ValueTag {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(Self::Null),
            1 => Some(Self::BoolFalse),
            2 => Some(Self::BoolTrue),
            3 => Some(Self::Int64),
            4 => Some(Self::UInt64),
            5 => Some(Self::Float32Bits),
            6 => Some(Self::Float64Bits),
            7 => Some(Self::Decimal64),
            8 => Some(Self::Decimal128),
            9 => Some(Self::DateDays),
            10 => Some(Self::TimestampMicros),
            11 => Some(Self::TimestampNanos),
            12 => Some(Self::Utf8),
            13 => Some(Self::Binary),
            14 => Some(Self::Uuid),
            15 => Some(Self::Json),
            16 => Some(Self::List),
            17 => Some(Self::Struct),
            18 => Some(Self::Map),
            _ => None,
        }
    }
}

// ── Storage classes (Section 16.4) ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StorageClass {
    Inline = 0,
    Payload = 1,
    Redacted = 2,
}

impl StorageClass {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Inline),
            1 => Some(Self::Payload),
            2 => Some(Self::Redacted),
            _ => None,
        }
    }
}

// ── Encoding kinds (Section 20.1) ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CoveEncodingKind {
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

impl CoveEncodingKind {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(Self::Canonical),
            1 => Some(Self::Validity),
            2 => Some(Self::Constant),
            3 => Some(Self::FileCode),
            4 => Some(Self::NumCode),
            5 => Some(Self::LocalCodebook),
            6 => Some(Self::Rle),
            7 => Some(Self::RunEnd),
            8 => Some(Self::BitPacked),
            9 => Some(Self::Delta),
            10 => Some(Self::FrameOfReference),
            11 => Some(Self::PatchedBase),
            12 => Some(Self::Sparse),
            13 => Some(Self::Sequence),
            14 => Some(Self::PlainFixed),
            15 => Some(Self::PlainVarint),
            16 => Some(Self::VarBytes),
            17 => Some(Self::Lz4Block),
            18 => Some(Self::ZstdBlock),
            _ => None,
        }
    }
}

// ── Digest algorithms (Section 8.7) ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum DigestAlgorithm {
    None = 0,
    Sha256 = 1,
    Blake3 = 2,
}

impl DigestAlgorithm {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Sha256),
            2 => Some(Self::Blake3),
            _ => None,
        }
    }
}

// ── Predicate zone outcome (Section 7.5) ─────────────────────────────────────

/// The result of evaluating a predicate against a zone's metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PredicateZoneOutcome {
    /// No row in the zone can satisfy the predicate.
    DefinitelyNo = 0,
    /// Every row in the zone satisfies the predicate.
    DefinitelyYes = 1,
    /// Metadata cannot prove exclusion or inclusion.
    Unknown = 2,
}

// ── Record kinds (Section 59.1) ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordKind {
    Delta = 0,
    Snapshot = 1,
    ReservedLegacyMaterializedDelta = 2,
    Baseline = 3,
    Tombstone = 4,
}

impl RecordKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Delta),
            1 => Some(Self::Snapshot),
            2 => Some(Self::ReservedLegacyMaterializedDelta),
            3 => Some(Self::Baseline),
            4 => Some(Self::Tombstone),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PrimaryProfile ────────────────────────────────────────────────────────

    #[test]
    fn primary_profile_from_u8_all_known() {
        assert_eq!(PrimaryProfile::from_u8(0), Some(PrimaryProfile::Mixed));
        assert_eq!(
            PrimaryProfile::from_u8(1),
            Some(PrimaryProfile::ObjectTemporal)
        );
        assert_eq!(PrimaryProfile::from_u8(2), Some(PrimaryProfile::TableScan));
        assert_eq!(
            PrimaryProfile::from_u8(3),
            Some(PrimaryProfile::ArchiveAcceleration)
        );
        assert_eq!(
            PrimaryProfile::from_u8(4),
            Some(PrimaryProfile::EngineExecution)
        );
        assert_eq!(
            PrimaryProfile::from_u8(5),
            Some(PrimaryProfile::HarborExecution)
        );
        assert_eq!(
            PrimaryProfile::from_u8(6),
            Some(PrimaryProfile::SemanticMapping)
        );
    }

    #[test]
    fn primary_profile_from_u8_unknown() {
        assert_eq!(PrimaryProfile::from_u8(7), None);
        assert_eq!(PrimaryProfile::from_u8(255), None);
    }

    // ── ProducerScopeKind ─────────────────────────────────────────────────────

    #[test]
    fn producer_scope_kind_from_u16_all_known() {
        assert_eq!(
            ProducerScopeKind::from_u16(0),
            Some(ProducerScopeKind::None)
        );
        assert_eq!(
            ProducerScopeKind::from_u16(1),
            Some(ProducerScopeKind::Tenant)
        );
        assert_eq!(
            ProducerScopeKind::from_u16(2),
            Some(ProducerScopeKind::Account)
        );
        assert_eq!(
            ProducerScopeKind::from_u16(3),
            Some(ProducerScopeKind::Organisation)
        );
        assert_eq!(
            ProducerScopeKind::from_u16(4),
            Some(ProducerScopeKind::Workspace)
        );
        assert_eq!(
            ProducerScopeKind::from_u16(5),
            Some(ProducerScopeKind::Catalog)
        );
        assert_eq!(
            ProducerScopeKind::from_u16(6),
            Some(ProducerScopeKind::Dataset)
        );
        assert_eq!(
            ProducerScopeKind::from_u16(255),
            Some(ProducerScopeKind::EngineSpecific)
        );
    }

    #[test]
    fn producer_scope_kind_from_u16_unknown() {
        assert_eq!(ProducerScopeKind::from_u16(7), None);
        assert_eq!(ProducerScopeKind::from_u16(100), None);
    }

    // ── SectionKind ──────────────────────────────────────────────────────────

    #[test]
    fn section_kind_from_u16_shared_sections() {
        assert_eq!(
            SectionKind::from_u16(1),
            Some(SectionKind::FileDictionaryIndex)
        );
        assert_eq!(
            SectionKind::from_u16(2),
            Some(SectionKind::FileDictionaryPayload)
        );
        assert_eq!(
            SectionKind::from_u16(3),
            Some(SectionKind::CollationRegistry)
        );
        assert_eq!(SectionKind::from_u16(4), Some(SectionKind::DigestManifest));
        assert_eq!(
            SectionKind::from_u16(5),
            Some(SectionKind::RedactionManifest)
        );
        assert_eq!(
            SectionKind::from_u16(6),
            Some(SectionKind::ArrowInteropHints)
        );
        assert_eq!(SectionKind::from_u16(7), Some(SectionKind::LakehouseHints));
        assert_eq!(
            SectionKind::from_u16(8),
            Some(SectionKind::ExtensionRegistry)
        );
        assert_eq!(
            SectionKind::from_u16(9),
            Some(SectionKind::ProfileCapabilityMatrix)
        );
    }

    #[test]
    fn section_kind_from_u16_cove_t_sections() {
        assert_eq!(SectionKind::from_u16(10), Some(SectionKind::TableCatalog));
        assert_eq!(
            SectionKind::from_u16(11),
            Some(SectionKind::TableSegmentIndex)
        );
        assert_eq!(
            SectionKind::from_u16(12),
            Some(SectionKind::TableSegmentData)
        );
        assert_eq!(SectionKind::from_u16(13), Some(SectionKind::ColumnDomain));
        assert_eq!(SectionKind::from_u16(14), Some(SectionKind::ZoneStats));
        assert_eq!(SectionKind::from_u16(15), Some(SectionKind::ExactSetIndex));
        assert_eq!(SectionKind::from_u16(16), Some(SectionKind::BloomIndex));
        assert_eq!(
            SectionKind::from_u16(17),
            Some(SectionKind::InvertedMorselIndex)
        );
        assert_eq!(SectionKind::from_u16(18), Some(SectionKind::LookupIndex));
        assert_eq!(
            SectionKind::from_u16(19),
            Some(SectionKind::AggregateSynopsis)
        );
        assert_eq!(
            SectionKind::from_u16(20),
            Some(SectionKind::CompositeZoneIndex)
        );
        assert_eq!(
            SectionKind::from_u16(21),
            Some(SectionKind::TopNZoneSummary)
        );
        assert_eq!(
            SectionKind::from_u16(22),
            Some(SectionKind::KernelCapabilities)
        );
    }

    #[test]
    fn section_kind_from_u16_cove_e_sections() {
        assert_eq!(
            SectionKind::from_u16(30),
            Some(SectionKind::EngineProfileRegistry)
        );
        assert_eq!(
            SectionKind::from_u16(31),
            Some(SectionKind::ExecutionCodeDescriptor)
        );
        assert_eq!(
            SectionKind::from_u16(32),
            Some(SectionKind::ExecutionScopeDescriptor)
        );
        assert_eq!(
            SectionKind::from_u16(33),
            Some(SectionKind::CodeSpaceDescriptor)
        );
        assert_eq!(
            SectionKind::from_u16(34),
            Some(SectionKind::EngineMountPolicy)
        );
    }

    #[test]
    fn section_kind_from_u16_cove_o_sections() {
        assert_eq!(
            SectionKind::from_u16(40),
            Some(SectionKind::ObjectTypeCatalog)
        );
        assert_eq!(
            SectionKind::from_u16(41),
            Some(SectionKind::TemporalSegmentIndex)
        );
        assert_eq!(
            SectionKind::from_u16(42),
            Some(SectionKind::TemporalSegmentData)
        );
        assert_eq!(
            SectionKind::from_u16(43),
            Some(SectionKind::TemporalBloomIndex)
        );
        assert_eq!(SectionKind::from_u16(44), Some(SectionKind::TrustManifest));
    }

    #[test]
    fn section_kind_from_u16_cove_h_and_vendor() {
        assert_eq!(
            SectionKind::from_u16(50),
            Some(SectionKind::HarborMountHints)
        );
        assert_eq!(
            SectionKind::from_u16(60),
            Some(SectionKind::MapSourceCatalog)
        );
        assert_eq!(
            SectionKind::from_u16(61),
            Some(SectionKind::MapFunctionRegistry)
        );
        assert_eq!(
            SectionKind::from_u16(62),
            Some(SectionKind::MapIdentityRuleCatalog)
        );
        assert_eq!(
            SectionKind::from_u16(63),
            Some(SectionKind::MapRowSemanticsCatalog)
        );
        assert_eq!(
            SectionKind::from_u16(64),
            Some(SectionKind::MapAssertionLog)
        );
        assert_eq!(
            SectionKind::from_u16(65),
            Some(SectionKind::MapIdentityEquivalenceIndex)
        );
        assert_eq!(
            SectionKind::from_u16(66),
            Some(SectionKind::MapEvidenceIndex)
        );
        assert_eq!(
            SectionKind::from_u16(67),
            Some(SectionKind::MapConversionReport)
        );
        assert_eq!(
            SectionKind::from_u16(68),
            Some(SectionKind::MapProjectionCatalog)
        );
        assert_eq!(
            SectionKind::from_u16(255),
            Some(SectionKind::VendorExtension)
        );
    }

    #[test]
    fn section_kind_from_u16_unknown() {
        assert_eq!(SectionKind::from_u16(0), None);
        assert_eq!(SectionKind::from_u16(23), None);
        assert_eq!(SectionKind::from_u16(100), None);
    }

    // ── CompressionCodec ─────────────────────────────────────────────────────

    #[test]
    fn compression_codec_from_u8_all_known() {
        assert_eq!(CompressionCodec::from_u8(0), Some(CompressionCodec::None));
        assert_eq!(CompressionCodec::from_u8(1), Some(CompressionCodec::Lz4));
        assert_eq!(CompressionCodec::from_u8(2), Some(CompressionCodec::Zstd));
    }

    #[test]
    fn compression_codec_from_u8_unknown() {
        assert_eq!(CompressionCodec::from_u8(3), None);
        assert_eq!(CompressionCodec::from_u8(255), None);
    }

    // ── CoveLogicalType ─────────────────────────────────────────────────────────

    #[test]
    fn logical_type_from_u16_all_known() {
        assert_eq!(CoveLogicalType::from_u16(0), Some(CoveLogicalType::Null));
        assert_eq!(CoveLogicalType::from_u16(1), Some(CoveLogicalType::Bool));
        assert_eq!(CoveLogicalType::from_u16(5), Some(CoveLogicalType::Int64));
        assert_eq!(CoveLogicalType::from_u16(9), Some(CoveLogicalType::UInt64));
        assert_eq!(
            CoveLogicalType::from_u16(11),
            Some(CoveLogicalType::Float64)
        );
        assert_eq!(
            CoveLogicalType::from_u16(14),
            Some(CoveLogicalType::DateDays)
        );
        assert_eq!(
            CoveLogicalType::from_u16(15),
            Some(CoveLogicalType::TimestampMicros)
        );
        assert_eq!(
            CoveLogicalType::from_u16(16),
            Some(CoveLogicalType::TimestampNanos)
        );
        assert_eq!(CoveLogicalType::from_u16(17), Some(CoveLogicalType::Utf8));
        assert_eq!(CoveLogicalType::from_u16(19), Some(CoveLogicalType::Uuid));
        assert_eq!(CoveLogicalType::from_u16(20), Some(CoveLogicalType::Json));
        assert_eq!(CoveLogicalType::from_u16(21), Some(CoveLogicalType::List));
        assert_eq!(CoveLogicalType::from_u16(22), Some(CoveLogicalType::Struct));
        assert_eq!(CoveLogicalType::from_u16(23), Some(CoveLogicalType::Map));
    }

    #[test]
    fn logical_type_from_u16_unknown() {
        assert_eq!(CoveLogicalType::from_u16(24), None);
        assert_eq!(CoveLogicalType::from_u16(1000), None);
    }

    // ── CovePhysicalKind ────────────────────────────────────────────────────────

    #[test]
    fn physical_kind_from_u8_all_known() {
        assert_eq!(
            CovePhysicalKind::from_u8(0),
            Some(CovePhysicalKind::FileCode)
        );
        assert_eq!(
            CovePhysicalKind::from_u8(1),
            Some(CovePhysicalKind::NumCode)
        );
        assert_eq!(
            CovePhysicalKind::from_u8(2),
            Some(CovePhysicalKind::Boolean)
        );
        assert_eq!(
            CovePhysicalKind::from_u8(3),
            Some(CovePhysicalKind::FixedBytes)
        );
        assert_eq!(
            CovePhysicalKind::from_u8(4),
            Some(CovePhysicalKind::VarBytes)
        );
        assert_eq!(CovePhysicalKind::from_u8(5), Some(CovePhysicalKind::List));
        assert_eq!(CovePhysicalKind::from_u8(6), Some(CovePhysicalKind::Struct));
        assert_eq!(CovePhysicalKind::from_u8(7), Some(CovePhysicalKind::Map));
    }

    #[test]
    fn physical_kind_from_u8_unknown() {
        assert_eq!(CovePhysicalKind::from_u8(8), None);
        assert_eq!(CovePhysicalKind::from_u8(255), None);
    }

    // ── ValueTag ─────────────────────────────────────────────────────────────

    #[test]
    fn value_tag_from_u16_all_known() {
        assert_eq!(ValueTag::from_u16(0), Some(ValueTag::Null));
        assert_eq!(ValueTag::from_u16(1), Some(ValueTag::BoolFalse));
        assert_eq!(ValueTag::from_u16(2), Some(ValueTag::BoolTrue));
        assert_eq!(ValueTag::from_u16(3), Some(ValueTag::Int64));
        assert_eq!(ValueTag::from_u16(4), Some(ValueTag::UInt64));
        assert_eq!(ValueTag::from_u16(5), Some(ValueTag::Float32Bits));
        assert_eq!(ValueTag::from_u16(6), Some(ValueTag::Float64Bits));
        assert_eq!(ValueTag::from_u16(7), Some(ValueTag::Decimal64));
        assert_eq!(ValueTag::from_u16(8), Some(ValueTag::Decimal128));
        assert_eq!(ValueTag::from_u16(9), Some(ValueTag::DateDays));
        assert_eq!(ValueTag::from_u16(10), Some(ValueTag::TimestampMicros));
        assert_eq!(ValueTag::from_u16(11), Some(ValueTag::TimestampNanos));
        assert_eq!(ValueTag::from_u16(12), Some(ValueTag::Utf8));
        assert_eq!(ValueTag::from_u16(13), Some(ValueTag::Binary));
        assert_eq!(ValueTag::from_u16(14), Some(ValueTag::Uuid));
        assert_eq!(ValueTag::from_u16(15), Some(ValueTag::Json));
        assert_eq!(ValueTag::from_u16(16), Some(ValueTag::List));
        assert_eq!(ValueTag::from_u16(17), Some(ValueTag::Struct));
        assert_eq!(ValueTag::from_u16(18), Some(ValueTag::Map));
    }

    #[test]
    fn value_tag_from_u16_unknown() {
        assert_eq!(ValueTag::from_u16(19), None);
        assert_eq!(ValueTag::from_u16(1000), None);
    }

    // ── StorageClass ─────────────────────────────────────────────────────────

    #[test]
    fn storage_class_from_u8_all_known() {
        assert_eq!(StorageClass::from_u8(0), Some(StorageClass::Inline));
        assert_eq!(StorageClass::from_u8(1), Some(StorageClass::Payload));
        assert_eq!(StorageClass::from_u8(2), Some(StorageClass::Redacted));
    }

    #[test]
    fn storage_class_from_u8_unknown() {
        assert_eq!(StorageClass::from_u8(3), None);
        assert_eq!(StorageClass::from_u8(255), None);
    }

    // ── CoveEncodingKind ───────────────────────────────────────────────────────

    #[test]
    fn encoding_kind_from_u16_all_known() {
        assert_eq!(
            CoveEncodingKind::from_u16(0),
            Some(CoveEncodingKind::Canonical)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(1),
            Some(CoveEncodingKind::Validity)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(2),
            Some(CoveEncodingKind::Constant)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(3),
            Some(CoveEncodingKind::FileCode)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(4),
            Some(CoveEncodingKind::NumCode)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(5),
            Some(CoveEncodingKind::LocalCodebook)
        );
        assert_eq!(CoveEncodingKind::from_u16(6), Some(CoveEncodingKind::Rle));
        assert_eq!(
            CoveEncodingKind::from_u16(7),
            Some(CoveEncodingKind::RunEnd)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(8),
            Some(CoveEncodingKind::BitPacked)
        );
        assert_eq!(CoveEncodingKind::from_u16(9), Some(CoveEncodingKind::Delta));
        assert_eq!(
            CoveEncodingKind::from_u16(10),
            Some(CoveEncodingKind::FrameOfReference)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(11),
            Some(CoveEncodingKind::PatchedBase)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(12),
            Some(CoveEncodingKind::Sparse)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(13),
            Some(CoveEncodingKind::Sequence)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(14),
            Some(CoveEncodingKind::PlainFixed)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(15),
            Some(CoveEncodingKind::PlainVarint)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(16),
            Some(CoveEncodingKind::VarBytes)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(17),
            Some(CoveEncodingKind::Lz4Block)
        );
        assert_eq!(
            CoveEncodingKind::from_u16(18),
            Some(CoveEncodingKind::ZstdBlock)
        );
    }

    #[test]
    fn encoding_kind_from_u16_unknown() {
        assert_eq!(CoveEncodingKind::from_u16(19), None);
        assert_eq!(CoveEncodingKind::from_u16(1000), None);
    }

    // ── DigestAlgorithm ──────────────────────────────────────────────────────

    #[test]
    fn digest_algorithm_from_u16_all_known() {
        assert_eq!(DigestAlgorithm::from_u16(0), Some(DigestAlgorithm::None));
        assert_eq!(DigestAlgorithm::from_u16(1), Some(DigestAlgorithm::Sha256));
        assert_eq!(DigestAlgorithm::from_u16(2), Some(DigestAlgorithm::Blake3));
    }

    #[test]
    fn digest_algorithm_from_u16_unknown() {
        assert_eq!(DigestAlgorithm::from_u16(3), None);
        assert_eq!(DigestAlgorithm::from_u16(255), None);
    }

    // ── RecordKind ───────────────────────────────────────────────────────────

    #[test]
    fn record_kind_from_u8_all_known() {
        assert_eq!(RecordKind::from_u8(0), Some(RecordKind::Delta));
        assert_eq!(RecordKind::from_u8(1), Some(RecordKind::Snapshot));
        assert_eq!(
            RecordKind::from_u8(2),
            Some(RecordKind::ReservedLegacyMaterializedDelta)
        );
        assert_eq!(RecordKind::from_u8(3), Some(RecordKind::Baseline));
        assert_eq!(RecordKind::from_u8(4), Some(RecordKind::Tombstone));
    }

    #[test]
    fn record_kind_from_u8_unknown() {
        assert_eq!(RecordKind::from_u8(5), None);
        assert_eq!(RecordKind::from_u8(255), None);
    }

    // ── KNOWN_FEATURE_BITS_MASK ──────────────────────────────────────────────

    #[test]
    fn known_feature_bits_mask_covers_all_defined_bits() {
        // Every defined feature constant must be covered by the mask.
        let defined = [
            FEATURE_OBJECT_PROFILE,
            FEATURE_TABLE_PROFILE,
            FEATURE_ARCHIVE_PROFILE,
            FEATURE_ENGINE_PROFILE,
            FEATURE_HARBOR_PROFILE,
            FEATURE_FILE_DICTIONARY,
            FEATURE_NUMCODES,
            FEATURE_COLUMN_DOMAINS,
            FEATURE_EXACT_SETS,
            FEATURE_BLOOM_FILTERS,
            FEATURE_INVERTED_INDEXES,
            FEATURE_LOOKUP_INDEXES,
            FEATURE_AGGREGATE_SYNOPSES,
            FEATURE_COMPOSITE_ZONES,
            FEATURE_TOPN_SUMMARIES,
            FEATURE_TRUST_CHAIN,
            FEATURE_REDACTIONS,
            FEATURE_NESTED_COLUMNS,
            FEATURE_DIGEST_MANIFEST,
            FEATURE_ARROW_INTEROP_HINTS,
            FEATURE_LAKEHOUSE_HINTS,
            FEATURE_EXTENSION_REGISTRY,
            FEATURE_CODEC_LZ4,
            FEATURE_CODEC_ZSTD,
            FEATURE_SEMANTIC_MAP,
            FEATURE_PAGE_PAYLOAD_ELISION,
        ];
        for bit in &defined {
            assert_eq!(
                KNOWN_FEATURE_BITS_MASK & bit,
                *bit,
                "bit 0x{bit:016x} not in mask"
            );
        }
    }

    #[test]
    fn known_feature_bits_mask_does_not_contain_future_bits() {
        // Bits beyond the defined range should not be in the mask.
        let future_bit: u64 = 0x0000_0000_0400_0000;
        assert_eq!(KNOWN_FEATURE_BITS_MASK & future_bit, 0);
    }
}
