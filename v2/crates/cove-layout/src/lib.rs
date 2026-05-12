//! COVE-L layout plans, scan splits, and scheduling metadata for COVE v2.

use std::collections::BTreeSet;

use cove_core::{checksum, CoveError};

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
    Ok(())
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
}
