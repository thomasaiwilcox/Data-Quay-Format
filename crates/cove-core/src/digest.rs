//! Cove Format (COVE) v1.0 — Digest manifest (Spec §65).
//!
//! Digest manifests bind cryptographic hashes to sections, pages, the whole
//! file, or custom targets. Validation is policy-aware: a missing digest entry
//! for a target is _not_ an error (digests are optional), but a present entry
//! that fails verification is reported as [`CoveError::DigestMismatch`].

use crate::{checksum, constants::DigestAlgorithm, CoveError};

pub const DIGEST_MANIFEST_HEADER_LEN: usize = 60;
pub const DIGEST_ENTRY_FIXED_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum DigestScope {
    File = 0,
    Section = 1,
    Page = 2,
    Merkle = 3,
}

impl DigestScope {
    pub fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            0 => Self::File,
            1 => Self::Section,
            2 => Self::Page,
            3 => Self::Merkle,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum DigestTargetKind {
    Section = 0,
    Page = 1,
    File = 2,
    Custom = 3,
}

impl DigestTargetKind {
    pub fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            0 => Self::Section,
            1 => Self::Page,
            2 => Self::File,
            3 => Self::Custom,
            _ => return None,
        })
    }
}

/// A single digest entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestEntry {
    pub target_kind: DigestTargetKind,
    pub section_id: u32,
    pub local_id: u64,
    pub offset: u64,
    pub length: u64,
    pub digest: Vec<u8>,
}

/// A parsed digest manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestManifest {
    pub algorithm: DigestAlgorithm,
    pub scope: DigestScope,
    pub root_digest: [u8; 32],
    /// All digest entries.
    pub entries: Vec<DigestEntry>,
}

impl Default for DigestManifest {
    fn default() -> Self {
        Self {
            algorithm: DigestAlgorithm::Sha256,
            scope: DigestScope::Section,
            root_digest: [0; 32],
            entries: Vec::new(),
        }
    }
}

impl DigestManifest {
    /// Parse a digest manifest from raw section bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < DIGEST_MANIFEST_HEADER_LEN {
            return Err(CoveError::BufferTooShort);
        }

        // INVARIANT: checksum verification happens before any entry bytes are
        // trusted. The checksum field itself is zeroed for CRC computation.
        let mut checksum_bytes = bytes[..DIGEST_MANIFEST_HEADER_LEN].to_vec();
        checksum_bytes[56..60].fill(0);
        let expected_checksum = u32::from_le_bytes(bytes[56..60].try_into().unwrap());
        if checksum::crc32c(&checksum_bytes) != expected_checksum {
            return Err(CoveError::ChecksumMismatch);
        }

        let algorithm_raw = u16::from_le_bytes(bytes[0..2].try_into().unwrap());
        let algorithm = DigestAlgorithm::from_u16(algorithm_raw)
            .filter(|algorithm| *algorithm != DigestAlgorithm::None)
            .ok_or_else(|| {
                CoveError::BadSection(format!(
                    "digest manifest algorithm must be SHA-256 or BLAKE3, got {algorithm_raw}"
                ))
            })?;
        let scope_raw = u16::from_le_bytes(bytes[2..4].try_into().unwrap());
        let scope = DigestScope::from_u16(scope_raw).ok_or_else(|| {
            CoveError::BadSection(format!("unknown digest manifest scope {scope_raw}"))
        })?;
        let entry_count = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let entries_offset = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let entries_length = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let mut root_digest = [0u8; 32];
        root_digest.copy_from_slice(&bytes[24..56]);

        let entries_offset = usize::try_from(entries_offset).map_err(|_| CoveError::OffsetRange)?;
        let entries_length = usize::try_from(entries_length).map_err(|_| CoveError::OffsetRange)?;
        if entries_offset < DIGEST_MANIFEST_HEADER_LEN {
            return Err(CoveError::OffsetRange);
        }
        let entries_end = entries_offset
            .checked_add(entries_length)
            .ok_or(CoveError::ArithOverflow)?;
        if entries_end > bytes.len() {
            return Err(CoveError::OffsetRange);
        }
        if entries_end != bytes.len() {
            return Err(CoveError::BadSection(
                "digest manifest has bytes outside the declared entries region".into(),
            ));
        }

        let mut pos = entries_offset;
        let mut entries = Vec::with_capacity(entry_count as usize);

        for _ in 0..entry_count {
            if pos
                .checked_add(DIGEST_ENTRY_FIXED_LEN)
                .ok_or(CoveError::ArithOverflow)?
                > entries_end
            {
                return Err(CoveError::BufferTooShort);
            }
            let target_kind_raw = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
            pos += 2;
            let target_kind = DigestTargetKind::from_u16(target_kind_raw).ok_or_else(|| {
                CoveError::BadSection(format!("unknown digest target kind {target_kind_raw}"))
            })?;
            let digest_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            let section_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let local_id = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let offset = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let length = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;

            let digest_end = pos
                .checked_add(digest_len)
                .ok_or(CoveError::ArithOverflow)?;
            if digest_end > entries_end {
                return Err(CoveError::BufferTooShort);
            }
            let digest = bytes[pos..digest_end].to_vec();
            pos = digest_end;

            // Length sanity check per Spec §65: SHA-256 and BLAKE3 always emit 32 bytes.
            if digest.len() != expected_digest_len(algorithm) {
                return Err(CoveError::BadSection(format!(
                    "digest for algorithm {algorithm:?} must be {} bytes, got {}",
                    expected_digest_len(algorithm),
                    digest.len()
                )));
            }

            entries.push(DigestEntry {
                target_kind,
                section_id,
                local_id,
                offset,
                length,
                digest,
            });
        }
        if pos != entries_end {
            return Err(CoveError::BadSection(
                "digest manifest entries region has trailing bytes".into(),
            ));
        }

        Ok(Self {
            algorithm,
            scope,
            root_digest,
            entries,
        })
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        if self.algorithm == DigestAlgorithm::None {
            return Err(CoveError::BadSection(
                "digest manifest algorithm cannot be None".into(),
            ));
        }
        let mut entries = Vec::new();
        for entry in &self.entries {
            let digest_len = u16::try_from(entry.digest.len()).map_err(|_| {
                CoveError::BadSection("digest entry exceeds u16 digest_len limit".into())
            })?;
            if entry.digest.len() != expected_digest_len(self.algorithm) {
                return Err(CoveError::BadSection(format!(
                    "digest for algorithm {:?} must be {} bytes, got {}",
                    self.algorithm,
                    expected_digest_len(self.algorithm),
                    entry.digest.len()
                )));
            }
            entries.extend_from_slice(&(entry.target_kind as u16).to_le_bytes());
            entries.extend_from_slice(&digest_len.to_le_bytes());
            entries.extend_from_slice(&entry.section_id.to_le_bytes());
            entries.extend_from_slice(&entry.local_id.to_le_bytes());
            entries.extend_from_slice(&entry.offset.to_le_bytes());
            entries.extend_from_slice(&entry.length.to_le_bytes());
            entries.extend_from_slice(&entry.digest);
        }
        let entry_count = u32::try_from(self.entries.len())
            .map_err(|_| CoveError::BadSection("too many digest entries".into()))?;
        let entries_length = u64::try_from(entries.len())
            .map_err(|_| CoveError::BadSection("digest entries too large".into()))?;
        let mut out = Vec::with_capacity(DIGEST_MANIFEST_HEADER_LEN + entries.len());
        out.extend_from_slice(&(self.algorithm as u16).to_le_bytes());
        out.extend_from_slice(&(self.scope as u16).to_le_bytes());
        out.extend_from_slice(&entry_count.to_le_bytes());
        out.extend_from_slice(&(DIGEST_MANIFEST_HEADER_LEN as u64).to_le_bytes());
        out.extend_from_slice(&entries_length.to_le_bytes());
        out.extend_from_slice(&self.root_digest);
        out.extend_from_slice(&0u32.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[56..60].copy_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&entries);
        Ok(out)
    }

    /// Verify a section's digest. Returns `Ok(())` if no entry exists for the
    /// section (digests are optional per Spec §65); returns
    /// [`CoveError::DigestMismatch`] if a present entry fails.
    pub fn verify_section(&self, section_id: u32, section_bytes: &[u8]) -> Result<(), CoveError> {
        let entry = match self
            .entries
            .iter()
            .find(|e| e.target_kind == DigestTargetKind::Section && e.section_id == section_id)
        {
            Some(e) => e,
            None => return Ok(()),
        };
        if entry.length != section_bytes.len() as u64 {
            return Err(CoveError::BadSection(format!(
                "digest entry length {} does not match section length {}",
                entry.length,
                section_bytes.len()
            )));
        }
        self.verify_bytes(entry, section_bytes)
    }

    pub fn verify_bytes(&self, entry: &DigestEntry, bytes: &[u8]) -> Result<(), CoveError> {
        verify_digest(self.algorithm, bytes, &entry.digest)
    }
}

fn expected_digest_len(algorithm: DigestAlgorithm) -> usize {
    match algorithm {
        DigestAlgorithm::None => 0,
        DigestAlgorithm::Sha256 | DigestAlgorithm::Blake3 => 32,
    }
}

/// Compute the digest of `data` under `algorithm`. Algorithms that are not
/// compiled in return [`CoveError::UnsupportedEncoding`].
pub fn compute_digest(algorithm: DigestAlgorithm, data: &[u8]) -> Result<Vec<u8>, CoveError> {
    match algorithm {
        DigestAlgorithm::None => Ok(Vec::new()),
        DigestAlgorithm::Sha256 => sha256_digest(data),
        DigestAlgorithm::Blake3 => blake3_digest(data),
    }
}

/// Verify that `data` hashes to `expected` under `algorithm`.
pub fn verify_digest(
    algorithm: DigestAlgorithm,
    data: &[u8],
    expected: &[u8],
) -> Result<(), CoveError> {
    match algorithm {
        DigestAlgorithm::None => Ok(()),
        DigestAlgorithm::Sha256 | DigestAlgorithm::Blake3 => {
            let actual = compute_digest(algorithm, data)?;
            if actual.as_slice() == expected {
                Ok(())
            } else {
                Err(CoveError::DigestMismatch)
            }
        }
    }
}

#[cfg(feature = "digest-sha2")]
fn sha256_digest(data: &[u8]) -> Result<Vec<u8>, CoveError> {
    use sha2::{Digest, Sha256};
    Ok(Sha256::digest(data).to_vec())
}

#[cfg(not(feature = "digest-sha2"))]
fn sha256_digest(_data: &[u8]) -> Result<Vec<u8>, CoveError> {
    Err(CoveError::UnsupportedEncoding(
        "SHA-256 not enabled (build with feature `digest-sha2`)".into(),
    ))
}

#[cfg(feature = "digest-blake3")]
fn blake3_digest(data: &[u8]) -> Result<Vec<u8>, CoveError> {
    Ok(blake3::hash(data).as_bytes().to_vec())
}

#[cfg(not(feature = "digest-blake3"))]
fn blake3_digest(_data: &[u8]) -> Result<Vec<u8>, CoveError> {
    Err(CoveError::UnsupportedEncoding(
        "BLAKE3 not enabled (build with feature `digest-blake3`)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section_entry(section_id: u32, payload: &[u8]) -> DigestEntry {
        DigestEntry {
            target_kind: DigestTargetKind::Section,
            section_id,
            local_id: 0,
            offset: 0,
            length: payload.len() as u64,
            digest: compute_digest(DigestAlgorithm::Sha256, payload).unwrap_or_else(|_| {
                // These unit tests mostly exercise parser structure. A fixed
                // 32-byte digest keeps them useful when optional digest crates
                // are not compiled in.
                vec![0xAB; 32]
            }),
        }
    }

    fn manifest(entries: Vec<DigestEntry>) -> DigestManifest {
        DigestManifest {
            algorithm: DigestAlgorithm::Sha256,
            scope: DigestScope::Section,
            root_digest: [0; 32],
            entries,
        }
    }

    #[test]
    fn empty_manifest_parses() {
        let m = DigestManifest::parse(&manifest(Vec::new()).serialize().unwrap()).unwrap();
        assert_eq!(m.algorithm, DigestAlgorithm::Sha256);
        assert_eq!(m.scope, DigestScope::Section);
        assert_eq!(m.entries.len(), 0);
    }

    #[test]
    fn parse_manifest_with_section_entry() {
        let bytes = manifest(vec![DigestEntry {
            target_kind: DigestTargetKind::Section,
            section_id: 42,
            local_id: 7,
            offset: 128,
            length: 16,
            digest: vec![0xCD; 32],
        }])
        .serialize()
        .unwrap();
        let m = DigestManifest::parse(&bytes).unwrap();
        assert_eq!(m.entries[0].section_id, 42);
        assert_eq!(m.entries[0].local_id, 7);
        assert_eq!(m.entries[0].offset, 128);
    }

    #[test]
    fn truncated_manifest_rejected() {
        let bytes = vec![0; DIGEST_MANIFEST_HEADER_LEN - 1];
        assert_eq!(
            DigestManifest::parse(&bytes),
            Err(CoveError::BufferTooShort)
        );
    }

    #[test]
    fn bad_header_checksum_rejected() {
        let mut bytes = manifest(Vec::new()).serialize().unwrap();
        bytes[0] ^= 0xFF;
        assert_eq!(
            DigestManifest::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        );
    }

    #[test]
    fn missing_entry_is_not_an_error() {
        let m = DigestManifest::default();
        assert!(m.verify_section(7, b"anything").is_ok());
    }

    #[cfg(feature = "digest-sha2")]
    #[test]
    fn sha256_round_trip_verifies() {
        let payload = b"Cove Format showcase";
        let bytes = manifest(vec![section_entry(5, payload)])
            .serialize()
            .unwrap();
        let m = DigestManifest::parse(&bytes).unwrap();
        assert!(m.verify_section(5, payload).is_ok());
    }

    #[cfg(feature = "digest-sha2")]
    #[test]
    fn sha256_mismatch_reports_digest_mismatch() {
        let bytes = manifest(vec![section_entry(5, b"original")])
            .serialize()
            .unwrap();
        let m = DigestManifest::parse(&bytes).unwrap();
        assert_eq!(
            m.verify_section(5, b"tampered"),
            Err(CoveError::DigestMismatch)
        );
    }

    #[cfg(feature = "digest-blake3")]
    #[test]
    fn blake3_round_trip_verifies() {
        let payload = b"Cove Format showcase";
        let digest = compute_digest(DigestAlgorithm::Blake3, payload).unwrap();
        let bytes = DigestManifest {
            algorithm: DigestAlgorithm::Blake3,
            scope: DigestScope::Section,
            root_digest: [0; 32],
            entries: vec![DigestEntry {
                target_kind: DigestTargetKind::Section,
                section_id: 5,
                local_id: 0,
                offset: 0,
                length: payload.len() as u64,
                digest,
            }],
        }
        .serialize()
        .unwrap();
        let m = DigestManifest::parse(&bytes).unwrap();
        assert!(m.verify_section(5, payload).is_ok());
    }

    #[test]
    fn wrong_length_digest_rejected_at_parse() {
        let mut bytes = manifest(vec![DigestEntry {
            target_kind: DigestTargetKind::Section,
            section_id: 1,
            local_id: 0,
            offset: 0,
            length: 4,
            digest: vec![0u8; 32],
        }])
        .serialize()
        .unwrap();
        let digest_len_pos = DIGEST_MANIFEST_HEADER_LEN + 2;
        bytes[digest_len_pos..digest_len_pos + 2].copy_from_slice(&4u16.to_le_bytes());
        bytes.truncate(DIGEST_MANIFEST_HEADER_LEN + DIGEST_ENTRY_FIXED_LEN + 4);
        let entries_length = (DIGEST_ENTRY_FIXED_LEN + 4) as u64;
        bytes[16..24].copy_from_slice(&entries_length.to_le_bytes());
        bytes[56..60].fill(0);
        let crc = checksum::crc32c(&bytes[..DIGEST_MANIFEST_HEADER_LEN]);
        bytes[56..60].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            DigestManifest::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn serialize_round_trip() {
        let m = DigestManifest {
            algorithm: DigestAlgorithm::Sha256,
            scope: DigestScope::Section,
            root_digest: [0x11; 32],
            entries: vec![
                DigestEntry {
                    target_kind: DigestTargetKind::Section,
                    section_id: 3,
                    local_id: 0,
                    offset: 100,
                    length: 20,
                    digest: vec![0xAB; 32],
                },
                DigestEntry {
                    target_kind: DigestTargetKind::Page,
                    section_id: 5,
                    local_id: 9,
                    offset: 120,
                    length: 30,
                    digest: vec![0xCD; 32],
                },
            ],
        };
        let bytes = m.serialize().unwrap();
        assert_eq!(DigestManifest::parse(&bytes).unwrap(), m);
    }
}
