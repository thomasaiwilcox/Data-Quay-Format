//! Spec §68 — QFX accelerator sidecar (spec-exact wire format).
//!
//! A QFX file extends one or more host QF files with rebuildable
//! acceleration metadata (lookup indexes, composite zone indexes, large
//! histograms, etc.) without mutating the host. Per Spec §68:
//!
//! * The file ends with the pattern
//!   `[postscript bytes][postscript_version: u16][postscript_len: u16][magic: "QYX1"]`.
//! * The header is [`QfxHeaderV1`] (Spec §68.1) and carries an
//!   `accelerator_id`, a `referenced_file_count`, and a CRC32C checksum.
//! * Each referenced file is described by a [`QfxReferencedFileV1`]
//!   (Spec §68.2) carrying `file_id`, `file_len`, `footer_crc32c`, and a
//!   variable-length cryptographic digest.
//!
//! Spec §68 Rules enforced by this module:
//! * QFX MUST be ignored if the referenced `file_id` does not match.
//! * QFX MUST be ignored if the referenced cryptographic `digest` does not
//!   match.
//! * Mismatch of `file_len` or `footer_crc32c` is also surfaced as a stale
//!   sidecar (`QF_E_SIDECAR_STALE`).
//!
//! The bytes between the header and the postscript hold accelerator
//! payload sections; their internal layout is not standardised by Spec §68
//! and is therefore left opaque by this parser.

use crate::checksum;
use crate::constants::{MAGIC_QFX, POSTSCRIPT_VERSION_V1};
use crate::error::QfError;

// ── Constants ────────────────────────────────────────────────────────────────

/// Encoded length of [`QfxHeaderV1`] in bytes.
///
/// Layout: magic(4) + header_len(2) + version_major(2) + version_minor(2)
///       + flags(4) + accelerator_id(16) + referenced_file_count(4)
///       + created_at_us(8) + reserved(40) + checksum(4) = 86.
pub const QFX_HEADER_LEN: u16 = 86;

/// Required `version_major` for QFX v1.
pub const QFX_VERSION_MAJOR_V1: u16 = 1;

/// Required `version_minor` for QFX v1.
pub const QFX_VERSION_MINOR_V1: u16 = 0;

/// Encoded length of [`QfxPostscriptV1`] in bytes (implementation-defined
/// payload; the tail framing of `[version u16][len u16][magic 4]` is
/// standardised by Spec §68).
///
/// Layout: header_offset(8) + header_len(8) + entries_offset(8)
///       + entries_len(8) + file_len(8) + flags(4) + checksum(4) = 48.
pub const QFX_POSTSCRIPT_LEN: u16 = 48;

/// Postscript version field value for QFX v1.
pub const QFX_POSTSCRIPT_VERSION_V1: u16 = POSTSCRIPT_VERSION_V1;

/// Size of the fixed tail after the postscript payload (`version` +
/// `len` + magic).
pub const QFX_POSTSCRIPT_TAIL_SIZE: usize = 2 + 2 + 4;

// ── QfxHeaderV1 ──────────────────────────────────────────────────────────────

/// Spec §68.1 `QfxHeaderV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfxHeaderV1 {
    /// Magic bytes — MUST equal [`MAGIC_QFX`] (`"QYX1"`).
    pub magic: [u8; 4],
    /// Header length in bytes — MUST equal [`QFX_HEADER_LEN`] for v1.
    pub header_len: u16,
    /// Major version — MUST equal [`QFX_VERSION_MAJOR_V1`] for v1.
    pub version_major: u16,
    /// Minor version — MUST equal [`QFX_VERSION_MINOR_V1`] for v1.
    pub version_minor: u16,
    /// Header flags (reserved for future use; v1 readers ignore unknown bits).
    pub flags: u32,
    /// Stable identifier for this accelerator instance.
    pub accelerator_id: [u8; 16],
    /// Number of [`QfxReferencedFileV1`] entries that follow the header.
    pub referenced_file_count: u32,
    /// Creation timestamp in microseconds since the Unix epoch.
    pub created_at_us: i64,
    /// Reserved — MUST be zero in v1.
    pub reserved: [u8; 40],
    /// CRC32C of the 86-byte header with this `checksum` field zeroed.
    pub checksum: u32,
}

impl QfxHeaderV1 {
    /// Serialise to the 86-byte wire form, recomputing the checksum.
    pub fn serialize(&self) -> [u8; QFX_HEADER_LEN as usize] {
        let mut buf = [0u8; QFX_HEADER_LEN as usize];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4..6].copy_from_slice(&self.header_len.to_le_bytes());
        buf[6..8].copy_from_slice(&self.version_major.to_le_bytes());
        buf[8..10].copy_from_slice(&self.version_minor.to_le_bytes());
        buf[10..14].copy_from_slice(&self.flags.to_le_bytes());
        buf[14..30].copy_from_slice(&self.accelerator_id);
        buf[30..34].copy_from_slice(&self.referenced_file_count.to_le_bytes());
        buf[34..42].copy_from_slice(&self.created_at_us.to_le_bytes());
        buf[42..82].copy_from_slice(&self.reserved);
        // Bytes [82..86] are the checksum, left zero during CRC computation.
        let crc = checksum::crc32c(&buf);
        buf[82..86].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    /// Parse the 86-byte wire form and verify magic, version, and checksum.
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < QFX_HEADER_LEN as usize {
            return Err(QfError::BufferTooShort);
        }
        let bytes = &bytes[..QFX_HEADER_LEN as usize];

        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[0..4]);
        if magic != MAGIC_QFX {
            return Err(QfError::BadMagic);
        }

        let header_len = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if header_len != QFX_HEADER_LEN {
            return Err(QfError::BadSection(format!(
                "QFX header_len must be {QFX_HEADER_LEN}, got {header_len}"
            )));
        }

        let version_major = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        let version_minor = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        if version_major != QFX_VERSION_MAJOR_V1 {
            return Err(QfError::BadVersion);
        }

        let flags = u32::from_le_bytes(bytes[10..14].try_into().unwrap());
        let mut accelerator_id = [0u8; 16];
        accelerator_id.copy_from_slice(&bytes[14..30]);
        let referenced_file_count = u32::from_le_bytes(bytes[30..34].try_into().unwrap());
        let created_at_us = i64::from_le_bytes(bytes[34..42].try_into().unwrap());
        let mut reserved = [0u8; 40];
        reserved.copy_from_slice(&bytes[42..82]);
        if reserved.iter().any(|b| *b != 0) {
            return Err(QfError::ReservedNotZero);
        }
        let checksum_field = u32::from_le_bytes(bytes[82..86].try_into().unwrap());

        // Verify CRC32C with the checksum field zeroed.
        let mut for_crc = [0u8; QFX_HEADER_LEN as usize];
        for_crc.copy_from_slice(bytes);
        for_crc[82..86].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }

        Ok(Self {
            magic,
            header_len,
            version_major,
            version_minor,
            flags,
            accelerator_id,
            referenced_file_count,
            created_at_us,
            reserved,
            checksum: checksum_field,
        })
    }

    /// Construct a v1 header with the given `accelerator_id` and number of
    /// referenced files; all other fields are defaulted.
    pub fn new(accelerator_id: [u8; 16], referenced_file_count: u32, created_at_us: i64) -> Self {
        Self {
            magic: MAGIC_QFX,
            header_len: QFX_HEADER_LEN,
            version_major: QFX_VERSION_MAJOR_V1,
            version_minor: QFX_VERSION_MINOR_V1,
            flags: 0,
            accelerator_id,
            referenced_file_count,
            created_at_us,
            reserved: [0u8; 40],
            checksum: 0,
        }
    }
}

// ── QfxReferencedFileV1 ──────────────────────────────────────────────────────

/// Spec §68.2 `QfxReferencedFileV1`.
///
/// Wire layout (little-endian):
/// `file_id(16) + file_len(8) + footer_crc32c(4) + digest_algorithm(2)
/// + digest_len(2) + digest(digest_len)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfxReferencedFileV1 {
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: u32,
    pub digest_algorithm: u16,
    pub digest: Vec<u8>,
}

impl QfxReferencedFileV1 {
    /// Encoded length of this entry on the wire.
    pub fn encoded_len(&self) -> usize {
        16 + 8 + 4 + 2 + 2 + self.digest.len()
    }

    /// Serialise to the wire form. `digest_len` is taken from `digest.len()`
    /// and MUST fit in a `u16`.
    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        if self.digest.len() > u16::MAX as usize {
            return Err(QfError::BadSection("digest_len exceeds u16::MAX".into()));
        }
        let mut out = Vec::with_capacity(self.encoded_len());
        out.extend_from_slice(&self.file_id);
        out.extend_from_slice(&self.file_len.to_le_bytes());
        out.extend_from_slice(&self.footer_crc32c.to_le_bytes());
        out.extend_from_slice(&self.digest_algorithm.to_le_bytes());
        out.extend_from_slice(&(self.digest.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.digest);
        Ok(out)
    }

    /// Parse one entry from the start of `bytes`; returns the entry and the
    /// number of bytes consumed.
    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), QfError> {
        const FIXED: usize = 16 + 8 + 4 + 2 + 2;
        if bytes.len() < FIXED {
            return Err(QfError::BufferTooShort);
        }
        let mut file_id = [0u8; 16];
        file_id.copy_from_slice(&bytes[0..16]);
        let file_len = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let footer_crc32c = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let digest_algorithm = u16::from_le_bytes(bytes[28..30].try_into().unwrap());
        let digest_len = u16::from_le_bytes(bytes[30..32].try_into().unwrap()) as usize;
        let end = FIXED
            .checked_add(digest_len)
            .ok_or(QfError::ArithOverflow)?;
        if bytes.len() < end {
            return Err(QfError::BufferTooShort);
        }
        let digest = bytes[FIXED..end].to_vec();
        Ok((
            Self {
                file_id,
                file_len,
                footer_crc32c,
                digest_algorithm,
                digest,
            },
            end,
        ))
    }

    /// Verify this entry against a host QF file's identity and digest.
    /// Mismatch of any of `file_id`, `file_len`, `footer_crc32c`, or
    /// `digest` yields [`QfError::SidecarStale`] (Spec §68 Rules).
    pub fn verify_against(
        &self,
        host_file_id: &[u8; 16],
        host_file_len: u64,
        host_footer_crc32c: u32,
        host_digest: &[u8],
    ) -> Result<(), QfError> {
        if &self.file_id != host_file_id
            || self.file_len != host_file_len
            || self.footer_crc32c != host_footer_crc32c
            || self.digest.as_slice() != host_digest
        {
            Err(QfError::SidecarStale)
        } else {
            Ok(())
        }
    }
}

// ── QfxPostscriptV1 ──────────────────────────────────────────────────────────

/// Implementation-defined postscript payload for a QFX file.
///
/// Spec §68 standardises only the trailing framing
/// `[postscript bytes][version u16][len u16][magic "QYX1"]`. This struct is
/// the on-disk shape this implementation writes for the postscript bytes.
/// It bootstraps the reader by recording where the header and the
/// referenced-file array live within the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfxPostscriptV1 {
    pub header_offset: u64,
    pub header_len: u64,
    pub entries_offset: u64,
    pub entries_len: u64,
    /// Total file length in bytes — MUST equal the actual file length.
    pub file_len: u64,
    pub flags: u32,
    /// CRC32C of the 48-byte payload with this field zeroed.
    pub checksum: u32,
}

impl QfxPostscriptV1 {
    /// Serialise the 48-byte payload, recomputing `checksum`.
    pub fn serialize(&self) -> [u8; QFX_POSTSCRIPT_LEN as usize] {
        let mut buf = [0u8; QFX_POSTSCRIPT_LEN as usize];
        buf[0..8].copy_from_slice(&self.header_offset.to_le_bytes());
        buf[8..16].copy_from_slice(&self.header_len.to_le_bytes());
        buf[16..24].copy_from_slice(&self.entries_offset.to_le_bytes());
        buf[24..32].copy_from_slice(&self.entries_len.to_le_bytes());
        buf[32..40].copy_from_slice(&self.file_len.to_le_bytes());
        buf[40..44].copy_from_slice(&self.flags.to_le_bytes());
        // Bytes [44..48] = checksum, left zero during CRC.
        let crc = checksum::crc32c(&buf);
        buf[44..48].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    /// Serialise the full postscript region: payload + version + len + magic.
    pub fn serialize_tail(&self) -> [u8; QFX_POSTSCRIPT_LEN as usize + QFX_POSTSCRIPT_TAIL_SIZE] {
        let mut tail = [0u8; QFX_POSTSCRIPT_LEN as usize + QFX_POSTSCRIPT_TAIL_SIZE];
        let payload = self.serialize();
        tail[..QFX_POSTSCRIPT_LEN as usize].copy_from_slice(&payload);
        let n = QFX_POSTSCRIPT_LEN as usize;
        tail[n..n + 2].copy_from_slice(&QFX_POSTSCRIPT_VERSION_V1.to_le_bytes());
        tail[n + 2..n + 4].copy_from_slice(&QFX_POSTSCRIPT_LEN.to_le_bytes());
        tail[n + 4..n + 8].copy_from_slice(&MAGIC_QFX);
        tail
    }

    /// Parse the postscript from the final bytes of a file buffer.
    pub fn parse_from_tail(file_data: &[u8]) -> Result<Self, QfError> {
        let total = QFX_POSTSCRIPT_LEN as usize + QFX_POSTSCRIPT_TAIL_SIZE;
        if file_data.len() < total {
            return Err(QfError::BufferTooShort);
        }
        let tail = &file_data[file_data.len() - total..];

        let n = QFX_POSTSCRIPT_LEN as usize;
        let version = u16::from_le_bytes(tail[n..n + 2].try_into().unwrap());
        let len = u16::from_le_bytes(tail[n + 2..n + 4].try_into().unwrap());
        let magic: [u8; 4] = tail[n + 4..n + 8].try_into().unwrap();

        if magic != MAGIC_QFX {
            return Err(QfError::BadMagic);
        }
        if version != QFX_POSTSCRIPT_VERSION_V1 {
            return Err(QfError::BadVersion);
        }
        if len != QFX_POSTSCRIPT_LEN {
            return Err(QfError::BadSection(format!(
                "QFX postscript_len must be {QFX_POSTSCRIPT_LEN}, got {len}"
            )));
        }

        let payload: [u8; QFX_POSTSCRIPT_LEN as usize] = tail[..n].try_into().unwrap();
        let header_offset = u64::from_le_bytes(payload[0..8].try_into().unwrap());
        let header_len = u64::from_le_bytes(payload[8..16].try_into().unwrap());
        let entries_offset = u64::from_le_bytes(payload[16..24].try_into().unwrap());
        let entries_len = u64::from_le_bytes(payload[24..32].try_into().unwrap());
        let file_len = u64::from_le_bytes(payload[32..40].try_into().unwrap());
        let flags = u32::from_le_bytes(payload[40..44].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(payload[44..48].try_into().unwrap());

        let mut for_crc = payload;
        for_crc[44..48].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }

        Ok(Self {
            header_offset,
            header_len,
            entries_offset,
            entries_len,
            file_len,
            flags,
            checksum: checksum_field,
        })
    }
}

// ── Top-level QFX file ───────────────────────────────────────────────────────

/// Parsed QFX file: header plus the list of referenced files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfxFile {
    pub header: QfxHeaderV1,
    pub referenced_files: Vec<QfxReferencedFileV1>,
    pub postscript: QfxPostscriptV1,
}

impl QfxFile {
    /// Parse a complete QFX file from its raw bytes (Spec §68).
    pub fn parse(file_data: &[u8]) -> Result<Self, QfError> {
        let postscript = QfxPostscriptV1::parse_from_tail(file_data)?;

        if postscript.file_len != file_data.len() as u64 {
            return Err(QfError::BadSection(format!(
                "QFX postscript file_len {} does not match actual file length {}",
                postscript.file_len,
                file_data.len()
            )));
        }

        // Locate and parse the header.
        let h_off = usize::try_from(postscript.header_offset).map_err(|_| QfError::OffsetRange)?;
        let h_len = usize::try_from(postscript.header_len).map_err(|_| QfError::OffsetRange)?;
        let h_end = h_off.checked_add(h_len).ok_or(QfError::ArithOverflow)?;
        if h_end > file_data.len() {
            return Err(QfError::OffsetRange);
        }
        let header = QfxHeaderV1::parse(&file_data[h_off..h_end])?;
        if postscript.header_len as u16 != header.header_len {
            return Err(QfError::BadSection(
                "QFX postscript header_len disagrees with header".into(),
            ));
        }

        // Locate and parse the referenced-file entries.
        let e_off = usize::try_from(postscript.entries_offset).map_err(|_| QfError::OffsetRange)?;
        let e_len = usize::try_from(postscript.entries_len).map_err(|_| QfError::OffsetRange)?;
        let e_end = e_off.checked_add(e_len).ok_or(QfError::ArithOverflow)?;
        if e_end > file_data.len() {
            return Err(QfError::OffsetRange);
        }
        let region = &file_data[e_off..e_end];

        let mut referenced_files = Vec::with_capacity(header.referenced_file_count as usize);
        let mut pos = 0usize;
        for _ in 0..header.referenced_file_count {
            let (entry, used) = QfxReferencedFileV1::parse(&region[pos..])?;
            pos = pos.checked_add(used).ok_or(QfError::ArithOverflow)?;
            referenced_files.push(entry);
        }
        if pos != region.len() {
            return Err(QfError::BadSection(
                "QFX referenced-file region has trailing bytes".into(),
            ));
        }

        Ok(Self {
            header,
            referenced_files,
            postscript,
        })
    }

    /// Serialise a QFX file with the canonical layout used by this writer:
    /// `[header][entries][postscript_tail]`. The postscript and header
    /// checksums are recomputed.
    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        let mut header = self.header.clone();
        header.referenced_file_count = u32::try_from(self.referenced_files.len())
            .map_err(|_| QfError::BadSection("too many QFX referenced files".into()))?;

        let header_bytes = header.serialize();

        let mut entries_bytes: Vec<u8> = Vec::new();
        for entry in &self.referenced_files {
            entries_bytes.extend_from_slice(&entry.serialize()?);
        }

        let header_offset = 0u64;
        let header_len_u64 = header_bytes.len() as u64;
        let entries_offset = header_len_u64;
        let entries_len = entries_bytes.len() as u64;
        let postscript_total = (QFX_POSTSCRIPT_LEN as u64) + (QFX_POSTSCRIPT_TAIL_SIZE as u64);
        let file_len = entries_offset + entries_len + postscript_total;

        let postscript = QfxPostscriptV1 {
            header_offset,
            header_len: header_len_u64,
            entries_offset,
            entries_len,
            file_len,
            flags: self.postscript.flags,
            checksum: 0,
        };
        let tail = postscript.serialize_tail();

        let mut out = Vec::with_capacity(file_len as usize);
        out.extend_from_slice(&header_bytes);
        out.extend_from_slice(&entries_bytes);
        out.extend_from_slice(&tail);
        debug_assert_eq!(out.len() as u64, file_len);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(file_id: u8, digest_byte: u8, digest_len: usize) -> QfxReferencedFileV1 {
        QfxReferencedFileV1 {
            file_id: [file_id; 16],
            file_len: 4096,
            footer_crc32c: 0xCAFEBABE,
            digest_algorithm: 1, // BLAKE3
            digest: vec![digest_byte; digest_len],
        }
    }

    fn sample_file() -> QfxFile {
        QfxFile {
            header: QfxHeaderV1::new([0x11; 16], 0, 1_700_000_000_000_000),
            referenced_files: vec![sample_entry(0x22, 0xAB, 32), sample_entry(0x33, 0xCD, 64)],
            postscript: QfxPostscriptV1 {
                header_offset: 0,
                header_len: 0,
                entries_offset: 0,
                entries_len: 0,
                file_len: 0,
                flags: 0,
                checksum: 0,
            },
        }
    }

    #[test]
    fn header_roundtrip_and_checksum() {
        let h = QfxHeaderV1::new([0xAA; 16], 3, 42);
        let bytes = h.serialize();
        let h2 = QfxHeaderV1::parse(&bytes).expect("parses");
        assert_eq!(h2.accelerator_id, [0xAA; 16]);
        assert_eq!(h2.referenced_file_count, 3);
        assert_eq!(h2.created_at_us, 42);
        assert_eq!(h2.header_len, QFX_HEADER_LEN);
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut bytes = QfxHeaderV1::new([0; 16], 0, 0).serialize();
        bytes[0] = b'X';
        assert_eq!(QfxHeaderV1::parse(&bytes), Err(QfError::BadMagic));
    }

    #[test]
    fn header_rejects_flipped_checksum() {
        let mut bytes = QfxHeaderV1::new([0; 16], 0, 0).serialize();
        bytes[82] ^= 0xFF;
        assert_eq!(QfxHeaderV1::parse(&bytes), Err(QfError::ChecksumMismatch));
    }

    #[test]
    fn header_rejects_bad_version() {
        let mut bytes = QfxHeaderV1::new([0; 16], 0, 0).serialize();
        // Bump version_major from 1 to 2 and fix checksum so we exercise
        // BadVersion (not ChecksumMismatch).
        bytes[6..8].copy_from_slice(&2u16.to_le_bytes());
        bytes[82..86].fill(0);
        let crc = checksum::crc32c(&bytes);
        bytes[82..86].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(QfxHeaderV1::parse(&bytes), Err(QfError::BadVersion));
    }

    #[test]
    fn entry_roundtrip_variable_digest() {
        for &dlen in &[0usize, 1, 16, 32, 64] {
            let e = sample_entry(0x11, 0xEE, dlen);
            let bytes = e.serialize().unwrap();
            let (e2, used) = QfxReferencedFileV1::parse(&bytes).unwrap();
            assert_eq!(used, bytes.len());
            assert_eq!(e, e2);
        }
    }

    #[test]
    fn file_roundtrip_two_entries() {
        let f = sample_file();
        let bytes = f.serialize().unwrap();
        let parsed = QfxFile::parse(&bytes).unwrap();
        assert_eq!(parsed.header.referenced_file_count, 2);
        assert_eq!(parsed.referenced_files.len(), 2);
        assert_eq!(parsed.referenced_files[0].file_id, [0x22; 16]);
        assert_eq!(parsed.referenced_files[1].file_id, [0x33; 16]);
        assert_eq!(parsed.postscript.file_len, bytes.len() as u64);
    }

    #[test]
    fn file_rejects_too_short() {
        // Anything shorter than the postscript region cannot parse.
        let total = QFX_POSTSCRIPT_LEN as usize + QFX_POSTSCRIPT_TAIL_SIZE;
        let bytes = vec![0u8; total - 1];
        assert_eq!(QfxFile::parse(&bytes), Err(QfError::BufferTooShort));
    }

    #[test]
    fn file_rejects_file_len_mismatch() {
        // Append a stray byte after a well-formed file: postscript file_len
        // no longer matches the actual length.
        let f = sample_file();
        let mut bytes = f.serialize().unwrap();
        bytes.insert(0, 0x00);
        let err = QfxFile::parse(&bytes).unwrap_err();
        // The trailing magic still parses, the postscript checksum still
        // matches, but file_len disagrees with actual length.
        assert!(
            matches!(&err, QfError::BadSection(s) if s.contains("file_len")),
            "got {err:?}"
        );
    }

    #[test]
    fn file_rejects_flipped_tail_magic() {
        let f = sample_file();
        let mut bytes = f.serialize().unwrap();
        let n = bytes.len();
        bytes[n - 1] ^= 0xFF;
        assert_eq!(QfxFile::parse(&bytes), Err(QfError::BadMagic));
    }

    #[test]
    fn entry_verify_detects_stale_in_each_field() {
        let e = sample_entry(0x44, 0x77, 32);
        let id = [0x44u8; 16];
        let dg = vec![0x77u8; 32];
        // All match
        assert!(e.verify_against(&id, 4096, 0xCAFEBABE, &dg).is_ok());
        // file_id mismatch
        assert_eq!(
            e.verify_against(&[0x55; 16], 4096, 0xCAFEBABE, &dg),
            Err(QfError::SidecarStale)
        );
        // digest mismatch
        let bad = vec![0x00u8; 32];
        assert_eq!(
            e.verify_against(&id, 4096, 0xCAFEBABE, &bad),
            Err(QfError::SidecarStale)
        );
        // file_len mismatch
        assert_eq!(
            e.verify_against(&id, 1, 0xCAFEBABE, &dg),
            Err(QfError::SidecarStale)
        );
        // footer_crc32c mismatch
        assert_eq!(
            e.verify_against(&id, 4096, 0, &dg),
            Err(QfError::SidecarStale)
        );
    }

    #[test]
    fn file_rejects_postscript_checksum_corruption() {
        let f = sample_file();
        let mut bytes = f.serialize().unwrap();
        // Postscript payload occupies the bytes immediately before the 8-byte tail.
        let n = bytes.len();
        let cksum_off = n - QFX_POSTSCRIPT_TAIL_SIZE - 4;
        bytes[cksum_off] ^= 0xFF;
        assert_eq!(QfxFile::parse(&bytes), Err(QfError::ChecksumMismatch));
    }
}
