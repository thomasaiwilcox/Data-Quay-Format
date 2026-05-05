//! Spec §69 — QFM dataset manifest (spec-exact wire format).
//!
//! A QFM aggregates file-level metadata for a collection of QF files so a
//! planner can prune at file level before opening any QF footer. Per
//! Spec §69:
//!
//! * The file ends with the pattern
//!   `[postscript bytes][postscript_version: u16][postscript_len: u16][magic: "QYM1"]`.
//! * The header is [`QfmHeaderV1`] (Spec §69.1) carrying a `dataset_id`,
//!   `table_count`, `file_count`, and a CRC32C checksum.
//! * Each file is described by a [`QfmFileEntryV1`] (Spec §69.2) with
//!   variable-length `uri` and variable-length cryptographic digest plus
//!   `file_len`, `footer_crc32c`, `row_count`, `segment_count`, and refs to
//!   optional file-level stats and exact-set artifacts.
//!
//! Spec §69 Rules enforced by this module:
//! * QFM MUST be ignored if stale (any of `file_id`, `file_len`,
//!   `footer_crc32c`, `digest` mismatches the host file).
//! * QFM MUST NOT change QF semantics — it is purely advisory pruning data.
//!
//! The bytes between the header and the postscript hold the file-entry
//! array; this implementation packs them sequentially in declaration order.
//! Spec §69 does not standardise table-schema-fingerprint or partition
//! payload layouts; they are not modelled here yet.

use crate::checksum;
use crate::constants::{MAGIC_QFM, POSTSCRIPT_VERSION_V1};
use crate::error::QfError;

// ── Constants ────────────────────────────────────────────────────────────────

/// Encoded length of [`QfmHeaderV1`] in bytes.
///
/// Layout: magic(4) + header_len(2) + version_major(2) + version_minor(2)
///       + flags(4) + dataset_id(16) + table_count(4) + file_count(4)
///       + created_at_us(8) + reserved(32) + checksum(4) = 82.
pub const QFM_HEADER_LEN: u16 = 82;

/// Required `version_major` for QFM v1.
pub const QFM_VERSION_MAJOR_V1: u16 = 1;

/// Required `version_minor` for QFM v1.
pub const QFM_VERSION_MINOR_V1: u16 = 0;

/// Encoded length of [`QfmPostscriptV1`] in bytes (implementation-defined
/// payload; the tail framing of `[version u16][len u16][magic 4]` is
/// standardised by Spec §69).
pub const QFM_POSTSCRIPT_LEN: u16 = 48;

/// Postscript version field value for QFM v1.
pub const QFM_POSTSCRIPT_VERSION_V1: u16 = POSTSCRIPT_VERSION_V1;

/// Size of the fixed tail after the postscript payload.
pub const QFM_POSTSCRIPT_TAIL_SIZE: usize = 2 + 2 + 4;

// ── QfmHeaderV1 ──────────────────────────────────────────────────────────────

/// Spec §69.1 `QfmHeaderV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfmHeaderV1 {
    /// Magic bytes — MUST equal [`MAGIC_QFM`] (`"QYM1"`).
    pub magic: [u8; 4],
    /// Header length in bytes — MUST equal [`QFM_HEADER_LEN`] for v1.
    pub header_len: u16,
    /// Major version — MUST equal [`QFM_VERSION_MAJOR_V1`] for v1.
    pub version_major: u16,
    /// Minor version — MUST equal [`QFM_VERSION_MINOR_V1`] for v1.
    pub version_minor: u16,
    /// Header flags (reserved; v1 readers ignore unknown bits).
    pub flags: u32,
    /// Stable identifier for this dataset.
    pub dataset_id: [u8; 16],
    /// Number of distinct tables aggregated by this manifest.
    pub table_count: u32,
    /// Number of [`QfmFileEntryV1`] entries that follow the header.
    pub file_count: u32,
    /// Creation timestamp in microseconds since the Unix epoch.
    pub created_at_us: i64,
    /// Reserved — MUST be zero in v1.
    pub reserved: [u8; 32],
    /// CRC32C of the 82-byte header with this `checksum` field zeroed.
    pub checksum: u32,
}

impl QfmHeaderV1 {
    pub fn serialize(&self) -> [u8; QFM_HEADER_LEN as usize] {
        let mut buf = [0u8; QFM_HEADER_LEN as usize];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4..6].copy_from_slice(&self.header_len.to_le_bytes());
        buf[6..8].copy_from_slice(&self.version_major.to_le_bytes());
        buf[8..10].copy_from_slice(&self.version_minor.to_le_bytes());
        buf[10..14].copy_from_slice(&self.flags.to_le_bytes());
        buf[14..30].copy_from_slice(&self.dataset_id);
        buf[30..34].copy_from_slice(&self.table_count.to_le_bytes());
        buf[34..38].copy_from_slice(&self.file_count.to_le_bytes());
        buf[38..46].copy_from_slice(&self.created_at_us.to_le_bytes());
        buf[46..78].copy_from_slice(&self.reserved);
        // Bytes [78..82] = checksum, left zero during CRC.
        let crc = checksum::crc32c(&buf);
        buf[78..82].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < QFM_HEADER_LEN as usize {
            return Err(QfError::BufferTooShort);
        }
        let bytes = &bytes[..QFM_HEADER_LEN as usize];

        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[0..4]);
        if magic != MAGIC_QFM {
            return Err(QfError::BadMagic);
        }
        let header_len = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if header_len != QFM_HEADER_LEN {
            return Err(QfError::BadSection(format!(
                "QFM header_len must be {QFM_HEADER_LEN}, got {header_len}"
            )));
        }
        let version_major = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        let version_minor = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        if version_major != QFM_VERSION_MAJOR_V1 {
            return Err(QfError::BadVersion);
        }
        let flags = u32::from_le_bytes(bytes[10..14].try_into().unwrap());
        let mut dataset_id = [0u8; 16];
        dataset_id.copy_from_slice(&bytes[14..30]);
        let table_count = u32::from_le_bytes(bytes[30..34].try_into().unwrap());
        let file_count = u32::from_le_bytes(bytes[34..38].try_into().unwrap());
        let created_at_us = i64::from_le_bytes(bytes[38..46].try_into().unwrap());
        let mut reserved = [0u8; 32];
        reserved.copy_from_slice(&bytes[46..78]);
        if reserved.iter().any(|b| *b != 0) {
            return Err(QfError::ReservedNotZero);
        }
        let checksum_field = u32::from_le_bytes(bytes[78..82].try_into().unwrap());

        let mut for_crc = [0u8; QFM_HEADER_LEN as usize];
        for_crc.copy_from_slice(bytes);
        for_crc[78..82].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }

        Ok(Self {
            magic,
            header_len,
            version_major,
            version_minor,
            flags,
            dataset_id,
            table_count,
            file_count,
            created_at_us,
            reserved,
            checksum: checksum_field,
        })
    }

    pub fn new(
        dataset_id: [u8; 16],
        table_count: u32,
        file_count: u32,
        created_at_us: i64,
    ) -> Self {
        Self {
            magic: MAGIC_QFM,
            header_len: QFM_HEADER_LEN,
            version_major: QFM_VERSION_MAJOR_V1,
            version_minor: QFM_VERSION_MINOR_V1,
            flags: 0,
            dataset_id,
            table_count,
            file_count,
            created_at_us,
            reserved: [0u8; 32],
            checksum: 0,
        }
    }
}

// ── QfmFileEntryV1 ───────────────────────────────────────────────────────────

/// Spec §69.2 `QfmFileEntryV1`.
///
/// Wire layout (little-endian):
/// `file_id(16) + uri_len(2) + uri(uri_len) + file_len(8) + footer_crc32c(4)
/// + digest_algorithm(2) + digest_len(2) + digest(digest_len) + row_count(8)
/// + segment_count(4) + file_stats_ref(4) + file_exact_set_ref(4) + flags(4)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfmFileEntryV1 {
    pub file_id: [u8; 16],
    pub uri: String,
    pub file_len: u64,
    pub footer_crc32c: u32,
    pub digest_algorithm: u16,
    pub digest: Vec<u8>,
    pub row_count: u64,
    pub segment_count: u32,
    pub file_stats_ref: u32,
    pub file_exact_set_ref: u32,
    pub flags: u32,
}

impl QfmFileEntryV1 {
    pub fn encoded_len(&self) -> usize {
        16 + 2 + self.uri.len() + 8 + 4 + 2 + 2 + self.digest.len() + 8 + 4 + 4 + 4 + 4
    }

    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        if self.uri.len() > u16::MAX as usize {
            return Err(QfError::BadSection("QFM uri_len exceeds u16::MAX".into()));
        }
        if self.digest.len() > u16::MAX as usize {
            return Err(QfError::BadSection(
                "QFM digest_len exceeds u16::MAX".into(),
            ));
        }
        let mut out = Vec::with_capacity(self.encoded_len());
        out.extend_from_slice(&self.file_id);
        out.extend_from_slice(&(self.uri.len() as u16).to_le_bytes());
        out.extend_from_slice(self.uri.as_bytes());
        out.extend_from_slice(&self.file_len.to_le_bytes());
        out.extend_from_slice(&self.footer_crc32c.to_le_bytes());
        out.extend_from_slice(&self.digest_algorithm.to_le_bytes());
        out.extend_from_slice(&(self.digest.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.digest);
        out.extend_from_slice(&self.row_count.to_le_bytes());
        out.extend_from_slice(&self.segment_count.to_le_bytes());
        out.extend_from_slice(&self.file_stats_ref.to_le_bytes());
        out.extend_from_slice(&self.file_exact_set_ref.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        Ok(out)
    }

    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), QfError> {
        // Fixed prefix up to and including uri_len.
        if bytes.len() < 16 + 2 {
            return Err(QfError::BufferTooShort);
        }
        let mut file_id = [0u8; 16];
        file_id.copy_from_slice(&bytes[0..16]);
        let uri_len = u16::from_le_bytes(bytes[16..18].try_into().unwrap()) as usize;
        let mut pos = 18usize;

        let uri_end = pos.checked_add(uri_len).ok_or(QfError::ArithOverflow)?;
        if uri_end > bytes.len() {
            return Err(QfError::BufferTooShort);
        }
        let uri = std::str::from_utf8(&bytes[pos..uri_end])
            .map_err(|_| QfError::BadSection("QFM uri is not UTF-8".into()))?
            .to_string();
        pos = uri_end;

        // file_len(8) + footer_crc32c(4) + digest_algorithm(2) + digest_len(2)
        if bytes.len() < pos + 8 + 4 + 2 + 2 {
            return Err(QfError::BufferTooShort);
        }
        let file_len = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let footer_crc32c = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let digest_algorithm = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let digest_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;

        let digest_end = pos.checked_add(digest_len).ok_or(QfError::ArithOverflow)?;
        if digest_end > bytes.len() {
            return Err(QfError::BufferTooShort);
        }
        let digest = bytes[pos..digest_end].to_vec();
        pos = digest_end;

        if bytes.len() < pos + 8 + 4 + 4 + 4 + 4 {
            return Err(QfError::BufferTooShort);
        }
        let row_count = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let segment_count = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let file_stats_ref = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let file_exact_set_ref = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let flags = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;

        Ok((
            Self {
                file_id,
                uri,
                file_len,
                footer_crc32c,
                digest_algorithm,
                digest,
                row_count,
                segment_count,
                file_stats_ref,
                file_exact_set_ref,
                flags,
            },
            pos,
        ))
    }

    /// Verify this entry against a host QF file's identity.
    /// Mismatch of `file_id`, `file_len`, `footer_crc32c`, or `digest`
    /// yields [`QfError::SidecarStale`] (Spec §69 Rules).
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

// ── QfmPostscriptV1 ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfmPostscriptV1 {
    pub header_offset: u64,
    pub header_len: u64,
    pub entries_offset: u64,
    pub entries_len: u64,
    pub file_len: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl QfmPostscriptV1 {
    pub fn serialize(&self) -> [u8; QFM_POSTSCRIPT_LEN as usize] {
        let mut buf = [0u8; QFM_POSTSCRIPT_LEN as usize];
        buf[0..8].copy_from_slice(&self.header_offset.to_le_bytes());
        buf[8..16].copy_from_slice(&self.header_len.to_le_bytes());
        buf[16..24].copy_from_slice(&self.entries_offset.to_le_bytes());
        buf[24..32].copy_from_slice(&self.entries_len.to_le_bytes());
        buf[32..40].copy_from_slice(&self.file_len.to_le_bytes());
        buf[40..44].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&buf);
        buf[44..48].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn serialize_tail(&self) -> [u8; QFM_POSTSCRIPT_LEN as usize + QFM_POSTSCRIPT_TAIL_SIZE] {
        let mut tail = [0u8; QFM_POSTSCRIPT_LEN as usize + QFM_POSTSCRIPT_TAIL_SIZE];
        let payload = self.serialize();
        tail[..QFM_POSTSCRIPT_LEN as usize].copy_from_slice(&payload);
        let n = QFM_POSTSCRIPT_LEN as usize;
        tail[n..n + 2].copy_from_slice(&QFM_POSTSCRIPT_VERSION_V1.to_le_bytes());
        tail[n + 2..n + 4].copy_from_slice(&QFM_POSTSCRIPT_LEN.to_le_bytes());
        tail[n + 4..n + 8].copy_from_slice(&MAGIC_QFM);
        tail
    }

    pub fn parse_from_tail(file_data: &[u8]) -> Result<Self, QfError> {
        let total = QFM_POSTSCRIPT_LEN as usize + QFM_POSTSCRIPT_TAIL_SIZE;
        if file_data.len() < total {
            return Err(QfError::BufferTooShort);
        }
        let tail = &file_data[file_data.len() - total..];

        let n = QFM_POSTSCRIPT_LEN as usize;
        let version = u16::from_le_bytes(tail[n..n + 2].try_into().unwrap());
        let len = u16::from_le_bytes(tail[n + 2..n + 4].try_into().unwrap());
        let magic: [u8; 4] = tail[n + 4..n + 8].try_into().unwrap();

        if magic != MAGIC_QFM {
            return Err(QfError::BadMagic);
        }
        if version != QFM_POSTSCRIPT_VERSION_V1 {
            return Err(QfError::BadVersion);
        }
        if len != QFM_POSTSCRIPT_LEN {
            return Err(QfError::BadSection(format!(
                "QFM postscript_len must be {QFM_POSTSCRIPT_LEN}, got {len}"
            )));
        }

        let payload: [u8; QFM_POSTSCRIPT_LEN as usize] = tail[..n].try_into().unwrap();
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

// ── Top-level QFM file ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfmFile {
    pub header: QfmHeaderV1,
    pub files: Vec<QfmFileEntryV1>,
    pub postscript: QfmPostscriptV1,
}

impl QfmFile {
    pub fn parse(file_data: &[u8]) -> Result<Self, QfError> {
        let postscript = QfmPostscriptV1::parse_from_tail(file_data)?;

        if postscript.file_len != file_data.len() as u64 {
            return Err(QfError::BadSection(format!(
                "QFM postscript file_len {} does not match actual file length {}",
                postscript.file_len,
                file_data.len()
            )));
        }

        let h_off = usize::try_from(postscript.header_offset).map_err(|_| QfError::OffsetRange)?;
        let h_len = usize::try_from(postscript.header_len).map_err(|_| QfError::OffsetRange)?;
        let h_end = h_off.checked_add(h_len).ok_or(QfError::ArithOverflow)?;
        if h_end > file_data.len() {
            return Err(QfError::OffsetRange);
        }
        let header = QfmHeaderV1::parse(&file_data[h_off..h_end])?;
        if postscript.header_len as u16 != header.header_len {
            return Err(QfError::BadSection(
                "QFM postscript header_len disagrees with header".into(),
            ));
        }

        let e_off = usize::try_from(postscript.entries_offset).map_err(|_| QfError::OffsetRange)?;
        let e_len = usize::try_from(postscript.entries_len).map_err(|_| QfError::OffsetRange)?;
        let e_end = e_off.checked_add(e_len).ok_or(QfError::ArithOverflow)?;
        if e_end > file_data.len() {
            return Err(QfError::OffsetRange);
        }
        let region = &file_data[e_off..e_end];

        let mut files = Vec::with_capacity(header.file_count as usize);
        let mut pos = 0usize;
        for _ in 0..header.file_count {
            let (entry, used) = QfmFileEntryV1::parse(&region[pos..])?;
            pos = pos.checked_add(used).ok_or(QfError::ArithOverflow)?;
            files.push(entry);
        }
        if pos != region.len() {
            return Err(QfError::BadSection(
                "QFM file-entry region has trailing bytes".into(),
            ));
        }

        Ok(Self {
            header,
            files,
            postscript,
        })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        let mut header = self.header.clone();
        header.file_count = u32::try_from(self.files.len())
            .map_err(|_| QfError::BadSection("too many QFM file entries".into()))?;
        let header_bytes = header.serialize();

        let mut entries_bytes: Vec<u8> = Vec::new();
        for entry in &self.files {
            entries_bytes.extend_from_slice(&entry.serialize()?);
        }

        let header_offset = 0u64;
        let header_len_u64 = header_bytes.len() as u64;
        let entries_offset = header_len_u64;
        let entries_len = entries_bytes.len() as u64;
        let postscript_total = (QFM_POSTSCRIPT_LEN as u64) + (QFM_POSTSCRIPT_TAIL_SIZE as u64);
        let file_len = entries_offset + entries_len + postscript_total;

        let postscript = QfmPostscriptV1 {
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

    fn sample_entry(file_id: u8, uri: &str, digest_byte: u8, digest_len: usize) -> QfmFileEntryV1 {
        QfmFileEntryV1 {
            file_id: [file_id; 16],
            uri: uri.to_string(),
            file_len: 8192,
            footer_crc32c: 0xDEADBEEF,
            digest_algorithm: 1,
            digest: vec![digest_byte; digest_len],
            row_count: 100_000,
            segment_count: 4,
            file_stats_ref: 0,
            file_exact_set_ref: 0,
            flags: 0,
        }
    }

    fn sample_file() -> QfmFile {
        QfmFile {
            header: QfmHeaderV1::new([0x55; 16], 1, 0, 1_700_000_000_000_000),
            files: vec![
                sample_entry(0x66, "s3://bucket/a.quay", 0x11, 32),
                sample_entry(0x77, "s3://bucket/b.quay", 0x22, 64),
            ],
            postscript: QfmPostscriptV1 {
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
        let h = QfmHeaderV1::new([0xCC; 16], 7, 13, 99);
        let bytes = h.serialize();
        let h2 = QfmHeaderV1::parse(&bytes).unwrap();
        assert_eq!(h2.dataset_id, [0xCC; 16]);
        assert_eq!(h2.table_count, 7);
        assert_eq!(h2.file_count, 13);
        assert_eq!(h2.created_at_us, 99);
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut bytes = QfmHeaderV1::new([0; 16], 0, 0, 0).serialize();
        bytes[0] = b'X';
        assert_eq!(QfmHeaderV1::parse(&bytes), Err(QfError::BadMagic));
    }

    #[test]
    fn header_rejects_flipped_checksum() {
        let mut bytes = QfmHeaderV1::new([0; 16], 0, 0, 0).serialize();
        bytes[78] ^= 0xFF;
        assert_eq!(QfmHeaderV1::parse(&bytes), Err(QfError::ChecksumMismatch));
    }

    #[test]
    fn header_rejects_bad_version() {
        let mut bytes = QfmHeaderV1::new([0; 16], 0, 0, 0).serialize();
        bytes[6..8].copy_from_slice(&2u16.to_le_bytes());
        bytes[78..82].fill(0);
        let crc = checksum::crc32c(&bytes);
        bytes[78..82].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(QfmHeaderV1::parse(&bytes), Err(QfError::BadVersion));
    }

    #[test]
    fn entry_roundtrip_with_uri_and_digest() {
        let e = sample_entry(0x42, "file:///x/y.quay", 0xCD, 48);
        let bytes = e.serialize().unwrap();
        let (e2, used) = QfmFileEntryV1::parse(&bytes).unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(e, e2);
    }

    #[test]
    fn file_roundtrip_two_entries() {
        let f = sample_file();
        let bytes = f.serialize().unwrap();
        let parsed = QfmFile::parse(&bytes).unwrap();
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(parsed.files[0].uri, "s3://bucket/a.quay");
        assert_eq!(parsed.files[1].digest.len(), 64);
        assert_eq!(parsed.postscript.file_len, bytes.len() as u64);
    }

    #[test]
    fn file_rejects_flipped_tail_magic() {
        let f = sample_file();
        let mut bytes = f.serialize().unwrap();
        let n = bytes.len();
        bytes[n - 1] ^= 0xFF;
        assert_eq!(QfmFile::parse(&bytes), Err(QfError::BadMagic));
    }

    #[test]
    fn file_rejects_postscript_checksum_corruption() {
        let f = sample_file();
        let mut bytes = f.serialize().unwrap();
        let n = bytes.len();
        let cksum_off = n - QFM_POSTSCRIPT_TAIL_SIZE - 4;
        bytes[cksum_off] ^= 0xFF;
        assert_eq!(QfmFile::parse(&bytes), Err(QfError::ChecksumMismatch));
    }

    #[test]
    fn entry_verify_detects_stale_in_each_field() {
        let e = sample_entry(0x88, "x", 0x99, 32);
        let id = [0x88u8; 16];
        let dg = vec![0x99u8; 32];
        assert!(e.verify_against(&id, 8192, 0xDEADBEEF, &dg).is_ok());
        assert_eq!(
            e.verify_against(&[0; 16], 8192, 0xDEADBEEF, &dg),
            Err(QfError::SidecarStale)
        );
        assert_eq!(
            e.verify_against(&id, 0, 0xDEADBEEF, &dg),
            Err(QfError::SidecarStale)
        );
        assert_eq!(
            e.verify_against(&id, 8192, 0, &dg),
            Err(QfError::SidecarStale)
        );
        assert_eq!(
            e.verify_against(&id, 8192, 0xDEADBEEF, &[0u8; 32]),
            Err(QfError::SidecarStale)
        );
    }
}
