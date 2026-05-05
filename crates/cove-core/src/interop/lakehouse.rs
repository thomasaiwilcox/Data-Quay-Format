//! Spec §50 — Lakehouse hints.
//!
//! Optional descriptive metadata that integrates a COVE file into a lakehouse
//! catalog (Iceberg, Delta, Hudi, …). Spec §50.6 makes hints **non-
//! authoritative**: they MUST never override COVE's own structural semantics.

use crate::CoveError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LakehouseHints {
    pub schema_fingerprint: [u8; 32],
    pub partition_values: Vec<(String, String)>,
    pub source_snapshot: Option<u64>,
    pub sequence_number: Option<u64>,
    pub catalog_identifier: String,
    pub provenance: String,
    pub conversion_digest: [u8; 32],
}

impl LakehouseHints {
    const PARTITION_HEADER_LEN: usize = 36;
    const MIN_PARTITION_ENTRY_LEN: usize = 4;
    const MIN_TRAILER_LEN: usize = 1 + 2 + 2 + 32;

    /// Wire format (LE):
    ///   `32` schema_fingerprint
    ///   `u32` partition_count
    ///   For each: `u16` k_len, k_len bytes, `u16` v_len, v_len bytes.
    ///   `u8` flags: bit 0 source_snapshot present, bit 1 sequence_number present.
    ///   if bit 0: `u64` source_snapshot.
    ///   if bit 1: `u64` sequence_number.
    ///   `u16` catalog_len, catalog bytes.
    ///   `u16` provenance_len, provenance bytes.
    ///   `32` conversion_digest.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::PARTITION_HEADER_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let mut sf = [0u8; 32];
        sf.copy_from_slice(&bytes[0..32]);
        let pc = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
        let remaining = bytes
            .len()
            .checked_sub(Self::PARTITION_HEADER_LEN)
            .ok_or(CoveError::BufferTooShort)?;
        let max_partitions =
            remaining.saturating_sub(Self::MIN_TRAILER_LEN) / Self::MIN_PARTITION_ENTRY_LEN;
        if pc > max_partitions {
            return Err(CoveError::BufferTooShort);
        }
        let mut pos = Self::PARTITION_HEADER_LEN;
        let mut partitions = Vec::with_capacity(pc);
        for _ in 0..pc {
            let k = read_str(bytes, &mut pos)?;
            let v = read_str(bytes, &mut pos)?;
            partitions.push((k, v));
        }
        if pos + 1 > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let flags = bytes[pos];
        pos += 1;
        let source_snapshot = if flags & 1 != 0 {
            if pos + 8 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let v = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            Some(v)
        } else {
            None
        };
        let sequence_number = if flags & 2 != 0 {
            if pos + 8 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let v = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            Some(v)
        } else {
            None
        };
        let catalog_identifier = read_str(bytes, &mut pos)?;
        let provenance = read_str(bytes, &mut pos)?;
        if pos + 32 > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let mut cd = [0u8; 32];
        cd.copy_from_slice(&bytes[pos..pos + 32]);
        Ok(Self {
            schema_fingerprint: sf,
            partition_values: partitions,
            source_snapshot,
            sequence_number,
            catalog_identifier,
            provenance,
            conversion_digest: cd,
        })
    }
}

fn read_str(bytes: &[u8], pos: &mut usize) -> Result<String, CoveError> {
    if *pos + 2 > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let len = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap()) as usize;
    *pos += 2;
    let end = pos.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let s = std::str::from_utf8(&bytes[*pos..end])
        .map_err(|_| CoveError::BadSection("lakehouse hint not UTF-8".into()))?
        .to_string();
    *pos = end;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_minimal_hints() {
        let mut bytes = vec![0u8; 32]; // sf
        bytes.extend_from_slice(&0u32.to_le_bytes()); // 0 partitions
        bytes.push(0); // no flags
        bytes.extend_from_slice(&0u16.to_le_bytes()); // catalog
        bytes.extend_from_slice(&0u16.to_le_bytes()); // provenance
        bytes.extend_from_slice(&[0u8; 32]); // conversion_digest
        let h = LakehouseHints::parse(&bytes).unwrap();
        assert!(h.partition_values.is_empty());
        assert!(h.source_snapshot.is_none());
    }

    #[test]
    fn rejects_oversized_partition_count_before_allocating() {
        let mut bytes = vec![0u8; 32];
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 32]);
        assert_eq!(
            LakehouseHints::parse(&bytes),
            Err(CoveError::BufferTooShort)
        );
    }
}
