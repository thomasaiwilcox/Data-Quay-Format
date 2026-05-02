//! Quay Format (QF) v1.0 — File header (`QfHeaderV1`).
//!
//! Corresponds to Section 10 of the QF v1.0 specification.
//!
//! The header is a fixed 128-byte structure at offset 0 of every QF file.
//! All multi-byte integers are little-endian.

use crate::{
    checksum,
    constants::{
        PrimaryProfile, ProducerScopeKind, ENDIANNESS_LITTLE, HEADER_LEN_V1,
        KNOWN_FEATURE_BITS_MASK, MAGIC_LEGACY_DRAFT, MAGIC_QF, VERSION_MAJOR_V1,
    },
    error::QfError,
};

/// Serialised size of the header in bytes.  Always 128 for v1.
pub const HEADER_SIZE: usize = 128;

/// Byte offset of the `checksum` field inside the serialised header.
const CHECKSUM_OFFSET: usize = 124;

/// Parsed QF v1 file header.
///
/// Corresponds to `QfHeaderV1` in Section 10 of the specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfHeaderV1 {
    /// Magic bytes — must be `b"QYF1"` (or `b"HQF1"` in legacy compatibility mode).
    pub magic: [u8; 4],
    /// Fixed at 128 for v1.
    pub header_len: u16,
    /// Major version — must be 1.
    pub version_major: u16,
    /// Minor version — 0 for the initial release.
    pub version_minor: u16,
    /// Primary profile of the file (see [`PrimaryProfile`]).
    pub primary_profile: u8,
    /// Byte order indicator — 1 means little-endian (the only valid v1 value).
    pub endianness: u8,
    /// File-level flags (usage reserved for future versions; MUST be zero in v1).
    pub flags: u32,
    /// Required feature bits that a reader MUST understand (Section 11).
    pub required_features: u64,
    /// Optional feature bits that a reader MAY ignore (Section 11).
    pub optional_features: u64,
    /// Globally unique file identifier (16-byte UUID).
    pub file_id: [u8; 16],
    /// Producer-scope identifier (16-byte UUID or equivalent stable ID).
    pub producer_scope_id: [u8; 16],
    /// Kind of the producer scope (see [`ProducerScopeKind`]).
    pub producer_scope_kind: u16,
    /// Reserved scope flags — MUST be zero in v1.
    pub reserved_scope_flags: u16,
    /// File creation timestamp in microseconds since Unix epoch.
    pub created_at_us: i64,
    /// Reserved bytes — MUST be zero in v1.
    pub reserved: [u8; 48],
    /// CRC32C of the entire 128-byte header with this field set to zero.
    pub checksum: u32,
}

impl QfHeaderV1 {
    /// Serialise the header to its canonical 128-byte little-endian wire format.
    ///
    /// The `checksum` field in the returned bytes is computed automatically over
    /// the other 124 bytes with the checksum field zeroed as required by the spec.
    pub fn serialize(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4..6].copy_from_slice(&self.header_len.to_le_bytes());
        buf[6..8].copy_from_slice(&self.version_major.to_le_bytes());
        buf[8..10].copy_from_slice(&self.version_minor.to_le_bytes());
        buf[10] = self.primary_profile;
        buf[11] = self.endianness;
        buf[12..16].copy_from_slice(&self.flags.to_le_bytes());
        buf[16..24].copy_from_slice(&self.required_features.to_le_bytes());
        buf[24..32].copy_from_slice(&self.optional_features.to_le_bytes());
        buf[32..48].copy_from_slice(&self.file_id);
        buf[48..64].copy_from_slice(&self.producer_scope_id);
        buf[64..66].copy_from_slice(&self.producer_scope_kind.to_le_bytes());
        buf[66..68].copy_from_slice(&self.reserved_scope_flags.to_le_bytes());
        buf[68..76].copy_from_slice(&self.created_at_us.to_le_bytes());
        buf[76..124].copy_from_slice(&self.reserved);
        // Checksum field stays zero during computation (bytes [124..128]).
        let crc = checksum::crc32c(&buf);
        buf[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    /// Parse and validate a QF header from a 128-byte buffer.
    ///
    /// Per the spec, the checksum MUST validate before any other header field
    /// is trusted.
    ///
    /// Set `allow_legacy_magic` to `true` to also accept `b"HQF1"` (pre-v1
    /// Harbor drafts) in compatibility mode.
    pub fn parse(buf: &[u8], allow_legacy_magic: bool) -> Result<Self, QfError> {
        if buf.len() < HEADER_SIZE {
            return Err(QfError::BufferTooShort);
        }
        let buf = &buf[..HEADER_SIZE];

        // 1. Verify checksum before trusting any other field (Section 10 rule).
        let stored_crc = u32::from_le_bytes(
            buf[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4]
                .try_into()
                .unwrap(),
        );
        let mut check_buf = [0u8; HEADER_SIZE];
        check_buf.copy_from_slice(buf);
        check_buf[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&[0, 0, 0, 0]);
        if checksum::crc32c(&check_buf) != stored_crc {
            return Err(QfError::ChecksumMismatch);
        }

        let magic: [u8; 4] = buf[0..4].try_into().unwrap();
        if magic != MAGIC_QF && !(allow_legacy_magic && magic == MAGIC_LEGACY_DRAFT) {
            return Err(QfError::BadMagic);
        }

        let header_len = u16::from_le_bytes(buf[4..6].try_into().unwrap());
        if header_len != HEADER_LEN_V1 {
            return Err(QfError::BadSection(format!(
                "header_len is {header_len}, expected {HEADER_LEN_V1}"
            )));
        }

        let version_major = u16::from_le_bytes(buf[6..8].try_into().unwrap());
        if version_major != VERSION_MAJOR_V1 {
            return Err(QfError::BadVersion);
        }

        let version_minor = u16::from_le_bytes(buf[8..10].try_into().unwrap());

        let primary_profile = buf[10];
        if PrimaryProfile::from_u8(primary_profile).is_none() {
            return Err(QfError::BadSection(format!(
                "unknown primary_profile {primary_profile}"
            )));
        }
        let endianness = buf[11];
        if endianness != ENDIANNESS_LITTLE {
            return Err(QfError::BadSection(format!(
                "endianness field is {endianness}, only little-endian (1) is supported"
            )));
        }

        let flags = u32::from_le_bytes(buf[12..16].try_into().unwrap());
        let required_features = u64::from_le_bytes(buf[16..24].try_into().unwrap());
        let optional_features = u64::from_le_bytes(buf[24..32].try_into().unwrap());

        // Section 11: Readers MUST reject unknown required feature bits.
        let unknown_required = required_features & !KNOWN_FEATURE_BITS_MASK;
        if unknown_required != 0 {
            return Err(QfError::UnknownRequiredFeature(unknown_required));
        }

        let mut file_id = [0u8; 16];
        file_id.copy_from_slice(&buf[32..48]);

        let mut producer_scope_id = [0u8; 16];
        producer_scope_id.copy_from_slice(&buf[48..64]);

        let producer_scope_kind = u16::from_le_bytes(buf[64..66].try_into().unwrap());
        if ProducerScopeKind::from_u16(producer_scope_kind).is_none() {
            return Err(QfError::BadSection(format!(
                "unknown producer_scope_kind {producer_scope_kind}"
            )));
        }
        let reserved_scope_flags = u16::from_le_bytes(buf[66..68].try_into().unwrap());
        if reserved_scope_flags != 0 {
            return Err(QfError::ReservedNotZero);
        }
        let created_at_us = i64::from_le_bytes(buf[68..76].try_into().unwrap());

        let mut reserved = [0u8; 48];
        reserved.copy_from_slice(&buf[76..124]);
        if reserved.iter().any(|&b| b != 0) {
            return Err(QfError::ReservedNotZero);
        }

        Ok(Self {
            magic,
            header_len,
            version_major,
            version_minor,
            primary_profile,
            endianness,
            flags,
            required_features,
            optional_features,
            file_id,
            producer_scope_id,
            producer_scope_kind,
            reserved_scope_flags,
            created_at_us,
            reserved,
            checksum: stored_crc,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{PrimaryProfile, FEATURE_TABLE_PROFILE};

    fn minimal_header() -> QfHeaderV1 {
        QfHeaderV1 {
            magic: MAGIC_QF,
            header_len: HEADER_LEN_V1,
            version_major: VERSION_MAJOR_V1,
            version_minor: 0,
            primary_profile: PrimaryProfile::TableScan as u8,
            endianness: ENDIANNESS_LITTLE,
            flags: 0,
            required_features: FEATURE_TABLE_PROFILE,
            optional_features: 0,
            file_id: [0u8; 16],
            producer_scope_id: [0u8; 16],
            producer_scope_kind: 0,
            reserved_scope_flags: 0,
            created_at_us: 0,
            reserved: [0u8; 48],
            checksum: 0, // will be computed by serialize()
        }
    }

    #[test]
    fn roundtrip_header() {
        let hdr = minimal_header();
        let bytes = hdr.serialize();
        assert_eq!(bytes.len(), HEADER_SIZE);
        let parsed = QfHeaderV1::parse(&bytes, false).expect("parse should succeed");
        assert_eq!(parsed.magic, MAGIC_QF);
        assert_eq!(parsed.header_len, HEADER_LEN_V1);
        assert_eq!(parsed.version_major, VERSION_MAJOR_V1);
        assert_eq!(parsed.primary_profile, PrimaryProfile::TableScan as u8);
        assert_eq!(parsed.required_features, FEATURE_TABLE_PROFILE);
    }

    #[test]
    fn bad_magic_rejected() {
        let hdr = minimal_header();
        let mut bytes = hdr.serialize();
        bytes[0] = b'X';
        // Recompute CRC so the checksum still passes.
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&[0, 0, 0, 0]);
        let crc = checksum::crc32c(&bytes);
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(QfHeaderV1::parse(&bytes, false), Err(QfError::BadMagic));
    }

    #[test]
    fn checksum_mismatch_rejected() {
        let hdr = minimal_header();
        let mut bytes = hdr.serialize();
        bytes[0] = b'X'; // corrupt magic without updating CRC
        assert_eq!(
            QfHeaderV1::parse(&bytes, false),
            Err(QfError::ChecksumMismatch)
        );
    }

    #[test]
    fn reserved_nonzero_rejected() {
        let hdr = minimal_header();
        let mut bytes = hdr.serialize();
        bytes[80] = 1; // inside reserved[4..52] (offset 76+4 = 80)
                       // Recompute CRC.
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&[0, 0, 0, 0]);
        let crc = checksum::crc32c(&bytes);
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            QfHeaderV1::parse(&bytes, false),
            Err(QfError::ReservedNotZero)
        );
    }

    #[test]
    fn legacy_magic_accepted_in_compatibility_mode() {
        let mut hdr = minimal_header();
        hdr.magic = MAGIC_LEGACY_DRAFT;
        let bytes = hdr.serialize();
        // Rejected in strict mode.
        assert_eq!(QfHeaderV1::parse(&bytes, false), Err(QfError::BadMagic));
        // Accepted in compatibility mode.
        let parsed = QfHeaderV1::parse(&bytes, true).expect("should accept HQF1 in compat mode");
        assert_eq!(parsed.magic, MAGIC_LEGACY_DRAFT);
    }

    #[test]
    fn bad_version_rejected() {
        let hdr = minimal_header();
        let mut bytes = hdr.serialize();
        // Overwrite version_major with 2.
        bytes[6..8].copy_from_slice(&2u16.to_le_bytes());
        // Recompute CRC.
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&[0, 0, 0, 0]);
        let crc = checksum::crc32c(&bytes);
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(QfHeaderV1::parse(&bytes, false), Err(QfError::BadVersion));
    }

    #[test]
    fn bad_endianness_rejected() {
        let hdr = minimal_header();
        let mut bytes = hdr.serialize();
        // Overwrite endianness with 2 (not little-endian).
        bytes[11] = 2;
        // Recompute CRC.
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&[0, 0, 0, 0]);
        let crc = checksum::crc32c(&bytes);
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            QfHeaderV1::parse(&bytes, false),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn unknown_required_feature_rejected() {
        let mut hdr = minimal_header();
        // Set a bit far beyond the defined range.
        hdr.required_features = FEATURE_TABLE_PROFILE | 0x0000_0001_0000_0000;
        let bytes = hdr.serialize();
        assert_eq!(
            QfHeaderV1::parse(&bytes, false),
            Err(QfError::UnknownRequiredFeature(0x0000_0001_0000_0000))
        );
    }

    #[test]
    fn unknown_optional_feature_accepted() {
        // Unknown optional feature bits MUST be accepted (Section 11).
        let mut hdr = minimal_header();
        hdr.optional_features = 0x0000_0001_0000_0000;
        let bytes = hdr.serialize();
        let parsed =
            QfHeaderV1::parse(&bytes, false).expect("unknown optional feature should be accepted");
        assert_eq!(parsed.optional_features, 0x0000_0001_0000_0000);
    }
}
