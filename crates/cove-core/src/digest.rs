//! Cove Format (COVE) v1.0 — Digest manifest (Spec §65).
//!
//! Digest manifests bind cryptographic hashes to sections, pages, the whole
//! file, or custom targets. Validation is policy-aware: a missing digest entry
//! for a target is _not_ an error (digests are optional), but a present entry
//! that fails verification is reported as [`CoveError::DigestMismatch`].

use crate::{constants::DigestAlgorithm, CoveError};

/// A single digest entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestEntry {
    /// Section ID this digest covers.
    pub section_id: u32,
    /// Algorithm used.
    pub algorithm: DigestAlgorithm,
    /// Digest bytes (32 bytes for SHA-256 / BLAKE3).
    pub digest: Vec<u8>,
}

/// A parsed digest manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DigestManifest {
    /// All digest entries.
    pub entries: Vec<DigestEntry>,
}

impl DigestManifest {
    /// Parse a digest manifest from raw section bytes.
    ///
    /// Wire format: `u32` LE entry count, then entries of:
    /// `u32` LE `section_id`, `u16` LE `algorithm`, `u16` LE `digest_len`,
    /// `digest_len` digest bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 4 {
            return Err(CoveError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let mut entries = Vec::with_capacity(entry_count as usize);

        for _ in 0..entry_count {
            if pos.checked_add(4).ok_or(CoveError::ArithOverflow)? > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let section_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;

            if pos.checked_add(2).ok_or(CoveError::ArithOverflow)? > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let alg_raw = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
            pos += 2;
            let algorithm = DigestAlgorithm::from_u16(alg_raw).ok_or_else(|| {
                CoveError::BadSection(format!("unknown digest algorithm {alg_raw}"))
            })?;

            if pos.checked_add(2).ok_or(CoveError::ArithOverflow)? > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let digest_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;

            let digest_end = pos
                .checked_add(digest_len)
                .ok_or(CoveError::ArithOverflow)?;
            if digest_end > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let digest = bytes[pos..digest_end].to_vec();
            pos = digest_end;

            // Length sanity check per Spec §65: SHA-256 and BLAKE3 always emit 32 bytes.
            match algorithm {
                DigestAlgorithm::Sha256 | DigestAlgorithm::Blake3 if digest.len() != 32 => {
                    return Err(CoveError::BadSection(format!(
                        "digest for algorithm {algorithm:?} must be 32 bytes, got {}",
                        digest.len()
                    )));
                }
                _ => {}
            }

            entries.push(DigestEntry {
                section_id,
                algorithm,
                digest,
            });
        }

        Ok(Self { entries })
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.entries.len() * 40);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            out.extend_from_slice(&entry.section_id.to_le_bytes());
            out.extend_from_slice(&(entry.algorithm as u16).to_le_bytes());
            out.extend_from_slice(&(entry.digest.len() as u16).to_le_bytes());
            out.extend_from_slice(&entry.digest);
        }
        out
    }

    /// Verify a section's digest. Returns `Ok(())` if no entry exists for the
    /// section (digests are optional per Spec §65); returns
    /// [`CoveError::DigestMismatch`] if a present entry fails.
    pub fn verify_section(&self, section_id: u32, section_bytes: &[u8]) -> Result<(), CoveError> {
        let entry = match self.entries.iter().find(|e| e.section_id == section_id) {
            Some(e) => e,
            None => return Ok(()),
        };
        verify_digest(entry.algorithm, section_bytes, &entry.digest)
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

    fn make_manifest_bytes(entries: &[(u32, u16, &[u8])]) -> Vec<u8> {
        let mut out = (entries.len() as u32).to_le_bytes().to_vec();
        for (section_id, alg, digest) in entries {
            out.extend_from_slice(&section_id.to_le_bytes());
            out.extend_from_slice(&alg.to_le_bytes());
            out.extend_from_slice(&(digest.len() as u16).to_le_bytes());
            out.extend_from_slice(digest);
        }
        out
    }

    #[test]
    fn empty_manifest_parses() {
        let m = DigestManifest::parse(&make_manifest_bytes(&[])).unwrap();
        assert_eq!(m.entries.len(), 0);
    }

    #[test]
    fn parse_manifest_with_none_entry() {
        let m = DigestManifest::parse(&make_manifest_bytes(&[(42, 0, b"")])).unwrap();
        assert_eq!(m.entries[0].section_id, 42);
        assert_eq!(m.entries[0].algorithm, DigestAlgorithm::None);
    }

    #[test]
    fn truncated_manifest_rejected() {
        let bytes = 1u32.to_le_bytes().to_vec();
        assert_eq!(
            DigestManifest::parse(&bytes),
            Err(CoveError::BufferTooShort)
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
        let digest = compute_digest(DigestAlgorithm::Sha256, payload).unwrap();
        let bytes = make_manifest_bytes(&[(5, 1, &digest)]);
        let m = DigestManifest::parse(&bytes).unwrap();
        assert!(m.verify_section(5, payload).is_ok());
    }

    #[cfg(feature = "digest-sha2")]
    #[test]
    fn sha256_mismatch_reports_digest_mismatch() {
        let digest = compute_digest(DigestAlgorithm::Sha256, b"original").unwrap();
        let bytes = make_manifest_bytes(&[(5, 1, &digest)]);
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
        let bytes = make_manifest_bytes(&[(5, 2, &digest)]);
        let m = DigestManifest::parse(&bytes).unwrap();
        assert!(m.verify_section(5, payload).is_ok());
    }

    #[test]
    fn wrong_length_digest_rejected_at_parse() {
        // SHA-256 with only 4 bytes of digest is invalid.
        let bytes = make_manifest_bytes(&[(1, 1, &[0u8; 4])]);
        assert!(matches!(
            DigestManifest::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }
}

#[cfg(test)]
mod serialize_tests {
    use super::*;

    #[test]
    fn serialize_round_trip() {
        let m = DigestManifest {
            entries: vec![
                DigestEntry {
                    section_id: 3,
                    algorithm: DigestAlgorithm::Sha256,
                    digest: vec![0xAB; 32],
                },
                DigestEntry {
                    section_id: 5,
                    algorithm: DigestAlgorithm::Blake3,
                    digest: vec![0xCD; 32],
                },
            ],
        };
        let bytes = m.serialize();
        assert_eq!(DigestManifest::parse(&bytes).unwrap(), m);
    }
}
