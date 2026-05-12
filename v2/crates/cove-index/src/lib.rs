//! COVE-I `.covi` secondary-index artifacts for COVE v2.

use std::collections::BTreeSet;

use cove_core::{
    checksum,
    constants::{
        CompressionCodec, KNOWN_FEATURE_BITS_MASK, MAGIC_COVI, POSTSCRIPT_VERSION_V1,
        VERSION_MAJOR_V1,
    },
    CoveError,
};
use cove_coverage::CoverageProofStrengthV2;

pub const COVI_POSTSCRIPT_LEN: usize = 44;
pub const COVI_TAIL_LEN: usize = COVI_POSTSCRIPT_LEN + 2 + 2 + 4;
pub const COVI_HEADER_LEN: u16 = 170;
pub const COVI_SECTION_ENTRY_LEN: usize = 68;
pub const INDEX_CAPABILITY_LEN: usize = 40;
pub const INDEX_ONLY_CAPABILITY_LEN: usize = 22;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviPostscriptV2 {
    pub required_features: u64,
    pub optional_features: u64,
    pub file_len: u64,
    pub header_offset: u64,
    pub header_length: u64,
    pub checksum: u32,
}

impl CoviPostscriptV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < COVI_POSTSCRIPT_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let ps = Self {
            required_features: read_u64(bytes, 0)?,
            optional_features: read_u64(bytes, 8)?,
            file_len: read_u64(bytes, 16)?,
            header_offset: read_u64(bytes, 24)?,
            header_length: read_u64(bytes, 32)?,
            checksum: read_u32(bytes, 40)?,
        };
        let unknown_required = ps.required_features & !KNOWN_FEATURE_BITS_MASK;
        if unknown_required != 0 {
            return Err(CoveError::UnknownRequiredFeature(unknown_required));
        }
        verify_crc(&bytes[..COVI_POSTSCRIPT_LEN], 40, ps.checksum)?;
        Ok(ps)
    }

    pub fn serialize(&self) -> [u8; COVI_POSTSCRIPT_LEN] {
        let mut out = [0u8; COVI_POSTSCRIPT_LEN];
        out[0..8].copy_from_slice(&self.required_features.to_le_bytes());
        out[8..16].copy_from_slice(&self.optional_features.to_le_bytes());
        out[16..24].copy_from_slice(&self.file_len.to_le_bytes());
        out[24..32].copy_from_slice(&self.header_offset.to_le_bytes());
        out[32..40].copy_from_slice(&self.header_length.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[40..44].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviHeaderV2 {
    pub magic: [u8; 4],
    pub header_len: u16,
    pub version_major: u16,
    pub version_minor: u16,
    pub flags: u32,
    pub index_artifact_id: [u8; 16],
    pub dataset_id: [u8; 16],
    pub snapshot_id: [u8; 16],
    pub section_count: u32,
    pub referenced_file_count: u32,
    pub snapshot_validity_count: u32,
    pub index_root_count: u32,
    pub capability_count: u32,
    pub section_directory_offset: u64,
    pub section_directory_length: u64,
    pub referenced_files_offset: u64,
    pub snapshot_validity_offset: u64,
    pub index_roots_offset: u64,
    pub capabilities_offset: u64,
    pub string_table_section_ref: u32,
    pub created_at_us: i64,
    pub reserved: [u8; 24],
    pub checksum: u32,
}

impl CoviHeaderV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < COVI_HEADER_LEN as usize {
            return Err(CoveError::BufferTooShort);
        }
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[0..4]);
        if magic != MAGIC_COVI {
            return Err(CoveError::BadMagic);
        }
        let header = Self {
            magic,
            header_len: read_u16(bytes, 4)?,
            version_major: read_u16(bytes, 6)?,
            version_minor: read_u16(bytes, 8)?,
            flags: read_u32(bytes, 10)?,
            index_artifact_id: read_uuid(bytes, 14)?,
            dataset_id: read_uuid(bytes, 30)?,
            snapshot_id: read_uuid(bytes, 46)?,
            section_count: read_u32(bytes, 62)?,
            referenced_file_count: read_u32(bytes, 66)?,
            snapshot_validity_count: read_u32(bytes, 70)?,
            index_root_count: read_u32(bytes, 74)?,
            capability_count: read_u32(bytes, 78)?,
            section_directory_offset: read_u64(bytes, 82)?,
            section_directory_length: read_u64(bytes, 90)?,
            referenced_files_offset: read_u64(bytes, 98)?,
            snapshot_validity_offset: read_u64(bytes, 106)?,
            index_roots_offset: read_u64(bytes, 114)?,
            capabilities_offset: read_u64(bytes, 122)?,
            string_table_section_ref: read_u32(bytes, 130)?,
            created_at_us: read_i64(bytes, 134)?,
            reserved: read_array(bytes, 142)?,
            checksum: read_u32(bytes, 166)?,
        };
        if header.header_len != COVI_HEADER_LEN {
            return Err(CoveError::BadCovi);
        }
        if header.version_major != VERSION_MAJOR_V1 {
            return Err(CoveError::BadVersion);
        }
        if header.reserved.iter().any(|byte| *byte != 0) {
            return Err(CoveError::ReservedNotZero);
        }
        verify_crc(&bytes[..COVI_HEADER_LEN as usize], 166, header.checksum)?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; COVI_HEADER_LEN as usize] {
        let mut out = [0u8; COVI_HEADER_LEN as usize];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.header_len.to_le_bytes());
        out[6..8].copy_from_slice(&self.version_major.to_le_bytes());
        out[8..10].copy_from_slice(&self.version_minor.to_le_bytes());
        out[10..14].copy_from_slice(&self.flags.to_le_bytes());
        out[14..30].copy_from_slice(&self.index_artifact_id);
        out[30..46].copy_from_slice(&self.dataset_id);
        out[46..62].copy_from_slice(&self.snapshot_id);
        out[62..66].copy_from_slice(&self.section_count.to_le_bytes());
        out[66..70].copy_from_slice(&self.referenced_file_count.to_le_bytes());
        out[70..74].copy_from_slice(&self.snapshot_validity_count.to_le_bytes());
        out[74..78].copy_from_slice(&self.index_root_count.to_le_bytes());
        out[78..82].copy_from_slice(&self.capability_count.to_le_bytes());
        out[82..90].copy_from_slice(&self.section_directory_offset.to_le_bytes());
        out[90..98].copy_from_slice(&self.section_directory_length.to_le_bytes());
        out[98..106].copy_from_slice(&self.referenced_files_offset.to_le_bytes());
        out[106..114].copy_from_slice(&self.snapshot_validity_offset.to_le_bytes());
        out[114..122].copy_from_slice(&self.index_roots_offset.to_le_bytes());
        out[122..130].copy_from_slice(&self.capabilities_offset.to_le_bytes());
        out[130..134].copy_from_slice(&self.string_table_section_ref.to_le_bytes());
        out[134..142].copy_from_slice(&self.created_at_us.to_le_bytes());
        out[142..166].copy_from_slice(&self.reserved);
        let crc = checksum::crc32c(&out);
        out[166..170].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CoviSectionKindV2 {
    ReferencedFiles = 0,
    SnapshotValidity = 1,
    StringTable = 2,
    IndexRoots = 3,
    IndexCapabilities = 4,
    KeyBlock = 5,
    EntryBlock = 6,
    PostingsBlock = 7,
    RowRangeBlock = 8,
    RowOrdinalSetBlock = 9,
    BitmapBlock = 10,
    AggregateAnswerBlock = 11,
    CoverageSetBlock = 12,
    DimensionalBucketBlock = 13,
    ObjectPathBlock = 14,
    ExtensionBlock = 255,
}

impl CoviSectionKindV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::ReferencedFiles),
            1 => Some(Self::SnapshotValidity),
            2 => Some(Self::StringTable),
            3 => Some(Self::IndexRoots),
            4 => Some(Self::IndexCapabilities),
            5 => Some(Self::KeyBlock),
            6 => Some(Self::EntryBlock),
            7 => Some(Self::PostingsBlock),
            8 => Some(Self::RowRangeBlock),
            9 => Some(Self::RowOrdinalSetBlock),
            10 => Some(Self::BitmapBlock),
            11 => Some(Self::AggregateAnswerBlock),
            12 => Some(Self::CoverageSetBlock),
            13 => Some(Self::DimensionalBucketBlock),
            14 => Some(Self::ObjectPathBlock),
            255 => Some(Self::ExtensionBlock),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviSectionEntryV2 {
    pub section_id: u32,
    pub section_kind: CoviSectionKindV2,
    pub flags: u16,
    pub offset: u64,
    pub length: u64,
    pub uncompressed_length: u64,
    pub item_count: u64,
    pub compression: u8,
    pub encryption: u8,
    pub alignment_log2: u8,
    pub reserved0: u8,
    pub required_features: u64,
    pub optional_features: u64,
    pub crc32c: u32,
    pub checksum: u32,
}

impl CoviSectionEntryV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < COVI_SECTION_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let section_kind =
            CoviSectionKindV2::from_u16(read_u16(bytes, 4)?).ok_or_else(|| CoveError::BadCovi)?;
        let entry = Self {
            section_id: read_u32(bytes, 0)?,
            section_kind,
            flags: read_u16(bytes, 6)?,
            offset: read_u64(bytes, 8)?,
            length: read_u64(bytes, 16)?,
            uncompressed_length: read_u64(bytes, 24)?,
            item_count: read_u64(bytes, 32)?,
            compression: read_u8(bytes, 40)?,
            encryption: read_u8(bytes, 41)?,
            alignment_log2: read_u8(bytes, 42)?,
            reserved0: read_u8(bytes, 43)?,
            required_features: read_u64(bytes, 44)?,
            optional_features: read_u64(bytes, 52)?,
            crc32c: read_u32(bytes, 60)?,
            checksum: read_u32(bytes, 64)?,
        };
        if CompressionCodec::from_u8(entry.compression).is_none() {
            return Err(CoveError::BadCovi);
        }
        if entry.encryption != 0 || entry.reserved0 != 0 {
            return Err(CoveError::BadCovi);
        }
        let unknown_required = entry.required_features & !KNOWN_FEATURE_BITS_MASK;
        if unknown_required != 0 {
            return Err(CoveError::UnknownRequiredFeature(unknown_required));
        }
        verify_crc(&bytes[..COVI_SECTION_ENTRY_LEN], 64, entry.checksum)?;
        Ok(entry)
    }

    pub fn serialize(&self) -> Result<[u8; COVI_SECTION_ENTRY_LEN], CoveError> {
        if CompressionCodec::from_u8(self.compression).is_none() {
            return Err(CoveError::BadCovi);
        }
        if self.encryption != 0 || self.reserved0 != 0 {
            return Err(CoveError::BadCovi);
        }
        let unknown_required = self.required_features & !KNOWN_FEATURE_BITS_MASK;
        if unknown_required != 0 {
            return Err(CoveError::UnknownRequiredFeature(unknown_required));
        }
        self.end_offset()?;

        let mut out = [0u8; COVI_SECTION_ENTRY_LEN];
        out[0..4].copy_from_slice(&self.section_id.to_le_bytes());
        out[4..6].copy_from_slice(&(self.section_kind as u16).to_le_bytes());
        out[6..8].copy_from_slice(&self.flags.to_le_bytes());
        out[8..16].copy_from_slice(&self.offset.to_le_bytes());
        out[16..24].copy_from_slice(&self.length.to_le_bytes());
        out[24..32].copy_from_slice(&self.uncompressed_length.to_le_bytes());
        out[32..40].copy_from_slice(&self.item_count.to_le_bytes());
        out[40] = self.compression;
        out[41] = self.encryption;
        out[42] = self.alignment_log2;
        out[43] = self.reserved0;
        out[44..52].copy_from_slice(&self.required_features.to_le_bytes());
        out[52..60].copy_from_slice(&self.optional_features.to_le_bytes());
        out[60..64].copy_from_slice(&self.crc32c.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[64..68].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn end_offset(&self) -> Result<u64, CoveError> {
        self.offset
            .checked_add(self.length)
            .ok_or(CoveError::ArithOverflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IndexCapabilityExactnessV2 {
    Exact = 0,
    Approximate = 1,
    Advisory = 2,
}

impl IndexCapabilityExactnessV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Exact),
            1 => Some(Self::Approximate),
            2 => Some(Self::Advisory),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexCapabilityV2 {
    pub capability_id: u32,
    pub index_root_id: u32,
    pub flags: u32,
    pub supports_eq: u8,
    pub supports_range: u8,
    pub supports_membership: u8,
    pub supports_prefix: u8,
    pub supports_contains: u8,
    pub supports_count: u8,
    pub supports_min: u8,
    pub supports_max: u8,
    pub supports_sum: u8,
    pub supports_distinct_count: u8,
    pub supports_join_coverage: u8,
    pub supports_index_only: u8,
    pub exactness: IndexCapabilityExactnessV2,
    pub proof_strength: CoverageProofStrengthV2,
    pub null_semantics: u8,
    pub reserved: u8,
    pub snapshot_validity_ref: u32,
    pub coverage_provider_ref: u32,
    pub checksum: u32,
}

impl IndexCapabilityV2 {
    pub const LEN: usize = INDEX_CAPABILITY_LEN;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let capability = Self {
            capability_id: read_u32(bytes, 0)?,
            index_root_id: read_u32(bytes, 4)?,
            flags: read_u32(bytes, 8)?,
            supports_eq: read_u8(bytes, 12)?,
            supports_range: read_u8(bytes, 13)?,
            supports_membership: read_u8(bytes, 14)?,
            supports_prefix: read_u8(bytes, 15)?,
            supports_contains: read_u8(bytes, 16)?,
            supports_count: read_u8(bytes, 17)?,
            supports_min: read_u8(bytes, 18)?,
            supports_max: read_u8(bytes, 19)?,
            supports_sum: read_u8(bytes, 20)?,
            supports_distinct_count: read_u8(bytes, 21)?,
            supports_join_coverage: read_u8(bytes, 22)?,
            supports_index_only: read_u8(bytes, 23)?,
            exactness: IndexCapabilityExactnessV2::from_u8(read_u8(bytes, 24)?)
                .ok_or(CoveError::BadCovi)?,
            proof_strength: CoverageProofStrengthV2::from_u8(read_u8(bytes, 25)?)
                .ok_or(CoveError::BadCovi)?,
            null_semantics: read_u8(bytes, 26)?,
            reserved: read_u8(bytes, 27)?,
            snapshot_validity_ref: read_u32(bytes, 28)?,
            coverage_provider_ref: read_u32(bytes, 32)?,
            checksum: read_u32(bytes, 36)?,
        };
        verify_crc(&bytes[..Self::LEN], 36, capability.checksum)?;
        capability.validate()?;
        Ok(capability)
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        if bytes.len() % Self::LEN != 0 {
            return Err(CoveError::BadCovi);
        }
        let mut ids = BTreeSet::new();
        let mut capabilities = Vec::new();
        for chunk in bytes.chunks_exact(Self::LEN) {
            let capability = Self::parse(chunk)?;
            if !ids.insert(capability.capability_id) {
                return Err(CoveError::BadCovi);
            }
            capabilities.push(capability);
        }
        Ok(capabilities)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.capability_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.index_root_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.flags.to_le_bytes());
        out[12] = self.supports_eq;
        out[13] = self.supports_range;
        out[14] = self.supports_membership;
        out[15] = self.supports_prefix;
        out[16] = self.supports_contains;
        out[17] = self.supports_count;
        out[18] = self.supports_min;
        out[19] = self.supports_max;
        out[20] = self.supports_sum;
        out[21] = self.supports_distinct_count;
        out[22] = self.supports_join_coverage;
        out[23] = self.supports_index_only;
        out[24] = self.exactness as u8;
        out[25] = self.proof_strength as u8;
        out[26] = self.null_semantics;
        out[27] = self.reserved;
        out[28..32].copy_from_slice(&self.snapshot_validity_ref.to_le_bytes());
        out[32..36].copy_from_slice(&self.coverage_provider_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[36..40].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        for flag in [
            self.supports_eq,
            self.supports_range,
            self.supports_membership,
            self.supports_prefix,
            self.supports_contains,
            self.supports_count,
            self.supports_min,
            self.supports_max,
            self.supports_sum,
            self.supports_distinct_count,
            self.supports_join_coverage,
            self.supports_index_only,
        ] {
            validate_bool(flag)?;
        }
        if self.reserved != 0 {
            return Err(CoveError::BadCovi);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexOnlyCapabilityV2 {
    pub capability_id: u32,
    pub aggregate_kind: u16,
    pub predicate_supported: u8,
    pub exactness: IndexCapabilityExactnessV2,
    pub null_semantics: u8,
    pub flags: u8,
    pub snapshot_validity_ref: u32,
    pub required_visibility_overlay_ref: u32,
    pub checksum: u32,
}

impl IndexOnlyCapabilityV2 {
    pub const LEN: usize = INDEX_ONLY_CAPABILITY_LEN;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let capability = Self {
            capability_id: read_u32(bytes, 0)?,
            aggregate_kind: read_u16(bytes, 4)?,
            predicate_supported: read_u8(bytes, 6)?,
            exactness: IndexCapabilityExactnessV2::from_u8(read_u8(bytes, 7)?)
                .ok_or(CoveError::BadCovi)?,
            null_semantics: read_u8(bytes, 8)?,
            flags: read_u8(bytes, 9)?,
            snapshot_validity_ref: read_u32(bytes, 10)?,
            required_visibility_overlay_ref: read_u32(bytes, 14)?,
            checksum: read_u32(bytes, 18)?,
        };
        verify_crc(&bytes[..Self::LEN], 18, capability.checksum)?;
        capability.validate()?;
        Ok(capability)
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        if bytes.len() % Self::LEN != 0 {
            return Err(CoveError::BadCovi);
        }
        let mut ids = BTreeSet::new();
        let mut capabilities = Vec::new();
        for chunk in bytes.chunks_exact(Self::LEN) {
            let capability = Self::parse(chunk)?;
            if !ids.insert(capability.capability_id) {
                return Err(CoveError::BadCovi);
            }
            capabilities.push(capability);
        }
        Ok(capabilities)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.capability_id.to_le_bytes());
        out[4..6].copy_from_slice(&self.aggregate_kind.to_le_bytes());
        out[6] = self.predicate_supported;
        out[7] = self.exactness as u8;
        out[8] = self.null_semantics;
        out[9] = self.flags;
        out[10..14].copy_from_slice(&self.snapshot_validity_ref.to_le_bytes());
        out[14..18].copy_from_slice(&self.required_visibility_overlay_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[18..22].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        validate_bool(self.predicate_supported)?;
        if self.aggregate_kind > 7 {
            return Err(CoveError::BadCovi);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviArtifactV2 {
    pub postscript: CoviPostscriptV2,
    pub header: CoviHeaderV2,
    pub sections: Vec<CoviSectionEntryV2>,
}

impl CoviArtifactV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < COVI_TAIL_LEN {
            return Err(CoveError::BufferTooShort);
        }
        if bytes[bytes.len() - 4..] != MAGIC_COVI {
            return Err(CoveError::BadMagic);
        }
        let postscript_len_offset = bytes.len() - 6;
        let postscript_version_offset = bytes.len() - 8;
        let postscript_version = read_u16(bytes, postscript_version_offset)?;
        let postscript_len = read_u16(bytes, postscript_len_offset)? as usize;
        if postscript_version != POSTSCRIPT_VERSION_V1 || postscript_len != COVI_POSTSCRIPT_LEN {
            return Err(CoveError::BadVersion);
        }
        let postscript_offset = bytes
            .len()
            .checked_sub(8 + postscript_len)
            .ok_or(CoveError::BufferTooShort)?;
        let postscript =
            CoviPostscriptV2::parse(&bytes[postscript_offset..postscript_offset + postscript_len])?;
        if postscript.file_len != bytes.len() as u64 {
            return Err(CoveError::OffsetRange);
        }
        let header_start =
            usize::try_from(postscript.header_offset).map_err(|_| CoveError::OffsetRange)?;
        let header_len =
            usize::try_from(postscript.header_length).map_err(|_| CoveError::OffsetRange)?;
        let header_end = header_start
            .checked_add(header_len)
            .ok_or(CoveError::ArithOverflow)?;
        if header_end > postscript_offset {
            return Err(CoveError::OffsetRange);
        }
        let header = CoviHeaderV2::parse(&bytes[header_start..header_end])?;
        let sections = parse_section_directory(bytes, &header, postscript_offset)?;
        Ok(Self {
            postscript,
            header,
            sections,
        })
    }

    pub fn new_empty(dataset_id: [u8; 16], snapshot_id: [u8; 16]) -> Self {
        let header = CoviHeaderV2 {
            magic: MAGIC_COVI,
            header_len: COVI_HEADER_LEN,
            version_major: VERSION_MAJOR_V1,
            version_minor: 0,
            flags: 0,
            index_artifact_id: [0u8; 16],
            dataset_id,
            snapshot_id,
            section_count: 0,
            referenced_file_count: 0,
            snapshot_validity_count: 0,
            index_root_count: 0,
            capability_count: 0,
            section_directory_offset: COVI_HEADER_LEN as u64,
            section_directory_length: 0,
            referenced_files_offset: 0,
            snapshot_validity_offset: 0,
            index_roots_offset: 0,
            capabilities_offset: 0,
            string_table_section_ref: u32::MAX,
            created_at_us: 0,
            reserved: [0u8; 24],
            checksum: 0,
        };
        let file_len = COVI_HEADER_LEN as u64 + COVI_TAIL_LEN as u64;
        let postscript = CoviPostscriptV2 {
            required_features: cove_core::constants::FEATURE_SECONDARY_INDEX_ARTIFACT,
            optional_features: 0,
            file_len,
            header_offset: 0,
            header_length: COVI_HEADER_LEN as u64,
            checksum: 0,
        };
        Self {
            postscript,
            header,
            sections: Vec::new(),
        }
    }

    pub fn serialize_empty(&self) -> Result<Vec<u8>, CoveError> {
        if !self.sections.is_empty() || self.header.section_count != 0 {
            return Err(CoveError::BadCovi);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&self.header.serialize());
        let mut postscript = self.postscript.clone();
        postscript.file_len = out.len() as u64 + COVI_TAIL_LEN as u64;
        postscript.header_offset = 0;
        postscript.header_length = COVI_HEADER_LEN as u64;
        out.extend_from_slice(&postscript.serialize());
        out.extend_from_slice(&POSTSCRIPT_VERSION_V1.to_le_bytes());
        out.extend_from_slice(&(COVI_POSTSCRIPT_LEN as u16).to_le_bytes());
        out.extend_from_slice(&MAGIC_COVI);
        Ok(out)
    }

    pub fn serialize_single_section(
        dataset_id: [u8; 16],
        snapshot_id: [u8; 16],
        section_kind: CoviSectionKindV2,
        payload: &[u8],
    ) -> Result<Vec<u8>, CoveError> {
        let section_directory_offset = COVI_HEADER_LEN as u64;
        let section_payload_offset = section_directory_offset
            .checked_add(COVI_SECTION_ENTRY_LEN as u64)
            .ok_or(CoveError::ArithOverflow)?;
        let payload_len = u64::try_from(payload.len()).map_err(|_| CoveError::ArithOverflow)?;
        let section_end = section_payload_offset
            .checked_add(payload_len)
            .ok_or(CoveError::ArithOverflow)?;
        let file_len = section_end
            .checked_add(COVI_TAIL_LEN as u64)
            .ok_or(CoveError::ArithOverflow)?;

        let header = CoviHeaderV2 {
            magic: MAGIC_COVI,
            header_len: COVI_HEADER_LEN,
            version_major: VERSION_MAJOR_V1,
            version_minor: 0,
            flags: 0,
            index_artifact_id: [0u8; 16],
            dataset_id,
            snapshot_id,
            section_count: 1,
            referenced_file_count: 0,
            snapshot_validity_count: 0,
            index_root_count: 0,
            capability_count: 0,
            section_directory_offset,
            section_directory_length: COVI_SECTION_ENTRY_LEN as u64,
            referenced_files_offset: 0,
            snapshot_validity_offset: 0,
            index_roots_offset: 0,
            capabilities_offset: 0,
            string_table_section_ref: if section_kind == CoviSectionKindV2::StringTable {
                1
            } else {
                u32::MAX
            },
            created_at_us: 0,
            reserved: [0u8; 24],
            checksum: 0,
        };
        let section = CoviSectionEntryV2 {
            section_id: 1,
            section_kind,
            flags: 0,
            offset: section_payload_offset,
            length: payload_len,
            uncompressed_length: payload_len,
            item_count: if payload.is_empty() { 0 } else { 1 },
            compression: CompressionCodec::None as u8,
            encryption: 0,
            alignment_log2: 0,
            reserved0: 0,
            required_features: 0,
            optional_features: 0,
            crc32c: checksum::crc32c(payload),
            checksum: 0,
        };
        let postscript = CoviPostscriptV2 {
            required_features: cove_core::constants::FEATURE_SECONDARY_INDEX_ARTIFACT,
            optional_features: 0,
            file_len,
            header_offset: 0,
            header_length: COVI_HEADER_LEN as u64,
            checksum: 0,
        };

        let mut out = Vec::with_capacity(file_len as usize);
        out.extend_from_slice(&header.serialize());
        out.extend_from_slice(&section.serialize()?);
        out.extend_from_slice(payload);
        out.extend_from_slice(&postscript.serialize());
        out.extend_from_slice(&POSTSCRIPT_VERSION_V1.to_le_bytes());
        out.extend_from_slice(&(COVI_POSTSCRIPT_LEN as u16).to_le_bytes());
        out.extend_from_slice(&MAGIC_COVI);
        Ok(out)
    }
}

fn parse_section_directory(
    bytes: &[u8],
    header: &CoviHeaderV2,
    section_limit: usize,
) -> Result<Vec<CoviSectionEntryV2>, CoveError> {
    let expected_len = header
        .section_count
        .checked_mul(COVI_SECTION_ENTRY_LEN as u32)
        .ok_or(CoveError::ArithOverflow)? as u64;
    if header.section_directory_length != expected_len {
        return Err(CoveError::BadCovi);
    }
    let start =
        usize::try_from(header.section_directory_offset).map_err(|_| CoveError::OffsetRange)?;
    let len =
        usize::try_from(header.section_directory_length).map_err(|_| CoveError::OffsetRange)?;
    let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > section_limit {
        return Err(CoveError::OffsetRange);
    }
    let mut sections = Vec::new();
    let mut ids = BTreeSet::new();
    for chunk in bytes[start..end].chunks_exact(COVI_SECTION_ENTRY_LEN) {
        let entry = CoviSectionEntryV2::parse(chunk)?;
        if !ids.insert(entry.section_id) {
            return Err(CoveError::BadCovi);
        }
        if entry.end_offset()? > section_limit as u64 {
            return Err(CoveError::OffsetRange);
        }
        let section_bytes = &bytes[entry.offset as usize..entry.end_offset()? as usize];
        if checksum::crc32c(section_bytes) != entry.crc32c {
            return Err(CoveError::ChecksumMismatch);
        }
        sections.push(entry);
    }
    Ok(sections)
}

fn verify_crc(bytes: &[u8], checksum_offset: usize, expected: u32) -> Result<(), CoveError> {
    let mut check = bytes.to_vec();
    check[checksum_offset..checksum_offset + 4].fill(0);
    if checksum::crc32c(&check) != expected {
        return Err(CoveError::ChecksumMismatch);
    }
    Ok(())
}

fn validate_bool(value: u8) -> Result<(), CoveError> {
    match value {
        0 | 1 => Ok(()),
        _ => Err(CoveError::BadCovi),
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

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, CoveError> {
    Ok(i64::from_le_bytes(read_array(bytes, offset)?))
}

fn read_uuid(bytes: &[u8], offset: usize) -> Result<[u8; 16], CoveError> {
    read_array(bytes, offset)
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], CoveError> {
    let end = offset.checked_add(N).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    Ok(bytes[offset..end].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_capability(capability_id: u32) -> IndexCapabilityV2 {
        IndexCapabilityV2 {
            capability_id,
            index_root_id: 1,
            flags: 0,
            supports_eq: 1,
            supports_range: 0,
            supports_membership: 1,
            supports_prefix: 0,
            supports_contains: 0,
            supports_count: 1,
            supports_min: 0,
            supports_max: 0,
            supports_sum: 0,
            supports_distinct_count: 1,
            supports_join_coverage: 0,
            supports_index_only: 1,
            exactness: IndexCapabilityExactnessV2::Exact,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            null_semantics: 0,
            reserved: 0,
            snapshot_validity_ref: 1,
            coverage_provider_ref: 1,
            checksum: 0,
        }
    }

    fn index_only_capability(capability_id: u32) -> IndexOnlyCapabilityV2 {
        IndexOnlyCapabilityV2 {
            capability_id,
            aggregate_kind: 0,
            predicate_supported: 1,
            exactness: IndexCapabilityExactnessV2::Exact,
            null_semantics: 0,
            flags: 0,
            snapshot_validity_ref: 1,
            required_visibility_overlay_ref: u32::MAX,
            checksum: 0,
        }
    }

    #[test]
    fn index_capability_round_trips() {
        let mut bytes = index_capability(1).serialize().unwrap().to_vec();
        bytes.extend_from_slice(&index_capability(2).serialize().unwrap());
        let parsed = IndexCapabilityV2::parse_many(&bytes).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].supports_index_only, 1);
    }

    #[test]
    fn index_capability_rejects_invalid_boolean() {
        let mut capability = index_capability(1);
        capability.supports_eq = 2;
        assert!(matches!(capability.serialize(), Err(CoveError::BadCovi)));
    }

    #[test]
    fn index_only_capability_round_trips() {
        let bytes = index_only_capability(1).serialize().unwrap();
        let parsed = IndexOnlyCapabilityV2::parse(&bytes).unwrap();
        assert_eq!(parsed.capability_id, 1);
        assert_eq!(parsed.aggregate_kind, 0);
    }

    #[test]
    fn index_only_capability_rejects_bad_checksum() {
        let mut bytes = index_only_capability(1).serialize().unwrap();
        bytes[4] ^= 1;
        assert!(matches!(
            IndexOnlyCapabilityV2::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        ));
    }

    #[test]
    fn empty_covi_round_trips() {
        let artifact = CoviArtifactV2::new_empty([1u8; 16], [2u8; 16]);
        let bytes = artifact.serialize_empty().unwrap();
        assert_eq!(&bytes[bytes.len() - 4..], &MAGIC_COVI);
        let parsed = CoviArtifactV2::parse(&bytes).unwrap();
        assert_eq!(parsed.header.dataset_id, [1u8; 16]);
        assert_eq!(parsed.sections.len(), 0);
    }

    #[test]
    fn bad_tail_magic_rejected() {
        let artifact = CoviArtifactV2::new_empty([0u8; 16], [0u8; 16]);
        let mut bytes = artifact.serialize_empty().unwrap();
        let len = bytes.len();
        bytes[len - 1] = b'X';
        assert!(matches!(
            CoviArtifactV2::parse(&bytes),
            Err(CoveError::BadMagic)
        ));
    }

    #[test]
    fn single_section_covi_round_trips() {
        let bytes = CoviArtifactV2::serialize_single_section(
            [1u8; 16],
            [2u8; 16],
            CoviSectionKindV2::StringTable,
            b"org.cove\0",
        )
        .unwrap();
        let parsed = CoviArtifactV2::parse(&bytes).unwrap();
        assert_eq!(parsed.header.section_count, 1);
        assert_eq!(
            parsed.sections[0].section_kind,
            CoviSectionKindV2::StringTable
        );
        assert_eq!(parsed.sections[0].length, 9);
    }

    #[test]
    fn single_section_covi_rejects_section_crc_corruption() {
        let mut bytes = CoviArtifactV2::serialize_single_section(
            [1u8; 16],
            [2u8; 16],
            CoviSectionKindV2::StringTable,
            b"org.cove\0",
        )
        .unwrap();
        let section_offset = COVI_HEADER_LEN as usize + COVI_SECTION_ENTRY_LEN;
        bytes[section_offset] ^= 1;
        assert!(matches!(
            CoviArtifactV2::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        ));
    }
}
