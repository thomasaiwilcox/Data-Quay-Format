//! Quay Format (QF) v1.0 — Digest manifest skeleton.

use crate::{constants::DigestAlgorithm, QfError};

/// A single section digest entry.
#[derive(Debug, Clone, PartialEq)]
pub struct DigestEntry {
    /// Section ID this digest covers.
    pub section_id: u32,
    /// Algorithm used.
    pub algorithm: DigestAlgorithm,
    /// Digest bytes (up to 32 bytes for SHA-256 / Blake3).
    pub digest: Vec<u8>,
}

/// A parsed digest manifest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DigestManifest {
    /// All digest entries.
    pub entries: Vec<DigestEntry>,
}

impl DigestManifest {
    /// Parse a digest manifest from raw section bytes.
    ///
    /// Wire format: `u32` LE entry count, then entries of:
    /// `u32` LE `section_id`, `u16` LE `algorithm`, `u16` LE `digest_len`, digest bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 {
            return Err(QfError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let mut entries = Vec::with_capacity(entry_count as usize);

        for _ in 0..entry_count {
            // u32 section_id
            if pos.checked_add(4).ok_or(QfError::ArithOverflow)? > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let section_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos = pos.checked_add(4).ok_or(QfError::ArithOverflow)?;

            // u16 algorithm
            if pos.checked_add(2).ok_or(QfError::ArithOverflow)? > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let alg_raw = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
            pos = pos.checked_add(2).ok_or(QfError::ArithOverflow)?;
            let algorithm = DigestAlgorithm::from_u16(alg_raw).ok_or_else(|| {
                QfError::BadSection(format!("unknown digest algorithm {alg_raw}"))
            })?;

            // u16 digest_len
            if pos.checked_add(2).ok_or(QfError::ArithOverflow)? > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let digest_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos = pos.checked_add(2).ok_or(QfError::ArithOverflow)?;

            // digest bytes
            let digest_end = pos.checked_add(digest_len).ok_or(QfError::ArithOverflow)?;
            if digest_end > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let digest = bytes[pos..digest_end].to_vec();
            pos = digest_end;

            entries.push(DigestEntry {
                section_id,
                algorithm,
                digest,
            });
        }

        Ok(Self { entries })
    }

    /// Verify a section's digest.
    ///
    /// - [`DigestAlgorithm::None`]: always returns `Ok(())`.
    /// - [`DigestAlgorithm::Sha256`]: returns [`QfError::UnsupportedEncoding`] (feature not built).
    /// - [`DigestAlgorithm::Blake3`]: returns [`QfError::UnsupportedEncoding`] (feature not built).
    pub fn verify_section(&self, section_id: u32, _section_bytes: &[u8]) -> Result<(), QfError> {
        let entry = match self.entries.iter().find(|e| e.section_id == section_id) {
            Some(e) => e,
            None => return Ok(()),
        };
        match entry.algorithm {
            DigestAlgorithm::None => Ok(()),
            DigestAlgorithm::Sha256 => Err(QfError::UnsupportedEncoding(
                "SHA-256 digest verification requires the sha2 crate feature".into(),
            )),
            DigestAlgorithm::Blake3 => Err(QfError::UnsupportedEncoding(
                "Blake3 digest verification requires the blake3 crate feature".into(),
            )),
        }
    }
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
        let bytes = make_manifest_bytes(&[]);
        let m = DigestManifest::parse(&bytes).unwrap();
        assert_eq!(m.entries.len(), 0);
    }

    #[test]
    fn parse_manifest_with_entries() {
        let bytes = make_manifest_bytes(&[(42, 0, b"")]);
        let m = DigestManifest::parse(&bytes).unwrap();
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].section_id, 42);
        assert_eq!(m.entries[0].algorithm, DigestAlgorithm::None);
    }

    #[test]
    fn verify_none_algorithm_always_passes() {
        let bytes = make_manifest_bytes(&[(1, 0, b"")]);
        let m = DigestManifest::parse(&bytes).unwrap();
        assert!(m.verify_section(1, b"anything").is_ok());
    }

    #[test]
    fn verify_sha256_returns_unsupported() {
        let bytes = make_manifest_bytes(&[(1, 1, &[0u8; 32])]);
        let m = DigestManifest::parse(&bytes).unwrap();
        assert!(matches!(
            m.verify_section(1, b"data"),
            Err(QfError::UnsupportedEncoding(_))
        ));
    }

    #[test]
    fn truncated_manifest_rejected() {
        let bytes = 1u32.to_le_bytes().to_vec(); // declares 1 entry but has no entry data
        assert_eq!(DigestManifest::parse(&bytes), Err(QfError::BufferTooShort));
    }
}
