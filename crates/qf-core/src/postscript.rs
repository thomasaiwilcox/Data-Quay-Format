//! Quay Format (QF) v1.0 — Postscript and section spec structures.
//!
//! Corresponds to Section 12 of the QF v1.0 specification.
//!
//! The postscript occupies the final bytes of every QF file, immediately
//! before the trailing tag:
//!
//! ```text
//! [postscript bytes  : postscript_len bytes]
//! [postscript_version: u16]
//! [postscript_len    : u16]
//! [magic             : b"QYF1"]
//! ```
//!
//! `postscript_len` excludes `postscript_version`, `postscript_len`, and the
//! trailing magic.  For v1, `postscript_len` is always 64.

use crate::{
    checksum,
    constants::{
        CompressionCodec, MAGIC_QF, POSTSCRIPT_LEN, POSTSCRIPT_VERSION_V1, SECTION_SPEC_LEN,
    },
    error::QfError,
};

// ── QfSectionSpecV1 ───────────────────────────────────────────────────────────

/// Size in bytes of the serialised [`QfSectionSpecV1`].
pub const SECTION_SPEC_SIZE: usize = SECTION_SPEC_LEN;

/// Location/compression descriptor for a single section or the footer.
///
/// Used in both the postscript (to locate the footer) and in the footer's
/// section directory (to locate every individual section).
///
/// Corresponds to `QfSectionSpecV1` in Section 12 of the specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfSectionSpecV1 {
    /// Byte offset of the section within the file.
    pub offset: u64,
    /// Byte length of the section on disk (after compression).
    pub length: u64,
    /// Uncompressed byte length of the section.
    pub uncompressed_length: u64,
    /// Compression codec applied to the section payload.
    pub compression: u8,
    /// Encryption scheme — MUST be 0 (None) in v1.
    pub encryption: u8,
    /// `log2` of the section's alignment (advisory).
    pub alignment_log2: u8,
    /// Section-level flags.
    pub flags: u8,
    /// CRC32C of the section's on-disk bytes (after compression).
    pub crc32c: u32,
    /// Reserved — MUST be zero in v1.
    pub reserved: u32,
}

impl QfSectionSpecV1 {
    /// Serialise to its 36-byte wire representation.
    pub fn serialize(&self) -> [u8; SECTION_SPEC_SIZE] {
        let mut buf = [0u8; SECTION_SPEC_SIZE];
        buf[0..8].copy_from_slice(&self.offset.to_le_bytes());
        buf[8..16].copy_from_slice(&self.length.to_le_bytes());
        buf[16..24].copy_from_slice(&self.uncompressed_length.to_le_bytes());
        buf[24] = self.compression;
        buf[25] = self.encryption;
        buf[26] = self.alignment_log2;
        buf[27] = self.flags;
        buf[28..32].copy_from_slice(&self.crc32c.to_le_bytes());
        buf[32..36].copy_from_slice(&self.reserved.to_le_bytes());
        buf
    }

    /// Parse a 36-byte wire buffer.
    pub fn parse(buf: &[u8]) -> Result<Self, QfError> {
        if buf.len() < SECTION_SPEC_SIZE {
            return Err(QfError::BufferTooShort);
        }
        let offset = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let length = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let uncompressed_length = u64::from_le_bytes(buf[16..24].try_into().unwrap());
        let compression = buf[24];
        let encryption = buf[25];
        let alignment_log2 = buf[26];
        let flags = buf[27];
        let crc32c = u32::from_le_bytes(buf[28..32].try_into().unwrap());
        let reserved = u32::from_le_bytes(buf[32..36].try_into().unwrap());

        if CompressionCodec::from_u8(compression).is_none() {
            return Err(QfError::BadSection(format!(
                "unknown compression codec {compression}"
            )));
        }
        if encryption != 0 {
            return Err(QfError::BadSection(format!(
                "encryption must be 0 in v1, got {encryption}"
            )));
        }
        if reserved != 0 {
            return Err(QfError::ReservedNotZero);
        }

        Ok(Self {
            offset,
            length,
            uncompressed_length,
            compression,
            encryption,
            alignment_log2,
            flags,
            crc32c,
            reserved,
        })
    }

    /// Return the end offset (exclusive) of this section, checking for overflow.
    pub fn end_offset(&self) -> Result<u64, QfError> {
        self.offset
            .checked_add(self.length)
            .ok_or(QfError::ArithOverflow)
    }
}

// ── QfPostscriptV1 ────────────────────────────────────────────────────────────

/// Postscript size in bytes (the `postscript_len` value for v1).
pub const POSTSCRIPT_SIZE: usize = POSTSCRIPT_LEN;

/// Size of the fixed tail after the postscript payload:
/// `postscript_version` (2) + `postscript_len` (2) + magic (4) = 8 bytes.
pub const POSTSCRIPT_TAIL_SIZE: usize = 8;

/// Total size of the postscript region at end of file (payload + fixed tail).
pub const POSTSCRIPT_TOTAL_SIZE: usize = POSTSCRIPT_SIZE + POSTSCRIPT_TAIL_SIZE;

/// Byte offset of the CRC32C field inside the serialised postscript payload.
const PS_CHECKSUM_OFFSET: usize = 60;

/// Parsed QF v1 postscript.
///
/// Discovered by reading the last [`POSTSCRIPT_TOTAL_SIZE`] bytes of the file
/// (or up to the last 64 KiB as per the spec recommendation).
///
/// Corresponds to `QfPostscriptV1` in Section 12 of the specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfPostscriptV1 {
    /// Required feature bits — MUST match the header's `required_features`.
    pub required_features: u64,
    /// Optional feature bits — MUST match the header's `optional_features`.
    pub optional_features: u64,
    /// Total file length in bytes — MUST equal the actual file length.
    pub file_len: u64,
    /// Location, size, and CRC of the footer.
    pub footer: QfSectionSpecV1,
    /// CRC32C of the 64-byte postscript payload with this field zeroed.
    pub checksum: u32,
}

impl QfPostscriptV1 {
    /// Serialise to the 64-byte postscript payload (without the trailing tag).
    ///
    /// The `checksum` is recomputed from the other fields; the caller's stored
    /// `checksum` value is ignored.
    pub fn serialize(&self) -> [u8; POSTSCRIPT_SIZE] {
        let mut buf = [0u8; POSTSCRIPT_SIZE];
        buf[0..8].copy_from_slice(&self.required_features.to_le_bytes());
        buf[8..16].copy_from_slice(&self.optional_features.to_le_bytes());
        buf[16..24].copy_from_slice(&self.file_len.to_le_bytes());
        let footer_bytes = self.footer.serialize();
        buf[24..60].copy_from_slice(&footer_bytes);
        // Checksum field at [60..64] stays zero during CRC computation.
        let crc = checksum::crc32c(&buf);
        buf[PS_CHECKSUM_OFFSET..PS_CHECKSUM_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    /// Serialise to the complete postscript tail (payload + version + len + magic).
    ///
    /// This is the data written at the very end of the file.
    pub fn serialize_tail(&self) -> [u8; POSTSCRIPT_TOTAL_SIZE] {
        let mut tail = [0u8; POSTSCRIPT_TOTAL_SIZE];
        let payload = self.serialize();
        tail[0..POSTSCRIPT_SIZE].copy_from_slice(&payload);
        let ps_len = POSTSCRIPT_SIZE as u16;
        tail[POSTSCRIPT_SIZE..POSTSCRIPT_SIZE + 2]
            .copy_from_slice(&POSTSCRIPT_VERSION_V1.to_le_bytes());
        tail[POSTSCRIPT_SIZE + 2..POSTSCRIPT_SIZE + 4].copy_from_slice(&ps_len.to_le_bytes());
        tail[POSTSCRIPT_SIZE + 4..POSTSCRIPT_SIZE + 8].copy_from_slice(&MAGIC_QF);
        tail
    }

    /// Parse the postscript from the final bytes of a file buffer.
    ///
    /// `file_data` should be the entire file or at least the last
    /// [`POSTSCRIPT_TOTAL_SIZE`] bytes.
    ///
    /// The caller MUST subsequently verify that `postscript.file_len` equals
    /// the actual file length and that the footer section is within bounds.
    pub fn parse_from_tail(file_data: &[u8]) -> Result<Self, QfError> {
        let file_len = file_data.len();
        if file_len < POSTSCRIPT_TOTAL_SIZE {
            return Err(QfError::BufferTooShort);
        }

        let tail_start = file_len - POSTSCRIPT_TAIL_SIZE;
        let magic: [u8; 4] = file_data[tail_start + 4..tail_start + 8]
            .try_into()
            .unwrap();
        if magic != MAGIC_QF {
            return Err(QfError::BadMagic);
        }

        let ps_version =
            u16::from_le_bytes(file_data[tail_start..tail_start + 2].try_into().unwrap());
        let ps_len = u16::from_le_bytes(
            file_data[tail_start + 2..tail_start + 4]
                .try_into()
                .unwrap(),
        );

        if ps_version != POSTSCRIPT_VERSION_V1 {
            return Err(QfError::BadVersion);
        }
        let ps_len_usize = ps_len as usize;

        if ps_len_usize > file_len.saturating_sub(POSTSCRIPT_TAIL_SIZE) {
            return Err(QfError::OffsetRange);
        }
        let ps_start = tail_start - ps_len_usize;
        let ps_buf = &file_data[ps_start..ps_start + ps_len_usize];

        Self::parse_payload(ps_buf)
    }

    /// Parse the 64-byte postscript payload (without the trailing tag).
    fn parse_payload(buf: &[u8]) -> Result<Self, QfError> {
        if buf.len() < POSTSCRIPT_SIZE {
            return Err(QfError::BufferTooShort);
        }
        let buf = &buf[..POSTSCRIPT_SIZE];

        // Verify checksum first.
        let stored_crc = u32::from_le_bytes(
            buf[PS_CHECKSUM_OFFSET..PS_CHECKSUM_OFFSET + 4]
                .try_into()
                .unwrap(),
        );
        let mut check_buf = [0u8; POSTSCRIPT_SIZE];
        check_buf.copy_from_slice(buf);
        check_buf[PS_CHECKSUM_OFFSET..PS_CHECKSUM_OFFSET + 4].copy_from_slice(&[0, 0, 0, 0]);
        if checksum::crc32c(&check_buf) != stored_crc {
            return Err(QfError::ChecksumMismatch);
        }

        let required_features = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let optional_features = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let file_len = u64::from_le_bytes(buf[16..24].try_into().unwrap());
        let footer = QfSectionSpecV1::parse(&buf[24..60])?;

        Ok(Self {
            required_features,
            optional_features,
            file_len,
            footer,
            checksum: stored_crc,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_spec() -> QfSectionSpecV1 {
        QfSectionSpecV1 {
            offset: 128,
            length: 44,
            uncompressed_length: 44,
            compression: 0,
            encryption: 0,
            alignment_log2: 0,
            flags: 0,
            crc32c: 0,
            reserved: 0,
        }
    }

    fn minimal_postscript(file_len: u64) -> QfPostscriptV1 {
        QfPostscriptV1 {
            required_features: 0,
            optional_features: 0,
            file_len,
            footer: minimal_spec(),
            checksum: 0,
        }
    }

    #[test]
    fn section_spec_roundtrip() {
        let spec = minimal_spec();
        let bytes = spec.serialize();
        assert_eq!(bytes.len(), SECTION_SPEC_SIZE);
        let parsed = QfSectionSpecV1::parse(&bytes).expect("parse should succeed");
        assert_eq!(parsed.offset, 128);
        assert_eq!(parsed.length, 44);
        assert_eq!(parsed.compression, 0);
    }

    #[test]
    fn postscript_tail_roundtrip() {
        // A minimal QF file: header (128) + footer (44) + postscript tail (72)
        // Total: 244 bytes
        let file_len: u64 = 244;
        let ps = minimal_postscript(file_len);
        let tail = ps.serialize_tail();
        assert_eq!(tail.len(), POSTSCRIPT_TOTAL_SIZE);

        // Build a fake file buffer: zeros for header+footer, then the tail.
        let mut file = vec![0u8; file_len as usize];
        file[file_len as usize - POSTSCRIPT_TOTAL_SIZE..].copy_from_slice(&tail);

        let parsed =
            QfPostscriptV1::parse_from_tail(&file).expect("parse_from_tail should succeed");
        assert_eq!(parsed.file_len, file_len);
        assert_eq!(parsed.footer.offset, 128);
        assert_eq!(parsed.footer.length, 44);
    }

    #[test]
    fn section_spec_reserved_nonzero_rejected() {
        let mut bytes = minimal_spec().serialize();
        bytes[32..36].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            QfSectionSpecV1::parse(&bytes),
            Err(QfError::ReservedNotZero)
        );
    }

    #[test]
    fn section_spec_unknown_compression_rejected() {
        let mut bytes = minimal_spec().serialize();
        bytes[24] = 255;
        assert!(matches!(
            QfSectionSpecV1::parse(&bytes),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn section_spec_end_offset_overflow_rejected() {
        let spec = QfSectionSpecV1 {
            offset: u64::MAX,
            length: 1,
            ..minimal_spec()
        };
        assert_eq!(spec.end_offset(), Err(QfError::ArithOverflow));
    }

    #[test]
    fn bad_postscript_version_rejected() {
        let file_len: u64 = 244;
        let ps = minimal_postscript(file_len);
        let mut tail = ps.serialize_tail();
        tail[POSTSCRIPT_SIZE..POSTSCRIPT_SIZE + 2].copy_from_slice(&2u16.to_le_bytes());
        let mut file = vec![0u8; file_len as usize];
        file[file_len as usize - POSTSCRIPT_TOTAL_SIZE..].copy_from_slice(&tail);
        assert_eq!(
            QfPostscriptV1::parse_from_tail(&file),
            Err(QfError::BadVersion)
        );
    }

    #[test]
    fn bad_postscript_checksum_rejected() {
        let file_len: u64 = 244;
        let ps = minimal_postscript(file_len);
        let mut tail = ps.serialize_tail();
        tail[0] ^= 0x01;
        let mut file = vec![0u8; file_len as usize];
        file[file_len as usize - POSTSCRIPT_TOTAL_SIZE..].copy_from_slice(&tail);
        assert_eq!(
            QfPostscriptV1::parse_from_tail(&file),
            Err(QfError::ChecksumMismatch)
        );
    }

    #[test]
    fn truncated_postscript_len_rejected() {
        let file_len: u64 = 244;
        let ps = minimal_postscript(file_len);
        let mut tail = ps.serialize_tail();
        tail[POSTSCRIPT_SIZE + 2..POSTSCRIPT_SIZE + 4].copy_from_slice(&32u16.to_le_bytes());
        let mut file = vec![0u8; file_len as usize];
        file[file_len as usize - POSTSCRIPT_TOTAL_SIZE..].copy_from_slice(&tail);
        assert_eq!(
            QfPostscriptV1::parse_from_tail(&file),
            Err(QfError::BufferTooShort)
        );
    }

    #[test]
    fn bad_magic_rejected() {
        let file_len: u64 = 244;
        let ps = minimal_postscript(file_len);
        let mut tail = ps.serialize_tail();
        tail[POSTSCRIPT_SIZE + 4] = b'X'; // corrupt magic
        let mut file = vec![0u8; file_len as usize];
        file[file_len as usize - POSTSCRIPT_TOTAL_SIZE..].copy_from_slice(&tail);
        assert_eq!(
            QfPostscriptV1::parse_from_tail(&file),
            Err(QfError::BadMagic)
        );
    }
}
