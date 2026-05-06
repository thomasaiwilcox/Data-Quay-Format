//! Cove Format (COVE) v1.0 — Error types.
//!
//! Corresponds to Section 76 of the COVE v1.0 specification.

use std::fmt;

/// All errors that can occur while reading, writing, or validating a COVE file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoveError {
    /// Missing or invalid magic bytes (COVE_E_BAD_MAGIC).
    BadMagic,
    /// Unsupported COVE version (COVE_E_BAD_VERSION).
    BadVersion,
    /// Unknown required feature bit set (COVE_E_UNKNOWN_REQUIRED_FEATURE).
    UnknownRequiredFeature(u64),
    /// Checksum mismatch — header, postscript, footer, section, segment, or page
    /// (COVE_E_CHECKSUM_MISMATCH).
    ChecksumMismatch,
    /// Cryptographic digest mismatch (COVE_E_DIGEST_MISMATCH).
    DigestMismatch,
    /// Offset/length/count exceeds file bounds (COVE_E_OFFSET_RANGE).
    OffsetRange,
    /// Arithmetic overflow in offset/count/size (COVE_E_ARITH_OVERFLOW).
    ArithOverflow,
    /// Section malformed or invalid (COVE_E_BAD_SECTION).
    BadSection(String),
    /// Catalog or schema malformed (COVE_E_BAD_SCHEMA).
    BadSchema(String),
    /// Logical type incompatible with physical kind (COVE_E_BAD_LOGICAL_PHYSICAL_PAIR).
    BadLogicalPhysicalPair,
    /// FileCode missing from dictionary (COVE_E_DICT_MISS).
    DictMiss,
    /// FileCode outside dictionary range (COVE_E_BAD_FILECODE).
    BadFileCode,
    /// NumCode invalid for declared logical type (COVE_E_BAD_NUMCODE).
    BadNumCode,
    /// ColumnDomain invalid (COVE_E_BAD_DOMAIN).
    BadDomain,
    /// Statistics invalid or unsafe (COVE_E_BAD_STATS).
    BadStats,
    /// Optional index invalid or corrupt (COVE_E_BAD_INDEX).
    BadIndex,
    /// Extension invalid or required extension unsupported (COVE_E_BAD_EXTENSION).
    BadExtension,
    /// Engine profile invalid or unsupported when required (COVE_E_BAD_ENGINE_PROFILE).
    BadEngineProfile,
    /// Engine-local code mapping failed (COVE_E_EXECUTION_CODE_MAP).
    ExecutionCodeMap,
    /// Harbor code lease resolution failed (COVE_E_HARBOR_MOUNT_LEASE).
    HarborMountLease,
    /// COVE-O prev_ref invalid (COVE_E_REF_INVALID).
    RefInvalid,
    /// COVE-O chain lacks baseline/snapshot/full chain (COVE_E_NOT_SELF_CONTAINED).
    NotSelfContained,
    /// Segment structure invalid (COVE_E_SEGMENT_CORRUPT).
    SegmentCorrupt,
    /// Page structure invalid (COVE_E_PAGE_CORRUPT).
    PageCorrupt,
    /// Redacted value cannot be surfaced under current policy (COVE_E_REDACTION_POLICY).
    RedactionPolicy,
    /// COVX/COVM sidecar does not match referenced COVE (COVE_E_SIDECAR_STALE).
    SidecarStale,
    /// Semantic mapping payload invalid (COVE_E_MAP_INVALID).
    MapInvalid,
    /// Referenced map function was not declared (COVE_E_MAP_FUNCTION_UNDECLARED).
    MapFunctionUndeclared,
    /// Semantic mapping identity rules conflict (COVE_E_MAP_IDENTITY_CONFLICT).
    MapIdentityConflict,
    /// Semantic mapping source metadata is stale (COVE_E_MAP_SOURCE_STALE).
    MapSourceStale,
    /// Semantic mapping evidence payload invalid (COVE_E_MAP_EVIDENCE_INVALID).
    MapEvidenceInvalid,
    /// Input buffer is too short to parse the requested structure.
    BufferTooShort,
    /// A field that MUST be zero contained a non-zero value.
    ReservedNotZero,
    /// I/O error during file reading or writing.
    Io(String),
    /// Encoding kind is not supported by this implementation (COVE_E_UNSUPPORTED_ENCODING).
    UnsupportedEncoding(String),
}

impl fmt::Display for CoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoveError::BadMagic => write!(f, "COVE_E_BAD_MAGIC: missing or invalid magic bytes"),
            CoveError::BadVersion => write!(f, "COVE_E_BAD_VERSION: unsupported COVE version"),
            CoveError::UnknownRequiredFeature(bits) => {
                write!(
                    f,
                    "COVE_E_UNKNOWN_REQUIRED_FEATURE: unknown required feature bits 0x{bits:016x}"
                )
            }
            CoveError::ChecksumMismatch => write!(f, "COVE_E_CHECKSUM_MISMATCH: CRC32C mismatch"),
            CoveError::DigestMismatch => {
                write!(f, "COVE_E_DIGEST_MISMATCH: cryptographic digest mismatch")
            }
            CoveError::OffsetRange => {
                write!(
                    f,
                    "COVE_E_OFFSET_RANGE: offset or length out of file bounds"
                )
            }
            CoveError::ArithOverflow => write!(
                f,
                "COVE_E_ARITH_OVERFLOW: arithmetic overflow in offset/count"
            ),
            CoveError::BadSection(s) => write!(f, "COVE_E_BAD_SECTION: {s}"),
            CoveError::BadSchema(s) => write!(f, "COVE_E_BAD_SCHEMA: {s}"),
            CoveError::BadLogicalPhysicalPair => {
                write!(
                    f,
                    "COVE_E_BAD_LOGICAL_PHYSICAL_PAIR: logical type incompatible with physical kind"
                )
            }
            CoveError::DictMiss => write!(f, "COVE_E_DICT_MISS: FileCode missing from dictionary"),
            CoveError::BadFileCode => {
                write!(f, "COVE_E_BAD_FILECODE: FileCode outside dictionary range")
            }
            CoveError::BadNumCode => write!(
                f,
                "COVE_E_BAD_NUMCODE: NumCode invalid for declared logical type"
            ),
            CoveError::BadDomain => write!(f, "COVE_E_BAD_DOMAIN: ColumnDomain invalid"),
            CoveError::BadStats => write!(f, "COVE_E_BAD_STATS: statistics invalid or unsafe"),
            CoveError::BadIndex => write!(f, "COVE_E_BAD_INDEX: optional index invalid or corrupt"),
            CoveError::BadExtension => {
                write!(f, "COVE_E_BAD_EXTENSION: extension invalid or unsupported")
            }
            CoveError::BadEngineProfile => {
                write!(
                    f,
                    "COVE_E_BAD_ENGINE_PROFILE: engine profile invalid or unsupported"
                )
            }
            CoveError::ExecutionCodeMap => {
                write!(
                    f,
                    "COVE_E_EXECUTION_CODE_MAP: execution code mapping failed"
                )
            }
            CoveError::HarborMountLease => {
                write!(
                    f,
                    "COVE_E_HARBOR_MOUNT_LEASE: Harbor lease resolution failed"
                )
            }
            CoveError::RefInvalid => write!(f, "COVE_E_REF_INVALID: COVE-O prev_ref invalid"),
            CoveError::NotSelfContained => {
                write!(
                    f,
                    "COVE_E_NOT_SELF_CONTAINED: COVE-O chain not self-contained"
                )
            }
            CoveError::SegmentCorrupt => {
                write!(f, "COVE_E_SEGMENT_CORRUPT: segment structure invalid")
            }
            CoveError::PageCorrupt => write!(f, "COVE_E_PAGE_CORRUPT: page structure invalid"),
            CoveError::RedactionPolicy => write!(
                f,
                "COVE_E_REDACTION_POLICY: redacted value cannot be surfaced"
            ),
            CoveError::SidecarStale => write!(
                f,
                "COVE_E_SIDECAR_STALE: COVX/COVM sidecar does not match COVE file"
            ),
            CoveError::MapInvalid => {
                write!(f, "COVE_E_MAP_INVALID: semantic mapping payload invalid")
            }
            CoveError::MapFunctionUndeclared => write!(
                f,
                "COVE_E_MAP_FUNCTION_UNDECLARED: semantic mapping function not declared"
            ),
            CoveError::MapIdentityConflict => write!(
                f,
                "COVE_E_MAP_IDENTITY_CONFLICT: semantic mapping identity rules conflict"
            ),
            CoveError::MapSourceStale => write!(
                f,
                "COVE_E_MAP_SOURCE_STALE: semantic mapping source metadata is stale"
            ),
            CoveError::MapEvidenceInvalid => write!(
                f,
                "COVE_E_MAP_EVIDENCE_INVALID: semantic mapping evidence invalid"
            ),
            CoveError::BufferTooShort => {
                write!(
                    f,
                    "COVE_E_OFFSET_RANGE: buffer too short to parse structure"
                )
            }
            CoveError::ReservedNotZero => {
                write!(f, "COVE_E_BAD_SECTION: reserved field is non-zero")
            }
            CoveError::Io(s) => write!(f, "I/O error: {s}"),
            CoveError::UnsupportedEncoding(s) => write!(f, "COVE_E_UNSUPPORTED_ENCODING: {s}"),
        }
    }
}

impl CoveError {
    /// Complete Spec §76 code inventory surfaced by [`Self::spec_code`].
    pub const ALL_SPEC_CODES: [&'static str; 31] = [
        "COVE_E_BAD_MAGIC",
        "COVE_E_BAD_VERSION",
        "COVE_E_UNKNOWN_REQUIRED_FEATURE",
        "COVE_E_CHECKSUM_MISMATCH",
        "COVE_E_DIGEST_MISMATCH",
        "COVE_E_OFFSET_RANGE",
        "COVE_E_ARITH_OVERFLOW",
        "COVE_E_BAD_SECTION",
        "COVE_E_BAD_SCHEMA",
        "COVE_E_BAD_LOGICAL_PHYSICAL_PAIR",
        "COVE_E_DICT_MISS",
        "COVE_E_BAD_FILECODE",
        "COVE_E_BAD_NUMCODE",
        "COVE_E_BAD_DOMAIN",
        "COVE_E_BAD_STATS",
        "COVE_E_BAD_INDEX",
        "COVE_E_BAD_EXTENSION",
        "COVE_E_BAD_ENGINE_PROFILE",
        "COVE_E_EXECUTION_CODE_MAP",
        "COVE_E_HARBOR_MOUNT_LEASE",
        "COVE_E_REF_INVALID",
        "COVE_E_NOT_SELF_CONTAINED",
        "COVE_E_SEGMENT_CORRUPT",
        "COVE_E_PAGE_CORRUPT",
        "COVE_E_REDACTION_POLICY",
        "COVE_E_SIDECAR_STALE",
        "COVE_E_MAP_INVALID",
        "COVE_E_MAP_FUNCTION_UNDECLARED",
        "COVE_E_MAP_IDENTITY_CONFLICT",
        "COVE_E_MAP_SOURCE_STALE",
        "COVE_E_MAP_EVIDENCE_INVALID",
    ];

    /// Return the closest Spec §76 error code for this error.
    ///
    /// Some implementation-level errors such as [`CoveError::BufferTooShort`] and
    /// [`CoveError::ReservedNotZero`] are normalized to their Spec §76 structural
    /// category so callers can report stable conformance diagnostics.
    pub fn spec_code(&self) -> Option<&'static str> {
        match self {
            CoveError::BadMagic => Some("COVE_E_BAD_MAGIC"),
            CoveError::BadVersion => Some("COVE_E_BAD_VERSION"),
            CoveError::UnknownRequiredFeature(_) => Some("COVE_E_UNKNOWN_REQUIRED_FEATURE"),
            CoveError::ChecksumMismatch => Some("COVE_E_CHECKSUM_MISMATCH"),
            CoveError::DigestMismatch => Some("COVE_E_DIGEST_MISMATCH"),
            CoveError::OffsetRange | CoveError::BufferTooShort => Some("COVE_E_OFFSET_RANGE"),
            CoveError::ArithOverflow => Some("COVE_E_ARITH_OVERFLOW"),
            CoveError::BadSection(_) | CoveError::ReservedNotZero => Some("COVE_E_BAD_SECTION"),
            CoveError::BadSchema(_) => Some("COVE_E_BAD_SCHEMA"),
            CoveError::BadLogicalPhysicalPair => Some("COVE_E_BAD_LOGICAL_PHYSICAL_PAIR"),
            CoveError::DictMiss => Some("COVE_E_DICT_MISS"),
            CoveError::BadFileCode => Some("COVE_E_BAD_FILECODE"),
            CoveError::BadNumCode => Some("COVE_E_BAD_NUMCODE"),
            CoveError::BadDomain => Some("COVE_E_BAD_DOMAIN"),
            CoveError::BadStats => Some("COVE_E_BAD_STATS"),
            CoveError::BadIndex => Some("COVE_E_BAD_INDEX"),
            CoveError::BadExtension => Some("COVE_E_BAD_EXTENSION"),
            CoveError::BadEngineProfile => Some("COVE_E_BAD_ENGINE_PROFILE"),
            CoveError::ExecutionCodeMap => Some("COVE_E_EXECUTION_CODE_MAP"),
            CoveError::HarborMountLease => Some("COVE_E_HARBOR_MOUNT_LEASE"),
            CoveError::RefInvalid => Some("COVE_E_REF_INVALID"),
            CoveError::NotSelfContained => Some("COVE_E_NOT_SELF_CONTAINED"),
            CoveError::SegmentCorrupt => Some("COVE_E_SEGMENT_CORRUPT"),
            CoveError::PageCorrupt => Some("COVE_E_PAGE_CORRUPT"),
            CoveError::RedactionPolicy => Some("COVE_E_REDACTION_POLICY"),
            CoveError::SidecarStale => Some("COVE_E_SIDECAR_STALE"),
            CoveError::MapInvalid => Some("COVE_E_MAP_INVALID"),
            CoveError::MapFunctionUndeclared => Some("COVE_E_MAP_FUNCTION_UNDECLARED"),
            CoveError::MapIdentityConflict => Some("COVE_E_MAP_IDENTITY_CONFLICT"),
            CoveError::MapSourceStale => Some("COVE_E_MAP_SOURCE_STALE"),
            CoveError::MapEvidenceInvalid => Some("COVE_E_MAP_EVIDENCE_INVALID"),
            CoveError::Io(_) | CoveError::UnsupportedEncoding(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn spec_76_errors_expose_stable_codes() {
        assert_eq!(CoveError::BadMagic.spec_code(), Some("COVE_E_BAD_MAGIC"));
        assert_eq!(
            CoveError::UnknownRequiredFeature(1).spec_code(),
            Some("COVE_E_UNKNOWN_REQUIRED_FEATURE")
        );
        assert_eq!(
            CoveError::BufferTooShort.spec_code(),
            Some("COVE_E_OFFSET_RANGE")
        );
        assert_eq!(
            CoveError::ReservedNotZero.spec_code(),
            Some("COVE_E_BAD_SECTION")
        );
        assert_eq!(
            CoveError::MapInvalid.spec_code(),
            Some("COVE_E_MAP_INVALID")
        );
        assert_eq!(CoveError::Io("disk".into()).spec_code(), None);
    }

    #[test]
    fn normalized_structural_errors_render_with_spec_codes() {
        assert_eq!(
            CoveError::BufferTooShort.to_string(),
            "COVE_E_OFFSET_RANGE: buffer too short to parse structure"
        );
        assert_eq!(
            CoveError::ReservedNotZero.to_string(),
            "COVE_E_BAD_SECTION: reserved field is non-zero"
        );
    }

    #[test]
    fn spec_76_code_inventory_is_unique() {
        let unique = CoveError::ALL_SPEC_CODES
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), CoveError::ALL_SPEC_CODES.len());
        assert!(unique.contains("COVE_E_BAD_MAGIC"));
        assert!(unique.contains("COVE_E_SIDECAR_STALE"));
        assert!(unique.contains("COVE_E_MAP_EVIDENCE_INVALID"));
    }
}

impl std::error::Error for CoveError {}

impl From<std::io::Error> for CoveError {
    fn from(e: std::io::Error) -> Self {
        CoveError::Io(e.to_string())
    }
}
