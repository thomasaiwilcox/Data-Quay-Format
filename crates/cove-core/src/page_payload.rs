//! Spec §20.2 / §27.3 — self-describing column page payloads.
//!
//! A column page payload is the page-local container that lets a generic
//! reader reconstruct the encoding tree and locate each physical buffer
//! without out-of-band writer state.

use crate::{
    checksum,
    constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
    CoveError,
};

pub const COLUMN_PAGE_PAYLOAD_MAGIC: [u8; 4] = *b"CPG1";
pub const COLUMN_PAGE_PAYLOAD_HEADER_LEN: usize = 36;
pub const COVE_ENCODING_NODE_LEN: usize = 30;
pub const PAGE_BUFFER_DESCRIPTOR_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
#[non_exhaustive]
pub enum PageBufferKind {
    NullBitmap = 0,
    Values = 1,
    Offsets = 2,
    ChildLayout = 3,
    Other = 255,
}

impl PageBufferKind {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::NullBitmap),
            1 => Some(Self::Values),
            2 => Some(Self::Offsets),
            3 => Some(Self::ChildLayout),
            255 => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnPagePayloadHeaderV1 {
    pub magic: [u8; 4],
    pub version_major: u16,
    pub header_len: u16,
    pub flags: u16,
    pub root_node_id: u16,
    pub node_count: u16,
    pub buffer_count: u16,
    pub row_count: u32,
    pub nodes_offset: u32,
    pub buffer_directory_offset: u32,
    pub buffers_offset: u32,
    pub reserved: u32,
}

impl ColumnPagePayloadHeaderV1 {
    pub fn serialize(&self) -> [u8; COLUMN_PAGE_PAYLOAD_HEADER_LEN] {
        let mut out = [0u8; COLUMN_PAGE_PAYLOAD_HEADER_LEN];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version_major.to_le_bytes());
        out[6..8].copy_from_slice(&self.header_len.to_le_bytes());
        out[8..10].copy_from_slice(&self.flags.to_le_bytes());
        out[10..12].copy_from_slice(&self.root_node_id.to_le_bytes());
        out[12..14].copy_from_slice(&self.node_count.to_le_bytes());
        out[14..16].copy_from_slice(&self.buffer_count.to_le_bytes());
        out[16..20].copy_from_slice(&self.row_count.to_le_bytes());
        out[20..24].copy_from_slice(&self.nodes_offset.to_le_bytes());
        out[24..28].copy_from_slice(&self.buffer_directory_offset.to_le_bytes());
        out[28..32].copy_from_slice(&self.buffers_offset.to_le_bytes());
        out[32..36].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < COLUMN_PAGE_PAYLOAD_HEADER_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..COLUMN_PAGE_PAYLOAD_HEADER_LEN];
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[0..4]);
        if magic != COLUMN_PAGE_PAYLOAD_MAGIC {
            return Err(CoveError::BadMagic);
        }
        let version_major = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if version_major != 1 {
            return Err(CoveError::BadVersion);
        }
        let header_len = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        if header_len as usize != COLUMN_PAGE_PAYLOAD_HEADER_LEN {
            return Err(CoveError::BadSection(format!(
                "column page payload header_len must be {COLUMN_PAGE_PAYLOAD_HEADER_LEN}, got {header_len}"
            )));
        }
        let flags = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        if flags != 0 {
            return Err(CoveError::BadSection(
                "column page payload flags are reserved and must be zero".into(),
            ));
        }
        let reserved = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
        if reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        Ok(Self {
            magic,
            version_major,
            header_len,
            flags,
            root_node_id: u16::from_le_bytes(bytes[10..12].try_into().unwrap()),
            node_count: u16::from_le_bytes(bytes[12..14].try_into().unwrap()),
            buffer_count: u16::from_le_bytes(bytes[14..16].try_into().unwrap()),
            row_count: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            nodes_offset: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            buffer_directory_offset: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            buffers_offset: u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
            reserved,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveEncodingNodeV1 {
    pub node_id: u16,
    pub encoding_kind: CoveEncodingKind,
    pub logical_type: CoveLogicalType,
    pub physical_kind: CovePhysicalKind,
    pub flags: u8,
    pub logical_len: u32,
    pub child_count: u16,
    pub buffer_count: u16,
    pub params_offset: u32,
    pub params_length: u32,
    pub stats_id: u32,
    pub reserved: u16,
}

impl CoveEncodingNodeV1 {
    pub fn serialize(&self) -> [u8; COVE_ENCODING_NODE_LEN] {
        let mut out = [0u8; COVE_ENCODING_NODE_LEN];
        out[0..2].copy_from_slice(&self.node_id.to_le_bytes());
        out[2..4].copy_from_slice(&(self.encoding_kind as u16).to_le_bytes());
        out[4..6].copy_from_slice(&(self.logical_type as u16).to_le_bytes());
        out[6] = self.physical_kind as u8;
        out[7] = self.flags;
        out[8..12].copy_from_slice(&self.logical_len.to_le_bytes());
        out[12..14].copy_from_slice(&self.child_count.to_le_bytes());
        out[14..16].copy_from_slice(&self.buffer_count.to_le_bytes());
        out[16..20].copy_from_slice(&self.params_offset.to_le_bytes());
        out[20..24].copy_from_slice(&self.params_length.to_le_bytes());
        out[24..28].copy_from_slice(&self.stats_id.to_le_bytes());
        out[28..30].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < COVE_ENCODING_NODE_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..COVE_ENCODING_NODE_LEN];
        let encoding_raw = u16::from_le_bytes(bytes[2..4].try_into().unwrap());
        let logical_raw = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        let physical_raw = bytes[6];
        let encoding_kind = CoveEncodingKind::from_u16(encoding_raw).ok_or_else(|| {
            CoveError::UnsupportedEncoding(format!("unknown encoding kind {encoding_raw}"))
        })?;
        let logical_type = CoveLogicalType::from_u16(logical_raw)
            .ok_or_else(|| CoveError::BadSchema(format!("unknown logical type {logical_raw}")))?;
        let physical_kind = CovePhysicalKind::from_u8(physical_raw)
            .ok_or_else(|| CoveError::BadSchema(format!("unknown physical kind {physical_raw}")))?;
        let reserved = u16::from_le_bytes(bytes[28..30].try_into().unwrap());
        if reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        Ok(Self {
            node_id: u16::from_le_bytes(bytes[0..2].try_into().unwrap()),
            encoding_kind,
            logical_type,
            physical_kind,
            flags: bytes[7],
            logical_len: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            child_count: u16::from_le_bytes(bytes[12..14].try_into().unwrap()),
            buffer_count: u16::from_le_bytes(bytes[14..16].try_into().unwrap()),
            params_offset: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            params_length: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            stats_id: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            reserved,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageBufferDescriptorV1 {
    pub buffer_id: u16,
    pub kind: PageBufferKind,
    pub flags: u32,
    pub offset: u64,
    pub length: u64,
    pub checksum: u32,
    pub reserved: u32,
}

impl PageBufferDescriptorV1 {
    pub fn serialize(&self) -> [u8; PAGE_BUFFER_DESCRIPTOR_LEN] {
        let mut out = [0u8; PAGE_BUFFER_DESCRIPTOR_LEN];
        out[0..2].copy_from_slice(&self.buffer_id.to_le_bytes());
        out[2..4].copy_from_slice(&(self.kind as u16).to_le_bytes());
        out[4..8].copy_from_slice(&self.flags.to_le_bytes());
        out[8..16].copy_from_slice(&self.offset.to_le_bytes());
        out[16..24].copy_from_slice(&self.length.to_le_bytes());
        out[24..28].copy_from_slice(&self.checksum.to_le_bytes());
        out[28..32].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < PAGE_BUFFER_DESCRIPTOR_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..PAGE_BUFFER_DESCRIPTOR_LEN];
        let kind_raw = u16::from_le_bytes(bytes[2..4].try_into().unwrap());
        let kind = PageBufferKind::from_u16(kind_raw)
            .ok_or_else(|| CoveError::BadSection(format!("unknown page buffer kind {kind_raw}")))?;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if flags != 0 {
            return Err(CoveError::BadSection(
                "page buffer flags are reserved and must be zero".into(),
            ));
        }
        let reserved = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        if reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        Ok(Self {
            buffer_id: u16::from_le_bytes(bytes[0..2].try_into().unwrap()),
            kind,
            flags,
            offset: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            length: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            checksum: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            reserved,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnPagePayloadV1 {
    pub header: ColumnPagePayloadHeaderV1,
    pub nodes: Vec<CoveEncodingNodeV1>,
    pub buffers: Vec<PageBufferDescriptorV1>,
    pub data: Vec<u8>,
}

impl ColumnPagePayloadV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = ColumnPagePayloadHeaderV1::parse(bytes)?;
        let nodes_offset =
            usize::try_from(header.nodes_offset).map_err(|_| CoveError::OffsetRange)?;
        let buffer_directory_offset =
            usize::try_from(header.buffer_directory_offset).map_err(|_| CoveError::OffsetRange)?;
        let buffers_offset =
            usize::try_from(header.buffers_offset).map_err(|_| CoveError::OffsetRange)?;
        if nodes_offset != COLUMN_PAGE_PAYLOAD_HEADER_LEN
            || buffer_directory_offset < nodes_offset
            || buffers_offset < buffer_directory_offset
            || buffers_offset > bytes.len()
        {
            return Err(CoveError::PageCorrupt);
        }
        let node_region_len = (header.node_count as usize)
            .checked_mul(COVE_ENCODING_NODE_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let expected_buffer_directory_offset = nodes_offset
            .checked_add(node_region_len)
            .ok_or(CoveError::ArithOverflow)?;
        if buffer_directory_offset != expected_buffer_directory_offset {
            return Err(CoveError::PageCorrupt);
        }
        let buffer_region_len = (header.buffer_count as usize)
            .checked_mul(PAGE_BUFFER_DESCRIPTOR_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let expected_buffers_offset = buffer_directory_offset
            .checked_add(buffer_region_len)
            .ok_or(CoveError::ArithOverflow)?;
        if buffers_offset != expected_buffers_offset {
            return Err(CoveError::PageCorrupt);
        }
        if header.node_count == 0 {
            return Err(CoveError::PageCorrupt);
        }

        let mut nodes = Vec::with_capacity(header.node_count as usize);
        let mut pos = nodes_offset;
        for _ in 0..header.node_count {
            nodes.push(CoveEncodingNodeV1::parse(
                &bytes[pos..pos + COVE_ENCODING_NODE_LEN],
            )?);
            pos += COVE_ENCODING_NODE_LEN;
        }
        let mut seen_node_ids = std::collections::BTreeSet::new();
        for node in &nodes {
            if !seen_node_ids.insert(node.node_id) {
                return Err(CoveError::PageCorrupt);
            }
        }

        let mut roots = nodes
            .iter()
            .filter(|node| node.node_id == header.root_node_id);
        let root = roots.next().ok_or(CoveError::PageCorrupt)?;
        if roots.next().is_some() {
            return Err(CoveError::PageCorrupt);
        }
        if root.logical_len != header.row_count {
            return Err(CoveError::PageCorrupt);
        }

        let mut buffers = Vec::with_capacity(header.buffer_count as usize);
        let mut pos = buffer_directory_offset;
        let mut previous_end = buffers_offset as u64;
        for expected_id in 0..header.buffer_count {
            let descriptor =
                PageBufferDescriptorV1::parse(&bytes[pos..pos + PAGE_BUFFER_DESCRIPTOR_LEN])?;
            if descriptor.buffer_id != expected_id {
                return Err(CoveError::PageCorrupt);
            }
            if descriptor.offset < buffers_offset as u64 || descriptor.offset < previous_end {
                return Err(CoveError::PageCorrupt);
            }
            let end = descriptor
                .offset
                .checked_add(descriptor.length)
                .ok_or(CoveError::ArithOverflow)?;
            let end_usize = usize::try_from(end).map_err(|_| CoveError::OffsetRange)?;
            if end_usize > bytes.len() {
                return Err(CoveError::OffsetRange);
            }
            let start = usize::try_from(descriptor.offset).map_err(|_| CoveError::OffsetRange)?;
            if checksum::crc32c(&bytes[start..end_usize]) != descriptor.checksum {
                return Err(CoveError::ChecksumMismatch);
            }
            previous_end = end;
            buffers.push(descriptor);
            pos += PAGE_BUFFER_DESCRIPTOR_LEN;
        }
        if previous_end != bytes.len() as u64 {
            return Err(CoveError::PageCorrupt);
        }

        Ok(Self {
            header,
            nodes,
            buffers,
            data: bytes.to_vec(),
        })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        if self.nodes.is_empty() {
            return Err(CoveError::PageCorrupt);
        }
        let mut out = Vec::with_capacity(
            COLUMN_PAGE_PAYLOAD_HEADER_LEN
                + self.nodes.len() * COVE_ENCODING_NODE_LEN
                + self.buffers.len() * PAGE_BUFFER_DESCRIPTOR_LEN,
        );
        out.extend_from_slice(&self.header.serialize());
        for node in &self.nodes {
            out.extend_from_slice(&node.serialize());
        }
        for buffer in &self.buffers {
            out.extend_from_slice(&buffer.serialize());
        }
        for buffer in &self.buffers {
            let start = usize::try_from(buffer.offset).map_err(|_| CoveError::OffsetRange)?;
            let end = usize::try_from(
                buffer
                    .offset
                    .checked_add(buffer.length)
                    .ok_or(CoveError::ArithOverflow)?,
            )
            .map_err(|_| CoveError::OffsetRange)?;
            if end > self.data.len() {
                return Err(CoveError::OffsetRange);
            }
            out.extend_from_slice(&self.data[start..end]);
        }
        let parsed = Self::parse(&out)?;
        Ok(parsed.data)
    }

    pub fn build_single_node(
        row_count: u32,
        encoding_kind: CoveEncodingKind,
        logical_type: CoveLogicalType,
        physical_kind: CovePhysicalKind,
        null_bitmap: Option<Vec<u8>>,
        values: Vec<u8>,
    ) -> Result<Vec<u8>, CoveError> {
        let buffer_count = u16::from(null_bitmap.is_some()) + u16::from(!values.is_empty());
        let nodes_offset = COLUMN_PAGE_PAYLOAD_HEADER_LEN as u32;
        let buffer_directory_offset = nodes_offset
            .checked_add(COVE_ENCODING_NODE_LEN as u32)
            .ok_or(CoveError::ArithOverflow)?;
        let buffers_offset = buffer_directory_offset
            .checked_add(u32::from(buffer_count) * PAGE_BUFFER_DESCRIPTOR_LEN as u32)
            .ok_or(CoveError::ArithOverflow)?;
        let header = ColumnPagePayloadHeaderV1 {
            magic: COLUMN_PAGE_PAYLOAD_MAGIC,
            version_major: 1,
            header_len: COLUMN_PAGE_PAYLOAD_HEADER_LEN as u16,
            flags: 0,
            root_node_id: 0,
            node_count: 1,
            buffer_count,
            row_count,
            nodes_offset,
            buffer_directory_offset,
            buffers_offset,
            reserved: 0,
        };
        let node = CoveEncodingNodeV1 {
            node_id: 0,
            encoding_kind,
            logical_type,
            physical_kind,
            flags: 0,
            logical_len: row_count,
            child_count: 0,
            buffer_count,
            params_offset: 0,
            params_length: 0,
            stats_id: 0,
            reserved: 0,
        };
        let mut out = Vec::new();
        out.extend_from_slice(&header.serialize());
        out.extend_from_slice(&node.serialize());

        let mut descriptors = Vec::with_capacity(buffer_count as usize);
        let mut data_bytes = Vec::new();
        if let Some(null_bitmap) = null_bitmap {
            let offset = buffers_offset as u64 + data_bytes.len() as u64;
            let checksum = checksum::crc32c(&null_bitmap);
            descriptors.push(PageBufferDescriptorV1 {
                buffer_id: descriptors.len() as u16,
                kind: PageBufferKind::NullBitmap,
                flags: 0,
                offset,
                length: null_bitmap.len() as u64,
                checksum,
                reserved: 0,
            });
            data_bytes.extend_from_slice(&null_bitmap);
        }
        if !values.is_empty() {
            let offset = buffers_offset as u64 + data_bytes.len() as u64;
            let checksum = checksum::crc32c(&values);
            descriptors.push(PageBufferDescriptorV1 {
                buffer_id: descriptors.len() as u16,
                kind: PageBufferKind::Values,
                flags: 0,
                offset,
                length: values.len() as u64,
                checksum,
                reserved: 0,
            });
            data_bytes.extend_from_slice(&values);
        }
        for descriptor in &descriptors {
            out.extend_from_slice(&descriptor.serialize());
        }
        out.extend_from_slice(&data_bytes);
        Self::parse(&out)?;
        Ok(out)
    }

    pub fn root_node(&self) -> Result<&CoveEncodingNodeV1, CoveError> {
        let mut roots = self
            .nodes
            .iter()
            .filter(|node| node.node_id == self.header.root_node_id);
        let root = roots.next().ok_or(CoveError::PageCorrupt)?;
        if roots.next().is_some() {
            return Err(CoveError::PageCorrupt);
        }
        Ok(root)
    }

    pub fn buffer_bytes(&self, kind: PageBufferKind) -> Result<Option<&[u8]>, CoveError> {
        let Some(buffer) = self.buffers.iter().find(|buffer| buffer.kind == kind) else {
            return Ok(None);
        };
        let start = usize::try_from(buffer.offset).map_err(|_| CoveError::OffsetRange)?;
        let end = usize::try_from(
            buffer
                .offset
                .checked_add(buffer.length)
                .ok_or(CoveError::ArithOverflow)?,
        )
        .map_err(|_| CoveError::OffsetRange)?;
        Ok(Some(&self.data[start..end]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_node_payload_round_trips() {
        let bytes = ColumnPagePayloadV1::build_single_node(
            3,
            CoveEncodingKind::PlainFixed,
            CoveLogicalType::Int32,
            CovePhysicalKind::NumCode,
            Some(vec![0b0000_0010]),
            vec![1, 2, 3, 4],
        )
        .unwrap();
        let parsed = ColumnPagePayloadV1::parse(&bytes).unwrap();
        assert_eq!(parsed.header.row_count, 3);
        assert_eq!(
            parsed.root_node().unwrap().logical_type,
            CoveLogicalType::Int32
        );
        assert_eq!(
            parsed.buffer_bytes(PageBufferKind::Values).unwrap(),
            Some(&[1, 2, 3, 4][..])
        );
    }

    #[test]
    fn rejects_raw_legacy_payload() {
        assert_eq!(
            ColumnPagePayloadV1::parse(b"raw"),
            Err(CoveError::BufferTooShort)
        );
        let mut bytes = vec![0; COLUMN_PAGE_PAYLOAD_HEADER_LEN];
        bytes[0..4].copy_from_slice(b"RAW!");
        assert_eq!(ColumnPagePayloadV1::parse(&bytes), Err(CoveError::BadMagic));
    }

    #[test]
    fn rejects_buffer_checksum_mismatch() {
        let mut bytes = ColumnPagePayloadV1::build_single_node(
            1,
            CoveEncodingKind::PlainFixed,
            CoveLogicalType::Bool,
            CovePhysicalKind::Boolean,
            None,
            vec![1],
        )
        .unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        assert_eq!(
            ColumnPagePayloadV1::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        );
    }

    #[test]
    fn rejects_nonzero_node_reserved_padding() {
        let mut bytes = ColumnPagePayloadV1::build_single_node(
            1,
            CoveEncodingKind::PlainFixed,
            CoveLogicalType::Bool,
            CovePhysicalKind::Boolean,
            None,
            vec![1],
        )
        .unwrap();
        let reserved_offset = COLUMN_PAGE_PAYLOAD_HEADER_LEN + 28;
        bytes[reserved_offset] = 1;
        assert_eq!(
            ColumnPagePayloadV1::parse(&bytes),
            Err(CoveError::ReservedNotZero)
        );
    }

    #[test]
    fn rejects_duplicate_node_ids() {
        let header = ColumnPagePayloadHeaderV1 {
            magic: COLUMN_PAGE_PAYLOAD_MAGIC,
            version_major: 1,
            header_len: COLUMN_PAGE_PAYLOAD_HEADER_LEN as u16,
            flags: 0,
            root_node_id: 0,
            node_count: 2,
            buffer_count: 0,
            row_count: 1,
            nodes_offset: COLUMN_PAGE_PAYLOAD_HEADER_LEN as u32,
            buffer_directory_offset: (COLUMN_PAGE_PAYLOAD_HEADER_LEN + 2 * COVE_ENCODING_NODE_LEN)
                as u32,
            buffers_offset: (COLUMN_PAGE_PAYLOAD_HEADER_LEN + 2 * COVE_ENCODING_NODE_LEN) as u32,
            reserved: 0,
        };
        let node = CoveEncodingNodeV1 {
            node_id: 0,
            encoding_kind: CoveEncodingKind::PlainFixed,
            logical_type: CoveLogicalType::Bool,
            physical_kind: CovePhysicalKind::Boolean,
            flags: 0,
            logical_len: 1,
            child_count: 0,
            buffer_count: 0,
            params_offset: 0,
            params_length: 0,
            stats_id: 0,
            reserved: 0,
        };
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header.serialize());
        bytes.extend_from_slice(&node.serialize());
        bytes.extend_from_slice(&node.serialize());
        assert_eq!(
            ColumnPagePayloadV1::parse(&bytes),
            Err(CoveError::PageCorrupt)
        );
    }
}
