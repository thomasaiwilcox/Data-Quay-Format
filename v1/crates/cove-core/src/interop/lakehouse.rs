//! Spec §50 — Lakehouse hints.
//!
//! Optional descriptive metadata that integrates a COVE file into a lakehouse
//! catalog (Iceberg, Delta, Hudi, …). Spec §50.6 makes hints **non-
//! authoritative**: they MUST never override COVE's own structural semantics.

use crate::CoveError;

pub const LAKEHOUSE_HINT_FLAG_SOURCE_SNAPSHOT: u8 = 0x01;
pub const LAKEHOUSE_HINT_FLAG_SEQUENCE_NUMBER: u8 = 0x02;
pub const LAKEHOUSE_HINT_FLAG_VISIBILITY_OVERLAY: u8 = 0x04;

pub const LAKEHOUSE_OVERLAY_FINGERPRINT_FILE_ID: u8 = 0x01;
pub const LAKEHOUSE_OVERLAY_FINGERPRINT_FILE_LEN: u8 = 0x02;
pub const LAKEHOUSE_OVERLAY_FINGERPRINT_FOOTER_CRC32C: u8 = 0x04;
pub const LAKEHOUSE_OVERLAY_FINGERPRINT_DIGEST: u8 = 0x08;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LakehouseHints {
    pub schema_fingerprint: [u8; 32],
    pub partition_values: Vec<(String, String)>,
    pub source_snapshot: Option<u64>,
    pub sequence_number: Option<u64>,
    pub catalog_identifier: String,
    pub provenance: String,
    pub conversion_digest: [u8; 32],
    pub visibility_overlay: Option<LakehouseVisibilityOverlayRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LakehouseVisibilityOverlayRef {
    pub overlay_kind: u8,
    pub file_id: Option<[u8; 16]>,
    pub file_len: Option<u64>,
    pub footer_crc32c: Option<u32>,
    pub digest: Option<[u8; 32]>,
    pub reference: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LakehouseMetadataUse {
    PhysicalPruning,
    LookupOrInvertedCandidates,
    VisibleExactDomain,
    VisibleAggregateAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LakehouseOverlayDecision {
    Allow,
    RequireOverlayApplication,
    ForbidVisibleExactness,
}

impl LakehouseHints {
    const PARTITION_HEADER_LEN: usize = 36;
    const MIN_PARTITION_ENTRY_LEN: usize = 4;
    const MIN_TRAILER_LEN: usize = 1 + 2 + 2 + 32;

    /// Wire format (LE):
    ///   `32` schema_fingerprint
    ///   `u32` partition_count
    ///   For each: `u16` k_len, k_len bytes, `u16` v_len, v_len bytes.
    ///   `u8` flags: bit 0 source_snapshot present, bit 1 sequence_number present,
    ///               bit 2 visibility_overlay present.
    ///   if bit 0: `u64` source_snapshot.
    ///   if bit 1: `u64` sequence_number.
    ///   `u16` catalog_len, catalog bytes.
    ///   `u16` provenance_len, provenance bytes.
    ///   `32` conversion_digest.
    ///   if bit 2: visibility overlay reference.
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
        if flags & !0x07 != 0 {
            return Err(CoveError::BadSection(
                "lakehouse hint flags contain reserved bits".into(),
            ));
        }
        let source_snapshot = if flags & LAKEHOUSE_HINT_FLAG_SOURCE_SNAPSHOT != 0 {
            if pos + 8 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let v = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            Some(v)
        } else {
            None
        };
        let sequence_number = if flags & LAKEHOUSE_HINT_FLAG_SEQUENCE_NUMBER != 0 {
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
        pos += 32;
        let visibility_overlay = if flags & LAKEHOUSE_HINT_FLAG_VISIBILITY_OVERLAY != 0 {
            Some(LakehouseVisibilityOverlayRef::parse(bytes, &mut pos)?)
        } else {
            None
        };
        if pos != bytes.len() {
            return Err(CoveError::BadSection(
                "lakehouse hint has trailing bytes".into(),
            ));
        }
        Ok(Self {
            schema_fingerprint: sf,
            partition_values: partitions,
            source_snapshot,
            sequence_number,
            catalog_identifier,
            provenance,
            conversion_digest: cd,
            visibility_overlay,
        })
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(&self.schema_fingerprint);
        out.extend_from_slice(&(self.partition_values.len() as u32).to_le_bytes());
        for (k, v) in &self.partition_values {
            let kb = k.as_bytes();
            let key_len = u16::try_from(kb.len()).map_err(|_| {
                CoveError::BadSection("lakehouse partition key exceeds u16 length limit".into())
            })?;
            out.extend_from_slice(&key_len.to_le_bytes());
            out.extend_from_slice(kb);
            let vb = v.as_bytes();
            let value_len = u16::try_from(vb.len()).map_err(|_| {
                CoveError::BadSection("lakehouse partition value exceeds u16 length limit".into())
            })?;
            out.extend_from_slice(&value_len.to_le_bytes());
            out.extend_from_slice(vb);
        }
        let mut flags = 0u8;
        if self.source_snapshot.is_some() {
            flags |= LAKEHOUSE_HINT_FLAG_SOURCE_SNAPSHOT;
        }
        if self.sequence_number.is_some() {
            flags |= LAKEHOUSE_HINT_FLAG_SEQUENCE_NUMBER;
        }
        if self.visibility_overlay.is_some() {
            flags |= LAKEHOUSE_HINT_FLAG_VISIBILITY_OVERLAY;
        }
        out.push(flags);
        if let Some(v) = self.source_snapshot {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if let Some(v) = self.sequence_number {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let cb = self.catalog_identifier.as_bytes();
        let catalog_len = u16::try_from(cb.len()).map_err(|_| {
            CoveError::BadSection("lakehouse catalog_identifier exceeds u16 length limit".into())
        })?;
        out.extend_from_slice(&catalog_len.to_le_bytes());
        out.extend_from_slice(cb);
        let pb = self.provenance.as_bytes();
        let provenance_len = u16::try_from(pb.len()).map_err(|_| {
            CoveError::BadSection("lakehouse provenance exceeds u16 length limit".into())
        })?;
        out.extend_from_slice(&provenance_len.to_le_bytes());
        out.extend_from_slice(pb);
        out.extend_from_slice(&self.conversion_digest);
        if let Some(overlay) = &self.visibility_overlay {
            overlay.serialize_into(&mut out)?;
        }
        Ok(out)
    }

    pub fn overlay_decision(
        &self,
        metadata_use: LakehouseMetadataUse,
        overlay_proven_empty: bool,
        overlay_aware_correction: bool,
    ) -> LakehouseOverlayDecision {
        if self.visibility_overlay.is_none() || overlay_proven_empty {
            return LakehouseOverlayDecision::Allow;
        }
        match metadata_use {
            LakehouseMetadataUse::PhysicalPruning => LakehouseOverlayDecision::Allow,
            LakehouseMetadataUse::LookupOrInvertedCandidates => {
                LakehouseOverlayDecision::RequireOverlayApplication
            }
            LakehouseMetadataUse::VisibleExactDomain
            | LakehouseMetadataUse::VisibleAggregateAnswer => {
                if overlay_aware_correction {
                    LakehouseOverlayDecision::Allow
                } else {
                    LakehouseOverlayDecision::ForbidVisibleExactness
                }
            }
        }
    }
}

impl LakehouseVisibilityOverlayRef {
    fn parse(bytes: &[u8], pos: &mut usize) -> Result<Self, CoveError> {
        if *pos + 2 > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let overlay_kind = bytes[*pos];
        *pos += 1;
        if overlay_kind == 0 {
            return Err(CoveError::BadSection(
                "lakehouse visibility overlay kind 0 is reserved".into(),
            ));
        }
        let fingerprint_flags = bytes[*pos];
        *pos += 1;
        if fingerprint_flags & !0x0f != 0 {
            return Err(CoveError::BadSection(
                "lakehouse overlay fingerprint flags contain reserved bits".into(),
            ));
        }
        let file_id = if fingerprint_flags & LAKEHOUSE_OVERLAY_FINGERPRINT_FILE_ID != 0 {
            if *pos + 16 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let mut value = [0u8; 16];
            value.copy_from_slice(&bytes[*pos..*pos + 16]);
            *pos += 16;
            Some(value)
        } else {
            None
        };
        let file_len = if fingerprint_flags & LAKEHOUSE_OVERLAY_FINGERPRINT_FILE_LEN != 0 {
            if *pos + 8 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let value = u64::from_le_bytes(bytes[*pos..*pos + 8].try_into().unwrap());
            *pos += 8;
            Some(value)
        } else {
            None
        };
        let footer_crc32c = if fingerprint_flags & LAKEHOUSE_OVERLAY_FINGERPRINT_FOOTER_CRC32C != 0
        {
            if *pos + 4 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let value = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
            *pos += 4;
            Some(value)
        } else {
            None
        };
        let digest = if fingerprint_flags & LAKEHOUSE_OVERLAY_FINGERPRINT_DIGEST != 0 {
            if *pos + 32 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let mut value = [0u8; 32];
            value.copy_from_slice(&bytes[*pos..*pos + 32]);
            *pos += 32;
            Some(value)
        } else {
            None
        };
        let reference = read_str(bytes, pos)?;
        Ok(Self {
            overlay_kind,
            file_id,
            file_len,
            footer_crc32c,
            digest,
            reference,
        })
    }

    fn serialize_into(&self, out: &mut Vec<u8>) -> Result<(), CoveError> {
        if self.overlay_kind == 0 {
            return Err(CoveError::BadSection(
                "lakehouse visibility overlay kind 0 is reserved".into(),
            ));
        }
        out.push(self.overlay_kind);
        let mut fingerprint_flags = 0u8;
        if self.file_id.is_some() {
            fingerprint_flags |= LAKEHOUSE_OVERLAY_FINGERPRINT_FILE_ID;
        }
        if self.file_len.is_some() {
            fingerprint_flags |= LAKEHOUSE_OVERLAY_FINGERPRINT_FILE_LEN;
        }
        if self.footer_crc32c.is_some() {
            fingerprint_flags |= LAKEHOUSE_OVERLAY_FINGERPRINT_FOOTER_CRC32C;
        }
        if self.digest.is_some() {
            fingerprint_flags |= LAKEHOUSE_OVERLAY_FINGERPRINT_DIGEST;
        }
        out.push(fingerprint_flags);
        if let Some(value) = self.file_id {
            out.extend_from_slice(&value);
        }
        if let Some(value) = self.file_len {
            out.extend_from_slice(&value.to_le_bytes());
        }
        if let Some(value) = self.footer_crc32c {
            out.extend_from_slice(&value.to_le_bytes());
        }
        if let Some(value) = self.digest {
            out.extend_from_slice(&value);
        }
        let rb = self.reference.as_bytes();
        let reference_len = u16::try_from(rb.len()).map_err(|_| {
            CoveError::BadSection("lakehouse overlay reference exceeds u16 length limit".into())
        })?;
        out.extend_from_slice(&reference_len.to_le_bytes());
        out.extend_from_slice(rb);
        Ok(())
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

#[cfg(test)]
mod serialize_tests {
    use super::*;

    #[test]
    fn serialize_round_trip_with_optional_fields() {
        let h = LakehouseHints {
            schema_fingerprint: [0x11; 32],
            partition_values: vec![
                ("year".into(), "2024".into()),
                ("region".into(), "eu".into()),
            ],
            source_snapshot: Some(123),
            sequence_number: Some(456),
            catalog_identifier: "glue://prod".into(),
            provenance: "writer-v1".into(),
            conversion_digest: [0x22; 32],
            visibility_overlay: None,
        };
        let bytes = h.serialize().unwrap();
        assert_eq!(LakehouseHints::parse(&bytes).unwrap(), h);
    }

    #[test]
    fn serialize_round_trip_minimal() {
        let h = LakehouseHints::default();
        let bytes = h.serialize().unwrap();
        assert_eq!(LakehouseHints::parse(&bytes).unwrap(), h);
    }

    #[test]
    fn serialize_rejects_catalog_identifier_longer_than_u16() {
        let h = LakehouseHints {
            catalog_identifier: "a".repeat(usize::from(u16::MAX) + 1),
            ..LakehouseHints::default()
        };

        assert!(matches!(h.serialize(), Err(CoveError::BadSection(_))));
    }

    #[test]
    fn serialize_round_trip_with_visibility_overlay() {
        let h = LakehouseHints {
            schema_fingerprint: [0x11; 32],
            conversion_digest: [0x22; 32],
            visibility_overlay: Some(LakehouseVisibilityOverlayRef {
                overlay_kind: 1,
                file_id: Some([0x33; 16]),
                file_len: Some(1024),
                footer_crc32c: Some(0x1234_5678),
                digest: Some([0x44; 32]),
                reference: "s3://bucket/delete-vector.dv".into(),
            }),
            ..LakehouseHints::default()
        };
        let bytes = h.serialize().unwrap();
        assert_eq!(LakehouseHints::parse(&bytes).unwrap(), h);
    }

    #[test]
    fn overlay_decisions_guard_visible_exactness() {
        let h = LakehouseHints {
            visibility_overlay: Some(LakehouseVisibilityOverlayRef {
                overlay_kind: 1,
                file_id: None,
                file_len: None,
                footer_crc32c: None,
                digest: None,
                reference: "deletes.dv".into(),
            }),
            ..LakehouseHints::default()
        };

        assert_eq!(
            h.overlay_decision(LakehouseMetadataUse::PhysicalPruning, false, false),
            LakehouseOverlayDecision::Allow
        );
        assert_eq!(
            h.overlay_decision(
                LakehouseMetadataUse::LookupOrInvertedCandidates,
                false,
                false
            ),
            LakehouseOverlayDecision::RequireOverlayApplication
        );
        assert_eq!(
            h.overlay_decision(LakehouseMetadataUse::VisibleExactDomain, false, false),
            LakehouseOverlayDecision::ForbidVisibleExactness
        );
        assert_eq!(
            h.overlay_decision(LakehouseMetadataUse::VisibleAggregateAnswer, false, true),
            LakehouseOverlayDecision::Allow
        );
        assert_eq!(
            h.overlay_decision(LakehouseMetadataUse::VisibleExactDomain, true, false),
            LakehouseOverlayDecision::Allow
        );
    }
}
