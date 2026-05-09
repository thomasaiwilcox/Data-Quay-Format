use crate::{trust_chain, CoveError};

use super::{segment::TemporalSegmentData, TRUST_MANIFEST_ENTRY_LEN};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustManifestEntryV1 {
    pub segment_id: u32,
    pub row_index: u32,
    pub expected_hash: [u8; 32],
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustManifest {
    pub entries: Vec<TrustManifestEntryV1>,
}

impl TrustManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 4 {
            return Err(CoveError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let needed = 4usize
            .checked_add(
                entry_count
                    .checked_mul(TRUST_MANIFEST_ENTRY_LEN)
                    .ok_or(CoveError::ArithOverflow)?,
            )
            .ok_or(CoveError::ArithOverflow)?;
        if needed > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let mut entries = Vec::with_capacity(entry_count);
        let mut pos = 4usize;
        for _ in 0..entry_count {
            let mut expected_hash = [0u8; 32];
            expected_hash.copy_from_slice(&bytes[pos + 8..pos + 40]);
            entries.push(TrustManifestEntryV1 {
                segment_id: u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()),
                row_index: u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()),
                expected_hash,
            });
            pos += TRUST_MANIFEST_ENTRY_LEN;
        }
        Ok(Self { entries })
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.entries.len() * TRUST_MANIFEST_ENTRY_LEN);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            out.extend_from_slice(&e.segment_id.to_le_bytes());
            out.extend_from_slice(&e.row_index.to_le_bytes());
            out.extend_from_slice(&e.expected_hash);
        }
        out
    }

    pub fn verify_against(&self, segments: &[TemporalSegmentData]) -> Result<(), CoveError> {
        let mut prev = [0u8; 32];
        for entry in &self.entries {
            let segment = segments
                .iter()
                .find(|segment| segment.header.segment_id == entry.segment_id)
                .ok_or(CoveError::RefInvalid)?;
            let row = segment
                .rows
                .get(entry.row_index as usize)
                .ok_or(CoveError::RefInvalid)?;
            let computed = trust_chain::chain(&prev, &row.trust_payload())?;
            if computed != entry.expected_hash {
                return Err(CoveError::DigestMismatch);
            }
            prev = computed;
        }
        Ok(())
    }
}
