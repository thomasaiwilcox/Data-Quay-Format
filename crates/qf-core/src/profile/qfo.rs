//! Spec §55–§63 — QF-O object-temporal profile.
//!
//! Tracks an append-only history of object records. Each record carries
//! `(timestamp_us, csn, branch_key, goid, record_id)` and Spec §58 mandates
//! that rows be sorted by that lexicographic key. The trust chain (Spec §63)
//! hashes canonical logical values so that re-encoding a file with new
//! FileCodes preserves the chain.

use crate::{
    checksum,
    constants::{QfLogicalType, QfPhysicalKind},
    types::validate_logical_physical_pair,
    QfError,
};

pub const TEMPORAL_SEGMENT_INDEX_ENTRY_LEN: usize = 112;

/// Record kinds (Spec §59.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    Create,
    Update,
    Delete,
    Snapshot,
    Baseline,
    /// Staging placeholder — MUST be rejected by readers (Spec §59.4).
    StagingPlaceholder,
}

impl RecordKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(RecordKind::Create),
            1 => Some(RecordKind::Update),
            2 => Some(RecordKind::Delete),
            3 => Some(RecordKind::Snapshot),
            4 => Some(RecordKind::Baseline),
            5 => Some(RecordKind::StagingPlaceholder),
            _ => None,
        }
    }

    /// Spec §59.4: staging placeholders must never appear in a published file.
    pub fn validate_published(self) -> Result<(), QfError> {
        if matches!(self, RecordKind::StagingPlaceholder) {
            Err(QfError::BadSchema(
                "staging placeholder leaked into published file (Spec §59.4)".into(),
            ))
        } else {
            Ok(())
        }
    }
}

/// One row in the temporal segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemporalRowKey {
    pub timestamp_us: i64,
    pub csn: u64,
    pub branch_key: u64,
    pub goid: u64,
    pub record_id: u64,
}

impl TemporalRowKey {
    /// Lexicographic compare per Spec §58.3.
    pub fn cmp_lex(&self, other: &Self) -> std::cmp::Ordering {
        (
            self.timestamp_us,
            self.csn,
            self.branch_key,
            self.goid,
            self.record_id,
        )
            .cmp(&(
                other.timestamp_us,
                other.csn,
                other.branch_key,
                other.goid,
                other.record_id,
            ))
    }
}

/// Validate that a slice of temporal rows is sorted in the §58.3 order.
pub fn validate_temporal_order(rows: &[TemporalRowKey]) -> Result<(), QfError> {
    for w in rows.windows(2) {
        if w[0].cmp_lex(&w[1]) == std::cmp::Ordering::Greater {
            return Err(QfError::BadSchema(
                "temporal rows out of order (Spec §58.3)".into(),
            ));
        }
    }
    Ok(())
}

/// Self-containment check (Spec §60). A QF-O file is self-contained if every
/// `prev_ref` points to a row inside the same file. v1 forbids cross-file
/// `prev_ref` chains.
pub fn validate_self_contained(
    prev_refs: &[Option<u64>],
    local_record_ids: &[u64],
) -> Result<(), QfError> {
    let local: std::collections::HashSet<u64> = local_record_ids.iter().copied().collect();
    for p in prev_refs.iter().flatten() {
        if !local.contains(p) {
            return Err(QfError::NotSelfContained);
        }
    }
    Ok(())
}

// ── Object type catalog (§56) ────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectTypeCatalog {
    pub flags: u32,
    pub types: Vec<ObjectTypeEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectTypeEntryV1 {
    pub object_type_id: u32,
    pub type_name: String,
    pub properties: Vec<PropertyEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyEntryV1 {
    pub property_id: u32,
    pub property_name: String,
    pub logical_type: QfLogicalType,
    pub physical_kind: QfPhysicalKind,
    pub nullable: bool,
    pub collation_id: u16,
    pub flags: u32,
}

impl ObjectTypeCatalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 8 {
            return Err(QfError::BufferTooShort);
        }
        let type_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let mut pos = 8usize;
        let mut types = Vec::with_capacity(type_count);
        for _ in 0..type_count {
            let (entry, used) = ObjectTypeEntryV1::parse(&bytes[pos..])?;
            pos = pos.checked_add(used).ok_or(QfError::ArithOverflow)?;
            types.push(entry);
        }
        let catalog = Self { flags, types };
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        let count = u32::try_from(self.types.len())
            .map_err(|_| QfError::BadSchema("too many object types".into()))?;
        let mut out = Vec::new();
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for ty in &self.types {
            out.extend_from_slice(&ty.serialize()?);
        }
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), QfError> {
        let mut seen_types = std::collections::HashSet::new();
        for ty in &self.types {
            if !seen_types.insert(ty.object_type_id) {
                return Err(QfError::BadSchema(format!(
                    "duplicate object_type_id {} (Spec §56)",
                    ty.object_type_id
                )));
            }
            let mut seen_props = std::collections::HashSet::new();
            for prop in &ty.properties {
                if !seen_props.insert(prop.property_id) {
                    return Err(QfError::BadSchema(format!(
                        "duplicate property_id {} in object_type_id {} (Spec §56)",
                        prop.property_id, ty.object_type_id
                    )));
                }
                if prop.logical_type == QfLogicalType::Null {
                    return Err(QfError::BadSchema(format!(
                        "property {} declares logical Null at top level (Spec §56)",
                        prop.property_id
                    )));
                }
                if validate_logical_physical_pair(prop.logical_type, prop.physical_kind).is_err() {
                    return Err(QfError::BadLogicalPhysicalPair);
                }
            }
        }
        Ok(())
    }
}

impl ObjectTypeEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), QfError> {
        if bytes.len() < 4 + 2 {
            return Err(QfError::BufferTooShort);
        }
        let object_type_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let type_name = read_str(bytes, &mut pos, "object type name")?;
        if bytes.len() < pos + 2 {
            return Err(QfError::BufferTooShort);
        }
        let property_count = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        let mut properties = Vec::with_capacity(property_count);
        for _ in 0..property_count {
            let (property, used) = PropertyEntryV1::parse(&bytes[pos..])?;
            pos = pos.checked_add(used).ok_or(QfError::ArithOverflow)?;
            properties.push(property);
        }
        Ok((
            Self {
                object_type_id,
                type_name,
                properties,
            },
            pos,
        ))
    }

    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        let property_count = u16::try_from(self.properties.len())
            .map_err(|_| QfError::BadSchema("too many properties".into()))?;
        let mut out = Vec::new();
        out.extend_from_slice(&self.object_type_id.to_le_bytes());
        write_str(&mut out, &self.type_name, "object type name")?;
        out.extend_from_slice(&property_count.to_le_bytes());
        for prop in &self.properties {
            out.extend_from_slice(&prop.serialize()?);
        }
        Ok(out)
    }
}

impl PropertyEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), QfError> {
        if bytes.len() < 4 + 2 {
            return Err(QfError::BufferTooShort);
        }
        let property_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let property_name = read_str(bytes, &mut pos, "property name")?;
        if bytes.len() < pos + 2 + 1 + 1 + 2 + 4 {
            return Err(QfError::BufferTooShort);
        }
        let logical_raw = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let physical_raw = bytes[pos];
        pos += 1;
        let nullable_raw = bytes[pos];
        pos += 1;
        let collation_id = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let flags = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let logical_type = QfLogicalType::from_u16(logical_raw).ok_or_else(|| {
            QfError::BadSchema(format!(
                "unknown object property logical type {logical_raw}"
            ))
        })?;
        let physical_kind = QfPhysicalKind::from_u8(physical_raw).ok_or_else(|| {
            QfError::BadSchema(format!(
                "unknown object property physical kind {physical_raw}"
            ))
        })?;
        let nullable = match nullable_raw {
            0 => false,
            1 => true,
            other => {
                return Err(QfError::BadSchema(format!(
                    "object property nullable flag must be 0 or 1, got {other}"
                )))
            }
        };
        Ok((
            Self {
                property_id,
                property_name,
                logical_type,
                physical_kind,
                nullable,
                collation_id,
                flags,
            },
            pos,
        ))
    }

    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.property_id.to_le_bytes());
        write_str(&mut out, &self.property_name, "property name")?;
        out.extend_from_slice(&(self.logical_type as u16).to_le_bytes());
        out.push(self.physical_kind as u8);
        out.push(if self.nullable { 1 } else { 0 });
        out.extend_from_slice(&self.collation_id.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        Ok(out)
    }
}

// ── Temporal segment index (§57) ─────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemporalSegmentIndex {
    pub flags: u32,
    pub entries: Vec<TemporalSegmentIndexEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalSegmentIndexEntryV1 {
    pub segment_id: u32,
    pub object_type_id: u32,
    pub time_range_start_us: i64,
    pub time_range_end_us: i64,
    pub csn_min: u64,
    pub csn_max: u64,
    pub row_count: u32,
    pub delta_count: u32,
    pub snapshot_count: u32,
    pub baseline_count: u32,
    pub tombstone_count: u32,
    pub min_goid: [u8; 16],
    pub max_goid: [u8; 16],
    pub offset: u64,
    pub length: u64,
    pub checksum: u32,
}

impl TemporalSegmentIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 8 {
            return Err(QfError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let needed = 8usize
            .checked_add(
                entry_count
                    .checked_mul(TEMPORAL_SEGMENT_INDEX_ENTRY_LEN)
                    .ok_or(QfError::ArithOverflow)?,
            )
            .ok_or(QfError::ArithOverflow)?;
        if needed > bytes.len() {
            return Err(QfError::BufferTooShort);
        }
        let mut entries = Vec::with_capacity(entry_count);
        let mut pos = 8usize;
        for _ in 0..entry_count {
            entries.push(TemporalSegmentIndexEntryV1::parse(
                &bytes[pos..pos + TEMPORAL_SEGMENT_INDEX_ENTRY_LEN],
            )?);
            pos += TEMPORAL_SEGMENT_INDEX_ENTRY_LEN;
        }
        let index = Self { flags, entries };
        index.validate()?;
        Ok(index)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        let count = u32::try_from(self.entries.len())
            .map_err(|_| QfError::BadSchema("too many temporal segments".into()))?;
        let mut out = Vec::with_capacity(8 + self.entries.len() * TEMPORAL_SEGMENT_INDEX_ENTRY_LEN);
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize());
        }
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), QfError> {
        let mut seen = std::collections::HashSet::new();
        for entry in &self.entries {
            if !seen.insert((entry.object_type_id, entry.segment_id)) {
                return Err(QfError::RefInvalid);
            }
            entry.validate()?;
        }
        Ok(())
    }
}

impl TemporalSegmentIndexEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < TEMPORAL_SEGMENT_INDEX_ENTRY_LEN {
            return Err(QfError::BufferTooShort);
        }
        let segment_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let object_type_id = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let time_range_start_us = i64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let time_range_end_us = i64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let csn_min = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
        let csn_max = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        let row_count = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        let delta_count = u32::from_le_bytes(bytes[44..48].try_into().unwrap());
        let snapshot_count = u32::from_le_bytes(bytes[48..52].try_into().unwrap());
        let baseline_count = u32::from_le_bytes(bytes[52..56].try_into().unwrap());
        let tombstone_count = u32::from_le_bytes(bytes[56..60].try_into().unwrap());
        let mut min_goid = [0u8; 16];
        min_goid.copy_from_slice(&bytes[60..76]);
        let mut max_goid = [0u8; 16];
        max_goid.copy_from_slice(&bytes[76..92]);
        let offset = u64::from_le_bytes(bytes[92..100].try_into().unwrap());
        let length = u64::from_le_bytes(bytes[100..108].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(bytes[108..112].try_into().unwrap());
        let mut for_crc = [0u8; TEMPORAL_SEGMENT_INDEX_ENTRY_LEN];
        for_crc.copy_from_slice(&bytes[..TEMPORAL_SEGMENT_INDEX_ENTRY_LEN]);
        for_crc[108..112].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }
        let entry = Self {
            segment_id,
            object_type_id,
            time_range_start_us,
            time_range_end_us,
            csn_min,
            csn_max,
            row_count,
            delta_count,
            snapshot_count,
            baseline_count,
            tombstone_count,
            min_goid,
            max_goid,
            offset,
            length,
            checksum: checksum_field,
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn serialize(&self) -> [u8; TEMPORAL_SEGMENT_INDEX_ENTRY_LEN] {
        let mut buf = [0u8; TEMPORAL_SEGMENT_INDEX_ENTRY_LEN];
        buf[0..4].copy_from_slice(&self.segment_id.to_le_bytes());
        buf[4..8].copy_from_slice(&self.object_type_id.to_le_bytes());
        buf[8..16].copy_from_slice(&self.time_range_start_us.to_le_bytes());
        buf[16..24].copy_from_slice(&self.time_range_end_us.to_le_bytes());
        buf[24..32].copy_from_slice(&self.csn_min.to_le_bytes());
        buf[32..40].copy_from_slice(&self.csn_max.to_le_bytes());
        buf[40..44].copy_from_slice(&self.row_count.to_le_bytes());
        buf[44..48].copy_from_slice(&self.delta_count.to_le_bytes());
        buf[48..52].copy_from_slice(&self.snapshot_count.to_le_bytes());
        buf[52..56].copy_from_slice(&self.baseline_count.to_le_bytes());
        buf[56..60].copy_from_slice(&self.tombstone_count.to_le_bytes());
        buf[60..76].copy_from_slice(&self.min_goid);
        buf[76..92].copy_from_slice(&self.max_goid);
        buf[92..100].copy_from_slice(&self.offset.to_le_bytes());
        buf[100..108].copy_from_slice(&self.length.to_le_bytes());
        let crc = checksum::crc32c(&buf);
        buf[108..112].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn validate(&self) -> Result<(), QfError> {
        if self.time_range_start_us > self.time_range_end_us || self.csn_min > self.csn_max {
            return Err(QfError::BadSchema(
                "temporal segment range is inverted (Spec §57)".into(),
            ));
        }
        if self.min_goid > self.max_goid {
            return Err(QfError::BadSchema(
                "temporal segment GOID range is inverted (Spec §57)".into(),
            ));
        }
        let counted = self
            .delta_count
            .checked_add(self.snapshot_count)
            .and_then(|v| v.checked_add(self.baseline_count))
            .and_then(|v| v.checked_add(self.tombstone_count))
            .ok_or(QfError::ArithOverflow)?;
        if counted != self.row_count {
            return Err(QfError::BadSchema(
                "temporal segment record-kind counts do not sum to row_count (Spec §57)".into(),
            ));
        }
        Ok(())
    }
}

fn read_str(bytes: &[u8], pos: &mut usize, what: &str) -> Result<String, QfError> {
    if *pos + 2 > bytes.len() {
        return Err(QfError::BufferTooShort);
    }
    let len = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap()) as usize;
    *pos += 2;
    let end = pos.checked_add(len).ok_or(QfError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(QfError::BufferTooShort);
    }
    let s = std::str::from_utf8(&bytes[*pos..end])
        .map_err(|_| QfError::BadSchema(format!("{what} is not valid UTF-8")))?
        .to_string();
    *pos = end;
    Ok(s)
}

fn write_str(out: &mut Vec<u8>, s: &str, what: &str) -> Result<(), QfError> {
    let len = u16::try_from(s.len())
        .map_err(|_| QfError::BadSchema(format!("{what} exceeds u16::MAX")))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(t: i64, csn: u64) -> TemporalRowKey {
        TemporalRowKey {
            timestamp_us: t,
            csn,
            branch_key: 0,
            goid: 0,
            record_id: 0,
        }
    }

    #[test]
    fn spec_58_3_lex_order_validates() {
        let rows = vec![k(1, 1), k(1, 2), k(2, 0)];
        assert!(validate_temporal_order(&rows).is_ok());
    }

    #[test]
    fn spec_58_3_out_of_order_rejected() {
        let rows = vec![k(2, 0), k(1, 0)];
        assert!(matches!(
            validate_temporal_order(&rows),
            Err(QfError::BadSchema(_))
        ));
    }

    #[test]
    fn spec_59_4_staging_placeholder_rejected() {
        assert!(RecordKind::StagingPlaceholder.validate_published().is_err());
        assert!(RecordKind::Create.validate_published().is_ok());
    }

    #[test]
    fn spec_60_dangling_prev_ref_rejected() {
        let prev = vec![Some(7), Some(99)];
        let local = vec![7];
        assert_eq!(
            validate_self_contained(&prev, &local),
            Err(QfError::NotSelfContained)
        );
    }

    fn property(property_id: u32) -> PropertyEntryV1 {
        PropertyEntryV1 {
            property_id,
            property_name: "name".into(),
            logical_type: QfLogicalType::Bool,
            physical_kind: QfPhysicalKind::Boolean,
            nullable: false,
            collation_id: 0,
            flags: 0,
        }
    }

    #[test]
    fn object_type_catalog_roundtrip() {
        let catalog = ObjectTypeCatalog {
            flags: 0,
            types: vec![ObjectTypeEntryV1 {
                object_type_id: 10,
                type_name: "Customer".into(),
                properties: vec![property(1)],
            }],
        };
        let parsed = ObjectTypeCatalog::parse(&catalog.serialize().unwrap()).unwrap();
        assert_eq!(parsed.types[0].type_name, "Customer");
    }

    #[test]
    fn object_type_catalog_rejects_duplicate_property() {
        let catalog = ObjectTypeCatalog {
            flags: 0,
            types: vec![ObjectTypeEntryV1 {
                object_type_id: 10,
                type_name: "Customer".into(),
                properties: vec![property(1), property(1)],
            }],
        };
        assert!(matches!(
            ObjectTypeCatalog::parse(&catalog.serialize().unwrap()),
            Err(QfError::BadSchema(_))
        ));
    }

    #[test]
    fn object_type_catalog_rejects_logical_null_property() {
        let mut p = property(1);
        p.logical_type = QfLogicalType::Null;
        p.physical_kind = QfPhysicalKind::FileCode;
        let catalog = ObjectTypeCatalog {
            flags: 0,
            types: vec![ObjectTypeEntryV1 {
                object_type_id: 10,
                type_name: "Customer".into(),
                properties: vec![p],
            }],
        };
        assert!(matches!(
            ObjectTypeCatalog::parse(&catalog.serialize().unwrap()),
            Err(QfError::BadSchema(_))
        ));
    }

    fn temporal_entry(segment_id: u32, row_count: u32) -> TemporalSegmentIndexEntryV1 {
        TemporalSegmentIndexEntryV1 {
            segment_id,
            object_type_id: 1,
            time_range_start_us: 10,
            time_range_end_us: 20,
            csn_min: 1,
            csn_max: 2,
            row_count,
            delta_count: row_count,
            snapshot_count: 0,
            baseline_count: 0,
            tombstone_count: 0,
            min_goid: [0; 16],
            max_goid: [1; 16],
            offset: 128,
            length: 4096,
            checksum: 0,
        }
    }

    #[test]
    fn temporal_segment_index_roundtrip() {
        let index = TemporalSegmentIndex {
            flags: 0,
            entries: vec![temporal_entry(1, 2)],
        };
        let parsed = TemporalSegmentIndex::parse(&index.serialize().unwrap()).unwrap();
        assert_eq!(parsed.entries[0].row_count, 2);
    }

    #[test]
    fn temporal_segment_index_rejects_duplicate_segment_id() {
        let index = TemporalSegmentIndex {
            flags: 0,
            entries: vec![temporal_entry(1, 1), temporal_entry(1, 1)],
        };
        assert_eq!(
            TemporalSegmentIndex::parse(&index.serialize().unwrap()),
            Err(QfError::RefInvalid)
        );
    }

    #[test]
    fn temporal_segment_index_rejects_bad_counts() {
        let mut entry = temporal_entry(1, 2);
        entry.tombstone_count = 1;
        let index = TemporalSegmentIndex {
            flags: 0,
            entries: vec![entry],
        };
        assert!(matches!(
            TemporalSegmentIndex::parse(&index.serialize().unwrap()),
            Err(QfError::BadSchema(_))
        ));
    }
}
