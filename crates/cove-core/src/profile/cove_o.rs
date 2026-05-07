//! Spec §55–§63 — COVE-O object-temporal profile.
//!
//! Tracks an append-only history of object records. Each record carries
//! `(timestamp_us, csn, branch_key, goid, record_id)` and Spec §58 mandates
//! that rows be sorted by that lexicographic key. The trust chain (Spec §63)
//! hashes canonical logical values so that re-encoding a file with new
//! FileCodes preserves the chain.

use crate::{
    checksum, compression,
    constants::{CoveLogicalType, CovePhysicalKind, FEATURE_PAGE_PAYLOAD_ELISION},
    page::{
        page_uses_payload_elision, ColumnPageIndex, ColumnPageIndexEntryV1, PAGE_FLAG_ALL_NON_NULL,
    },
    page_payload::ColumnPagePayloadV1,
    page_validation::{
        validate_column_page_payload, validate_stats_only_constant_page, PageValidationContext,
    },
    segment::{TableColumnDirectoryEntryV1, TABLE_COLUMN_DIRECTORY_ENTRY_LEN},
    trust_chain,
    types::{validate_logical_physical_pair_with_options, LogicalPhysicalOptions},
    CoveError,
};

pub const TEMPORAL_SEGMENT_HEADER_LEN: usize = 96;
pub const TEMPORAL_SEGMENT_INDEX_ENTRY_LEN: usize = 112;
pub const TEMPORAL_ROW_ENTRY_LEN: usize = 68;
pub const TEMPORAL_BLOOM_ENTRY_LEN: usize = 40;
pub const TRUST_MANIFEST_ENTRY_LEN: usize = 40;

/// Record kinds (Spec §59.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecordKind {
    Delta,
    Snapshot,
    ReservedLegacyMaterializedDelta,
    Baseline,
    Tombstone,
}

impl RecordKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(RecordKind::Delta),
            1 => Some(RecordKind::Snapshot),
            2 => Some(RecordKind::ReservedLegacyMaterializedDelta),
            3 => Some(RecordKind::Baseline),
            4 => Some(RecordKind::Tombstone),
            _ => None,
        }
    }

    /// Reserved legacy materialized-delta records are not valid published rows.
    pub fn validate_published(self) -> Result<(), CoveError> {
        if matches!(self, RecordKind::ReservedLegacyMaterializedDelta) {
            Err(CoveError::BadSchema(
                "reserved legacy materialized delta is not valid in published files (Spec §59.1)"
                    .into(),
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
    pub goid: [u8; 16],
    pub record_id: [u8; 16],
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
pub fn validate_temporal_order(rows: &[TemporalRowKey]) -> Result<(), CoveError> {
    for w in rows.windows(2) {
        if w[0].cmp_lex(&w[1]) == std::cmp::Ordering::Greater {
            return Err(CoveError::BadSchema(
                "temporal rows out of order (Spec §58.3)".into(),
            ));
        }
    }
    Ok(())
}

/// Self-containment check (Spec §60). A COVE-O file is self-contained if every
/// `prev_ref` points to a row inside the same file. v1 forbids cross-file
/// `prev_ref` chains.
pub fn validate_self_contained(
    prev_refs: &[Option<u64>],
    local_record_ids: &[u64],
) -> Result<(), CoveError> {
    let local: std::collections::HashSet<u64> = local_record_ids.iter().copied().collect();
    for p in prev_refs.iter().flatten() {
        if !local.contains(p) {
            return Err(CoveError::NotSelfContained);
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
    pub flags: u32,
    pub properties: Vec<PropertyEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyEntryV1 {
    pub property_id: u32,
    pub property_name: String,
    pub logical_type: CoveLogicalType,
    pub physical_kind: CovePhysicalKind,
    pub nullable: bool,
    pub collation_id: u16,
    pub flags: u32,
}

pub const OBJECT_TYPE_FLAG_ENTITY_OBJECT: u32 = 0x0000_0001;
pub const OBJECT_TYPE_FLAG_EVENT_OBJECT: u32 = 0x0000_0002;
pub const OBJECT_TYPE_FLAG_LINK_OBJECT: u32 = 0x0000_0004;
pub const OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT: u32 = 0x0000_0008;
pub const OBJECT_TYPE_FLAG_EVIDENCE_OBJECT: u32 = 0x0000_0010;
pub const OBJECT_TYPE_FLAG_PROJECTION_OBJECT: u32 = 0x0000_0020;

pub const PROPERTY_FLAG_ASSOCIATION_FROM_GOID: u32 = 0x0000_0001;
pub const PROPERTY_FLAG_ASSOCIATION_TO_GOID: u32 = 0x0000_0002;
pub const PROPERTY_FLAG_ASSOCIATION_TYPE: u32 = 0x0000_0004;
pub const PROPERTY_FLAG_ASSOCIATION_VALID_FROM: u32 = 0x0000_0008;
pub const PROPERTY_FLAG_ASSOCIATION_VALID_TO: u32 = 0x0000_0010;
pub const PROPERTY_FLAG_ASSOCIATION_OBSERVED_AT: u32 = 0x0000_0020;
pub const PROPERTY_FLAG_EVIDENCE_REF: u32 = 0x0000_0040;
pub const PROPERTY_FLAG_MAPPING_RULE_REF: u32 = 0x0000_0080;
pub const PROPERTY_FLAG_BOOL_DECLARED_NUMERIC: u32 = 0x0000_0100;

impl ObjectTypeCatalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 8 {
            return Err(CoveError::BufferTooShort);
        }
        let type_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let mut pos = 8usize;
        let mut types = Vec::with_capacity(type_count);
        for _ in 0..type_count {
            let (entry, used) = ObjectTypeEntryV1::parse(&bytes[pos..])?;
            pos = pos.checked_add(used).ok_or(CoveError::ArithOverflow)?;
            types.push(entry);
        }
        if pos != bytes.len() {
            return Err(CoveError::BadSchema(
                "object type catalog has trailing bytes".into(),
            ));
        }
        let catalog = Self { flags, types };
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let count = u32::try_from(self.types.len())
            .map_err(|_| CoveError::BadSchema("too many object types".into()))?;
        let mut out = Vec::new();
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for ty in &self.types {
            out.extend_from_slice(&ty.serialize()?);
        }
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        let mut seen_types = std::collections::HashSet::new();
        for ty in &self.types {
            if !seen_types.insert(ty.object_type_id) {
                return Err(CoveError::BadSchema(format!(
                    "duplicate object_type_id {} (Spec §56)",
                    ty.object_type_id
                )));
            }
            let mut seen_props = std::collections::HashSet::new();
            for prop in &ty.properties {
                if !seen_props.insert(prop.property_id) {
                    return Err(CoveError::BadSchema(format!(
                        "duplicate property_id {} in object_type_id {} (Spec §56)",
                        prop.property_id, ty.object_type_id
                    )));
                }
                if prop.logical_type == CoveLogicalType::Null {
                    return Err(CoveError::BadSchema(format!(
                        "property {} declares logical Null at top level (Spec §56)",
                        prop.property_id
                    )));
                }
                if validate_logical_physical_pair_with_options(
                    prop.logical_type,
                    prop.physical_kind,
                    LogicalPhysicalOptions {
                        bool_declared_numeric: prop.flags & PROPERTY_FLAG_BOOL_DECLARED_NUMERIC
                            != 0,
                    },
                )
                .is_err()
                {
                    return Err(CoveError::BadLogicalPhysicalPair);
                }
            }
        }
        Ok(())
    }
}

impl ObjectTypeEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        if bytes.len() < 4 + 2 {
            return Err(CoveError::BufferTooShort);
        }
        let object_type_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let type_name = read_str(bytes, &mut pos, "object type name")?;
        if bytes.len() < pos + 4 + 2 {
            return Err(CoveError::BufferTooShort);
        }
        let flags = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let property_count = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        let mut properties = Vec::with_capacity(property_count);
        for _ in 0..property_count {
            let (property, used) = PropertyEntryV1::parse(&bytes[pos..])?;
            pos = pos.checked_add(used).ok_or(CoveError::ArithOverflow)?;
            properties.push(property);
        }
        Ok((
            Self {
                object_type_id,
                type_name,
                flags,
                properties,
            },
            pos,
        ))
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let property_count = u16::try_from(self.properties.len())
            .map_err(|_| CoveError::BadSchema("too many properties".into()))?;
        let mut out = Vec::new();
        out.extend_from_slice(&self.object_type_id.to_le_bytes());
        write_str(&mut out, &self.type_name, "object type name")?;
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&property_count.to_le_bytes());
        for prop in &self.properties {
            out.extend_from_slice(&prop.serialize()?);
        }
        Ok(out)
    }
}

impl PropertyEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        if bytes.len() < 4 + 2 {
            return Err(CoveError::BufferTooShort);
        }
        let property_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let property_name = read_str(bytes, &mut pos, "property name")?;
        if bytes.len() < pos + 2 + 1 + 1 + 2 + 4 {
            return Err(CoveError::BufferTooShort);
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
        let logical_type = CoveLogicalType::from_u16(logical_raw).ok_or_else(|| {
            CoveError::BadSchema(format!(
                "unknown object property logical type {logical_raw}"
            ))
        })?;
        let physical_kind = CovePhysicalKind::from_u8(physical_raw).ok_or_else(|| {
            CoveError::BadSchema(format!(
                "unknown object property physical kind {physical_raw}"
            ))
        })?;
        let nullable = match nullable_raw {
            0 => false,
            1 => true,
            other => {
                return Err(CoveError::BadSchema(format!(
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

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
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
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 8 {
            return Err(CoveError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let needed = 8usize
            .checked_add(
                entry_count
                    .checked_mul(TEMPORAL_SEGMENT_INDEX_ENTRY_LEN)
                    .ok_or(CoveError::ArithOverflow)?,
            )
            .ok_or(CoveError::ArithOverflow)?;
        if needed > bytes.len() {
            return Err(CoveError::BufferTooShort);
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

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let count = u32::try_from(self.entries.len())
            .map_err(|_| CoveError::BadSchema("too many temporal segments".into()))?;
        let mut out = Vec::with_capacity(8 + self.entries.len() * TEMPORAL_SEGMENT_INDEX_ENTRY_LEN);
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize());
        }
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        let mut seen = std::collections::HashSet::new();
        for entry in &self.entries {
            if !seen.insert((entry.object_type_id, entry.segment_id)) {
                return Err(CoveError::RefInvalid);
            }
            entry.validate()?;
        }
        Ok(())
    }
}

impl TemporalSegmentIndexEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < TEMPORAL_SEGMENT_INDEX_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
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
            return Err(CoveError::ChecksumMismatch);
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

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.time_range_start_us > self.time_range_end_us || self.csn_min > self.csn_max {
            return Err(CoveError::BadSchema(
                "temporal segment range is inverted (Spec §57)".into(),
            ));
        }
        if self.min_goid > self.max_goid {
            return Err(CoveError::BadSchema(
                "temporal segment GOID range is inverted (Spec §57)".into(),
            ));
        }
        let counted = self
            .delta_count
            .checked_add(self.snapshot_count)
            .and_then(|v| v.checked_add(self.baseline_count))
            .and_then(|v| v.checked_add(self.tombstone_count))
            .ok_or(CoveError::ArithOverflow)?;
        if counted != self.row_count {
            return Err(CoveError::BadSchema(
                "temporal segment record-kind counts do not sum to row_count (Spec §57)".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalSegmentHeaderV1 {
    pub segment_id: u32,
    pub object_type_id: u32,
    pub time_range_start_us: i64,
    pub time_range_end_us: i64,
    pub csn_min: u64,
    pub csn_max: u64,
    pub row_count: u32,
    pub morsel_count: u32,
    pub morsel_row_count: u32,
    pub column_count: u32,
    pub row_directory_offset: u64,
    pub column_directory_offset: u64,
    pub page_index_offset: u64,
    pub data_offset: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl TemporalSegmentHeaderV1 {
    pub fn serialize(&self) -> [u8; TEMPORAL_SEGMENT_HEADER_LEN] {
        let mut out = [0u8; TEMPORAL_SEGMENT_HEADER_LEN];
        out[0..4].copy_from_slice(&self.segment_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.object_type_id.to_le_bytes());
        out[8..16].copy_from_slice(&self.time_range_start_us.to_le_bytes());
        out[16..24].copy_from_slice(&self.time_range_end_us.to_le_bytes());
        out[24..32].copy_from_slice(&self.csn_min.to_le_bytes());
        out[32..40].copy_from_slice(&self.csn_max.to_le_bytes());
        out[40..44].copy_from_slice(&self.row_count.to_le_bytes());
        out[44..48].copy_from_slice(&self.morsel_count.to_le_bytes());
        out[48..52].copy_from_slice(&self.morsel_row_count.to_le_bytes());
        out[52..56].copy_from_slice(&self.column_count.to_le_bytes());
        out[56..64].copy_from_slice(&self.row_directory_offset.to_le_bytes());
        out[64..72].copy_from_slice(&self.column_directory_offset.to_le_bytes());
        out[72..80].copy_from_slice(&self.page_index_offset.to_le_bytes());
        out[80..88].copy_from_slice(&self.data_offset.to_le_bytes());
        out[88..92].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[92..96].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < TEMPORAL_SEGMENT_HEADER_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..TEMPORAL_SEGMENT_HEADER_LEN];
        let checksum_field = u32::from_le_bytes(bytes[92..96].try_into().unwrap());
        let mut for_crc = [0u8; TEMPORAL_SEGMENT_HEADER_LEN];
        for_crc.copy_from_slice(bytes);
        for_crc[92..96].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(CoveError::ChecksumMismatch);
        }

        Ok(Self {
            segment_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            object_type_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            time_range_start_us: i64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            time_range_end_us: i64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            csn_min: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            csn_max: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
            row_count: u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            morsel_count: u32::from_le_bytes(bytes[44..48].try_into().unwrap()),
            morsel_row_count: u32::from_le_bytes(bytes[48..52].try_into().unwrap()),
            column_count: u32::from_le_bytes(bytes[52..56].try_into().unwrap()),
            row_directory_offset: u64::from_le_bytes(bytes[56..64].try_into().unwrap()),
            column_directory_offset: u64::from_le_bytes(bytes[64..72].try_into().unwrap()),
            page_index_offset: u64::from_le_bytes(bytes[72..80].try_into().unwrap()),
            data_offset: u64::from_le_bytes(bytes[80..88].try_into().unwrap()),
            flags: u32::from_le_bytes(bytes[88..92].try_into().unwrap()),
            checksum: checksum_field,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoveRecordRefV1 {
    pub segment_id: u32,
    pub row_index: u32,
    pub target_kind: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalRowEntryV1 {
    pub timestamp_us: i64,
    pub csn: u64,
    pub branch_key: u64,
    pub goid: [u8; 16],
    pub record_id: [u8; 16],
    pub record_kind: RecordKind,
    pub prev_ref: Option<CoveRecordRefV1>,
}

impl TemporalRowEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < TEMPORAL_ROW_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..TEMPORAL_ROW_ENTRY_LEN];
        let timestamp_us = i64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let csn = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let branch_key = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let mut goid = [0u8; 16];
        goid.copy_from_slice(&bytes[24..40]);
        let mut record_id = [0u8; 16];
        record_id.copy_from_slice(&bytes[40..56]);
        let record_kind = RecordKind::from_u8(bytes[56]).ok_or_else(|| {
            CoveError::BadSchema(format!("unknown temporal record kind {}", bytes[56]))
        })?;
        let prev_present = bytes[57];
        let target_kind = bytes[58];
        if bytes[59] != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        if target_kind > 1 {
            return Err(CoveError::RefInvalid);
        }
        let prev_segment_id = u32::from_le_bytes(bytes[60..64].try_into().unwrap());
        let prev_row_index = u32::from_le_bytes(bytes[64..68].try_into().unwrap());
        let prev_ref = match prev_present {
            0 => {
                if target_kind != 0 || prev_segment_id != 0 || prev_row_index != 0 {
                    return Err(CoveError::RefInvalid);
                }
                None
            }
            1 => Some(CoveRecordRefV1 {
                segment_id: prev_segment_id,
                row_index: prev_row_index,
                target_kind,
            }),
            _ => return Err(CoveError::RefInvalid),
        };
        record_kind.validate_published()?;
        Ok(Self {
            timestamp_us,
            csn,
            branch_key,
            goid,
            record_id,
            record_kind,
            prev_ref,
        })
    }

    pub fn serialize(&self) -> [u8; TEMPORAL_ROW_ENTRY_LEN] {
        let mut out = [0u8; TEMPORAL_ROW_ENTRY_LEN];
        out[0..8].copy_from_slice(&self.timestamp_us.to_le_bytes());
        out[8..16].copy_from_slice(&self.csn.to_le_bytes());
        out[16..24].copy_from_slice(&self.branch_key.to_le_bytes());
        out[24..40].copy_from_slice(&self.goid);
        out[40..56].copy_from_slice(&self.record_id);
        out[56] = match self.record_kind {
            RecordKind::Delta => 0,
            RecordKind::Snapshot => 1,
            RecordKind::ReservedLegacyMaterializedDelta => 2,
            RecordKind::Baseline => 3,
            RecordKind::Tombstone => 4,
        };
        if let Some(prev_ref) = self.prev_ref {
            out[57] = 1;
            out[58] = prev_ref.target_kind;
            out[60..64].copy_from_slice(&prev_ref.segment_id.to_le_bytes());
            out[64..68].copy_from_slice(&prev_ref.row_index.to_le_bytes());
        }
        out
    }

    pub fn row_key(&self) -> TemporalRowKey {
        TemporalRowKey {
            timestamp_us: self.timestamp_us,
            csn: self.csn,
            branch_key: self.branch_key,
            goid: self.goid,
            record_id: self.record_id,
        }
    }

    pub fn trust_payload(&self) -> Vec<u8> {
        self.serialize().to_vec()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalSegmentData {
    pub header: TemporalSegmentHeaderV1,
    pub rows: Vec<TemporalRowEntryV1>,
    pub property_columns: Vec<TemporalPropertyColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalPropertyColumn {
    pub directory: TableColumnDirectoryEntryV1,
    pub page_index: ColumnPageIndex,
    pub pages: Vec<TemporalPropertyPage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalPropertyPage {
    pub index_entry: ColumnPageIndexEntryV1,
    pub payload: Option<ColumnPagePayloadV1>,
}

impl TemporalSegmentData {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        Self::parse_inner(bytes, None)
    }

    pub fn parse_with_required_features(
        bytes: &[u8],
        required_features: u64,
    ) -> Result<Self, CoveError> {
        Self::parse_inner(bytes, Some(required_features))
    }

    fn parse_inner(bytes: &[u8], required_features: Option<u64>) -> Result<Self, CoveError> {
        let header = TemporalSegmentHeaderV1::parse(bytes)?;
        if header.row_count == 0 && header.morsel_count != 0 {
            return Err(CoveError::BadSchema(
                "temporal segment with zero rows cannot have morsels".into(),
            ));
        }
        if header.row_count != 0 && header.morsel_row_count == 0 {
            return Err(CoveError::BadSchema(
                "temporal segment with rows must declare morsel_row_count".into(),
            ));
        }
        let row_directory_offset =
            usize::try_from(header.row_directory_offset).map_err(|_| CoveError::OffsetRange)?;
        let column_directory_offset =
            usize::try_from(header.column_directory_offset).map_err(|_| CoveError::OffsetRange)?;
        let page_index_offset =
            usize::try_from(header.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
        let data_offset =
            usize::try_from(header.data_offset).map_err(|_| CoveError::OffsetRange)?;
        if row_directory_offset < TEMPORAL_SEGMENT_HEADER_LEN
            || column_directory_offset < row_directory_offset
            || page_index_offset < column_directory_offset
            || data_offset < page_index_offset
            || data_offset > bytes.len()
        {
            return Err(CoveError::BadSchema(
                "temporal segment offsets are invalid".into(),
            ));
        }
        let row_bytes_len = (header.row_count as usize)
            .checked_mul(TEMPORAL_ROW_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let row_end = row_directory_offset
            .checked_add(row_bytes_len)
            .ok_or(CoveError::ArithOverflow)?;
        if row_end > column_directory_offset {
            return Err(CoveError::BadSchema(
                "temporal row directory exceeds declared boundary".into(),
            ));
        }
        let column_dir_len = (header.column_count as usize)
            .checked_mul(TABLE_COLUMN_DIRECTORY_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let column_dir_end = column_directory_offset
            .checked_add(column_dir_len)
            .ok_or(CoveError::ArithOverflow)?;
        if column_dir_end > page_index_offset {
            return Err(CoveError::BadSchema(
                "temporal property column directory exceeds declared boundary".into(),
            ));
        }
        let mut rows = Vec::with_capacity(header.row_count as usize);
        let mut pos = row_directory_offset;
        for _ in 0..header.row_count {
            rows.push(TemporalRowEntryV1::parse(
                &bytes[pos..pos + TEMPORAL_ROW_ENTRY_LEN],
            )?);
            pos += TEMPORAL_ROW_ENTRY_LEN;
        }
        let property_columns = parse_temporal_property_columns(
            bytes,
            &header,
            column_directory_offset,
            required_features,
        )?;
        let segment = Self {
            header,
            rows,
            property_columns,
        };
        segment.validate()?;
        Ok(segment)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        let row_keys = self
            .rows
            .iter()
            .map(TemporalRowEntryV1::row_key)
            .collect::<Vec<_>>();
        validate_temporal_order(&row_keys)?;
        for pair in self.rows.windows(2) {
            if pair[1].csn < pair[0].csn {
                return Err(CoveError::BadSchema(
                    "temporal segment csn decreases in row order".into(),
                ));
            }
        }

        for (row_index, row) in self.rows.iter().enumerate() {
            if let Some(prev_ref) = row.prev_ref {
                if prev_ref.segment_id == self.header.segment_id
                    && prev_ref.row_index >= row_index as u32
                {
                    return Err(CoveError::RefInvalid);
                }
                if prev_ref.segment_id > self.header.segment_id {
                    return Err(CoveError::RefInvalid);
                }
            }
        }

        if let Some(first) = self.rows.first() {
            if first.timestamp_us < self.header.time_range_start_us
                || first.csn < self.header.csn_min
            {
                return Err(CoveError::BadSchema(
                    "temporal segment row falls before declared min range".into(),
                ));
            }
        }
        if let Some(last) = self.rows.last() {
            if last.timestamp_us > self.header.time_range_end_us || last.csn > self.header.csn_max {
                return Err(CoveError::BadSchema(
                    "temporal segment row falls after declared max range".into(),
                ));
            }
        }

        Ok(())
    }
}

fn parse_temporal_property_columns(
    bytes: &[u8],
    header: &TemporalSegmentHeaderV1,
    column_directory_offset: usize,
    required_features: Option<u64>,
) -> Result<Vec<TemporalPropertyColumn>, CoveError> {
    let page_index_offset =
        usize::try_from(header.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
    let data_offset = usize::try_from(header.data_offset).map_err(|_| CoveError::OffsetRange)?;
    let mut out = Vec::with_capacity(header.column_count as usize);
    let mut pos = column_directory_offset;
    for _ in 0..header.column_count {
        let directory = TableColumnDirectoryEntryV1::parse(
            &bytes[pos..pos + TABLE_COLUMN_DIRECTORY_ENTRY_LEN],
        )?;
        pos += TABLE_COLUMN_DIRECTORY_ENTRY_LEN;

        let page_index_start =
            usize::try_from(directory.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
        let page_index_end = usize::try_from(
            directory
                .page_index_offset
                .checked_add(directory.page_index_length)
                .ok_or(CoveError::ArithOverflow)?,
        )
        .map_err(|_| CoveError::OffsetRange)?;
        if page_index_start < page_index_offset || page_index_end > data_offset {
            return Err(CoveError::SegmentCorrupt);
        }
        let page_index = ColumnPageIndex::parse(&bytes[page_index_start..page_index_end])?;

        let data_start =
            usize::try_from(directory.data_offset).map_err(|_| CoveError::OffsetRange)?;
        let data_end = usize::try_from(
            directory
                .data_offset
                .checked_add(directory.data_length)
                .ok_or(CoveError::ArithOverflow)?,
        )
        .map_err(|_| CoveError::OffsetRange)?;
        if data_start < data_offset || data_end > bytes.len() {
            return Err(CoveError::SegmentCorrupt);
        }

        let mut pages = Vec::with_capacity(page_index.entries.len());
        for page in &page_index.entries {
            if page.column_id != directory.column_id {
                return Err(CoveError::PageCorrupt);
            }
            validate_temporal_property_page_elision_features(page, required_features)?;
            let context = PageValidationContext {
                table_id: None,
                segment_id: Some(header.segment_id),
                column_id: directory.column_id,
                logical_type: directory.logical_type,
                physical_kind: directory.physical_kind,
                dictionary: None,
                zone_stats: None,
            };
            if page.page_length == 0 {
                validate_temporal_property_stats_only_page(&context, page)?;
                pages.push(TemporalPropertyPage {
                    index_entry: page.clone(),
                    payload: None,
                });
                continue;
            }
            let page_start =
                usize::try_from(page.page_offset).map_err(|_| CoveError::OffsetRange)?;
            let page_end = usize::try_from(
                page.page_offset
                    .checked_add(page.page_length)
                    .ok_or(CoveError::ArithOverflow)?,
            )
            .map_err(|_| CoveError::OffsetRange)?;
            if page_start < data_start || page_end > data_end {
                return Err(CoveError::PageCorrupt);
            }
            let decoded = compression::column_page_payload(&bytes[page_start..page_end], page)?;
            let payload = ColumnPagePayloadV1::parse(decoded.as_ref())?;
            validate_column_page_payload(&context, page, &payload)?;
            pages.push(TemporalPropertyPage {
                index_entry: page.clone(),
                payload: Some(payload),
            });
        }
        out.push(TemporalPropertyColumn {
            directory,
            page_index,
            pages,
        });
    }
    Ok(out)
}

pub(crate) fn validate_temporal_property_page_elision_features(
    page: &ColumnPageIndexEntryV1,
    required_features: Option<u64>,
) -> Result<(), CoveError> {
    if page_uses_payload_elision(page.flags)
        && required_features.is_some_and(|bits| bits & FEATURE_PAGE_PAYLOAD_ELISION == 0)
    {
        return Err(CoveError::BadSection(
            "COVE-O property page payload-elision flags require FEATURE_PAGE_PAYLOAD_ELISION in required_features"
                .into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_temporal_property_stats_only_page(
    context: &PageValidationContext<'_>,
    page: &ColumnPageIndexEntryV1,
) -> Result<(), CoveError> {
    validate_stats_only_constant_page(context, page)?;
    if page.flags & PAGE_FLAG_ALL_NON_NULL != 0 {
        return Err(CoveError::PageCorrupt);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalBloomEntryV1 {
    pub segment_id: u32,
    pub time_bucket_start_us: i64,
    pub time_bucket_end_us: i64,
    pub filter_offset: u64,
    pub filter_length: u64,
    pub checksum: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemporalBloomIndex {
    pub flags: u32,
    pub entries: Vec<TemporalBloomEntryV1>,
}

impl TemporalBloomIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 8 {
            return Err(CoveError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let entries_len = entry_count
            .checked_mul(TEMPORAL_BLOOM_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let entries_end = 8usize
            .checked_add(entries_len)
            .ok_or(CoveError::ArithOverflow)?;
        if entries_end > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let mut entries = Vec::with_capacity(entry_count);
        let mut pos = 8usize;
        for _ in 0..entry_count {
            let segment_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            let time_bucket_start_us =
                i64::from_le_bytes(bytes[pos + 4..pos + 12].try_into().unwrap());
            let time_bucket_end_us =
                i64::from_le_bytes(bytes[pos + 12..pos + 20].try_into().unwrap());
            if time_bucket_start_us > time_bucket_end_us {
                return Err(CoveError::BadIndex);
            }
            let checksum_field = u32::from_le_bytes(bytes[pos + 36..pos + 40].try_into().unwrap());
            let mut for_crc = [0u8; TEMPORAL_BLOOM_ENTRY_LEN];
            for_crc.copy_from_slice(&bytes[pos..pos + TEMPORAL_BLOOM_ENTRY_LEN]);
            for_crc[36..40].fill(0);
            if checksum::crc32c(&for_crc) != checksum_field {
                return Err(CoveError::ChecksumMismatch);
            }
            let filter_offset = u64::from_le_bytes(bytes[pos + 20..pos + 28].try_into().unwrap());
            let filter_length = u64::from_le_bytes(bytes[pos + 28..pos + 36].try_into().unwrap());
            let filter_end = filter_offset
                .checked_add(filter_length)
                .ok_or(CoveError::ArithOverflow)?;
            if filter_end > bytes.len() as u64 {
                return Err(CoveError::OffsetRange);
            }
            entries.push(TemporalBloomEntryV1 {
                segment_id,
                time_bucket_start_us,
                time_bucket_end_us,
                filter_offset,
                filter_length,
                checksum: checksum_field,
            });
            pos += TEMPORAL_BLOOM_ENTRY_LEN;
        }
        Ok(Self { flags, entries })
    }

    /// Inverse of [`Self::parse`]; computes filter offsets, lengths, and entry
    /// checksums canonically from the provided filter payloads.
    pub fn serialize(&self, filters: &[Vec<u8>]) -> Result<Vec<u8>, CoveError> {
        if self.entries.len() != filters.len() {
            return Err(CoveError::BadIndex);
        }
        let entry_count = u32::try_from(self.entries.len())
            .map_err(|_| CoveError::BadSection("too many temporal bloom entries".into()))?;
        let entries_len = self
            .entries
            .len()
            .checked_mul(TEMPORAL_BLOOM_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let payload_start = 8usize
            .checked_add(entries_len)
            .ok_or(CoveError::ArithOverflow)?;
        let mut out = Vec::with_capacity(
            payload_start
                .checked_add(filters.iter().map(Vec::len).sum())
                .ok_or(CoveError::ArithOverflow)?,
        );
        out.extend_from_slice(&entry_count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        let mut next_filter_offset = payload_start as u64;
        for (entry, filter) in self.entries.iter().zip(filters) {
            if entry.time_bucket_start_us > entry.time_bucket_end_us {
                return Err(CoveError::BadIndex);
            }
            let filter_length = u64::try_from(filter.len()).map_err(|_| CoveError::OffsetRange)?;
            let mut entry_bytes = [0u8; TEMPORAL_BLOOM_ENTRY_LEN];
            entry_bytes[0..4].copy_from_slice(&entry.segment_id.to_le_bytes());
            entry_bytes[4..12].copy_from_slice(&entry.time_bucket_start_us.to_le_bytes());
            entry_bytes[12..20].copy_from_slice(&entry.time_bucket_end_us.to_le_bytes());
            entry_bytes[20..28].copy_from_slice(&next_filter_offset.to_le_bytes());
            entry_bytes[28..36].copy_from_slice(&filter_length.to_le_bytes());
            let crc = checksum::crc32c(&entry_bytes);
            entry_bytes[36..40].copy_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&entry_bytes);
            next_filter_offset = next_filter_offset
                .checked_add(filter_length)
                .ok_or(CoveError::ArithOverflow)?;
        }
        for filter in filters {
            out.extend_from_slice(filter);
        }
        Ok(out)
    }
}

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

fn read_str(bytes: &[u8], pos: &mut usize, what: &str) -> Result<String, CoveError> {
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
        .map_err(|_| CoveError::BadSchema(format!("{what} is not valid UTF-8")))?
        .to_string();
    *pos = end;
    Ok(s)
}

fn write_str(out: &mut Vec<u8>, s: &str, what: &str) -> Result<(), CoveError> {
    let len = u16::try_from(s.len())
        .map_err(|_| CoveError::BadSchema(format!("{what} exceeds u16::MAX")))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::{FEATURE_OBJECT_PROFILE, FEATURE_PAGE_PAYLOAD_ELISION},
        page::{PAGE_FLAG_ALL_NON_NULL, PAGE_FLAG_ALL_NULL, PAGE_FLAG_STATS_ONLY_CONSTANT},
    };

    fn k(t: i64, csn: u64) -> TemporalRowKey {
        TemporalRowKey {
            timestamp_us: t,
            csn,
            branch_key: 0,
            goid: [0; 16],
            record_id: [0; 16],
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
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn spec_59_1_reserved_legacy_record_kind_rejected() {
        assert!(RecordKind::ReservedLegacyMaterializedDelta
            .validate_published()
            .is_err());
        assert!(RecordKind::Delta.validate_published().is_ok());
    }

    #[test]
    fn spec_60_dangling_prev_ref_rejected() {
        let prev = vec![Some(7), Some(99)];
        let local = vec![7];
        assert_eq!(
            validate_self_contained(&prev, &local),
            Err(CoveError::NotSelfContained)
        );
    }

    fn property(property_id: u32) -> PropertyEntryV1 {
        PropertyEntryV1 {
            property_id,
            property_name: "name".into(),
            logical_type: CoveLogicalType::Bool,
            physical_kind: CovePhysicalKind::Boolean,
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
                flags: OBJECT_TYPE_FLAG_ENTITY_OBJECT,
                properties: vec![property(1)],
            }],
        };
        let parsed = ObjectTypeCatalog::parse(&catalog.serialize().unwrap()).unwrap();
        assert_eq!(parsed.types[0].type_name, "Customer");
        assert_eq!(parsed.types[0].flags, OBJECT_TYPE_FLAG_ENTITY_OBJECT);
    }

    #[test]
    fn object_type_catalog_rejects_duplicate_property() {
        let catalog = ObjectTypeCatalog {
            flags: 0,
            types: vec![ObjectTypeEntryV1 {
                object_type_id: 10,
                type_name: "Customer".into(),
                flags: OBJECT_TYPE_FLAG_ENTITY_OBJECT,
                properties: vec![property(1), property(1)],
            }],
        };
        assert!(matches!(
            ObjectTypeCatalog::parse(&catalog.serialize().unwrap()),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn object_type_catalog_rejects_logical_null_property() {
        let mut p = property(1);
        p.logical_type = CoveLogicalType::Null;
        p.physical_kind = CovePhysicalKind::FileCode;
        let catalog = ObjectTypeCatalog {
            flags: 0,
            types: vec![ObjectTypeEntryV1 {
                object_type_id: 10,
                type_name: "Customer".into(),
                flags: OBJECT_TYPE_FLAG_ENTITY_OBJECT,
                properties: vec![p],
            }],
        };
        assert!(matches!(
            ObjectTypeCatalog::parse(&catalog.serialize().unwrap()),
            Err(CoveError::BadSchema(_))
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
            Err(CoveError::RefInvalid)
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
            Err(CoveError::BadSchema(_))
        ));
    }

    fn temporal_row(timestamp_us: i64, csn: u64) -> TemporalRowEntryV1 {
        TemporalRowEntryV1 {
            timestamp_us,
            csn,
            branch_key: 0,
            goid: [0; 16],
            record_id: [0; 16],
            record_kind: RecordKind::Delta,
            prev_ref: None,
        }
    }

    fn temporal_segment_bytes(rows: &[TemporalRowEntryV1]) -> Vec<u8> {
        let row_directory_offset = TEMPORAL_SEGMENT_HEADER_LEN as u64;
        let row_bytes = (rows.len() * TEMPORAL_ROW_ENTRY_LEN) as u64;
        let row_end = row_directory_offset + row_bytes;
        let header = TemporalSegmentHeaderV1 {
            segment_id: 7,
            object_type_id: 1,
            time_range_start_us: rows.first().map(|row| row.timestamp_us).unwrap_or(0),
            time_range_end_us: rows.last().map(|row| row.timestamp_us).unwrap_or(0),
            csn_min: rows.first().map(|row| row.csn).unwrap_or(0),
            csn_max: rows.last().map(|row| row.csn).unwrap_or(0),
            row_count: rows.len() as u32,
            morsel_count: u32::from(!rows.is_empty()),
            morsel_row_count: if rows.is_empty() {
                0
            } else {
                rows.len() as u32
            },
            column_count: 0,
            row_directory_offset,
            column_directory_offset: row_end,
            page_index_offset: row_end,
            data_offset: row_end,
            flags: 0,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        for row in rows {
            bytes.extend_from_slice(&row.serialize());
        }
        bytes
    }

    fn temporal_segment_with_stats_only_property_page(
        rows: &[TemporalRowEntryV1],
        non_null_count: u32,
        null_count: u32,
        flags: u32,
    ) -> Vec<u8> {
        let row_directory_offset = TEMPORAL_SEGMENT_HEADER_LEN as u64;
        let row_bytes = (rows.len() * TEMPORAL_ROW_ENTRY_LEN) as u64;
        let row_end = row_directory_offset + row_bytes;
        let column_directory_offset = row_end;
        let page_index_offset = column_directory_offset + TABLE_COLUMN_DIRECTORY_ENTRY_LEN as u64;
        let page_index_length = crate::page::COLUMN_PAGE_INDEX_ENTRY_LEN as u64;
        let data_offset = page_index_offset + page_index_length;
        let header = TemporalSegmentHeaderV1 {
            segment_id: 7,
            object_type_id: 1,
            time_range_start_us: rows.first().map(|row| row.timestamp_us).unwrap_or(0),
            time_range_end_us: rows.last().map(|row| row.timestamp_us).unwrap_or(0),
            csn_min: rows.first().map(|row| row.csn).unwrap_or(0),
            csn_max: rows.last().map(|row| row.csn).unwrap_or(0),
            row_count: rows.len() as u32,
            morsel_count: u32::from(!rows.is_empty()),
            morsel_row_count: if rows.is_empty() {
                0
            } else {
                rows.len() as u32
            },
            column_count: 1,
            row_directory_offset,
            column_directory_offset,
            page_index_offset,
            data_offset,
            flags: 0,
            checksum: 0,
        };
        let directory = TableColumnDirectoryEntryV1 {
            column_id: 1,
            logical_type: CoveLogicalType::Bool,
            physical_kind: CovePhysicalKind::Boolean,
            flags: 0,
            page_index_offset,
            page_index_length,
            data_offset,
            data_length: 0,
            stats_ref: u32::MAX,
            domain_ref: u32::MAX,
            checksum: 0,
        };
        let page = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 0,
            row_count: rows.len() as u32,
            non_null_count,
            null_count,
            encoding_root: u32::MAX,
            page_offset: 0,
            page_length: 0,
            uncompressed_length: 0,
            stats_ref: 0,
            flags,
            checksum: checksum::crc32c(&[]),
        };

        let mut bytes = header.serialize().to_vec();
        for row in rows {
            bytes.extend_from_slice(&row.serialize());
        }
        bytes.extend_from_slice(&directory.serialize());
        bytes.extend_from_slice(&page.serialize());
        bytes
    }

    #[test]
    fn temporal_segment_data_roundtrip_validates() {
        let bytes = temporal_segment_bytes(&[temporal_row(10, 1), temporal_row(20, 2)]);
        let parsed = TemporalSegmentData::parse(&bytes).unwrap();
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.header.segment_id, 7);
    }

    #[test]
    fn temporal_property_all_null_stats_only_requires_elision_feature() {
        let bytes = temporal_segment_with_stats_only_property_page(
            &[temporal_row(10, 1)],
            0,
            1,
            PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NULL,
        );
        assert!(matches!(
            TemporalSegmentData::parse_with_required_features(&bytes, FEATURE_OBJECT_PROFILE),
            Err(CoveError::BadSection(_))
        ));
        assert!(TemporalSegmentData::parse_with_required_features(
            &bytes,
            FEATURE_OBJECT_PROFILE | FEATURE_PAGE_PAYLOAD_ELISION
        )
        .is_ok());
    }

    #[test]
    fn temporal_property_all_non_null_stats_only_requires_validated_stats() {
        let bytes = temporal_segment_with_stats_only_property_page(
            &[temporal_row(10, 1)],
            1,
            0,
            PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NON_NULL,
        );
        assert_eq!(
            TemporalSegmentData::parse_with_required_features(
                &bytes,
                FEATURE_OBJECT_PROFILE | FEATURE_PAGE_PAYLOAD_ELISION
            ),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn temporal_segment_data_rejects_out_of_order_rows() {
        let bytes = temporal_segment_bytes(&[temporal_row(20, 2), temporal_row(10, 1)]);
        assert!(matches!(
            TemporalSegmentData::parse(&bytes),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn temporal_segment_data_rejects_csn_decrease_in_row_order() {
        let bytes = temporal_segment_bytes(&[temporal_row(10, 100), temporal_row(20, 50)]);
        assert_eq!(
            TemporalSegmentData::parse(&bytes),
            Err(CoveError::BadSchema(
                "temporal segment csn decreases in row order".into()
            ))
        );
    }

    #[test]
    fn temporal_segment_data_rejects_forward_prev_ref() {
        let mut first = temporal_row(10, 1);
        first.prev_ref = Some(CoveRecordRefV1 {
            segment_id: 7,
            row_index: 1,
            target_kind: 0,
        });
        let bytes = temporal_segment_bytes(&[first, temporal_row(20, 2)]);
        assert_eq!(
            TemporalSegmentData::parse(&bytes),
            Err(CoveError::RefInvalid)
        );
    }

    #[test]
    fn temporal_segment_data_allows_backward_cross_segment_prev_ref() {
        let mut row = temporal_row(20, 2);
        row.prev_ref = Some(CoveRecordRefV1 {
            segment_id: 6,
            row_index: 0,
            target_kind: 0,
        });
        let bytes = temporal_segment_bytes(&[row]);
        let parsed = TemporalSegmentData::parse(&bytes).unwrap();
        assert_eq!(parsed.rows[0].prev_ref.unwrap().segment_id, 6);
    }

    fn temporal_bloom_bytes() -> Vec<u8> {
        let filter_offset = 8 + TEMPORAL_BLOOM_ENTRY_LEN as u64;
        let filter = [1u8, 2, 3, 4];
        let mut entry = [0u8; TEMPORAL_BLOOM_ENTRY_LEN];
        entry[0..4].copy_from_slice(&7u32.to_le_bytes());
        entry[4..12].copy_from_slice(&10i64.to_le_bytes());
        entry[12..20].copy_from_slice(&20i64.to_le_bytes());
        entry[20..28].copy_from_slice(&filter_offset.to_le_bytes());
        entry[28..36].copy_from_slice(&(filter.len() as u64).to_le_bytes());
        let crc = checksum::crc32c(&entry);
        entry[36..40].copy_from_slice(&crc.to_le_bytes());

        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&entry);
        bytes.extend_from_slice(&filter);
        bytes
    }

    #[test]
    fn temporal_bloom_index_roundtrip_validates() {
        let parsed = TemporalBloomIndex::parse(&temporal_bloom_bytes()).unwrap();
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].segment_id, 7);
    }

    #[cfg(feature = "digest-sha2")]
    fn trust_manifest_bytes(segment: &TemporalSegmentData) -> Vec<u8> {
        let mut bytes = 2u32.to_le_bytes().to_vec();
        let mut prev = [0u8; 32];
        for (row_index, row) in segment.rows.iter().enumerate() {
            bytes.extend_from_slice(&segment.header.segment_id.to_le_bytes());
            bytes.extend_from_slice(&(row_index as u32).to_le_bytes());
            prev = trust_chain::chain(&prev, &row.trust_payload()).unwrap();
            bytes.extend_from_slice(&prev);
        }
        bytes
    }

    #[cfg(feature = "digest-sha2")]
    #[test]
    fn trust_manifest_verifies_temporal_rows() {
        let segment = TemporalSegmentData::parse(&temporal_segment_bytes(&[
            temporal_row(10, 1),
            temporal_row(20, 2),
        ]))
        .unwrap();
        let manifest = TrustManifest::parse(&trust_manifest_bytes(&segment)).unwrap();
        assert!(manifest.verify_against(&[segment]).is_ok());
    }

    #[cfg(feature = "digest-sha2")]
    #[test]
    fn trust_manifest_rejects_bad_digest() {
        let segment = TemporalSegmentData::parse(&temporal_segment_bytes(&[
            temporal_row(10, 1),
            temporal_row(20, 2),
        ]))
        .unwrap();
        let mut bytes = trust_manifest_bytes(&segment);
        *bytes.last_mut().unwrap() ^= 0xFF;
        let manifest = TrustManifest::parse(&bytes).unwrap();
        assert_eq!(
            manifest.verify_against(&[segment]),
            Err(CoveError::DigestMismatch)
        );
    }

    #[test]
    fn trust_payload_matches_temporal_row_wire_encoding() {
        let mut row = temporal_row(20, 2);
        row.prev_ref = Some(CoveRecordRefV1 {
            segment_id: 7,
            row_index: 1,
            target_kind: 1,
        });
        assert_eq!(row.trust_payload(), row.serialize());
    }

    #[test]
    fn trust_manifest_serialize_round_trip() {
        let m = TrustManifest {
            entries: vec![
                TrustManifestEntryV1 {
                    segment_id: 1,
                    row_index: 0,
                    expected_hash: [0xAA; 32],
                },
                TrustManifestEntryV1 {
                    segment_id: 2,
                    row_index: 5,
                    expected_hash: [0xBB; 32],
                },
            ],
        };
        let bytes = m.serialize();
        assert_eq!(TrustManifest::parse(&bytes).unwrap(), m);
    }
}
