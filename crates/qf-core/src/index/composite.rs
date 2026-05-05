//! Spec §35 — Composite zone index.
//!
//! Maps a tuple of (column_id, FileCode) bindings to the morsels that
//! contain at least one matching row. Lets the planner prune multi-column
//! equality predicates without scanning each column's inverted index.

use crate::QfError;

use super::{checked_region, verify_checksum_field};

pub const COMPOSITE_ZONE_INDEX_HEADER_LEN: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompositeTransformKind {
    Tuple = 0,
    ZOrder = 1,
    Hilbert = 2,
    WriterDefined = 3,
}

impl CompositeTransformKind {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Tuple),
            1 => Some(Self::ZOrder),
            2 => Some(Self::Hilbert),
            3 => Some(Self::WriterDefined),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeZoneIndexHeaderV1 {
    pub table_id: u32,
    pub key_column_count: u16,
    pub transform_kind: CompositeTransformKind,
    pub flags: u8,
    pub zone_count: u32,
    pub key_columns_offset: u64,
    pub entries_offset: u64,
    pub entries_length: u64,
    pub checksum: u32,
}

impl CompositeZoneIndexHeaderV1 {
    pub fn serialize(&self) -> [u8; COMPOSITE_ZONE_INDEX_HEADER_LEN] {
        let mut out = [0u8; COMPOSITE_ZONE_INDEX_HEADER_LEN];
        out[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        out[4..6].copy_from_slice(&self.key_column_count.to_le_bytes());
        out[6] = self.transform_kind as u8;
        out[7] = self.flags;
        out[8..12].copy_from_slice(&self.zone_count.to_le_bytes());
        out[12..20].copy_from_slice(&self.key_columns_offset.to_le_bytes());
        out[20..28].copy_from_slice(&self.entries_offset.to_le_bytes());
        out[28..36].copy_from_slice(&self.entries_length.to_le_bytes());
        let crc = crate::checksum::crc32c(&out);
        out[36..40].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < COMPOSITE_ZONE_INDEX_HEADER_LEN {
            return Err(QfError::BufferTooShort);
        }
        let bytes = &bytes[..COMPOSITE_ZONE_INDEX_HEADER_LEN];
        let checksum = verify_checksum_field(bytes, 36)?;
        let transform_kind = CompositeTransformKind::from_u8(bytes[6]).ok_or(QfError::BadIndex)?;
        Ok(Self {
            table_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            key_column_count: u16::from_le_bytes(bytes[4..6].try_into().unwrap()),
            transform_kind,
            flags: bytes[7],
            zone_count: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            key_columns_offset: u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
            entries_offset: u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
            entries_length: u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
            checksum,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeBinding {
    pub column_id: u32,
    pub file_code: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeEntry {
    pub bindings: Vec<CompositeBinding>,
    pub morsel_ids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeIndex {
    pub header: CompositeZoneIndexHeaderV1,
    pub key_columns: Vec<u32>,
    pub entries: Vec<u8>,
}

impl CompositeIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        let header = CompositeZoneIndexHeaderV1::parse(bytes)?;
        if header.key_column_count == 0 {
            return Err(QfError::BadIndex);
        }
        let key_columns_len = (header.key_column_count as u64)
            .checked_mul(4)
            .ok_or(QfError::ArithOverflow)?;
        let key_column_bytes = checked_region(bytes, header.key_columns_offset, key_columns_len)?;
        let mut key_columns = Vec::with_capacity(header.key_column_count as usize);
        for chunk in key_column_bytes.chunks_exact(4) {
            key_columns.push(u32::from_le_bytes(chunk.try_into().unwrap()));
        }
        let entries = checked_region(bytes, header.entries_offset, header.entries_length)?;
        Ok(Self {
            header,
            key_columns,
            entries: entries.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_key_columns_and_entries() {
        let key_columns = [1u32, 2];
        let entries = [0xABu8; 12];
        let header = CompositeZoneIndexHeaderV1 {
            table_id: 1,
            key_column_count: key_columns.len() as u16,
            transform_kind: CompositeTransformKind::Tuple,
            flags: 0,
            zone_count: 1,
            key_columns_offset: COMPOSITE_ZONE_INDEX_HEADER_LEN as u64,
            entries_offset: (COMPOSITE_ZONE_INDEX_HEADER_LEN + key_columns.len() * 4) as u64,
            entries_length: entries.len() as u64,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        for column in key_columns {
            bytes.extend_from_slice(&column.to_le_bytes());
        }
        bytes.extend_from_slice(&entries);
        let i = CompositeIndex::parse(&bytes).unwrap();
        assert_eq!(i.key_columns, vec![1, 2]);
        assert_eq!(i.entries, entries);
    }

    #[test]
    fn rejects_zero_key_columns() {
        let header = CompositeZoneIndexHeaderV1 {
            table_id: 1,
            key_column_count: 0,
            transform_kind: CompositeTransformKind::Tuple,
            flags: 0,
            zone_count: 0,
            key_columns_offset: COMPOSITE_ZONE_INDEX_HEADER_LEN as u64,
            entries_offset: COMPOSITE_ZONE_INDEX_HEADER_LEN as u64,
            entries_length: 0,
            checksum: 0,
        };
        let bytes = header.serialize();
        assert_eq!(CompositeIndex::parse(&bytes), Err(QfError::BadIndex));
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let header = CompositeZoneIndexHeaderV1 {
            table_id: 1,
            key_column_count: 1,
            transform_kind: CompositeTransformKind::Tuple,
            flags: 0,
            zone_count: 0,
            key_columns_offset: COMPOSITE_ZONE_INDEX_HEADER_LEN as u64,
            entries_offset: (COMPOSITE_ZONE_INDEX_HEADER_LEN + 4) as u64,
            entries_length: 0,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        bytes[36] ^= 0xff;
        bytes.extend_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            CompositeIndex::parse(&bytes),
            Err(QfError::ChecksumMismatch)
        );
    }
}
