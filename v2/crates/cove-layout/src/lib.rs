//! COVE-L layout plans, scan splits, and scheduling metadata for COVE v2.

use std::collections::BTreeSet;

use cove_core::{
    checksum,
    constants::{CoveLogicalType, CovePhysicalKind, SectionKind},
    footer::{CoveFooter, CoveSectionEntryV1},
    segment::TableSegmentIndexEntryV1,
    table::TableEntry,
    CoveError,
};

const ABSENT_ID: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPlanHeaderV2 {
    pub layout_id: u32,
    pub node_count: u32,
    pub root_node_id: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl LayoutPlanHeaderV2 {
    pub const LEN: usize = 20;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            layout_id: read_u32(bytes, 0)?,
            node_count: read_u32(bytes, 4)?,
            root_node_id: read_u32(bytes, 8)?,
            flags: read_u32(bytes, 12)?,
            checksum: read_u32(bytes, 16)?,
        };
        verify_crc(&bytes[..Self::LEN], 16, header.checksum)?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.layout_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.node_count.to_le_bytes());
        out[8..12].copy_from_slice(&self.root_node_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[16..20].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPlanNodeV2 {
    pub node_id: u32,
    pub parent_node_id: u32,
    pub node_kind: u16,
    pub flags: u16,
    pub table_id: u32,
    pub column_id: u32,
    pub segment_id: u32,
    pub first_morsel_id: u32,
    pub morsel_count: u32,
    pub row_start: u64,
    pub row_count: u64,
    pub section_id: u32,
    pub cluster_id: u32,
    pub first_child_index: u32,
    pub child_count: u32,
    pub stats_ref: u32,
    pub split_ref: u32,
    pub checksum: u32,
}

impl LayoutPlanNodeV2 {
    pub const LEN: usize = 76;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let node = Self {
            node_id: read_u32(bytes, 0)?,
            parent_node_id: read_u32(bytes, 4)?,
            node_kind: read_u16(bytes, 8)?,
            flags: read_u16(bytes, 10)?,
            table_id: read_u32(bytes, 12)?,
            column_id: read_u32(bytes, 16)?,
            segment_id: read_u32(bytes, 20)?,
            first_morsel_id: read_u32(bytes, 24)?,
            morsel_count: read_u32(bytes, 28)?,
            row_start: read_u64(bytes, 32)?,
            row_count: read_u64(bytes, 40)?,
            section_id: read_u32(bytes, 48)?,
            cluster_id: read_u32(bytes, 52)?,
            first_child_index: read_u32(bytes, 56)?,
            child_count: read_u32(bytes, 60)?,
            stats_ref: read_u32(bytes, 64)?,
            split_ref: read_u32(bytes, 68)?,
            checksum: read_u32(bytes, 72)?,
        };
        verify_crc(&bytes[..Self::LEN], 72, node.checksum)?;
        if !matches!(node.node_kind, 0..=7 | 255) {
            return Err(CoveError::BadLayoutPlan);
        }
        Ok(node)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.node_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.parent_node_id.to_le_bytes());
        out[8..10].copy_from_slice(&self.node_kind.to_le_bytes());
        out[10..12].copy_from_slice(&self.flags.to_le_bytes());
        out[12..16].copy_from_slice(&self.table_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.column_id.to_le_bytes());
        out[20..24].copy_from_slice(&self.segment_id.to_le_bytes());
        out[24..28].copy_from_slice(&self.first_morsel_id.to_le_bytes());
        out[28..32].copy_from_slice(&self.morsel_count.to_le_bytes());
        out[32..40].copy_from_slice(&self.row_start.to_le_bytes());
        out[40..48].copy_from_slice(&self.row_count.to_le_bytes());
        out[48..52].copy_from_slice(&self.section_id.to_le_bytes());
        out[52..56].copy_from_slice(&self.cluster_id.to_le_bytes());
        out[56..60].copy_from_slice(&self.first_child_index.to_le_bytes());
        out[60..64].copy_from_slice(&self.child_count.to_le_bytes());
        out[64..68].copy_from_slice(&self.stats_ref.to_le_bytes());
        out[68..72].copy_from_slice(&self.split_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[72..76].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPlanV2 {
    pub header: LayoutPlanHeaderV2,
    pub nodes: Vec<LayoutPlanNodeV2>,
}

impl LayoutPlanV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = LayoutPlanHeaderV2::parse(bytes)?;
        let count = header.node_count as usize;
        let nodes_start = LayoutPlanHeaderV2::LEN;
        let nodes_len = count
            .checked_mul(LayoutPlanNodeV2::LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let end = nodes_start
            .checked_add(nodes_len)
            .ok_or(CoveError::ArithOverflow)?;
        if bytes.len() != end {
            return Err(CoveError::BadLayoutPlan);
        }
        let mut nodes = Vec::with_capacity(count);
        for chunk in bytes[nodes_start..end].chunks_exact(LayoutPlanNodeV2::LEN) {
            nodes.push(LayoutPlanNodeV2::parse(chunk)?);
        }
        validate_layout_nodes(&header, &nodes)?;
        Ok(Self { header, nodes })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        validate_layout_nodes(&self.header, &self.nodes)?;
        let mut header = self.header.clone();
        header.node_count = self.nodes.len() as u32;
        let mut out =
            Vec::with_capacity(LayoutPlanHeaderV2::LEN + self.nodes.len() * LayoutPlanNodeV2::LEN);
        out.extend_from_slice(&header.serialize());
        for node in &self.nodes {
            out.extend_from_slice(&node.serialize());
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanSplitIndexHeaderV2 {
    pub split_count: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl ScanSplitIndexHeaderV2 {
    pub const LEN: usize = 12;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            split_count: read_u32(bytes, 0)?,
            flags: read_u32(bytes, 4)?,
            checksum: read_u32(bytes, 8)?,
        };
        verify_crc(&bytes[..Self::LEN], 8, header.checksum)?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.split_count.to_le_bytes());
        out[4..8].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[8..12].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanSplitEntryV2 {
    pub split_id: u32,
    pub table_id: u32,
    pub row_start: u64,
    pub row_count: u64,
    pub first_segment_id: u32,
    pub segment_count: u32,
    pub first_morsel_id: u32,
    pub morsel_count: u32,
    pub first_cluster_id: u32,
    pub cluster_count: u32,
    pub stats_ref: u32,
    pub estimated_uncompressed_bytes: u64,
    pub estimated_encoded_bytes: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl ScanSplitEntryV2 {
    pub const LEN: usize = 76;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let entry = Self {
            split_id: read_u32(bytes, 0)?,
            table_id: read_u32(bytes, 4)?,
            row_start: read_u64(bytes, 8)?,
            row_count: read_u64(bytes, 16)?,
            first_segment_id: read_u32(bytes, 24)?,
            segment_count: read_u32(bytes, 28)?,
            first_morsel_id: read_u32(bytes, 32)?,
            morsel_count: read_u32(bytes, 36)?,
            first_cluster_id: read_u32(bytes, 40)?,
            cluster_count: read_u32(bytes, 44)?,
            stats_ref: read_u32(bytes, 48)?,
            estimated_uncompressed_bytes: read_u64(bytes, 52)?,
            estimated_encoded_bytes: read_u64(bytes, 60)?,
            flags: read_u32(bytes, 68)?,
            checksum: read_u32(bytes, 72)?,
        };
        verify_crc(&bytes[..Self::LEN], 72, entry.checksum)?;
        Ok(entry)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.split_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.table_id.to_le_bytes());
        out[8..16].copy_from_slice(&self.row_start.to_le_bytes());
        out[16..24].copy_from_slice(&self.row_count.to_le_bytes());
        out[24..28].copy_from_slice(&self.first_segment_id.to_le_bytes());
        out[28..32].copy_from_slice(&self.segment_count.to_le_bytes());
        out[32..36].copy_from_slice(&self.first_morsel_id.to_le_bytes());
        out[36..40].copy_from_slice(&self.morsel_count.to_le_bytes());
        out[40..44].copy_from_slice(&self.first_cluster_id.to_le_bytes());
        out[44..48].copy_from_slice(&self.cluster_count.to_le_bytes());
        out[48..52].copy_from_slice(&self.stats_ref.to_le_bytes());
        out[52..60].copy_from_slice(&self.estimated_uncompressed_bytes.to_le_bytes());
        out[60..68].copy_from_slice(&self.estimated_encoded_bytes.to_le_bytes());
        out[68..72].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[72..76].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanSplitIndexV2 {
    pub header: ScanSplitIndexHeaderV2,
    pub entries: Vec<ScanSplitEntryV2>,
}

impl ScanSplitIndexV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = ScanSplitIndexHeaderV2::parse(bytes)?;
        let count = header.split_count as usize;
        let entries_start = ScanSplitIndexHeaderV2::LEN;
        let entries_len = count
            .checked_mul(ScanSplitEntryV2::LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let end = entries_start
            .checked_add(entries_len)
            .ok_or(CoveError::ArithOverflow)?;
        if bytes.len() != end {
            return Err(CoveError::BadLayoutPlan);
        }
        let mut entries = Vec::with_capacity(count);
        let mut split_ids = BTreeSet::new();
        for chunk in bytes[entries_start..end].chunks_exact(ScanSplitEntryV2::LEN) {
            let entry = ScanSplitEntryV2::parse(chunk)?;
            if !split_ids.insert(entry.split_id) {
                return Err(CoveError::BadLayoutPlan);
            }
            entries.push(entry);
        }
        validate_scan_splits(&header, &entries)?;
        Ok(Self { header, entries })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        validate_scan_splits(&self.header, &self.entries)?;
        let mut header = self.header.clone();
        header.split_count = self.entries.len() as u32;
        let mut out = Vec::with_capacity(
            ScanSplitIndexHeaderV2::LEN + self.entries.len() * ScanSplitEntryV2::LEN,
        );
        out.extend_from_slice(&header.serialize());
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize());
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastMetadataIndexHeaderV2 {
    pub entry_count: u32,
    pub entry_len: u16,
    pub index_kind: u8,
    pub flags: u8,
    pub entries_offset: u64,
    pub entries_length: u64,
    pub checksum: u32,
}

impl FastMetadataIndexHeaderV2 {
    pub const LEN: usize = 28;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            entry_count: read_u32(bytes, 0)?,
            entry_len: read_u16(bytes, 4)?,
            index_kind: read_u8(bytes, 6)?,
            flags: read_u8(bytes, 7)?,
            entries_offset: read_u64(bytes, 8)?,
            entries_length: read_u64(bytes, 16)?,
            checksum: read_u32(bytes, 24)?,
        };
        verify_crc(&bytes[..Self::LEN], 24, header.checksum)?;
        if header.entry_len as usize != FastMetadataIndexEntryV2::LEN {
            return Err(CoveError::BadLayoutPlan);
        }
        Ok(header)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        if self.entry_len as usize != FastMetadataIndexEntryV2::LEN {
            return Err(CoveError::BadLayoutPlan);
        }
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.entry_count.to_le_bytes());
        out[4..6].copy_from_slice(&self.entry_len.to_le_bytes());
        out[6] = self.index_kind;
        out[7] = self.flags;
        out[8..16].copy_from_slice(&self.entries_offset.to_le_bytes());
        out[16..24].copy_from_slice(&self.entries_length.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[24..28].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FastMetadataIndexEntryV2 {
    pub target_kind: u16,
    pub flags: u16,
    pub table_id: u32,
    pub column_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub section_id: u32,
    pub local_id: u32,
    pub offset: u64,
    pub length: u64,
    pub checksum_or_crc32c: u32,
    pub reserved: u32,
}

impl FastMetadataIndexEntryV2 {
    pub const LEN: usize = 52;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let entry = Self {
            target_kind: read_u16(bytes, 0)?,
            flags: read_u16(bytes, 2)?,
            table_id: read_u32(bytes, 4)?,
            column_id: read_u32(bytes, 8)?,
            segment_id: read_u32(bytes, 12)?,
            morsel_id: read_u32(bytes, 16)?,
            section_id: read_u32(bytes, 20)?,
            local_id: read_u32(bytes, 24)?,
            offset: read_u64(bytes, 28)?,
            length: read_u64(bytes, 36)?,
            checksum_or_crc32c: read_u32(bytes, 44)?,
            reserved: read_u32(bytes, 48)?,
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..2].copy_from_slice(&self.target_kind.to_le_bytes());
        out[2..4].copy_from_slice(&self.flags.to_le_bytes());
        out[4..8].copy_from_slice(&self.table_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.column_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.segment_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[20..24].copy_from_slice(&self.section_id.to_le_bytes());
        out[24..28].copy_from_slice(&self.local_id.to_le_bytes());
        out[28..36].copy_from_slice(&self.offset.to_le_bytes());
        out[36..44].copy_from_slice(&self.length.to_le_bytes());
        out[44..48].copy_from_slice(&self.checksum_or_crc32c.to_le_bytes());
        out[48..52].copy_from_slice(&self.reserved.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.target_kind > 7 {
            return Err(CoveError::BadLayoutPlan);
        }
        if self.reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        checked_end(self.offset, self.length)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastMetadataIndexV2 {
    pub header: FastMetadataIndexHeaderV2,
    pub entries: Vec<FastMetadataIndexEntryV2>,
}

impl FastMetadataIndexV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = FastMetadataIndexHeaderV2::parse(bytes)?;
        let start = usize::try_from(header.entries_offset).map_err(|_| CoveError::OffsetRange)?;
        let len = usize::try_from(header.entries_length).map_err(|_| CoveError::OffsetRange)?;
        let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        let expected = header
            .entry_count
            .checked_mul(FastMetadataIndexEntryV2::LEN as u32)
            .ok_or(CoveError::ArithOverflow)? as usize;
        if start < FastMetadataIndexHeaderV2::LEN || end != bytes.len() || len != expected {
            return Err(CoveError::BadLayoutPlan);
        }
        let entries = bytes[start..end]
            .chunks_exact(FastMetadataIndexEntryV2::LEN)
            .map(FastMetadataIndexEntryV2::parse)
            .collect::<Result<Vec<_>, _>>()?;
        validate_fast_metadata_entries(&header, &entries)?;
        Ok(Self { header, entries })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        validate_fast_metadata_entries(&self.header, &self.entries)?;
        let mut header = self.header.clone();
        header.entry_count = self.entries.len() as u32;
        header.entry_len = FastMetadataIndexEntryV2::LEN as u16;
        header.entries_offset = FastMetadataIndexHeaderV2::LEN as u64;
        header.entries_length = (self.entries.len() * FastMetadataIndexEntryV2::LEN) as u64;
        let mut out = Vec::with_capacity(
            FastMetadataIndexHeaderV2::LEN + self.entries.len() * FastMetadataIndexEntryV2::LEN,
        );
        out.extend_from_slice(&header.serialize()?);
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize()?);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageClusterDirectoryHeaderV2 {
    pub cluster_count: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl PageClusterDirectoryHeaderV2 {
    pub const LEN: usize = 12;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            cluster_count: read_u32(bytes, 0)?,
            flags: read_u32(bytes, 4)?,
            checksum: read_u32(bytes, 8)?,
        };
        verify_crc(&bytes[..Self::LEN], 8, header.checksum)?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.cluster_count.to_le_bytes());
        out[4..8].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[8..12].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageClusterEntryV2 {
    pub cluster_id: u32,
    pub section_id: u32,
    pub offset: u64,
    pub length: u64,
    pub table_id: u32,
    pub segment_id: u32,
    pub first_morsel_id: u32,
    pub morsel_count: u32,
    pub first_page_ref: u32,
    pub page_count: u32,
    pub preferred_read_alignment: u32,
    pub preferred_coalesce_distance: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl PageClusterEntryV2 {
    pub const LEN: usize = 64;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let entry = Self {
            cluster_id: read_u32(bytes, 0)?,
            section_id: read_u32(bytes, 4)?,
            offset: read_u64(bytes, 8)?,
            length: read_u64(bytes, 16)?,
            table_id: read_u32(bytes, 24)?,
            segment_id: read_u32(bytes, 28)?,
            first_morsel_id: read_u32(bytes, 32)?,
            morsel_count: read_u32(bytes, 36)?,
            first_page_ref: read_u32(bytes, 40)?,
            page_count: read_u32(bytes, 44)?,
            preferred_read_alignment: read_u32(bytes, 48)?,
            preferred_coalesce_distance: read_u32(bytes, 52)?,
            flags: read_u32(bytes, 56)?,
            checksum: read_u32(bytes, 60)?,
        };
        verify_crc(&bytes[..Self::LEN], 60, entry.checksum)?;
        entry.validate()?;
        Ok(entry)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.cluster_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.section_id.to_le_bytes());
        out[8..16].copy_from_slice(&self.offset.to_le_bytes());
        out[16..24].copy_from_slice(&self.length.to_le_bytes());
        out[24..28].copy_from_slice(&self.table_id.to_le_bytes());
        out[28..32].copy_from_slice(&self.segment_id.to_le_bytes());
        out[32..36].copy_from_slice(&self.first_morsel_id.to_le_bytes());
        out[36..40].copy_from_slice(&self.morsel_count.to_le_bytes());
        out[40..44].copy_from_slice(&self.first_page_ref.to_le_bytes());
        out[44..48].copy_from_slice(&self.page_count.to_le_bytes());
        out[48..52].copy_from_slice(&self.preferred_read_alignment.to_le_bytes());
        out[52..56].copy_from_slice(&self.preferred_coalesce_distance.to_le_bytes());
        out[56..60].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[60..64].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.section_id == 0 || self.length == 0 || self.page_count == 0 {
            return Err(CoveError::BadLayoutPlan);
        }
        checked_end(self.offset, self.length)?;
        self.first_morsel_id
            .checked_add(self.morsel_count)
            .ok_or(CoveError::ArithOverflow)?;
        self.first_page_ref
            .checked_add(self.page_count)
            .ok_or(CoveError::ArithOverflow)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageClusterDirectoryV2 {
    pub header: PageClusterDirectoryHeaderV2,
    pub entries: Vec<PageClusterEntryV2>,
}

impl PageClusterDirectoryV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = PageClusterDirectoryHeaderV2::parse(bytes)?;
        let count = header.cluster_count as usize;
        let entries_start = PageClusterDirectoryHeaderV2::LEN;
        let entries_len = count
            .checked_mul(PageClusterEntryV2::LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let end = entries_start
            .checked_add(entries_len)
            .ok_or(CoveError::ArithOverflow)?;
        if bytes.len() != end {
            return Err(CoveError::BadLayoutPlan);
        }
        let entries = bytes[entries_start..end]
            .chunks_exact(PageClusterEntryV2::LEN)
            .map(PageClusterEntryV2::parse)
            .collect::<Result<Vec<_>, _>>()?;
        validate_page_clusters(&header, &entries)?;
        Ok(Self { header, entries })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        validate_page_clusters(&self.header, &self.entries)?;
        let mut header = self.header.clone();
        header.cluster_count = self.entries.len() as u32;
        let mut out = Vec::with_capacity(
            PageClusterDirectoryHeaderV2::LEN + self.entries.len() * PageClusterEntryV2::LEN,
        );
        out.extend_from_slice(&header.serialize());
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize()?);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZeroCopyBufferMapHeaderV2 {
    pub map_count: u32,
    pub target_count: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl ZeroCopyBufferMapHeaderV2 {
    pub const LEN: usize = 16;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            map_count: read_u32(bytes, 0)?,
            target_count: read_u32(bytes, 4)?,
            flags: read_u32(bytes, 8)?,
            checksum: read_u32(bytes, 12)?,
        };
        verify_crc(&bytes[..Self::LEN], 12, header.checksum)?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.map_count.to_le_bytes());
        out[4..8].copy_from_slice(&self.target_count.to_le_bytes());
        out[8..12].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[12..16].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZeroCopyTargetV2 {
    pub target_id: u32,
    pub namespace: String,
    pub target_name: String,
    pub version_major: u16,
    pub version_minor: u16,
    pub flags: u32,
}

impl ZeroCopyTargetV2 {
    pub fn parse_one(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        let mut cursor = Cursor::new(bytes);
        let target_id = cursor.u32()?;
        let namespace_len = cursor.u16()? as usize;
        let namespace = parse_utf8(cursor.bytes(namespace_len)?)?;
        let target_name_len = cursor.u16()? as usize;
        let target_name = parse_utf8(cursor.bytes(target_name_len)?)?;
        let version_major = cursor.u16()?;
        let version_minor = cursor.u16()?;
        let flags = cursor.u32()?;
        let target = Self {
            target_id,
            namespace,
            target_name,
            version_major,
            version_minor,
            flags,
        };
        target.validate()?;
        Ok((target, cursor.position))
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        self.validate()?;
        if self.namespace.len() > u16::MAX as usize || self.target_name.len() > u16::MAX as usize {
            return Err(CoveError::BadLayoutPlan);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&self.target_id.to_le_bytes());
        out.extend_from_slice(&(self.namespace.len() as u16).to_le_bytes());
        out.extend_from_slice(self.namespace.as_bytes());
        out.extend_from_slice(&(self.target_name.len() as u16).to_le_bytes());
        out.extend_from_slice(self.target_name.as_bytes());
        out.extend_from_slice(&self.version_major.to_le_bytes());
        out.extend_from_slice(&self.version_minor.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.namespace.is_empty() || self.target_name.is_empty() {
            return Err(CoveError::BadLayoutPlan);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ZeroCopyNullBitmapPolarityV2 {
    OneMeansNull = 0,
    OneMeansValid = 1,
    NoNullBitmap = 2,
    TargetDefines = 255,
}

impl ZeroCopyNullBitmapPolarityV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::OneMeansNull),
            1 => Some(Self::OneMeansValid),
            2 => Some(Self::NoNullBitmap),
            255 => Some(Self::TargetDefines),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ZeroCopyLifetimeScopeV2 {
    Page = 0,
    Segment = 1,
    FileMapping = 2,
    ReaderSession = 3,
    ExternalOwner = 4,
    InvalidAfterDecode = 5,
}

impl ZeroCopyLifetimeScopeV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Page),
            1 => Some(Self::Segment),
            2 => Some(Self::FileMapping),
            3 => Some(Self::ReaderSession),
            4 => Some(Self::ExternalOwner),
            5 => Some(Self::InvalidAfterDecode),
            _ => None,
        }
    }

    fn lifetime_rank(self) -> u8 {
        match self {
            Self::InvalidAfterDecode => 0,
            Self::Page => 1,
            Self::Segment => 2,
            Self::FileMapping => 3,
            Self::ReaderSession => 4,
            Self::ExternalOwner => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ZeroCopyDictionarySemanticsV2 {
    NoDictionary = 0,
    FileCodeDictionary = 1,
    ArrowDictionaryValues = 2,
    EngineDictionary = 3,
    RequiresRemap = 4,
    Incompatible = 255,
}

impl ZeroCopyDictionarySemanticsV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::NoDictionary),
            1 => Some(Self::FileCodeDictionary),
            2 => Some(Self::ArrowDictionaryValues),
            3 => Some(Self::EngineDictionary),
            4 => Some(Self::RequiresRemap),
            255 => Some(Self::Incompatible),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ZeroCopyNestedLayoutKindV2 {
    NotNested = 0,
    ArrowListOffsets32 = 1,
    ArrowLargeListOffsets64 = 2,
    ArrowStructChildren = 3,
    ArrowMapOffsets32 = 4,
    CoveNativeNested = 5,
    Extension = 255,
}

impl ZeroCopyNestedLayoutKindV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::NotNested),
            1 => Some(Self::ArrowListOffsets32),
            2 => Some(Self::ArrowLargeListOffsets64),
            3 => Some(Self::ArrowStructChildren),
            4 => Some(Self::ArrowMapOffsets32),
            5 => Some(Self::CoveNativeNested),
            255 => Some(Self::Extension),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ZeroCopyTargetBufferRoleV2 {
    Values = 0,
    ValidityBitmap = 1,
    NullBitmap = 2,
    Offsets32 = 3,
    Offsets64 = 4,
    TypeIds = 5,
    DictionaryKeys = 6,
    DictionaryValues = 7,
    ChildData = 8,
    SelectionBitmap = 9,
    RunEnds = 10,
    Extension = 255,
}

impl ZeroCopyTargetBufferRoleV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::Values),
            1 => Some(Self::ValidityBitmap),
            2 => Some(Self::NullBitmap),
            3 => Some(Self::Offsets32),
            4 => Some(Self::Offsets64),
            5 => Some(Self::TypeIds),
            6 => Some(Self::DictionaryKeys),
            7 => Some(Self::DictionaryValues),
            8 => Some(Self::ChildData),
            9 => Some(Self::SelectionBitmap),
            10 => Some(Self::RunEnds),
            255 => Some(Self::Extension),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ZeroCopySourceBufferRoleV2 {
    CoveValues = 0,
    CoveNullBitmap = 1,
    CoveOffsets = 2,
    CoveChildLayout = 3,
    CoveDictionaryCodes = 4,
    CoveDictionaryPayload = 5,
    CoveEncodedPayload = 6,
    CoveSelectionBitmap = 7,
    CoveRunEnds = 8,
    Extension = 255,
}

impl ZeroCopySourceBufferRoleV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::CoveValues),
            1 => Some(Self::CoveNullBitmap),
            2 => Some(Self::CoveOffsets),
            3 => Some(Self::CoveChildLayout),
            4 => Some(Self::CoveDictionaryCodes),
            5 => Some(Self::CoveDictionaryPayload),
            6 => Some(Self::CoveEncodedPayload),
            7 => Some(Self::CoveSelectionBitmap),
            8 => Some(Self::CoveRunEnds),
            255 => Some(Self::Extension),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZeroCopyBufferMapEntryV2 {
    pub target_id: u32,
    pub table_id: u32,
    pub column_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub page_ref: u32,
    pub buffer_id: u16,
    pub buffer_kind: u16,
    pub logical_type: u16,
    pub physical_kind: u8,
    pub source_endianness: u8,
    pub required_alignment_log2: u8,
    pub null_bitmap_polarity: ZeroCopyNullBitmapPolarityV2,
    pub source_offset_width_bits: u16,
    pub target_offset_width_bits: u16,
    pub dictionary_key_width_bits: u16,
    pub dictionary_semantics: ZeroCopyDictionarySemanticsV2,
    pub lifetime_scope: ZeroCopyLifetimeScopeV2,
    pub nested_layout_kind: ZeroCopyNestedLayoutKindV2,
    pub compression_required_none: u8,
    pub target_buffer_role: ZeroCopyTargetBufferRoleV2,
    pub source_buffer_role: ZeroCopySourceBufferRoleV2,
    pub target_type_ref: u32,
    pub dictionary_values_ref: u32,
    pub child_layout_ref: u32,
    pub owner_lifetime_ref: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl ZeroCopyBufferMapEntryV2 {
    pub const LEN: usize = 72;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let entry = Self {
            target_id: read_u32(bytes, 0)?,
            table_id: read_u32(bytes, 4)?,
            column_id: read_u32(bytes, 8)?,
            segment_id: read_u32(bytes, 12)?,
            morsel_id: read_u32(bytes, 16)?,
            page_ref: read_u32(bytes, 20)?,
            buffer_id: read_u16(bytes, 24)?,
            buffer_kind: read_u16(bytes, 26)?,
            logical_type: read_u16(bytes, 28)?,
            physical_kind: read_u8(bytes, 30)?,
            source_endianness: read_u8(bytes, 31)?,
            required_alignment_log2: read_u8(bytes, 32)?,
            null_bitmap_polarity: ZeroCopyNullBitmapPolarityV2::from_u8(read_u8(bytes, 33)?)
                .ok_or(CoveError::BadLayoutPlan)?,
            source_offset_width_bits: read_u16(bytes, 34)?,
            target_offset_width_bits: read_u16(bytes, 36)?,
            dictionary_key_width_bits: read_u16(bytes, 38)?,
            dictionary_semantics: ZeroCopyDictionarySemanticsV2::from_u8(read_u8(bytes, 40)?)
                .ok_or(CoveError::BadLayoutPlan)?,
            lifetime_scope: ZeroCopyLifetimeScopeV2::from_u8(read_u8(bytes, 41)?)
                .ok_or(CoveError::BadLayoutPlan)?,
            nested_layout_kind: ZeroCopyNestedLayoutKindV2::from_u8(read_u8(bytes, 42)?)
                .ok_or(CoveError::BadLayoutPlan)?,
            compression_required_none: read_u8(bytes, 43)?,
            target_buffer_role: ZeroCopyTargetBufferRoleV2::from_u16(read_u16(bytes, 44)?)
                .ok_or(CoveError::BadLayoutPlan)?,
            source_buffer_role: ZeroCopySourceBufferRoleV2::from_u16(read_u16(bytes, 46)?)
                .ok_or(CoveError::BadLayoutPlan)?,
            target_type_ref: read_u32(bytes, 48)?,
            dictionary_values_ref: read_u32(bytes, 52)?,
            child_layout_ref: read_u32(bytes, 56)?,
            owner_lifetime_ref: read_u32(bytes, 60)?,
            flags: read_u32(bytes, 64)?,
            checksum: read_u32(bytes, 68)?,
        };
        verify_crc(&bytes[..Self::LEN], 68, entry.checksum)?;
        entry.validate()?;
        Ok(entry)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.target_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.table_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.column_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.segment_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[20..24].copy_from_slice(&self.page_ref.to_le_bytes());
        out[24..26].copy_from_slice(&self.buffer_id.to_le_bytes());
        out[26..28].copy_from_slice(&self.buffer_kind.to_le_bytes());
        out[28..30].copy_from_slice(&self.logical_type.to_le_bytes());
        out[30] = self.physical_kind;
        out[31] = self.source_endianness;
        out[32] = self.required_alignment_log2;
        out[33] = self.null_bitmap_polarity as u8;
        out[34..36].copy_from_slice(&self.source_offset_width_bits.to_le_bytes());
        out[36..38].copy_from_slice(&self.target_offset_width_bits.to_le_bytes());
        out[38..40].copy_from_slice(&self.dictionary_key_width_bits.to_le_bytes());
        out[40] = self.dictionary_semantics as u8;
        out[41] = self.lifetime_scope as u8;
        out[42] = self.nested_layout_kind as u8;
        out[43] = self.compression_required_none;
        out[44..46].copy_from_slice(&(self.target_buffer_role as u16).to_le_bytes());
        out[46..48].copy_from_slice(&(self.source_buffer_role as u16).to_le_bytes());
        out[48..52].copy_from_slice(&self.target_type_ref.to_le_bytes());
        out[52..56].copy_from_slice(&self.dictionary_values_ref.to_le_bytes());
        out[56..60].copy_from_slice(&self.child_layout_ref.to_le_bytes());
        out[60..64].copy_from_slice(&self.owner_lifetime_ref.to_le_bytes());
        out[64..68].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[68..72].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        match self.compression_required_none {
            0 | 1 => {}
            _ => return Err(CoveError::BadLayoutPlan),
        }
        if self.source_offset_width_bits != 0 && !matches!(self.source_offset_width_bits, 32 | 64) {
            return Err(CoveError::BadLayoutPlan);
        }
        if self.target_offset_width_bits != 0 && !matches!(self.target_offset_width_bits, 32 | 64) {
            return Err(CoveError::BadLayoutPlan);
        }
        Ok(())
    }

    pub fn compatibility(&self, context: &ZeroCopyCompatibilityContext) -> ZeroCopyCompatibilityV2 {
        use ZeroCopyMaterializationReasonV2 as Reason;
        if matches!(
            self.target_buffer_role,
            ZeroCopyTargetBufferRoleV2::Extension
        ) || matches!(
            self.source_buffer_role,
            ZeroCopySourceBufferRoleV2::Extension
        ) {
            return ZeroCopyCompatibilityV2::MaterializeRequired(Reason::UnknownRole);
        }
        if context.active_visibility_overlay
            && self.target_buffer_role != ZeroCopyTargetBufferRoleV2::SelectionBitmap
        {
            return ZeroCopyCompatibilityV2::MaterializeRequired(Reason::ActiveVisibilityOverlay);
        }
        if self.compression_required_none != 1 {
            return ZeroCopyCompatibilityV2::MaterializeRequired(Reason::CompressedBuffer);
        }
        if self.null_bitmap_polarity == ZeroCopyNullBitmapPolarityV2::OneMeansValid
            && !context.accepts_cove_null_bitmap_polarity
        {
            return ZeroCopyCompatibilityV2::MaterializeRequired(Reason::NullPolarityMismatch);
        }
        if matches!(
            self.dictionary_semantics,
            ZeroCopyDictionarySemanticsV2::RequiresRemap
                | ZeroCopyDictionarySemanticsV2::Incompatible
        ) || self.dictionary_semantics != context.expected_dictionary_semantics
        {
            return ZeroCopyCompatibilityV2::MaterializeRequired(Reason::DictionaryMismatch);
        }
        if self.nested_layout_kind != context.expected_nested_layout_kind {
            return ZeroCopyCompatibilityV2::MaterializeRequired(Reason::NestedLayoutMismatch);
        }
        if self.lifetime_scope.lifetime_rank() < context.required_lifetime_scope.lifetime_rank() {
            return ZeroCopyCompatibilityV2::MaterializeRequired(Reason::InsufficientLifetime);
        }
        ZeroCopyCompatibilityV2::Compatible
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZeroCopyBufferMapV2 {
    pub header: ZeroCopyBufferMapHeaderV2,
    pub targets: Vec<ZeroCopyTargetV2>,
    pub entries: Vec<ZeroCopyBufferMapEntryV2>,
}

impl ZeroCopyBufferMapV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = ZeroCopyBufferMapHeaderV2::parse(bytes)?;
        let mut offset = ZeroCopyBufferMapHeaderV2::LEN;
        let mut targets = Vec::with_capacity(header.target_count as usize);
        let mut target_ids = BTreeSet::new();
        for _ in 0..header.target_count {
            let (target, consumed) = ZeroCopyTargetV2::parse_one(&bytes[offset..])?;
            if !target_ids.insert(target.target_id) {
                return Err(CoveError::BadLayoutPlan);
            }
            offset = offset
                .checked_add(consumed)
                .ok_or(CoveError::ArithOverflow)?;
            targets.push(target);
        }
        let entries_len = (header.map_count as usize)
            .checked_mul(ZeroCopyBufferMapEntryV2::LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let end = offset
            .checked_add(entries_len)
            .ok_or(CoveError::ArithOverflow)?;
        if end != bytes.len() {
            return Err(CoveError::BadLayoutPlan);
        }
        let mut entries = Vec::with_capacity(header.map_count as usize);
        for chunk in bytes[offset..end].chunks_exact(ZeroCopyBufferMapEntryV2::LEN) {
            let entry = ZeroCopyBufferMapEntryV2::parse(chunk)?;
            if !target_ids.contains(&entry.target_id) {
                return Err(CoveError::BadLayoutPlan);
            }
            entries.push(entry);
        }
        Ok(Self {
            header,
            targets,
            entries,
        })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let mut target_ids = BTreeSet::new();
        for target in &self.targets {
            if !target_ids.insert(target.target_id) {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        for entry in &self.entries {
            if !target_ids.contains(&entry.target_id) {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        let header = ZeroCopyBufferMapHeaderV2 {
            map_count: self.entries.len() as u32,
            target_count: self.targets.len() as u32,
            flags: self.header.flags,
            checksum: 0,
        };
        let mut out = Vec::new();
        out.extend_from_slice(&header.serialize());
        for target in &self.targets {
            out.extend_from_slice(&target.serialize()?);
        }
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize()?);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZeroCopyCompatibilityContext {
    pub active_visibility_overlay: bool,
    pub accepts_cove_null_bitmap_polarity: bool,
    pub expected_dictionary_semantics: ZeroCopyDictionarySemanticsV2,
    pub expected_nested_layout_kind: ZeroCopyNestedLayoutKindV2,
    pub required_lifetime_scope: ZeroCopyLifetimeScopeV2,
}

impl Default for ZeroCopyCompatibilityContext {
    fn default() -> Self {
        Self {
            active_visibility_overlay: false,
            accepts_cove_null_bitmap_polarity: true,
            expected_dictionary_semantics: ZeroCopyDictionarySemanticsV2::NoDictionary,
            expected_nested_layout_kind: ZeroCopyNestedLayoutKindV2::NotNested,
            required_lifetime_scope: ZeroCopyLifetimeScopeV2::Page,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroCopyCompatibilityV2 {
    Compatible,
    MaterializeRequired(ZeroCopyMaterializationReasonV2),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroCopyMaterializationReasonV2 {
    UnknownRole,
    NullPolarityMismatch,
    CompressedBuffer,
    DictionaryMismatch,
    NestedLayoutMismatch,
    InsufficientLifetime,
    ActiveVisibilityOverlay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedLayoutPlanV2 {
    pub plan: LayoutPlanV2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedScanSplitIndexV2 {
    pub index: ScanSplitIndexV2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedZeroCopyBufferMapV2 {
    pub map: ZeroCopyBufferMapV2,
}

impl ValidatedLayoutPlanV2 {
    pub fn validate(
        plan: LayoutPlanV2,
        footer: &CoveFooter,
        table: &TableEntry,
        segments: &[TableSegmentIndexEntryV1],
        page_clusters: Option<&PageClusterDirectoryV2>,
        scan_splits: Option<&ScanSplitIndexV2>,
    ) -> Result<Self, CoveError> {
        validate_layout_plan_authority(&plan, footer, table, segments, page_clusters, scan_splits)?;
        Ok(Self { plan })
    }
}

impl ValidatedScanSplitIndexV2 {
    pub fn validate(
        index: ScanSplitIndexV2,
        table: &TableEntry,
        segments: &[TableSegmentIndexEntryV1],
        page_clusters: Option<&PageClusterDirectoryV2>,
    ) -> Result<Self, CoveError> {
        validate_scan_split_authority(&index, table, segments, page_clusters)?;
        Ok(Self { index })
    }
}

impl ValidatedZeroCopyBufferMapV2 {
    pub fn validate(
        map: ZeroCopyBufferMapV2,
        table: &TableEntry,
        segments: &[TableSegmentIndexEntryV1],
    ) -> Result<Self, CoveError> {
        validate_zero_copy_map_authority(&map, table, segments)?;
        Ok(Self { map })
    }
}

pub fn validate_scan_splits(
    header: &ScanSplitIndexHeaderV2,
    entries: &[ScanSplitEntryV2],
) -> Result<(), CoveError> {
    if header.split_count as usize != entries.len() {
        return Err(CoveError::BadLayoutPlan);
    }
    let mut split_ids = BTreeSet::new();
    for entry in entries {
        if !split_ids.insert(entry.split_id) {
            return Err(CoveError::BadLayoutPlan);
        }
        checked_end(entry.row_start, entry.row_count)?;
    }
    Ok(())
}

pub fn validate_scan_split_authority(
    index: &ScanSplitIndexV2,
    table: &TableEntry,
    segments: &[TableSegmentIndexEntryV1],
    page_clusters: Option<&PageClusterDirectoryV2>,
) -> Result<(), CoveError> {
    validate_scan_splits(&index.header, &index.entries)?;
    let clusters = page_clusters.map(|directory| {
        directory
            .entries
            .iter()
            .map(|entry| entry.cluster_id)
            .collect::<BTreeSet<_>>()
    });
    for split in &index.entries {
        if split.table_id != table.table_id || split.segment_count == 0 || split.morsel_count == 0 {
            return Err(CoveError::BadLayoutPlan);
        }
        let first_segment_end = split
            .first_segment_id
            .checked_add(split.segment_count)
            .ok_or(CoveError::ArithOverflow)?;
        let mut remaining_morsels = split.morsel_count;
        let mut actual_row_start = None;
        let mut actual_row_count = 0u64;
        for segment_id in split.first_segment_id..first_segment_end {
            let segment = segment_by_id(segments, table.table_id, segment_id)?;
            let start_morsel = if segment_id == split.first_segment_id {
                split.first_morsel_id
            } else {
                0
            };
            if start_morsel >= segment.morsel_count {
                return Err(CoveError::BadLayoutPlan);
            }
            let available = segment
                .morsel_count
                .checked_sub(start_morsel)
                .ok_or(CoveError::ArithOverflow)?;
            let take = available.min(remaining_morsels);
            if take == 0 {
                return Err(CoveError::BadLayoutPlan);
            }
            let start_row = segment_morsel_row_start(segment, start_morsel)?;
            if actual_row_start.is_none() {
                actual_row_start = Some(start_row);
            }
            actual_row_count = actual_row_count
                .checked_add(segment_morsel_span_rows(segment, start_morsel, take)?)
                .ok_or(CoveError::ArithOverflow)?;
            remaining_morsels = remaining_morsels
                .checked_sub(take)
                .ok_or(CoveError::ArithOverflow)?;
            if remaining_morsels == 0 {
                break;
            }
        }
        if remaining_morsels != 0 {
            return Err(CoveError::BadLayoutPlan);
        }
        if actual_row_start != Some(split.row_start) || actual_row_count != split.row_count {
            return Err(CoveError::BadLayoutPlan);
        }
        if split.cluster_count != 0 {
            let Some(cluster_ids) = clusters.as_ref() else {
                return Err(CoveError::BadLayoutPlan);
            };
            let cluster_end = split
                .first_cluster_id
                .checked_add(split.cluster_count)
                .ok_or(CoveError::ArithOverflow)?;
            for cluster_id in split.first_cluster_id..cluster_end {
                if !cluster_ids.contains(&cluster_id) {
                    return Err(CoveError::BadLayoutPlan);
                }
            }
        }
    }
    Ok(())
}

pub fn validate_fast_metadata_entries(
    header: &FastMetadataIndexHeaderV2,
    entries: &[FastMetadataIndexEntryV2],
) -> Result<(), CoveError> {
    if header.entry_count as usize != entries.len() {
        return Err(CoveError::BadLayoutPlan);
    }
    let mut prev = None;
    let mut seen = BTreeSet::new();
    for entry in entries {
        entry.validate()?;
        let key = (
            entry.target_kind,
            entry.table_id,
            entry.column_id,
            entry.segment_id,
            entry.morsel_id,
            entry.section_id,
            entry.local_id,
        );
        if let Some(previous) = prev {
            if key <= previous {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        if !seen.insert(key) {
            return Err(CoveError::BadLayoutPlan);
        }
        prev = Some(key);
    }
    Ok(())
}

pub fn validate_page_clusters(
    header: &PageClusterDirectoryHeaderV2,
    entries: &[PageClusterEntryV2],
) -> Result<(), CoveError> {
    if header.cluster_count as usize != entries.len() {
        return Err(CoveError::BadLayoutPlan);
    }
    let mut cluster_ids = BTreeSet::new();
    let mut prev_id = None;
    for entry in entries {
        entry.validate()?;
        if let Some(previous) = prev_id {
            if entry.cluster_id <= previous {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        if !cluster_ids.insert(entry.cluster_id) {
            return Err(CoveError::BadLayoutPlan);
        }
        prev_id = Some(entry.cluster_id);
    }
    Ok(())
}

pub fn validate_fast_metadata_authority(
    index: &FastMetadataIndexV2,
    footer: &CoveFooter,
) -> Result<(), CoveError> {
    validate_fast_metadata_entries(&index.header, &index.entries)?;
    for entry in &index.entries {
        let section = section_by_id(footer, entry.section_id)?;
        let section_end = section.end_offset()?;
        let entry_end = entry
            .offset
            .checked_add(entry.length)
            .ok_or(CoveError::ArithOverflow)?;
        if entry.offset < section.offset || entry_end > section_end {
            return Err(CoveError::BadLayoutPlan);
        }
    }
    Ok(())
}

pub fn validate_page_cluster_authority(
    directory: &PageClusterDirectoryV2,
    footer: &CoveFooter,
    table: &TableEntry,
    segments: &[TableSegmentIndexEntryV1],
) -> Result<(), CoveError> {
    validate_page_clusters(&directory.header, &directory.entries)?;
    for cluster in &directory.entries {
        let section = section_by_id(footer, cluster.section_id)?;
        if section.section_kind != SectionKind::TableSegmentData as u16 {
            return Err(CoveError::BadLayoutPlan);
        }
        if cluster.table_id != table.table_id {
            return Err(CoveError::BadLayoutPlan);
        }
        let section_end = section.end_offset()?;
        let cluster_end = cluster
            .offset
            .checked_add(cluster.length)
            .ok_or(CoveError::ArithOverflow)?;
        if cluster.offset < section.offset || cluster_end > section_end {
            return Err(CoveError::BadLayoutPlan);
        }
        let segment = segments
            .iter()
            .find(|segment| {
                segment.table_id == cluster.table_id && segment.segment_id == cluster.segment_id
            })
            .ok_or(CoveError::BadLayoutPlan)?;
        let segment_end = segment
            .offset
            .checked_add(segment.length)
            .ok_or(CoveError::ArithOverflow)?;
        if cluster.offset < segment.offset || cluster_end > segment_end {
            return Err(CoveError::BadLayoutPlan);
        }
        let morsel_end = cluster
            .first_morsel_id
            .checked_add(cluster.morsel_count)
            .ok_or(CoveError::ArithOverflow)?;
        if morsel_end > segment.morsel_count {
            return Err(CoveError::BadLayoutPlan);
        }
    }
    Ok(())
}

pub fn validate_layout_plan_authority(
    plan: &LayoutPlanV2,
    footer: &CoveFooter,
    table: &TableEntry,
    segments: &[TableSegmentIndexEntryV1],
    page_clusters: Option<&PageClusterDirectoryV2>,
    scan_splits: Option<&ScanSplitIndexV2>,
) -> Result<(), CoveError> {
    validate_layout_nodes(&plan.header, &plan.nodes)?;
    for node in &plan.nodes {
        validate_layout_node_authority(node, footer, table, segments, page_clusters, scan_splits)?;
    }
    Ok(())
}

pub fn validate_zero_copy_map_authority(
    map: &ZeroCopyBufferMapV2,
    table: &TableEntry,
    segments: &[TableSegmentIndexEntryV1],
) -> Result<(), CoveError> {
    let target_ids = map
        .targets
        .iter()
        .map(|target| target.target_id)
        .collect::<BTreeSet<_>>();
    for entry in &map.entries {
        if !target_ids.contains(&entry.target_id) || entry.table_id != table.table_id {
            return Err(CoveError::BadLayoutPlan);
        }
        let column = table
            .columns
            .iter()
            .find(|column| column.column_id == entry.column_id)
            .ok_or(CoveError::BadLayoutPlan)?;
        if CoveLogicalType::from_u16(entry.logical_type) != Some(column.logical)
            || CovePhysicalKind::from_u8(entry.physical_kind) != Some(column.physical)
        {
            return Err(CoveError::BadLayoutPlan);
        }
        let segment = segment_by_id(segments, table.table_id, entry.segment_id)?;
        if entry.morsel_id >= segment.morsel_count || is_absent_ref(entry.page_ref) {
            return Err(CoveError::BadLayoutPlan);
        }
        if entry.source_endianness != 0 {
            return Err(CoveError::BadLayoutPlan);
        }
    }
    Ok(())
}

pub fn build_default_scan_split_index(
    table: &TableEntry,
    segments: &[TableSegmentIndexEntryV1],
) -> Result<ScanSplitIndexV2, CoveError> {
    let entries = segments
        .iter()
        .filter(|segment| segment.table_id == table.table_id && segment.morsel_count != 0)
        .enumerate()
        .map(|(index, segment)| ScanSplitEntryV2 {
            split_id: u32::try_from(index + 1).unwrap_or(u32::MAX),
            table_id: table.table_id,
            row_start: segment.row_start,
            row_count: u64::from(segment.row_count),
            first_segment_id: segment.segment_id,
            segment_count: 1,
            first_morsel_id: 0,
            morsel_count: segment.morsel_count,
            first_cluster_id: 0,
            cluster_count: 0,
            stats_ref: segment.stats_ref,
            estimated_uncompressed_bytes: segment.length,
            estimated_encoded_bytes: segment.length,
            flags: 0,
            checksum: 0,
        })
        .collect::<Vec<_>>();
    let index = ScanSplitIndexV2 {
        header: ScanSplitIndexHeaderV2 {
            split_count: entries.len() as u32,
            flags: 0,
            checksum: 0,
        },
        entries,
    };
    validate_scan_split_authority(&index, table, segments, None)?;
    Ok(index)
}

pub fn build_default_layout_plan(
    table: &TableEntry,
    segments: &[TableSegmentIndexEntryV1],
    scan_splits: Option<&ScanSplitIndexV2>,
) -> Result<LayoutPlanV2, CoveError> {
    let mut nodes = Vec::with_capacity(segments.len() + 2);
    let segment_child_start = 2u32;
    nodes.push(LayoutPlanNodeV2 {
        node_id: 1,
        parent_node_id: ABSENT_ID,
        node_kind: 0,
        flags: 0,
        table_id: ABSENT_ID,
        column_id: ABSENT_ID,
        segment_id: ABSENT_ID,
        first_morsel_id: 0,
        morsel_count: 0,
        row_start: 0,
        row_count: table.row_count,
        section_id: 0,
        cluster_id: 0,
        first_child_index: 1,
        child_count: 1,
        stats_ref: ABSENT_ID,
        split_ref: ABSENT_ID,
        checksum: 0,
    });
    nodes.push(LayoutPlanNodeV2 {
        node_id: 2,
        parent_node_id: 1,
        node_kind: 1,
        flags: 0,
        table_id: table.table_id,
        column_id: ABSENT_ID,
        segment_id: ABSENT_ID,
        first_morsel_id: 0,
        morsel_count: 0,
        row_start: 0,
        row_count: table.row_count,
        section_id: 0,
        cluster_id: 0,
        first_child_index: segment_child_start,
        child_count: segments
            .iter()
            .filter(|segment| segment.table_id == table.table_id)
            .count() as u32,
        stats_ref: ABSENT_ID,
        split_ref: ABSENT_ID,
        checksum: 0,
    });
    for segment in segments
        .iter()
        .filter(|segment| segment.table_id == table.table_id)
    {
        let split_ref = scan_splits
            .and_then(|splits| {
                splits
                    .entries
                    .iter()
                    .find(|split| split.first_segment_id == segment.segment_id)
                    .map(|split| split.split_id)
            })
            .unwrap_or(ABSENT_ID);
        nodes.push(LayoutPlanNodeV2 {
            node_id: u32::try_from(nodes.len() + 1).map_err(|_| CoveError::ArithOverflow)?,
            parent_node_id: 2,
            node_kind: 3,
            flags: 0,
            table_id: table.table_id,
            column_id: ABSENT_ID,
            segment_id: segment.segment_id,
            first_morsel_id: 0,
            morsel_count: segment.morsel_count,
            row_start: segment.row_start,
            row_count: u64::from(segment.row_count),
            section_id: 0,
            cluster_id: 0,
            first_child_index: 0,
            child_count: 0,
            stats_ref: segment.stats_ref,
            split_ref,
            checksum: 0,
        });
    }
    let plan = LayoutPlanV2 {
        header: LayoutPlanHeaderV2 {
            layout_id: 1,
            node_count: nodes.len() as u32,
            root_node_id: 1,
            flags: 0,
            checksum: 0,
        },
        nodes,
    };
    validate_layout_nodes(&plan.header, &plan.nodes)?;
    Ok(plan)
}

fn section_by_id(footer: &CoveFooter, section_id: u32) -> Result<&CoveSectionEntryV1, CoveError> {
    if section_id == 0 {
        return Err(CoveError::BadLayoutPlan);
    }
    footer
        .sections
        .iter()
        .find(|section| section.section_id == section_id)
        .ok_or(CoveError::BadLayoutPlan)
}

pub fn validate_layout_nodes(
    header: &LayoutPlanHeaderV2,
    nodes: &[LayoutPlanNodeV2],
) -> Result<(), CoveError> {
    if header.node_count as usize != nodes.len() {
        return Err(CoveError::BadLayoutPlan);
    }
    let mut ids = BTreeSet::new();
    for node in nodes {
        if !ids.insert(node.node_id) {
            return Err(CoveError::BadLayoutPlan);
        }
        checked_end(node.row_start, node.row_count)?;
        let child_end = node
            .first_child_index
            .checked_add(node.child_count)
            .ok_or(CoveError::ArithOverflow)?;
        if child_end as usize > nodes.len() {
            return Err(CoveError::BadLayoutPlan);
        }
    }
    if !ids.contains(&header.root_node_id) {
        return Err(CoveError::BadLayoutPlan);
    }
    for node in nodes {
        if node.node_id == header.root_node_id {
            if node.parent_node_id != ABSENT_ID || node.node_kind != 0 {
                return Err(CoveError::BadLayoutPlan);
            }
        } else if !ids.contains(&node.parent_node_id) {
            return Err(CoveError::BadLayoutPlan);
        }
    }
    for node in nodes {
        let mut parent = node.parent_node_id;
        let mut hops = 0usize;
        while parent != ABSENT_ID {
            hops += 1;
            if hops > nodes.len() {
                return Err(CoveError::BadLayoutPlan);
            }
            let parent_node = nodes
                .iter()
                .find(|candidate| candidate.node_id == parent)
                .ok_or(CoveError::BadLayoutPlan)?;
            parent = parent_node.parent_node_id;
        }
    }
    Ok(())
}

fn validate_layout_node_authority(
    node: &LayoutPlanNodeV2,
    footer: &CoveFooter,
    table: &TableEntry,
    segments: &[TableSegmentIndexEntryV1],
    page_clusters: Option<&PageClusterDirectoryV2>,
    scan_splits: Option<&ScanSplitIndexV2>,
) -> Result<(), CoveError> {
    if !is_absent_ref(node.table_id) && node.table_id != table.table_id {
        return Err(CoveError::BadLayoutPlan);
    }
    if !is_absent_ref(node.column_id)
        && !table
            .columns
            .iter()
            .any(|column| column.column_id == node.column_id)
    {
        return Err(CoveError::BadLayoutPlan);
    }
    let segment = if !is_absent_ref(node.segment_id) {
        Some(segment_by_id(segments, table.table_id, node.segment_id)?)
    } else {
        None
    };
    if let Some(segment) = segment {
        if node.morsel_count != 0 {
            let morsel_end = node
                .first_morsel_id
                .checked_add(node.morsel_count)
                .ok_or(CoveError::ArithOverflow)?;
            if morsel_end > segment.morsel_count {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        if node.row_count != 0 {
            let row_end = checked_end(node.row_start, node.row_count)?;
            let segment_end = checked_end(segment.row_start, u64::from(segment.row_count))?;
            if node.row_start < segment.row_start || row_end > segment_end {
                return Err(CoveError::BadLayoutPlan);
            }
        }
    } else if node.row_count != 0 {
        let row_end = checked_end(node.row_start, node.row_count)?;
        if row_end > table.row_count {
            return Err(CoveError::BadLayoutPlan);
        }
    }
    if !is_absent_ref(node.section_id) {
        section_by_id(footer, node.section_id)?;
    }
    if !is_absent_ref(node.cluster_id) {
        let Some(directory) = page_clusters else {
            return Err(CoveError::BadLayoutPlan);
        };
        if !directory
            .entries
            .iter()
            .any(|cluster| cluster.cluster_id == node.cluster_id)
        {
            return Err(CoveError::BadLayoutPlan);
        }
    }
    if !is_absent_ref(node.split_ref) {
        let Some(splits) = scan_splits else {
            return Err(CoveError::BadLayoutPlan);
        };
        if !splits
            .entries
            .iter()
            .any(|split| split.split_id == node.split_ref)
        {
            return Err(CoveError::BadLayoutPlan);
        }
    }
    match node.node_kind {
        0 => {
            if node.parent_node_id != ABSENT_ID {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        1 => {
            if node.table_id != table.table_id {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        3 => {
            if segment.is_none() {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        4 => {
            if segment.is_none() || node.morsel_count == 0 {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        5 => {
            if is_absent_ref(node.column_id) {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        6 => {
            if is_absent_ref(node.cluster_id) {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        7 => {
            if is_absent_ref(node.section_id) {
                return Err(CoveError::BadLayoutPlan);
            }
        }
        2 | 255 => {}
        _ => return Err(CoveError::BadLayoutPlan),
    }
    Ok(())
}

fn segment_by_id(
    segments: &[TableSegmentIndexEntryV1],
    table_id: u32,
    segment_id: u32,
) -> Result<&TableSegmentIndexEntryV1, CoveError> {
    segments
        .iter()
        .find(|segment| segment.table_id == table_id && segment.segment_id == segment_id)
        .ok_or(CoveError::BadLayoutPlan)
}

fn segment_morsel_row_start(
    segment: &TableSegmentIndexEntryV1,
    morsel_id: u32,
) -> Result<u64, CoveError> {
    if morsel_id >= segment.morsel_count {
        return Err(CoveError::BadLayoutPlan);
    }
    let offset = u64::from(morsel_id)
        .checked_mul(u64::from(segment.morsel_row_count))
        .ok_or(CoveError::ArithOverflow)?;
    segment
        .row_start
        .checked_add(offset)
        .ok_or(CoveError::ArithOverflow)
}

fn segment_morsel_row_count(
    segment: &TableSegmentIndexEntryV1,
    morsel_id: u32,
) -> Result<u64, CoveError> {
    let start = u64::from(morsel_id)
        .checked_mul(u64::from(segment.morsel_row_count))
        .ok_or(CoveError::ArithOverflow)?;
    if start >= u64::from(segment.row_count) {
        return Err(CoveError::BadLayoutPlan);
    }
    let remaining = u64::from(segment.row_count)
        .checked_sub(start)
        .ok_or(CoveError::ArithOverflow)?;
    Ok(remaining.min(u64::from(segment.morsel_row_count)))
}

fn segment_morsel_span_rows(
    segment: &TableSegmentIndexEntryV1,
    first_morsel_id: u32,
    morsel_count: u32,
) -> Result<u64, CoveError> {
    let mut rows = 0u64;
    let end = first_morsel_id
        .checked_add(morsel_count)
        .ok_or(CoveError::ArithOverflow)?;
    if end > segment.morsel_count {
        return Err(CoveError::BadLayoutPlan);
    }
    for morsel_id in first_morsel_id..end {
        rows = rows
            .checked_add(segment_morsel_row_count(segment, morsel_id)?)
            .ok_or(CoveError::ArithOverflow)?;
    }
    Ok(rows)
}

fn is_absent_ref(value: u32) -> bool {
    value == 0 || value == ABSENT_ID
}

fn verify_crc(bytes: &[u8], checksum_offset: usize, expected: u32) -> Result<(), CoveError> {
    let mut check = bytes.to_vec();
    check[checksum_offset..checksum_offset + 4].fill(0);
    if checksum::crc32c(&check) != expected {
        return Err(CoveError::ChecksumMismatch);
    }
    Ok(())
}

fn parse_utf8(bytes: &[u8]) -> Result<String, CoveError> {
    std::str::from_utf8(bytes)
        .map(|value| value.to_string())
        .map_err(|_| CoveError::BadLayoutPlan)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], CoveError> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(CoveError::ArithOverflow)?;
        if end > self.bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, CoveError> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, CoveError> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, CoveError> {
    if offset >= bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    Ok(bytes[offset])
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, CoveError> {
    Ok(u16::from_le_bytes(read_array(bytes, offset)?))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, CoveError> {
    Ok(u32::from_le_bytes(read_array(bytes, offset)?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, CoveError> {
    Ok(u64::from_le_bytes(read_array(bytes, offset)?))
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], CoveError> {
    let end = offset.checked_add(N).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    Ok(bytes[offset..end].try_into().unwrap())
}

fn checked_end(offset: u64, length: u64) -> Result<u64, CoveError> {
    offset.checked_add(length).ok_or(CoveError::ArithOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_node() -> LayoutPlanNodeV2 {
        LayoutPlanNodeV2 {
            node_id: 1,
            parent_node_id: ABSENT_ID,
            node_kind: 0,
            flags: 0,
            table_id: ABSENT_ID,
            column_id: ABSENT_ID,
            segment_id: ABSENT_ID,
            first_morsel_id: 0,
            morsel_count: 0,
            row_start: 0,
            row_count: 0,
            section_id: 0,
            cluster_id: 0,
            first_child_index: 0,
            child_count: 0,
            stats_ref: ABSENT_ID,
            split_ref: ABSENT_ID,
            checksum: 0,
        }
    }

    #[test]
    fn layout_plan_round_trips() {
        let plan = LayoutPlanV2 {
            header: LayoutPlanHeaderV2 {
                layout_id: 7,
                node_count: 1,
                root_node_id: 1,
                flags: 0,
                checksum: 0,
            },
            nodes: vec![root_node()],
        };
        let bytes = plan.serialize().unwrap();
        let parsed = LayoutPlanV2::parse(&bytes).unwrap();
        assert_eq!(parsed.header.layout_id, 7);
        assert_eq!(parsed.nodes.len(), 1);
    }

    #[test]
    fn layout_plan_rejects_duplicate_nodes() {
        let node = root_node();
        let header = LayoutPlanHeaderV2 {
            layout_id: 7,
            node_count: 2,
            root_node_id: 1,
            flags: 0,
            checksum: 0,
        };
        assert!(matches!(
            validate_layout_nodes(&header, &[node.clone(), node]),
            Err(CoveError::BadLayoutPlan)
        ));
    }

    fn scan_split(split_id: u32) -> ScanSplitEntryV2 {
        ScanSplitEntryV2 {
            split_id,
            table_id: 1,
            row_start: 0,
            row_count: 1024,
            first_segment_id: 1,
            segment_count: 1,
            first_morsel_id: 0,
            morsel_count: 4,
            first_cluster_id: 0,
            cluster_count: 1,
            stats_ref: ABSENT_ID,
            estimated_uncompressed_bytes: 8192,
            estimated_encoded_bytes: 2048,
            flags: 0,
            checksum: 0,
        }
    }

    #[test]
    fn scan_split_index_round_trips() {
        let index = ScanSplitIndexV2 {
            header: ScanSplitIndexHeaderV2 {
                split_count: 2,
                flags: 0,
                checksum: 0,
            },
            entries: vec![scan_split(1), scan_split(2)],
        };
        let bytes = index.serialize().unwrap();
        let parsed = ScanSplitIndexV2::parse(&bytes).unwrap();
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].estimated_encoded_bytes, 2048);
    }

    #[test]
    fn scan_split_index_rejects_duplicate_split_ids() {
        let index = ScanSplitIndexV2 {
            header: ScanSplitIndexHeaderV2 {
                split_count: 2,
                flags: 0,
                checksum: 0,
            },
            entries: vec![scan_split(1), scan_split(1)],
        };
        assert!(matches!(index.serialize(), Err(CoveError::BadLayoutPlan)));
    }

    #[test]
    fn scan_split_index_rejects_bad_checksum() {
        let index = ScanSplitIndexV2 {
            header: ScanSplitIndexHeaderV2 {
                split_count: 1,
                flags: 0,
                checksum: 0,
            },
            entries: vec![scan_split(1)],
        };
        let mut bytes = index.serialize().unwrap();
        bytes[ScanSplitIndexHeaderV2::LEN + 4] ^= 1;
        assert!(matches!(
            ScanSplitIndexV2::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        ));
    }

    fn zero_copy_entry() -> ZeroCopyBufferMapEntryV2 {
        ZeroCopyBufferMapEntryV2 {
            target_id: 1,
            table_id: 1,
            column_id: 1,
            segment_id: 1,
            morsel_id: 0,
            page_ref: 1,
            buffer_id: 0,
            buffer_kind: 0,
            logical_type: 7,
            physical_kind: 1,
            source_endianness: 0,
            required_alignment_log2: 3,
            null_bitmap_polarity: ZeroCopyNullBitmapPolarityV2::OneMeansNull,
            source_offset_width_bits: 0,
            target_offset_width_bits: 0,
            dictionary_key_width_bits: 0,
            dictionary_semantics: ZeroCopyDictionarySemanticsV2::NoDictionary,
            lifetime_scope: ZeroCopyLifetimeScopeV2::ReaderSession,
            nested_layout_kind: ZeroCopyNestedLayoutKindV2::NotNested,
            compression_required_none: 1,
            target_buffer_role: ZeroCopyTargetBufferRoleV2::Values,
            source_buffer_role: ZeroCopySourceBufferRoleV2::CoveValues,
            target_type_ref: u32::MAX,
            dictionary_values_ref: u32::MAX,
            child_layout_ref: u32::MAX,
            owner_lifetime_ref: u32::MAX,
            flags: 0,
            checksum: 0,
        }
    }

    fn zero_copy_map(entry: ZeroCopyBufferMapEntryV2) -> ZeroCopyBufferMapV2 {
        ZeroCopyBufferMapV2 {
            header: ZeroCopyBufferMapHeaderV2 {
                map_count: 1,
                target_count: 1,
                flags: 0,
                checksum: 0,
            },
            targets: vec![ZeroCopyTargetV2 {
                target_id: 1,
                namespace: "org.apache.arrow".into(),
                target_name: "arrow".into(),
                version_major: 1,
                version_minor: 0,
                flags: 0,
            }],
            entries: vec![entry],
        }
    }

    #[test]
    fn zero_copy_map_round_trips_and_is_compatible() {
        let bytes = zero_copy_map(zero_copy_entry()).serialize().unwrap();
        let parsed = ZeroCopyBufferMapV2::parse(&bytes).unwrap();
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(
            parsed.entries[0].compatibility(&ZeroCopyCompatibilityContext::default()),
            ZeroCopyCompatibilityV2::Compatible
        );
    }

    #[test]
    fn zero_copy_materialisation_reasons_are_reported() {
        let mut entry = zero_copy_entry();
        entry.compression_required_none = 0;
        assert_eq!(
            entry.compatibility(&ZeroCopyCompatibilityContext::default()),
            ZeroCopyCompatibilityV2::MaterializeRequired(
                ZeroCopyMaterializationReasonV2::CompressedBuffer
            )
        );

        let mut entry = zero_copy_entry();
        entry.null_bitmap_polarity = ZeroCopyNullBitmapPolarityV2::OneMeansValid;
        assert_eq!(
            entry.compatibility(&ZeroCopyCompatibilityContext {
                accepts_cove_null_bitmap_polarity: false,
                ..ZeroCopyCompatibilityContext::default()
            }),
            ZeroCopyCompatibilityV2::MaterializeRequired(
                ZeroCopyMaterializationReasonV2::NullPolarityMismatch
            )
        );

        let mut entry = zero_copy_entry();
        entry.dictionary_semantics = ZeroCopyDictionarySemanticsV2::RequiresRemap;
        assert_eq!(
            entry.compatibility(&ZeroCopyCompatibilityContext::default()),
            ZeroCopyCompatibilityV2::MaterializeRequired(
                ZeroCopyMaterializationReasonV2::DictionaryMismatch
            )
        );

        let mut entry = zero_copy_entry();
        entry.nested_layout_kind = ZeroCopyNestedLayoutKindV2::CoveNativeNested;
        assert_eq!(
            entry.compatibility(&ZeroCopyCompatibilityContext::default()),
            ZeroCopyCompatibilityV2::MaterializeRequired(
                ZeroCopyMaterializationReasonV2::NestedLayoutMismatch
            )
        );

        let mut entry = zero_copy_entry();
        entry.lifetime_scope = ZeroCopyLifetimeScopeV2::Page;
        assert_eq!(
            entry.compatibility(&ZeroCopyCompatibilityContext {
                required_lifetime_scope: ZeroCopyLifetimeScopeV2::ReaderSession,
                ..ZeroCopyCompatibilityContext::default()
            }),
            ZeroCopyCompatibilityV2::MaterializeRequired(
                ZeroCopyMaterializationReasonV2::InsufficientLifetime
            )
        );

        assert_eq!(
            zero_copy_entry().compatibility(&ZeroCopyCompatibilityContext {
                active_visibility_overlay: true,
                ..ZeroCopyCompatibilityContext::default()
            }),
            ZeroCopyCompatibilityV2::MaterializeRequired(
                ZeroCopyMaterializationReasonV2::ActiveVisibilityOverlay
            )
        );

        let mut entry = zero_copy_entry();
        entry.target_buffer_role = ZeroCopyTargetBufferRoleV2::Extension;
        assert_eq!(
            entry.compatibility(&ZeroCopyCompatibilityContext::default()),
            ZeroCopyCompatibilityV2::MaterializeRequired(
                ZeroCopyMaterializationReasonV2::UnknownRole
            )
        );
    }

    fn authority_table() -> cove_core::table::TableEntry {
        cove_core::table::TableEntry {
            table_id: 1,
            namespace: String::new(),
            name: "t".into(),
            row_count: 1024,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![cove_core::table::ColumnEntry {
                column_id: 1,
                name: "c".into(),
                logical: CoveLogicalType::UInt16,
                physical: CovePhysicalKind::NumCode,
                nullable: false,
                sort_order: 0,
                collation_id: 0,
                precision: 0,
                scale: 0,
                flags: 0,
            }],
        }
    }

    fn authority_segments() -> Vec<TableSegmentIndexEntryV1> {
        vec![TableSegmentIndexEntryV1 {
            table_id: 1,
            segment_id: 1,
            row_start: 0,
            row_count: 1024,
            morsel_count: 4,
            morsel_row_count: 256,
            column_count: 1,
            offset: 4096,
            length: 8192,
            stats_ref: 0,
            flags: 0,
            checksum: 0,
        }]
    }

    fn authority_footer(sections: Vec<CoveSectionEntryV1>) -> CoveFooter {
        CoveFooter {
            header: cove_core::footer::CoveFooterHeaderV1 {
                footer_magic: *b"COVF",
                footer_version: 1,
                header_len: cove_core::constants::FOOTER_HEADER_LEN as u16,
                section_count: sections.len() as u32,
                section_entry_len: cove_core::constants::SECTION_ENTRY_LEN,
                flags: 0,
                metadata_len: 0,
                reserved: [0; 24],
            },
            sections,
            metadata_json: Vec::new(),
        }
    }

    fn authority_section(section_id: u32, kind: SectionKind) -> CoveSectionEntryV1 {
        CoveSectionEntryV1 {
            section_id,
            section_kind: kind as u16,
            profile: cove_core::constants::PrimaryProfile::LayoutPlanning as u8,
            flags: 0,
            offset: 1024,
            length: 128,
            uncompressed_length: 128,
            item_count: 1,
            row_count: 0,
            compression: cove_core::constants::CompressionCodec::None as u8,
            encryption: 0,
            alignment_log2: 0,
            reserved0: 0,
            required_features: 0,
            optional_features: 0,
            crc32c: 0,
            reserved1: 0,
        }
    }

    #[test]
    fn scan_split_authority_rejects_stale_ranges() {
        let table = authority_table();
        let segments = authority_segments();
        let mut index = build_default_scan_split_index(&table, &segments).unwrap();
        index.entries[0].row_count = 1023;
        assert!(matches!(
            validate_scan_split_authority(&index, &table, &segments, None),
            Err(CoveError::BadLayoutPlan)
        ));
    }

    #[test]
    fn layout_plan_authority_rejects_missing_section_refs() {
        let table = authority_table();
        let segments = authority_segments();
        let splits = build_default_scan_split_index(&table, &segments).unwrap();
        let mut plan = build_default_layout_plan(&table, &segments, Some(&splits)).unwrap();
        plan.nodes[2].section_id = 99;
        let footer = authority_footer(vec![authority_section(1, SectionKind::TableSegmentData)]);
        assert!(matches!(
            validate_layout_plan_authority(&plan, &footer, &table, &segments, None, Some(&splits)),
            Err(CoveError::BadLayoutPlan)
        ));
    }

    #[test]
    fn zero_copy_authority_rejects_missing_page_refs() {
        let table = authority_table();
        let segments = authority_segments();
        let mut entry = zero_copy_entry();
        entry.page_ref = ABSENT_ID;
        let map = zero_copy_map(entry);
        assert!(matches!(
            validate_zero_copy_map_authority(&map, &table, &segments),
            Err(CoveError::BadLayoutPlan)
        ));
    }

    #[test]
    fn default_covel_helpers_build_authoritative_metadata() {
        let table = authority_table();
        let segments = authority_segments();
        let splits = build_default_scan_split_index(&table, &segments).unwrap();
        validate_scan_split_authority(&splits, &table, &segments, None).unwrap();
        let plan = build_default_layout_plan(&table, &segments, Some(&splits)).unwrap();
        let footer = authority_footer(Vec::new());
        validate_layout_plan_authority(&plan, &footer, &table, &segments, None, Some(&splits))
            .unwrap();
    }
}
