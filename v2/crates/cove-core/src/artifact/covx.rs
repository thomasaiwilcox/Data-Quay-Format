//! Spec §68 — COVX accelerator sidecar (spec-exact wire format).
//!
//! A COVX file extends one or more host COVE files with rebuildable
//! acceleration metadata (lookup indexes, composite zone indexes, large
//! histograms, etc.) without mutating the host. Per Spec §68:
//!
//! * The file ends with the pattern
//!   `[postscript bytes][postscript_version: u16][postscript_len: u16][magic: "CVX2"]`.
//! * The header is [`CovxHeaderV1`] (Spec §68.1) and carries an
//!   `accelerator_id`, a `referenced_file_count`, and a CRC32C checksum.
//! * Each referenced file is described by a [`CovxReferencedFileV1`]
//!   (Spec §68.2) carrying `file_id`, `file_len`, `footer_crc32c`, and a
//!   variable-length cryptographic digest.
//!
//! Spec §68 Rules enforced by this module:
//! * COVX MUST be ignored if the referenced `file_id` does not match.
//! * COVX MUST be ignored if the referenced cryptographic `digest` does not
//!   match.
//! * Mismatch of `file_len` or `footer_crc32c` is also surfaced as a stale
//!   sidecar (`COVE_E_SIDECAR_STALE`).
//!
//! The bytes between the header and the postscript hold accelerator
//! payload sections; their internal layout is not standardised by Spec §68
//! and is therefore left opaque by this parser.

use crate::checksum;
use crate::constants::{MAGIC_COVX, POSTSCRIPT_VERSION_V1};
use crate::error::CoveError;

// ── Constants ────────────────────────────────────────────────────────────────

/// Encoded length of [`CovxHeaderV1`] in bytes.
///
/// Layout: magic(4) + header_len(2) + version_major(2) + version_minor(2)
///       + flags(4) + accelerator_id(16) + referenced_file_count(4)
///       + created_at_us(8) + reserved(40) + checksum(4) = 86.
pub const COVX_HEADER_LEN: u16 = 86;

/// Required artifact header `version_major` for COVX v2.
pub const COVX_VERSION_MAJOR_V1: u16 = 1;

/// Required artifact header `version_minor` for COVX v2.
pub const COVX_VERSION_MINOR_V1: u16 = 0;

/// Encoded length of [`CovxPostscriptV1`] in bytes (implementation-defined
/// payload; the tail framing of `[version u16][len u16][magic 4]` is
/// standardised by Spec §68).
///
/// Layout: header_offset(8) + header_len(8) + entries_offset(8)
///       + entries_len(8) + file_len(8) + flags(4) + checksum(4) = 48.
pub const COVX_POSTSCRIPT_LEN: u16 = 48;

/// Postscript version field value for COVX v2.
pub const COVX_POSTSCRIPT_VERSION_V1: u16 = POSTSCRIPT_VERSION_V1;

/// Size of the fixed tail after the postscript payload (`version` +
/// `len` + magic).
pub const COVX_POSTSCRIPT_TAIL_SIZE: usize = 2 + 2 + 4;

// ── CovxHeaderV1 ──────────────────────────────────────────────────────────────

/// Spec §68.1 `CovxHeaderV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovxHeaderV1 {
    /// Magic bytes — MUST equal [`MAGIC_COVX`] (`"CVX2"`).
    pub magic: [u8; 4],
    /// Header length in bytes — MUST equal [`COVX_HEADER_LEN`] for the v2 artifact.
    pub header_len: u16,
    /// Major version — MUST equal [`COVX_VERSION_MAJOR_V1`] for the v2 artifact.
    pub version_major: u16,
    /// Minor version — MUST equal [`COVX_VERSION_MINOR_V1`] for the v2 artifact.
    pub version_minor: u16,
    /// Header flags reserved for future artifact versions.
    pub flags: u32,
    /// Stable identifier for this accelerator instance.
    pub accelerator_id: [u8; 16],
    /// Number of [`CovxReferencedFileV1`] entries that follow the header.
    pub referenced_file_count: u32,
    /// Creation timestamp in microseconds since the Unix epoch.
    pub created_at_us: i64,
    /// Reserved — MUST be zero in the v2 artifact.
    pub reserved: [u8; 40],
    /// CRC32C of the 86-byte header with this `checksum` field zeroed.
    pub checksum: u32,
}

impl CovxHeaderV1 {
    /// Serialise to the 86-byte wire form, recomputing the checksum.
    pub fn serialize(&self) -> [u8; COVX_HEADER_LEN as usize] {
        let mut buf = [0u8; COVX_HEADER_LEN as usize];
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
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < COVX_HEADER_LEN as usize {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..COVX_HEADER_LEN as usize];

        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[0..4]);
        if magic != MAGIC_COVX {
            return Err(CoveError::BadMagic);
        }

        let header_len = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if header_len != COVX_HEADER_LEN {
            return Err(CoveError::BadSection(format!(
                "COVX header_len must be {COVX_HEADER_LEN}, got {header_len}"
            )));
        }

        let version_major = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        let version_minor = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        if version_major != COVX_VERSION_MAJOR_V1 {
            return Err(CoveError::BadVersion);
        }

        let flags = u32::from_le_bytes(bytes[10..14].try_into().unwrap());
        let mut accelerator_id = [0u8; 16];
        accelerator_id.copy_from_slice(&bytes[14..30]);
        let referenced_file_count = u32::from_le_bytes(bytes[30..34].try_into().unwrap());
        let created_at_us = i64::from_le_bytes(bytes[34..42].try_into().unwrap());
        let mut reserved = [0u8; 40];
        reserved.copy_from_slice(&bytes[42..82]);
        if reserved.iter().any(|b| *b != 0) {
            return Err(CoveError::ReservedNotZero);
        }
        let checksum_field = u32::from_le_bytes(bytes[82..86].try_into().unwrap());

        // Verify CRC32C with the checksum field zeroed.
        let mut for_crc = [0u8; COVX_HEADER_LEN as usize];
        for_crc.copy_from_slice(bytes);
        for_crc[82..86].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(CoveError::ChecksumMismatch);
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
            magic: MAGIC_COVX,
            header_len: COVX_HEADER_LEN,
            version_major: COVX_VERSION_MAJOR_V1,
            version_minor: COVX_VERSION_MINOR_V1,
            flags: 0,
            accelerator_id,
            referenced_file_count,
            created_at_us,
            reserved: [0u8; 40],
            checksum: 0,
        }
    }
}

// ── CovxReferencedFileV1 ──────────────────────────────────────────────────────

/// Spec §68.2 `CovxReferencedFileV1`.
///
/// Wire layout (little-endian):
/// `file_id(16) + file_len(8) + footer_crc32c(4) + digest_algorithm(2)
/// + digest_len(2) + digest(digest_len)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovxReferencedFileV1 {
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: u32,
    pub digest_algorithm: u16,
    pub digest: Vec<u8>,
}

impl CovxReferencedFileV1 {
    /// Encoded length of this entry on the wire.
    pub fn encoded_len(&self) -> usize {
        16 + 8 + 4 + 2 + 2 + self.digest.len()
    }

    /// Serialise to the wire form. `digest_len` is taken from `digest.len()`
    /// and MUST fit in a `u16`.
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        if self.digest.len() > u16::MAX as usize {
            return Err(CoveError::BadSection("digest_len exceeds u16::MAX".into()));
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
    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        const FIXED: usize = 16 + 8 + 4 + 2 + 2;
        if bytes.len() < FIXED {
            return Err(CoveError::BufferTooShort);
        }
        let mut file_id = [0u8; 16];
        file_id.copy_from_slice(&bytes[0..16]);
        let file_len = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let footer_crc32c = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let digest_algorithm = u16::from_le_bytes(bytes[28..30].try_into().unwrap());
        let digest_len = u16::from_le_bytes(bytes[30..32].try_into().unwrap()) as usize;
        let end = FIXED
            .checked_add(digest_len)
            .ok_or(CoveError::ArithOverflow)?;
        if bytes.len() < end {
            return Err(CoveError::BufferTooShort);
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

    /// Verify this entry against a host COVE file's identity and digest.
    /// Mismatch of any of `file_id`, `file_len`, `footer_crc32c`, or
    /// `digest` yields [`CoveError::SidecarStale`] (Spec §68 Rules).
    pub fn verify_against(
        &self,
        host_file_id: &[u8; 16],
        host_file_len: u64,
        host_footer_crc32c: u32,
        host_digest: &[u8],
    ) -> Result<(), CoveError> {
        if &self.file_id != host_file_id
            || self.file_len != host_file_len
            || self.footer_crc32c != host_footer_crc32c
            || self.digest.as_slice() != host_digest
        {
            Err(CoveError::SidecarStale)
        } else {
            Ok(())
        }
    }
}

// ── CovxPostscriptV1 ──────────────────────────────────────────────────────────

/// Implementation-defined postscript payload for a COVX file.
///
/// Spec §68 standardises only the trailing framing
/// `[postscript bytes][version u16][len u16][magic "CVX2"]`. This struct is
/// the on-disk shape this implementation writes for the postscript bytes.
/// It bootstraps the reader by recording where the header and the
/// referenced-file array live within the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovxPostscriptV1 {
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

impl CovxPostscriptV1 {
    /// Serialise the 48-byte payload, recomputing `checksum`.
    pub fn serialize(&self) -> [u8; COVX_POSTSCRIPT_LEN as usize] {
        let mut buf = [0u8; COVX_POSTSCRIPT_LEN as usize];
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
    pub fn serialize_tail(&self) -> [u8; COVX_POSTSCRIPT_LEN as usize + COVX_POSTSCRIPT_TAIL_SIZE] {
        let mut tail = [0u8; COVX_POSTSCRIPT_LEN as usize + COVX_POSTSCRIPT_TAIL_SIZE];
        let payload = self.serialize();
        tail[..COVX_POSTSCRIPT_LEN as usize].copy_from_slice(&payload);
        let n = COVX_POSTSCRIPT_LEN as usize;
        tail[n..n + 2].copy_from_slice(&COVX_POSTSCRIPT_VERSION_V1.to_le_bytes());
        tail[n + 2..n + 4].copy_from_slice(&COVX_POSTSCRIPT_LEN.to_le_bytes());
        tail[n + 4..n + 8].copy_from_slice(&MAGIC_COVX);
        tail
    }

    /// Parse the postscript from the final bytes of a file buffer.
    pub fn parse_from_tail(file_data: &[u8]) -> Result<Self, CoveError> {
        let total = COVX_POSTSCRIPT_LEN as usize + COVX_POSTSCRIPT_TAIL_SIZE;
        if file_data.len() < total {
            return Err(CoveError::BufferTooShort);
        }
        let tail = &file_data[file_data.len() - total..];

        let n = COVX_POSTSCRIPT_LEN as usize;
        let version = u16::from_le_bytes(tail[n..n + 2].try_into().unwrap());
        let len = u16::from_le_bytes(tail[n + 2..n + 4].try_into().unwrap());
        let magic: [u8; 4] = tail[n + 4..n + 8].try_into().unwrap();

        if magic != MAGIC_COVX {
            return Err(CoveError::BadMagic);
        }
        if version != COVX_POSTSCRIPT_VERSION_V1 {
            return Err(CoveError::BadVersion);
        }
        if len != COVX_POSTSCRIPT_LEN {
            return Err(CoveError::BadSection(format!(
                "COVX postscript_len must be {COVX_POSTSCRIPT_LEN}, got {len}"
            )));
        }

        let payload: [u8; COVX_POSTSCRIPT_LEN as usize] = tail[..n].try_into().unwrap();
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
            return Err(CoveError::ChecksumMismatch);
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

// ── Top-level COVX file ───────────────────────────────────────────────────────

/// Parsed COVX file: header plus the list of referenced files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovxFile {
    pub header: CovxHeaderV1,
    pub referenced_files: Vec<CovxReferencedFileV1>,
    pub postscript: CovxPostscriptV1,
}

impl CovxFile {
    /// Parse a complete COVX file from its raw bytes (Spec §68).
    pub fn parse(file_data: &[u8]) -> Result<Self, CoveError> {
        let postscript = CovxPostscriptV1::parse_from_tail(file_data)?;

        if postscript.file_len != file_data.len() as u64 {
            return Err(CoveError::BadSection(format!(
                "COVX postscript file_len {} does not match actual file length {}",
                postscript.file_len,
                file_data.len()
            )));
        }

        // Locate and parse the header.
        let h_off =
            usize::try_from(postscript.header_offset).map_err(|_| CoveError::OffsetRange)?;
        let h_len = usize::try_from(postscript.header_len).map_err(|_| CoveError::OffsetRange)?;
        let h_end = h_off.checked_add(h_len).ok_or(CoveError::ArithOverflow)?;
        if h_end > file_data.len() {
            return Err(CoveError::OffsetRange);
        }
        let header = CovxHeaderV1::parse(&file_data[h_off..h_end])?;
        if postscript.header_len as u16 != header.header_len {
            return Err(CoveError::BadSection(
                "COVX postscript header_len disagrees with header".into(),
            ));
        }

        // Locate and parse the referenced-file entries.
        let e_off =
            usize::try_from(postscript.entries_offset).map_err(|_| CoveError::OffsetRange)?;
        let e_len = usize::try_from(postscript.entries_len).map_err(|_| CoveError::OffsetRange)?;
        let e_end = e_off.checked_add(e_len).ok_or(CoveError::ArithOverflow)?;
        if e_end > file_data.len() {
            return Err(CoveError::OffsetRange);
        }
        let region = &file_data[e_off..e_end];

        let mut referenced_files = Vec::with_capacity(header.referenced_file_count as usize);
        let mut pos = 0usize;
        for _ in 0..header.referenced_file_count {
            let (entry, used) = CovxReferencedFileV1::parse(&region[pos..])?;
            pos = pos.checked_add(used).ok_or(CoveError::ArithOverflow)?;
            referenced_files.push(entry);
        }
        if pos != region.len() {
            return Err(CoveError::BadSection(
                "COVX referenced-file region has trailing bytes".into(),
            ));
        }

        Ok(Self {
            header,
            referenced_files,
            postscript,
        })
    }

    /// Serialise a COVX file with the canonical layout used by this writer:
    /// `[header][entries][postscript_tail]`. The postscript and header
    /// checksums are recomputed.
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let mut header = self.header.clone();
        header.referenced_file_count = u32::try_from(self.referenced_files.len())
            .map_err(|_| CoveError::BadSection("too many COVX referenced files".into()))?;

        let header_bytes = header.serialize();

        let mut entries_bytes: Vec<u8> = Vec::new();
        for entry in &self.referenced_files {
            entries_bytes.extend_from_slice(&entry.serialize()?);
        }

        let header_offset = 0u64;
        let header_len_u64 = header_bytes.len() as u64;
        let entries_offset = header_len_u64;
        let entries_len = entries_bytes.len() as u64;
        let postscript_total = (COVX_POSTSCRIPT_LEN as u64) + (COVX_POSTSCRIPT_TAIL_SIZE as u64);
        let file_len = entries_offset + entries_len + postscript_total;

        let postscript = CovxPostscriptV1 {
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

    fn sample_entry(file_id: u8, digest_byte: u8, digest_len: usize) -> CovxReferencedFileV1 {
        CovxReferencedFileV1 {
            file_id: [file_id; 16],
            file_len: 4096,
            footer_crc32c: 0xCAFEBABE,
            digest_algorithm: 1, // BLAKE3
            digest: vec![digest_byte; digest_len],
        }
    }

    fn sample_file() -> CovxFile {
        CovxFile {
            header: CovxHeaderV1::new([0x11; 16], 0, 1_700_000_000_000_000),
            referenced_files: vec![sample_entry(0x22, 0xAB, 32), sample_entry(0x33, 0xCD, 64)],
            postscript: CovxPostscriptV1 {
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
        let h = CovxHeaderV1::new([0xAA; 16], 3, 42);
        let bytes = h.serialize();
        let h2 = CovxHeaderV1::parse(&bytes).expect("parses");
        assert_eq!(h2.accelerator_id, [0xAA; 16]);
        assert_eq!(h2.referenced_file_count, 3);
        assert_eq!(h2.created_at_us, 42);
        assert_eq!(h2.header_len, COVX_HEADER_LEN);
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut bytes = CovxHeaderV1::new([0; 16], 0, 0).serialize();
        bytes[0] = b'X';
        assert_eq!(CovxHeaderV1::parse(&bytes), Err(CoveError::BadMagic));
    }

    #[test]
    fn header_rejects_flipped_checksum() {
        let mut bytes = CovxHeaderV1::new([0; 16], 0, 0).serialize();
        bytes[82] ^= 0xFF;
        assert_eq!(
            CovxHeaderV1::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        );
    }

    #[test]
    fn header_rejects_bad_version() {
        let mut bytes = CovxHeaderV1::new([0; 16], 0, 0).serialize();
        // Bump version_major from 1 to 2 and fix checksum so we exercise
        // BadVersion (not ChecksumMismatch).
        bytes[6..8].copy_from_slice(&2u16.to_le_bytes());
        bytes[82..86].fill(0);
        let crc = checksum::crc32c(&bytes);
        bytes[82..86].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(CovxHeaderV1::parse(&bytes), Err(CoveError::BadVersion));
    }

    #[test]
    fn entry_roundtrip_variable_digest() {
        for &dlen in &[0usize, 1, 16, 32, 64] {
            let e = sample_entry(0x11, 0xEE, dlen);
            let bytes = e.serialize().unwrap();
            let (e2, used) = CovxReferencedFileV1::parse(&bytes).unwrap();
            assert_eq!(used, bytes.len());
            assert_eq!(e, e2);
        }
    }

    #[test]
    fn file_roundtrip_two_entries() {
        let f = sample_file();
        let bytes = f.serialize().unwrap();
        let parsed = CovxFile::parse(&bytes).unwrap();
        assert_eq!(parsed.header.referenced_file_count, 2);
        assert_eq!(parsed.referenced_files.len(), 2);
        assert_eq!(parsed.referenced_files[0].file_id, [0x22; 16]);
        assert_eq!(parsed.referenced_files[1].file_id, [0x33; 16]);
        assert_eq!(parsed.postscript.file_len, bytes.len() as u64);
    }

    #[test]
    fn file_rejects_too_short() {
        // Anything shorter than the postscript region cannot parse.
        let total = COVX_POSTSCRIPT_LEN as usize + COVX_POSTSCRIPT_TAIL_SIZE;
        let bytes = vec![0u8; total - 1];
        assert_eq!(CovxFile::parse(&bytes), Err(CoveError::BufferTooShort));
    }

    #[test]
    fn file_rejects_file_len_mismatch() {
        // Append a stray byte after a well-formed file: postscript file_len
        // no longer matches the actual length.
        let f = sample_file();
        let mut bytes = f.serialize().unwrap();
        bytes.insert(0, 0x00);
        let err = CovxFile::parse(&bytes).unwrap_err();
        // The trailing magic still parses, the postscript checksum still
        // matches, but file_len disagrees with actual length.
        assert!(
            matches!(&err, CoveError::BadSection(s) if s.contains("file_len")),
            "got {err:?}"
        );
    }

    #[test]
    fn file_rejects_flipped_tail_magic() {
        let f = sample_file();
        let mut bytes = f.serialize().unwrap();
        let n = bytes.len();
        bytes[n - 1] ^= 0xFF;
        assert_eq!(CovxFile::parse(&bytes), Err(CoveError::BadMagic));
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
            Err(CoveError::SidecarStale)
        );
        // digest mismatch
        let bad = vec![0x00u8; 32];
        assert_eq!(
            e.verify_against(&id, 4096, 0xCAFEBABE, &bad),
            Err(CoveError::SidecarStale)
        );
        // file_len mismatch
        assert_eq!(
            e.verify_against(&id, 1, 0xCAFEBABE, &dg),
            Err(CoveError::SidecarStale)
        );
        // footer_crc32c mismatch
        assert_eq!(
            e.verify_against(&id, 4096, 0, &dg),
            Err(CoveError::SidecarStale)
        );
    }

    #[test]
    fn file_rejects_postscript_checksum_corruption() {
        let f = sample_file();
        let mut bytes = f.serialize().unwrap();
        // Postscript payload occupies the bytes immediately before the 8-byte tail.
        let n = bytes.len();
        let cksum_off = n - COVX_POSTSCRIPT_TAIL_SIZE - 4;
        bytes[cksum_off] ^= 0xFF;
        assert_eq!(CovxFile::parse(&bytes), Err(CoveError::ChecksumMismatch));
    }
}
