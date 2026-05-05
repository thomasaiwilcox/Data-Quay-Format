//! Quay Format (QF) v1.0 — Error types.
//!
//! Corresponds to Section 75 of the QF v1.0 specification.

use std::fmt;

/// All errors that can occur while reading, writing, or validating a QF file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QfError {
    /// Missing or invalid magic bytes (QF_E_BAD_MAGIC).
    BadMagic,
    /// Unsupported QF version (QF_E_BAD_VERSION).
    BadVersion,
    /// Unknown required feature bit set (QF_E_UNKNOWN_REQUIRED_FEATURE).
    UnknownRequiredFeature(u64),
    /// Checksum mismatch — header, postscript, footer, section, segment, or page
    /// (QF_E_CHECKSUM_MISMATCH).
    ChecksumMismatch,
    /// Cryptographic digest mismatch (QF_E_DIGEST_MISMATCH).
    DigestMismatch,
    /// Offset/length/count exceeds file bounds (QF_E_OFFSET_RANGE).
    OffsetRange,
    /// Arithmetic overflow in offset/count/size (QF_E_ARITH_OVERFLOW).
    ArithOverflow,
    /// Section malformed or invalid (QF_E_BAD_SECTION).
    BadSection(String),
    /// Catalog or schema malformed (QF_E_BAD_SCHEMA).
    BadSchema(String),
    /// Logical type incompatible with physical kind (QF_E_BAD_LOGICAL_PHYSICAL_PAIR).
    BadLogicalPhysicalPair,
    /// FileCode missing from dictionary (QF_E_DICT_MISS).
    DictMiss,
    /// FileCode outside dictionary range (QF_E_BAD_FILECODE).
    BadFileCode,
    /// NumCode invalid for declared logical type (QF_E_BAD_NUMCODE).
    BadNumCode,
    /// ColumnDomain invalid (QF_E_BAD_DOMAIN).
    BadDomain,
    /// Statistics invalid or unsafe (QF_E_BAD_STATS).
    BadStats,
    /// Optional index invalid or corrupt (QF_E_BAD_INDEX).
    BadIndex,
    /// Extension invalid or required extension unsupported (QF_E_BAD_EXTENSION).
    BadExtension,
    /// Engine profile invalid or unsupported when required (QF_E_BAD_ENGINE_PROFILE).
    BadEngineProfile,
    /// Engine-local code mapping failed (QF_E_EXECUTION_CODE_MAP).
    ExecutionCodeMap,
    /// Harbor code lease resolution failed (QF_E_HARBOR_MOUNT_LEASE).
    HarborMountLease,
    /// QF-O prev_ref invalid (QF_E_REF_INVALID).
    RefInvalid,
    /// QF-O chain lacks baseline/snapshot/full chain (QF_E_NOT_SELF_CONTAINED).
    NotSelfContained,
    /// Segment structure invalid (QF_E_SEGMENT_CORRUPT).
    SegmentCorrupt,
    /// Page structure invalid (QF_E_PAGE_CORRUPT).
    PageCorrupt,
    /// Redacted value cannot be surfaced under current policy (QF_E_REDACTION_POLICY).
    RedactionPolicy,
    /// QFX/QFM sidecar does not match referenced QF (QF_E_SIDECAR_STALE).
    SidecarStale,
    /// Input buffer is too short to parse the requested structure.
    BufferTooShort,
    /// A field that MUST be zero contained a non-zero value.
    ReservedNotZero,
    /// I/O error during file reading or writing.
    Io(String),
    /// Encoding kind is not supported by this implementation (QF_E_UNSUPPORTED_ENCODING).
    UnsupportedEncoding(String),
}

impl fmt::Display for QfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QfError::BadMagic => write!(f, "QF_E_BAD_MAGIC: missing or invalid magic bytes"),
            QfError::BadVersion => write!(f, "QF_E_BAD_VERSION: unsupported QF version"),
            QfError::UnknownRequiredFeature(bits) => {
                write!(
                    f,
                    "QF_E_UNKNOWN_REQUIRED_FEATURE: unknown required feature bits 0x{bits:016x}"
                )
            }
            QfError::ChecksumMismatch => write!(f, "QF_E_CHECKSUM_MISMATCH: CRC32C mismatch"),
            QfError::DigestMismatch => {
                write!(f, "QF_E_DIGEST_MISMATCH: cryptographic digest mismatch")
            }
            QfError::OffsetRange => {
                write!(f, "QF_E_OFFSET_RANGE: offset or length out of file bounds")
            }
            QfError::ArithOverflow => write!(
                f,
                "QF_E_ARITH_OVERFLOW: arithmetic overflow in offset/count"
            ),
            QfError::BadSection(s) => write!(f, "QF_E_BAD_SECTION: {s}"),
            QfError::BadSchema(s) => write!(f, "QF_E_BAD_SCHEMA: {s}"),
            QfError::BadLogicalPhysicalPair => {
                write!(
                    f,
                    "QF_E_BAD_LOGICAL_PHYSICAL_PAIR: logical type incompatible with physical kind"
                )
            }
            QfError::DictMiss => write!(f, "QF_E_DICT_MISS: FileCode missing from dictionary"),
            QfError::BadFileCode => {
                write!(f, "QF_E_BAD_FILECODE: FileCode outside dictionary range")
            }
            QfError::BadNumCode => write!(
                f,
                "QF_E_BAD_NUMCODE: NumCode invalid for declared logical type"
            ),
            QfError::BadDomain => write!(f, "QF_E_BAD_DOMAIN: ColumnDomain invalid"),
            QfError::BadStats => write!(f, "QF_E_BAD_STATS: statistics invalid or unsafe"),
            QfError::BadIndex => write!(f, "QF_E_BAD_INDEX: optional index invalid or corrupt"),
            QfError::BadExtension => {
                write!(f, "QF_E_BAD_EXTENSION: extension invalid or unsupported")
            }
            QfError::BadEngineProfile => {
                write!(
                    f,
                    "QF_E_BAD_ENGINE_PROFILE: engine profile invalid or unsupported"
                )
            }
            QfError::ExecutionCodeMap => {
                write!(f, "QF_E_EXECUTION_CODE_MAP: execution code mapping failed")
            }
            QfError::HarborMountLease => {
                write!(f, "QF_E_HARBOR_MOUNT_LEASE: Harbor lease resolution failed")
            }
            QfError::RefInvalid => write!(f, "QF_E_REF_INVALID: QF-O prev_ref invalid"),
            QfError::NotSelfContained => {
                write!(f, "QF_E_NOT_SELF_CONTAINED: QF-O chain not self-contained")
            }
            QfError::SegmentCorrupt => write!(f, "QF_E_SEGMENT_CORRUPT: segment structure invalid"),
            QfError::PageCorrupt => write!(f, "QF_E_PAGE_CORRUPT: page structure invalid"),
            QfError::RedactionPolicy => write!(
                f,
                "QF_E_REDACTION_POLICY: redacted value cannot be surfaced"
            ),
            QfError::SidecarStale => write!(
                f,
                "QF_E_SIDECAR_STALE: QFX/QFM sidecar does not match QF file"
            ),
            QfError::BufferTooShort => {
                write!(f, "QF_E_OFFSET_RANGE: buffer too short to parse structure")
            }
            QfError::ReservedNotZero => write!(f, "QF_E_BAD_SECTION: reserved field is non-zero"),
            QfError::Io(s) => write!(f, "I/O error: {s}"),
            QfError::UnsupportedEncoding(s) => write!(f, "QF_E_UNSUPPORTED_ENCODING: {s}"),
        }
    }
}

impl QfError {
    /// Return the closest Spec §75 error code for this error.
    ///
    /// Some implementation-level errors such as [`QfError::BufferTooShort`] and
    /// [`QfError::ReservedNotZero`] are normalized to their Spec §75 structural
    /// category so callers can report stable conformance diagnostics.
    pub fn spec_code(&self) -> Option<&'static str> {
        match self {
            QfError::BadMagic => Some("QF_E_BAD_MAGIC"),
            QfError::BadVersion => Some("QF_E_BAD_VERSION"),
            QfError::UnknownRequiredFeature(_) => Some("QF_E_UNKNOWN_REQUIRED_FEATURE"),
            QfError::ChecksumMismatch => Some("QF_E_CHECKSUM_MISMATCH"),
            QfError::DigestMismatch => Some("QF_E_DIGEST_MISMATCH"),
            QfError::OffsetRange | QfError::BufferTooShort => Some("QF_E_OFFSET_RANGE"),
            QfError::ArithOverflow => Some("QF_E_ARITH_OVERFLOW"),
            QfError::BadSection(_) | QfError::ReservedNotZero => Some("QF_E_BAD_SECTION"),
            QfError::BadSchema(_) => Some("QF_E_BAD_SCHEMA"),
            QfError::BadLogicalPhysicalPair => Some("QF_E_BAD_LOGICAL_PHYSICAL_PAIR"),
            QfError::DictMiss => Some("QF_E_DICT_MISS"),
            QfError::BadFileCode => Some("QF_E_BAD_FILECODE"),
            QfError::BadNumCode => Some("QF_E_BAD_NUMCODE"),
            QfError::BadDomain => Some("QF_E_BAD_DOMAIN"),
            QfError::BadStats => Some("QF_E_BAD_STATS"),
            QfError::BadIndex => Some("QF_E_BAD_INDEX"),
            QfError::BadExtension => Some("QF_E_BAD_EXTENSION"),
            QfError::BadEngineProfile => Some("QF_E_BAD_ENGINE_PROFILE"),
            QfError::ExecutionCodeMap => Some("QF_E_EXECUTION_CODE_MAP"),
            QfError::HarborMountLease => Some("QF_E_HARBOR_MOUNT_LEASE"),
            QfError::RefInvalid => Some("QF_E_REF_INVALID"),
            QfError::NotSelfContained => Some("QF_E_NOT_SELF_CONTAINED"),
            QfError::SegmentCorrupt => Some("QF_E_SEGMENT_CORRUPT"),
            QfError::PageCorrupt => Some("QF_E_PAGE_CORRUPT"),
            QfError::RedactionPolicy => Some("QF_E_REDACTION_POLICY"),
            QfError::SidecarStale => Some("QF_E_SIDECAR_STALE"),
            QfError::Io(_) | QfError::UnsupportedEncoding(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_75_errors_expose_stable_codes() {
        assert_eq!(QfError::BadMagic.spec_code(), Some("QF_E_BAD_MAGIC"));
        assert_eq!(
            QfError::UnknownRequiredFeature(1).spec_code(),
            Some("QF_E_UNKNOWN_REQUIRED_FEATURE")
        );
        assert_eq!(
            QfError::BufferTooShort.spec_code(),
            Some("QF_E_OFFSET_RANGE")
        );
        assert_eq!(
            QfError::ReservedNotZero.spec_code(),
            Some("QF_E_BAD_SECTION")
        );
        assert_eq!(QfError::Io("disk".into()).spec_code(), None);
    }

    #[test]
    fn normalized_structural_errors_render_with_spec_codes() {
        assert_eq!(
            QfError::BufferTooShort.to_string(),
            "QF_E_OFFSET_RANGE: buffer too short to parse structure"
        );
        assert_eq!(
            QfError::ReservedNotZero.to_string(),
            "QF_E_BAD_SECTION: reserved field is non-zero"
        );
    }
}

impl std::error::Error for QfError {}

impl From<std::io::Error> for QfError {
    fn from(e: std::io::Error) -> Self {
        QfError::Io(e.to_string())
    }
}
