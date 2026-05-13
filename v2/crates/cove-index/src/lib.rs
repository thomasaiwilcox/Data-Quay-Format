//! COVE-I `.covi` secondary-index artifacts for COVE v2.

pub mod build;
pub mod execution;

use std::collections::BTreeSet;

use cove_core::{
    checksum,
    constants::{
        CompressionCodec, DigestAlgorithm, KNOWN_FEATURE_BITS_MASK, MAGIC_COVI,
        POSTSCRIPT_VERSION_V1, VERSION_MAJOR_V1,
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
const ABSENT_U32: u32 = u32::MAX;

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
pub struct CoviReferencedFileV2 {
    pub file_ref: u32,
    pub flags: u32,
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: u32,
    pub digest_algorithm: u16,
    pub digest_len: u16,
    pub digest_offset: u64,
    pub uri_ref: u32,
    pub schema_fingerprint_ref: u32,
    pub checksum: u32,
}

impl CoviReferencedFileV2 {
    pub const LEN: usize = 60;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            file_ref: read_u32(bytes, 0)?,
            flags: read_u32(bytes, 4)?,
            file_id: read_uuid(bytes, 8)?,
            file_len: read_u64(bytes, 24)?,
            footer_crc32c: read_u32(bytes, 32)?,
            digest_algorithm: read_u16(bytes, 36)?,
            digest_len: read_u16(bytes, 38)?,
            digest_offset: read_u64(bytes, 40)?,
            uri_ref: read_u32(bytes, 48)?,
            schema_fingerprint_ref: read_u32(bytes, 52)?,
            checksum: read_u32(bytes, 56)?,
        };
        verify_crc(&bytes[..Self::LEN], 56, item.checksum)?;
        item.validate()?;
        Ok(item)
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        parse_dense_many(bytes, Self::LEN, Self::parse, |item| item.file_ref)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.file_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.flags.to_le_bytes());
        out[8..24].copy_from_slice(&self.file_id);
        out[24..32].copy_from_slice(&self.file_len.to_le_bytes());
        out[32..36].copy_from_slice(&self.footer_crc32c.to_le_bytes());
        out[36..38].copy_from_slice(&self.digest_algorithm.to_le_bytes());
        out[38..40].copy_from_slice(&self.digest_len.to_le_bytes());
        out[40..48].copy_from_slice(&self.digest_offset.to_le_bytes());
        out[48..52].copy_from_slice(&self.uri_ref.to_le_bytes());
        out[52..56].copy_from_slice(&self.schema_fingerprint_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[56..60].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.file_len == 0 {
            return Err(CoveError::BadCovi);
        }
        let Some(algorithm) = DigestAlgorithm::from_u16(self.digest_algorithm) else {
            return Err(CoveError::BadCovi);
        };
        if algorithm == DigestAlgorithm::None {
            if self.digest_len != 0 || self.digest_offset != 0 {
                return Err(CoveError::BadCovi);
            }
        } else if self.digest_len != 32 {
            return Err(CoveError::BadCovi);
        }
        checked_end(self.digest_offset, u64::from(self.digest_len))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviSnapshotValidityV2 {
    pub snapshot_validity_ref: u32,
    pub dataset_id: [u8; 16],
    pub snapshot_id: [u8; 16],
    pub schema_fingerprint_ref: u32,
    pub semantic_map_fingerprint_ref: u32,
    pub external_visibility_ref: u32,
    pub data_checksum_root_ref: u32,
    pub valid_from_us: i64,
    pub valid_until_us: i64,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviSnapshotValidityV2 {
    pub const LEN: usize = 76;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            snapshot_validity_ref: read_u32(bytes, 0)?,
            dataset_id: read_uuid(bytes, 4)?,
            snapshot_id: read_uuid(bytes, 20)?,
            schema_fingerprint_ref: read_u32(bytes, 36)?,
            semantic_map_fingerprint_ref: read_u32(bytes, 40)?,
            external_visibility_ref: read_u32(bytes, 44)?,
            data_checksum_root_ref: read_u32(bytes, 48)?,
            valid_from_us: read_i64(bytes, 52)?,
            valid_until_us: read_i64(bytes, 60)?,
            flags: read_u32(bytes, 68)?,
            checksum: read_u32(bytes, 72)?,
        };
        verify_crc(&bytes[..Self::LEN], 72, item.checksum)?;
        if item.valid_until_us < item.valid_from_us {
            return Err(CoveError::BadCovi);
        }
        Ok(item)
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        parse_dense_many(bytes, Self::LEN, Self::parse, |item| {
            item.snapshot_validity_ref
        })
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.snapshot_validity_ref.to_le_bytes());
        out[4..20].copy_from_slice(&self.dataset_id);
        out[20..36].copy_from_slice(&self.snapshot_id);
        out[36..40].copy_from_slice(&self.schema_fingerprint_ref.to_le_bytes());
        out[40..44].copy_from_slice(&self.semantic_map_fingerprint_ref.to_le_bytes());
        out[44..48].copy_from_slice(&self.external_visibility_ref.to_le_bytes());
        out[48..52].copy_from_slice(&self.data_checksum_root_ref.to_le_bytes());
        out[52..60].copy_from_slice(&self.valid_from_us.to_le_bytes());
        out[60..68].copy_from_slice(&self.valid_until_us.to_le_bytes());
        out[68..72].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[72..76].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CoviIndexedTargetKindV2 {
    TableColumn = 0,
    ObjectProperty = 1,
    ObjectPath = 2,
    AssociationEndpoint = 3,
    ProjectionColumn = 4,
    SemanticDimension = 5,
    DimensionalTuple = 6,
    ExternalTarget = 255,
}

impl CoviIndexedTargetKindV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::TableColumn),
            1 => Some(Self::ObjectProperty),
            2 => Some(Self::ObjectPath),
            3 => Some(Self::AssociationEndpoint),
            4 => Some(Self::ProjectionColumn),
            5 => Some(Self::SemanticDimension),
            6 => Some(Self::DimensionalTuple),
            255 => Some(Self::ExternalTarget),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CoviIndexKindV2 {
    Hash = 0,
    Sorted = 1,
    SparseSorted = 2,
    Trie = 3,
    RangeBucket = 4,
    Bitmap = 5,
    MinimalPerfectHash = 6,
    AggregateOnly = 7,
    Extension = 255,
}

impl CoviIndexKindV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::Hash),
            1 => Some(Self::Sorted),
            2 => Some(Self::SparseSorted),
            3 => Some(Self::Trie),
            4 => Some(Self::RangeBucket),
            5 => Some(Self::Bitmap),
            6 => Some(Self::MinimalPerfectHash),
            7 => Some(Self::AggregateOnly),
            255 => Some(Self::Extension),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviIndexRootV2 {
    pub index_root_id: u32,
    pub indexed_target_kind: CoviIndexedTargetKindV2,
    pub index_kind: CoviIndexKindV2,
    pub coverage_granularity: u8,
    pub proof_strength: u8,
    pub exactness: u8,
    pub flags: u8,
    pub table_id: u32,
    pub column_id: u32,
    pub object_type_id: u32,
    pub property_id: u32,
    pub path_ref: u32,
    pub semantic_dimension_ref: u32,
    pub logical_type: u16,
    pub physical_kind: u8,
    pub key_encoding_kind: u8,
    pub comparator_kind: u16,
    pub collation_id: u16,
    pub null_semantics: u8,
    pub sort_order: u8,
    pub value_count: u64,
    pub distinct_count: u64,
    pub null_count: u64,
    pub min_key_ref: u32,
    pub max_key_ref: u32,
    pub key_block_section_id: u32,
    pub entry_block_section_id: u32,
    pub postings_block_section_id: u32,
    pub aggregate_block_section_id: u32,
    pub coverage_set_ref: u32,
    pub capability_ref: u32,
    pub snapshot_validity_ref: u32,
    pub checksum: u32,
}

impl CoviIndexRootV2 {
    pub const LEN: usize = 110;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            index_root_id: read_u32(bytes, 0)?,
            indexed_target_kind: CoviIndexedTargetKindV2::from_u16(read_u16(bytes, 4)?)
                .ok_or(CoveError::BadCovi)?,
            index_kind: CoviIndexKindV2::from_u16(read_u16(bytes, 6)?).ok_or(CoveError::BadCovi)?,
            coverage_granularity: read_u8(bytes, 8)?,
            proof_strength: read_u8(bytes, 9)?,
            exactness: read_u8(bytes, 10)?,
            flags: read_u8(bytes, 11)?,
            table_id: read_u32(bytes, 12)?,
            column_id: read_u32(bytes, 16)?,
            object_type_id: read_u32(bytes, 20)?,
            property_id: read_u32(bytes, 24)?,
            path_ref: read_u32(bytes, 28)?,
            semantic_dimension_ref: read_u32(bytes, 32)?,
            logical_type: read_u16(bytes, 36)?,
            physical_kind: read_u8(bytes, 38)?,
            key_encoding_kind: read_u8(bytes, 39)?,
            comparator_kind: read_u16(bytes, 40)?,
            collation_id: read_u16(bytes, 42)?,
            null_semantics: read_u8(bytes, 44)?,
            sort_order: read_u8(bytes, 45)?,
            value_count: read_u64(bytes, 46)?,
            distinct_count: read_u64(bytes, 54)?,
            null_count: read_u64(bytes, 62)?,
            min_key_ref: read_u32(bytes, 70)?,
            max_key_ref: read_u32(bytes, 74)?,
            key_block_section_id: read_u32(bytes, 78)?,
            entry_block_section_id: read_u32(bytes, 82)?,
            postings_block_section_id: read_u32(bytes, 86)?,
            aggregate_block_section_id: read_u32(bytes, 90)?,
            coverage_set_ref: read_u32(bytes, 94)?,
            capability_ref: read_u32(bytes, 98)?,
            snapshot_validity_ref: read_u32(bytes, 102)?,
            checksum: read_u32(bytes, 106)?,
        };
        verify_crc(&bytes[..Self::LEN], 106, item.checksum)?;
        item.validate()?;
        Ok(item)
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        parse_dense_many(bytes, Self::LEN, Self::parse, |item| item.index_root_id)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.null_count > self.value_count || self.distinct_count > self.value_count {
            return Err(CoveError::BadCovi);
        }
        if self.snapshot_validity_ref == ABSENT_U32 {
            return Err(CoveError::BadCovi);
        }
        Ok(())
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.index_root_id.to_le_bytes());
        out[4..6].copy_from_slice(&(self.indexed_target_kind as u16).to_le_bytes());
        out[6..8].copy_from_slice(&(self.index_kind as u16).to_le_bytes());
        out[8] = self.coverage_granularity;
        out[9] = self.proof_strength;
        out[10] = self.exactness;
        out[11] = self.flags;
        out[12..16].copy_from_slice(&self.table_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.column_id.to_le_bytes());
        out[20..24].copy_from_slice(&self.object_type_id.to_le_bytes());
        out[24..28].copy_from_slice(&self.property_id.to_le_bytes());
        out[28..32].copy_from_slice(&self.path_ref.to_le_bytes());
        out[32..36].copy_from_slice(&self.semantic_dimension_ref.to_le_bytes());
        out[36..38].copy_from_slice(&self.logical_type.to_le_bytes());
        out[38] = self.physical_kind;
        out[39] = self.key_encoding_kind;
        out[40..42].copy_from_slice(&self.comparator_kind.to_le_bytes());
        out[42..44].copy_from_slice(&self.collation_id.to_le_bytes());
        out[44] = self.null_semantics;
        out[45] = self.sort_order;
        out[46..54].copy_from_slice(&self.value_count.to_le_bytes());
        out[54..62].copy_from_slice(&self.distinct_count.to_le_bytes());
        out[62..70].copy_from_slice(&self.null_count.to_le_bytes());
        out[70..74].copy_from_slice(&self.min_key_ref.to_le_bytes());
        out[74..78].copy_from_slice(&self.max_key_ref.to_le_bytes());
        out[78..82].copy_from_slice(&self.key_block_section_id.to_le_bytes());
        out[82..86].copy_from_slice(&self.entry_block_section_id.to_le_bytes());
        out[86..90].copy_from_slice(&self.postings_block_section_id.to_le_bytes());
        out[90..94].copy_from_slice(&self.aggregate_block_section_id.to_le_bytes());
        out[94..98].copy_from_slice(&self.coverage_set_ref.to_le_bytes());
        out[98..102].copy_from_slice(&self.capability_ref.to_le_bytes());
        out[102..106].copy_from_slice(&self.snapshot_validity_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[106..110].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoviBlockKindV2 {
    KeyBlock = 0,
    EntryBlock = 1,
    PostingsBlock = 2,
    RowOrdinalSetBlock = 3,
    AggregateAnswerBlock = 4,
    CoverageSetBlock = 5,
    Extension = 255,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoviKeyEncodingKindV2 {
    FileCode = 0,
    NumCode = 1,
    CanonicalValueBytes = 2,
    CanonicalHash64 = 3,
    CanonicalHash128 = 4,
    FixedBytes = 5,
    Utf8BytewisePrefix = 6,
    IntervalTuple = 7,
    DimensionalTuple = 8,
    ObjectPathTuple = 9,
    Extension = 255,
}

impl CoviKeyEncodingKindV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::FileCode),
            1 => Some(Self::NumCode),
            2 => Some(Self::CanonicalValueBytes),
            3 => Some(Self::CanonicalHash64),
            4 => Some(Self::CanonicalHash128),
            5 => Some(Self::FixedBytes),
            6 => Some(Self::Utf8BytewisePrefix),
            7 => Some(Self::IntervalTuple),
            8 => Some(Self::DimensionalTuple),
            9 => Some(Self::ObjectPathTuple),
            255 => Some(Self::Extension),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CoviComparatorKindV2 {
    CanonicalEquality = 0,
    CanonicalOrdering = 1,
    DomainRankOrdering = 2,
    NumCodeLogicalOrdering = 3,
    Utf8BytewisePrefix = 4,
    IntervalOverlap = 5,
    DimensionalTupleLexicographic = 6,
    ObjectPathLexicographic = 7,
    ExtensionRequired = 255,
}

impl CoviComparatorKindV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::CanonicalEquality),
            1 => Some(Self::CanonicalOrdering),
            2 => Some(Self::DomainRankOrdering),
            3 => Some(Self::NumCodeLogicalOrdering),
            4 => Some(Self::Utf8BytewisePrefix),
            5 => Some(Self::IntervalOverlap),
            6 => Some(Self::DimensionalTupleLexicographic),
            7 => Some(Self::ObjectPathLexicographic),
            255 => Some(Self::ExtensionRequired),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviKeyBlockHeaderV2 {
    pub magic: [u8; 4],
    pub version_major: u16,
    pub version_minor: u16,
    pub header_len: u16,
    pub reserved0: u16,
    pub key_block_id: u32,
    pub index_root_id: u32,
    pub key_count: u64,
    pub encoding_kind: CoviKeyEncodingKindV2,
    pub comparator_kind: CoviComparatorKindV2,
    pub flags: u8,
    pub key_data_offset: u64,
    pub key_data_length: u64,
    pub checksum: u32,
}

impl CoviKeyBlockHeaderV2 {
    pub const LEN: usize = 52;
    pub const MAGIC: [u8; 4] = *b"CIK2";

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            magic: read_array(bytes, 0)?,
            version_major: read_u16(bytes, 4)?,
            version_minor: read_u16(bytes, 6)?,
            header_len: read_u16(bytes, 8)?,
            reserved0: read_u16(bytes, 10)?,
            key_block_id: read_u32(bytes, 12)?,
            index_root_id: read_u32(bytes, 16)?,
            key_count: read_u64(bytes, 20)?,
            encoding_kind: CoviKeyEncodingKindV2::from_u8(read_u8(bytes, 28)?)
                .ok_or(CoveError::BadCovi)?,
            comparator_kind: CoviComparatorKindV2::from_u16(read_u16(bytes, 29)?)
                .ok_or(CoveError::BadCovi)?,
            flags: read_u8(bytes, 31)?,
            key_data_offset: read_u64(bytes, 32)?,
            key_data_length: read_u64(bytes, 40)?,
            checksum: read_u32(bytes, 48)?,
        };
        if header.magic != Self::MAGIC {
            return Err(CoveError::BadMagic);
        }
        if header.version_major != 2 || header.header_len as usize != Self::LEN {
            return Err(CoveError::BadVersion);
        }
        if header.reserved0 != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        checked_end(header.key_data_offset, header.key_data_length)?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version_major.to_le_bytes());
        out[6..8].copy_from_slice(&self.version_minor.to_le_bytes());
        out[8..10].copy_from_slice(&self.header_len.to_le_bytes());
        out[10..12].copy_from_slice(&self.reserved0.to_le_bytes());
        out[12..16].copy_from_slice(&self.key_block_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.index_root_id.to_le_bytes());
        out[20..28].copy_from_slice(&self.key_count.to_le_bytes());
        out[28] = self.encoding_kind as u8;
        out[29..31].copy_from_slice(&(self.comparator_kind as u16).to_le_bytes());
        out[31] = self.flags;
        out[32..40].copy_from_slice(&self.key_data_offset.to_le_bytes());
        out[40..48].copy_from_slice(&self.key_data_length.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviKeyBlockV2 {
    pub header: CoviKeyBlockHeaderV2,
    pub key_data: Vec<u8>,
}

impl CoviKeyBlockV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = CoviKeyBlockHeaderV2::parse(bytes)?;
        let start = usize::try_from(header.key_data_offset).map_err(|_| CoveError::OffsetRange)?;
        let len = usize::try_from(header.key_data_length).map_err(|_| CoveError::OffsetRange)?;
        let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        if start < CoviKeyBlockHeaderV2::LEN || end != bytes.len() {
            return Err(CoveError::BadCovi);
        }
        let mut check = bytes[..end].to_vec();
        check[48..52].fill(0);
        if checksum::crc32c(&check) != header.checksum {
            return Err(CoveError::ChecksumMismatch);
        }
        Ok(Self {
            header,
            key_data: bytes[start..end].to_vec(),
        })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let mut header = self.header.clone();
        header.magic = CoviKeyBlockHeaderV2::MAGIC;
        header.version_major = 2;
        header.header_len = CoviKeyBlockHeaderV2::LEN as u16;
        header.reserved0 = 0;
        header.key_data_offset = CoviKeyBlockHeaderV2::LEN as u64;
        header.key_data_length =
            u64::try_from(self.key_data.len()).map_err(|_| CoveError::ArithOverflow)?;
        let mut out = Vec::with_capacity(CoviKeyBlockHeaderV2::LEN + self.key_data.len());
        out.extend_from_slice(&header.serialize());
        out.extend_from_slice(&self.key_data);
        let crc = checksum::crc32c(&out);
        out[48..52].copy_from_slice(&crc.to_le_bytes());
        CoviKeyBlockV2::parse(&out)?;
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviEntryBlockHeaderV2 {
    pub magic: [u8; 4],
    pub version_major: u16,
    pub version_minor: u16,
    pub header_len: u16,
    pub entry_len: u16,
    pub entry_block_id: u32,
    pub index_root_id: u32,
    pub entry_count: u32,
    pub key_block_id: u32,
    pub postings_block_id: u32,
    pub aggregate_block_id: u32,
    pub entries_offset: u64,
    pub entries_length: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviEntryBlockHeaderV2 {
    pub const LEN: usize = 60;
    pub const MAGIC: [u8; 4] = *b"CIE2";

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            magic: read_array(bytes, 0)?,
            version_major: read_u16(bytes, 4)?,
            version_minor: read_u16(bytes, 6)?,
            header_len: read_u16(bytes, 8)?,
            entry_len: read_u16(bytes, 10)?,
            entry_block_id: read_u32(bytes, 12)?,
            index_root_id: read_u32(bytes, 16)?,
            entry_count: read_u32(bytes, 20)?,
            key_block_id: read_u32(bytes, 24)?,
            postings_block_id: read_u32(bytes, 28)?,
            aggregate_block_id: read_u32(bytes, 32)?,
            entries_offset: read_u64(bytes, 36)?,
            entries_length: read_u64(bytes, 44)?,
            flags: read_u32(bytes, 52)?,
            checksum: read_u32(bytes, 56)?,
        };
        if header.magic != Self::MAGIC {
            return Err(CoveError::BadMagic);
        }
        if header.version_major != 2
            || header.header_len as usize != Self::LEN
            || header.entry_len as usize != CoviIndexEntryV2::LEN
        {
            return Err(CoveError::BadVersion);
        }
        verify_crc(&bytes[..Self::LEN], 56, header.checksum)?;
        let expected = header
            .entry_count
            .checked_mul(CoviIndexEntryV2::LEN as u32)
            .ok_or(CoveError::ArithOverflow)? as u64;
        if header.entries_length != expected {
            return Err(CoveError::BadCovi);
        }
        checked_end(header.entries_offset, header.entries_length)?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version_major.to_le_bytes());
        out[6..8].copy_from_slice(&self.version_minor.to_le_bytes());
        out[8..10].copy_from_slice(&self.header_len.to_le_bytes());
        out[10..12].copy_from_slice(&self.entry_len.to_le_bytes());
        out[12..16].copy_from_slice(&self.entry_block_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.index_root_id.to_le_bytes());
        out[20..24].copy_from_slice(&self.entry_count.to_le_bytes());
        out[24..28].copy_from_slice(&self.key_block_id.to_le_bytes());
        out[28..32].copy_from_slice(&self.postings_block_id.to_le_bytes());
        out[32..36].copy_from_slice(&self.aggregate_block_id.to_le_bytes());
        out[36..44].copy_from_slice(&self.entries_offset.to_le_bytes());
        out[44..52].copy_from_slice(&self.entries_length.to_le_bytes());
        out[52..56].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[56..60].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviIndexEntryV2 {
    pub entry_ref: u32,
    pub index_root_id: u32,
    pub entry_id: u64,
    pub key_kind: CoviKeyEncodingKindV2,
    pub comparator_kind: CoviComparatorKindV2,
    pub flags: u8,
    pub key_offset: u64,
    pub key_length: u32,
    pub key_hash64: u64,
    pub postings_ref: u32,
    pub coverage_set_ref: u32,
    pub aggregate_answer_ref: u32,
    pub next_duplicate_ref: u32,
    pub checksum: u32,
}

impl CoviIndexEntryV2 {
    pub const LEN: usize = 60;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            entry_ref: read_u32(bytes, 0)?,
            index_root_id: read_u32(bytes, 4)?,
            entry_id: read_u64(bytes, 8)?,
            key_kind: CoviKeyEncodingKindV2::from_u8(read_u8(bytes, 16)?)
                .ok_or(CoveError::BadCovi)?,
            comparator_kind: CoviComparatorKindV2::from_u16(read_u16(bytes, 17)?)
                .ok_or(CoveError::BadCovi)?,
            flags: read_u8(bytes, 19)?,
            key_offset: read_u64(bytes, 20)?,
            key_length: read_u32(bytes, 28)?,
            key_hash64: read_u64(bytes, 32)?,
            postings_ref: read_u32(bytes, 40)?,
            coverage_set_ref: read_u32(bytes, 44)?,
            aggregate_answer_ref: read_u32(bytes, 48)?,
            next_duplicate_ref: read_u32(bytes, 52)?,
            checksum: read_u32(bytes, 56)?,
        };
        verify_crc(&bytes[..Self::LEN], 56, item.checksum)?;
        checked_end(item.key_offset, u64::from(item.key_length))?;
        Ok(item)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.entry_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.index_root_id.to_le_bytes());
        out[8..16].copy_from_slice(&self.entry_id.to_le_bytes());
        out[16] = self.key_kind as u8;
        out[17..19].copy_from_slice(&(self.comparator_kind as u16).to_le_bytes());
        out[19] = self.flags;
        out[20..28].copy_from_slice(&self.key_offset.to_le_bytes());
        out[28..32].copy_from_slice(&self.key_length.to_le_bytes());
        out[32..40].copy_from_slice(&self.key_hash64.to_le_bytes());
        out[40..44].copy_from_slice(&self.postings_ref.to_le_bytes());
        out[44..48].copy_from_slice(&self.coverage_set_ref.to_le_bytes());
        out[48..52].copy_from_slice(&self.aggregate_answer_ref.to_le_bytes());
        out[52..56].copy_from_slice(&self.next_duplicate_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[56..60].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

pub fn parse_covi_index_entries(bytes: &[u8]) -> Result<Vec<CoviIndexEntryV2>, CoveError> {
    if bytes.len() % CoviIndexEntryV2::LEN != 0 {
        return Err(CoveError::BadCovi);
    }
    let entries = bytes
        .chunks_exact(CoviIndexEntryV2::LEN)
        .map(CoviIndexEntryV2::parse)
        .collect::<Result<Vec<_>, _>>()?;
    validate_covi_entry_refs(&entries)?;
    Ok(entries)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviEntryBlockV2 {
    pub header: CoviEntryBlockHeaderV2,
    pub entries: Vec<CoviIndexEntryV2>,
}

impl CoviEntryBlockV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = CoviEntryBlockHeaderV2::parse(bytes)?;
        let start = usize::try_from(header.entries_offset).map_err(|_| CoveError::OffsetRange)?;
        let len = usize::try_from(header.entries_length).map_err(|_| CoveError::OffsetRange)?;
        let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        if start < CoviEntryBlockHeaderV2::LEN || end != bytes.len() {
            return Err(CoveError::BadCovi);
        }
        let entries = parse_covi_index_entries(&bytes[start..end])?;
        if entries.len() != header.entry_count as usize {
            return Err(CoveError::BadCovi);
        }
        Ok(Self { header, entries })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        validate_covi_entry_refs(&self.entries)?;
        let mut header = self.header.clone();
        header.magic = CoviEntryBlockHeaderV2::MAGIC;
        header.version_major = 2;
        header.header_len = CoviEntryBlockHeaderV2::LEN as u16;
        header.entry_len = CoviIndexEntryV2::LEN as u16;
        header.entry_count =
            u32::try_from(self.entries.len()).map_err(|_| CoveError::ArithOverflow)?;
        header.entries_offset = CoviEntryBlockHeaderV2::LEN as u64;
        header.entries_length = u64::try_from(
            self.entries
                .len()
                .checked_mul(CoviIndexEntryV2::LEN)
                .ok_or(CoveError::ArithOverflow)?,
        )
        .map_err(|_| CoveError::ArithOverflow)?;
        let mut out = Vec::with_capacity(
            CoviEntryBlockHeaderV2::LEN + self.entries.len() * CoviIndexEntryV2::LEN,
        );
        out.extend_from_slice(&header.serialize());
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize());
        }
        CoviEntryBlockV2::parse(&out)?;
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoviPostingRepresentationV2 {
    SortedFileRefs = 0,
    SortedSegmentRefs = 1,
    SortedPageRefs = 2,
    SortedMorselRefs = 3,
    RowRangeList = 4,
    RowOrdinalBitmap = 5,
    RowOrdinalDeltaVarint = 6,
    ByteRangeList = 7,
    ObjectPathRefs = 8,
    DimensionalBucketRefs = 9,
    CoverageSetRef = 10,
    Extension = 255,
}

impl CoviPostingRepresentationV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::SortedFileRefs),
            1 => Some(Self::SortedSegmentRefs),
            2 => Some(Self::SortedPageRefs),
            3 => Some(Self::SortedMorselRefs),
            4 => Some(Self::RowRangeList),
            5 => Some(Self::RowOrdinalBitmap),
            6 => Some(Self::RowOrdinalDeltaVarint),
            7 => Some(Self::ByteRangeList),
            8 => Some(Self::ObjectPathRefs),
            9 => Some(Self::DimensionalBucketRefs),
            10 => Some(Self::CoverageSetRef),
            255 => Some(Self::Extension),
            _ => None,
        }
    }

    pub fn fixed_payload_len(self) -> Option<usize> {
        match self {
            Self::SortedFileRefs => Some(CoviFileRefPostingV2::LEN),
            Self::SortedSegmentRefs => Some(CoviSegmentRefPostingV2::LEN),
            Self::SortedPageRefs => Some(CoviPageRefPostingV2::LEN),
            Self::SortedMorselRefs => Some(CoviMorselRefPostingV2::LEN),
            Self::RowRangeList => Some(CoviRowRangePostingV2::LEN),
            Self::ByteRangeList => Some(CoviByteRangePostingV2::LEN),
            Self::ObjectPathRefs => Some(CoviObjectPathPostingV2::LEN),
            Self::DimensionalBucketRefs => Some(CoviDimensionalBucketPostingV2::LEN),
            Self::RowOrdinalBitmap | Self::RowOrdinalDeltaVarint => Some(4),
            Self::CoverageSetRef => Some(0),
            Self::Extension => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviPostingsBlockHeaderV2 {
    pub magic: [u8; 4],
    pub version_major: u16,
    pub version_minor: u16,
    pub header_len: u16,
    pub postings_header_len: u16,
    pub postings_block_id: u32,
    pub index_root_id: u32,
    pub postings_count: u32,
    pub row_ordinal_set_count: u32,
    pub postings_headers_offset: u64,
    pub row_ordinal_headers_offset: u64,
    pub postings_payload_offset: u64,
    pub postings_payload_length: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviPostingsBlockHeaderV2 {
    pub const LEN: usize = 68;
    pub const MAGIC: [u8; 4] = *b"CIP2";

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            magic: read_array(bytes, 0)?,
            version_major: read_u16(bytes, 4)?,
            version_minor: read_u16(bytes, 6)?,
            header_len: read_u16(bytes, 8)?,
            postings_header_len: read_u16(bytes, 10)?,
            postings_block_id: read_u32(bytes, 12)?,
            index_root_id: read_u32(bytes, 16)?,
            postings_count: read_u32(bytes, 20)?,
            row_ordinal_set_count: read_u32(bytes, 24)?,
            postings_headers_offset: read_u64(bytes, 28)?,
            row_ordinal_headers_offset: read_u64(bytes, 36)?,
            postings_payload_offset: read_u64(bytes, 44)?,
            postings_payload_length: read_u64(bytes, 52)?,
            flags: read_u32(bytes, 60)?,
            checksum: read_u32(bytes, 64)?,
        };
        if header.magic != Self::MAGIC {
            return Err(CoveError::BadMagic);
        }
        if header.version_major != 2
            || header.header_len as usize != Self::LEN
            || header.postings_header_len as usize != CoviPostingsHeaderV2::LEN
        {
            return Err(CoveError::BadVersion);
        }
        verify_crc(&bytes[..Self::LEN], 64, header.checksum)?;
        checked_end(
            header.postings_headers_offset,
            header.postings_count as u64 * CoviPostingsHeaderV2::LEN as u64,
        )?;
        if header.row_ordinal_set_count == 0 {
            if header.row_ordinal_headers_offset != 0 {
                return Err(CoveError::BadCovi);
            }
        } else {
            checked_end(
                header.row_ordinal_headers_offset,
                header.row_ordinal_set_count as u64 * CoviRowOrdinalSetHeaderV2::LEN as u64,
            )?;
        }
        checked_end(
            header.postings_payload_offset,
            header.postings_payload_length,
        )?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version_major.to_le_bytes());
        out[6..8].copy_from_slice(&self.version_minor.to_le_bytes());
        out[8..10].copy_from_slice(&self.header_len.to_le_bytes());
        out[10..12].copy_from_slice(&self.postings_header_len.to_le_bytes());
        out[12..16].copy_from_slice(&self.postings_block_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.index_root_id.to_le_bytes());
        out[20..24].copy_from_slice(&self.postings_count.to_le_bytes());
        out[24..28].copy_from_slice(&self.row_ordinal_set_count.to_le_bytes());
        out[28..36].copy_from_slice(&self.postings_headers_offset.to_le_bytes());
        out[36..44].copy_from_slice(&self.row_ordinal_headers_offset.to_le_bytes());
        out[44..52].copy_from_slice(&self.postings_payload_offset.to_le_bytes());
        out[52..60].copy_from_slice(&self.postings_payload_length.to_le_bytes());
        out[60..64].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[64..68].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviPostingsHeaderV2 {
    pub postings_ref: u32,
    pub index_root_id: u32,
    pub representation: CoviPostingRepresentationV2,
    pub target_granularity: u8,
    pub flags: u16,
    pub item_count: u64,
    pub payload_offset: u64,
    pub payload_length: u64,
    pub coverage_set_ref: u32,
    pub checksum: u32,
}

impl CoviPostingsHeaderV2 {
    pub const LEN: usize = 44;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            postings_ref: read_u32(bytes, 0)?,
            index_root_id: read_u32(bytes, 4)?,
            representation: CoviPostingRepresentationV2::from_u8(read_u8(bytes, 8)?)
                .ok_or(CoveError::BadCovi)?,
            target_granularity: read_u8(bytes, 9)?,
            flags: read_u16(bytes, 10)?,
            item_count: read_u64(bytes, 12)?,
            payload_offset: read_u64(bytes, 20)?,
            payload_length: read_u64(bytes, 28)?,
            coverage_set_ref: read_u32(bytes, 36)?,
            checksum: read_u32(bytes, 40)?,
        };
        verify_crc(&bytes[..Self::LEN], 40, item.checksum)?;
        item.validate()?;
        Ok(item)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if let Some(width) = self.representation.fixed_payload_len() {
            let expected = (self.item_count as u64)
                .checked_mul(width as u64)
                .ok_or(CoveError::ArithOverflow)?;
            if self.payload_length != expected {
                return Err(CoveError::BadCovi);
            }
        }
        if self.representation == CoviPostingRepresentationV2::CoverageSetRef
            && (self.item_count != 1 || self.coverage_set_ref == ABSENT_U32)
        {
            return Err(CoveError::BadCovi);
        }
        checked_end(self.payload_offset, self.payload_length)?;
        Ok(())
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.postings_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.index_root_id.to_le_bytes());
        out[8] = self.representation as u8;
        out[9] = self.target_granularity;
        out[10..12].copy_from_slice(&self.flags.to_le_bytes());
        out[12..20].copy_from_slice(&self.item_count.to_le_bytes());
        out[20..28].copy_from_slice(&self.payload_offset.to_le_bytes());
        out[28..36].copy_from_slice(&self.payload_length.to_le_bytes());
        out[36..40].copy_from_slice(&self.coverage_set_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[40..44].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoviFileRefPostingV2 {
    pub file_ref: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviFileRefPostingV2 {
    pub const LEN: usize = 12;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            file_ref: read_u32(bytes, 0)?,
            flags: read_u32(bytes, 4)?,
            checksum: read_u32(bytes, 8)?,
        };
        verify_crc(&bytes[..Self::LEN], 8, item.checksum)?;
        Ok(item)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.file_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[8..12].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoviSegmentRefPostingV2 {
    pub file_ref: u32,
    pub table_id: u32,
    pub segment_id: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviSegmentRefPostingV2 {
    pub const LEN: usize = 20;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            file_ref: read_u32(bytes, 0)?,
            table_id: read_u32(bytes, 4)?,
            segment_id: read_u32(bytes, 8)?,
            flags: read_u32(bytes, 12)?,
            checksum: read_u32(bytes, 16)?,
        };
        verify_crc(&bytes[..Self::LEN], 16, item.checksum)?;
        Ok(item)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.file_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.table_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.segment_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[16..20].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoviMorselRefPostingV2 {
    pub file_ref: u32,
    pub table_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviMorselRefPostingV2 {
    pub const LEN: usize = 24;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            file_ref: read_u32(bytes, 0)?,
            table_id: read_u32(bytes, 4)?,
            segment_id: read_u32(bytes, 8)?,
            morsel_id: read_u32(bytes, 12)?,
            flags: read_u32(bytes, 16)?,
            checksum: read_u32(bytes, 20)?,
        };
        verify_crc(&bytes[..Self::LEN], 20, item.checksum)?;
        Ok(item)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.file_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.table_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.segment_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[20..24].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoviPageRefPostingV2 {
    pub file_ref: u32,
    pub table_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub page_ref: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviPageRefPostingV2 {
    pub const LEN: usize = 28;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            file_ref: read_u32(bytes, 0)?,
            table_id: read_u32(bytes, 4)?,
            segment_id: read_u32(bytes, 8)?,
            morsel_id: read_u32(bytes, 12)?,
            page_ref: read_u32(bytes, 16)?,
            flags: read_u32(bytes, 20)?,
            checksum: read_u32(bytes, 24)?,
        };
        verify_crc(&bytes[..Self::LEN], 24, item.checksum)?;
        Ok(item)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.file_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.table_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.segment_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.page_ref.to_le_bytes());
        out[20..24].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[24..28].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoviRowRangePostingV2 {
    pub file_ref: u32,
    pub table_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub row_start: u64,
    pub row_count: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviRowRangePostingV2 {
    pub const LEN: usize = 40;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            file_ref: read_u32(bytes, 0)?,
            table_id: read_u32(bytes, 4)?,
            segment_id: read_u32(bytes, 8)?,
            morsel_id: read_u32(bytes, 12)?,
            row_start: read_u64(bytes, 16)?,
            row_count: read_u64(bytes, 24)?,
            flags: read_u32(bytes, 32)?,
            checksum: read_u32(bytes, 36)?,
        };
        verify_crc(&bytes[..Self::LEN], 36, item.checksum)?;
        if item.row_count == 0 {
            return Err(CoveError::BadCovi);
        }
        checked_end(item.row_start, item.row_count)?;
        Ok(item)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        if self.row_count == 0 {
            return Err(CoveError::BadCovi);
        }
        checked_end(self.row_start, self.row_count)?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.file_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.table_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.segment_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[16..24].copy_from_slice(&self.row_start.to_le_bytes());
        out[24..32].copy_from_slice(&self.row_count.to_le_bytes());
        out[32..36].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[36..40].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoviByteRangePostingV2 {
    pub file_ref: u32,
    pub section_id: u32,
    pub offset: u64,
    pub length: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviByteRangePostingV2 {
    pub const LEN: usize = 32;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            file_ref: read_u32(bytes, 0)?,
            section_id: read_u32(bytes, 4)?,
            offset: read_u64(bytes, 8)?,
            length: read_u64(bytes, 16)?,
            flags: read_u32(bytes, 24)?,
            checksum: read_u32(bytes, 28)?,
        };
        verify_crc(&bytes[..Self::LEN], 28, item.checksum)?;
        if item.length == 0 {
            return Err(CoveError::BadCovi);
        }
        checked_end(item.offset, item.length)?;
        Ok(item)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        if self.length == 0 {
            return Err(CoveError::BadCovi);
        }
        checked_end(self.offset, self.length)?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.file_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.section_id.to_le_bytes());
        out[8..16].copy_from_slice(&self.offset.to_le_bytes());
        out[16..24].copy_from_slice(&self.length.to_le_bytes());
        out[24..28].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[28..32].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoviObjectPathPostingV2 {
    pub file_ref: u32,
    pub object_type_id: u32,
    pub path_ref: u32,
    pub segment_id: u32,
    pub row_start: u64,
    pub row_count: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviObjectPathPostingV2 {
    pub const LEN: usize = 40;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            file_ref: read_u32(bytes, 0)?,
            object_type_id: read_u32(bytes, 4)?,
            path_ref: read_u32(bytes, 8)?,
            segment_id: read_u32(bytes, 12)?,
            row_start: read_u64(bytes, 16)?,
            row_count: read_u64(bytes, 24)?,
            flags: read_u32(bytes, 32)?,
            checksum: read_u32(bytes, 36)?,
        };
        verify_crc(&bytes[..Self::LEN], 36, item.checksum)?;
        if item.row_count == 0 {
            return Err(CoveError::BadCovi);
        }
        checked_end(item.row_start, item.row_count)?;
        Ok(item)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        if self.row_count == 0 {
            return Err(CoveError::BadCovi);
        }
        checked_end(self.row_start, self.row_count)?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.file_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.object_type_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.path_ref.to_le_bytes());
        out[12..16].copy_from_slice(&self.segment_id.to_le_bytes());
        out[16..24].copy_from_slice(&self.row_start.to_le_bytes());
        out[24..32].copy_from_slice(&self.row_count.to_le_bytes());
        out[32..36].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[36..40].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoviDimensionalBucketPostingV2 {
    pub file_ref: u32,
    pub table_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub dimensional_bucket_ref: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviDimensionalBucketPostingV2 {
    pub const LEN: usize = 28;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            file_ref: read_u32(bytes, 0)?,
            table_id: read_u32(bytes, 4)?,
            segment_id: read_u32(bytes, 8)?,
            morsel_id: read_u32(bytes, 12)?,
            dimensional_bucket_ref: read_u32(bytes, 16)?,
            flags: read_u32(bytes, 20)?,
            checksum: read_u32(bytes, 24)?,
        };
        verify_crc(&bytes[..Self::LEN], 24, item.checksum)?;
        Ok(item)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.file_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.table_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.segment_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.dimensional_bucket_ref.to_le_bytes());
        out[20..24].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[24..28].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoviBitmapKindV2 {
    DenseBitsetLsb0 = 0,
    SortedU32 = 1,
    SortedU64 = 2,
    DeltaVarintU32 = 3,
    RangeList = 4,
    RegisteredRoaring32 = 5,
    RegisteredRoaring64 = 6,
    Extension = 255,
}

impl CoviBitmapKindV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::DenseBitsetLsb0),
            1 => Some(Self::SortedU32),
            2 => Some(Self::SortedU64),
            3 => Some(Self::DeltaVarintU32),
            4 => Some(Self::RangeList),
            5 => Some(Self::RegisteredRoaring32),
            6 => Some(Self::RegisteredRoaring64),
            255 => Some(Self::Extension),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviRowOrdinalSetHeaderV2 {
    pub row_ordinal_set_ref: u32,
    pub file_ref: u32,
    pub table_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub bitmap_kind: CoviBitmapKindV2,
    pub flags: u8,
    pub reserved: u16,
    pub universe_row_count: u64,
    pub set_row_count: u64,
    pub payload_offset: u64,
    pub payload_length: u64,
    pub checksum: u32,
}

impl CoviRowOrdinalSetHeaderV2 {
    pub const LEN: usize = 60;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            row_ordinal_set_ref: read_u32(bytes, 0)?,
            file_ref: read_u32(bytes, 4)?,
            table_id: read_u32(bytes, 8)?,
            segment_id: read_u32(bytes, 12)?,
            morsel_id: read_u32(bytes, 16)?,
            bitmap_kind: CoviBitmapKindV2::from_u8(read_u8(bytes, 20)?)
                .ok_or(CoveError::BadCovi)?,
            flags: read_u8(bytes, 21)?,
            reserved: read_u16(bytes, 22)?,
            universe_row_count: read_u64(bytes, 24)?,
            set_row_count: read_u64(bytes, 32)?,
            payload_offset: read_u64(bytes, 40)?,
            payload_length: read_u64(bytes, 48)?,
            checksum: read_u32(bytes, 56)?,
        };
        verify_crc(&bytes[..Self::LEN], 56, item.checksum)?;
        if item.reserved != 0 || item.set_row_count > item.universe_row_count {
            return Err(CoveError::BadCovi);
        }
        checked_end(item.payload_offset, item.payload_length)?;
        Ok(item)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        if self.reserved != 0 || self.set_row_count > self.universe_row_count {
            return Err(CoveError::BadCovi);
        }
        checked_end(self.payload_offset, self.payload_length)?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.row_ordinal_set_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.file_ref.to_le_bytes());
        out[8..12].copy_from_slice(&self.table_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.segment_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[20] = self.bitmap_kind as u8;
        out[21] = self.flags;
        out[22..24].copy_from_slice(&self.reserved.to_le_bytes());
        out[24..32].copy_from_slice(&self.universe_row_count.to_le_bytes());
        out[32..40].copy_from_slice(&self.set_row_count.to_le_bytes());
        out[40..48].copy_from_slice(&self.payload_offset.to_le_bytes());
        out[48..56].copy_from_slice(&self.payload_length.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[56..60].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

pub fn parse_covi_row_range_postings(
    bytes: &[u8],
) -> Result<Vec<CoviRowRangePostingV2>, CoveError> {
    if bytes.len() % CoviRowRangePostingV2::LEN != 0 {
        return Err(CoveError::BadCovi);
    }
    let rows = bytes
        .chunks_exact(CoviRowRangePostingV2::LEN)
        .map(CoviRowRangePostingV2::parse)
        .collect::<Result<Vec<_>, _>>()?;
    validate_row_range_postings(&rows)?;
    Ok(rows)
}

fn parse_fixed_postings<T, F, K>(
    bytes: &[u8],
    item_len: usize,
    parse: F,
    key: impl Fn(&T) -> K,
) -> Result<Vec<T>, CoveError>
where
    F: Fn(&[u8]) -> Result<T, CoveError>,
    K: Ord,
{
    if bytes.len() % item_len != 0 {
        return Err(CoveError::BadCovi);
    }
    let items = bytes
        .chunks_exact(item_len)
        .map(parse)
        .collect::<Result<Vec<_>, _>>()?;
    validate_strictly_sorted_unique(&items, key)?;
    Ok(items)
}

fn validate_strictly_sorted_unique<T, K>(
    items: &[T],
    key: impl Fn(&T) -> K,
) -> Result<(), CoveError>
where
    K: Ord,
{
    let mut previous: Option<K> = None;
    for item in items {
        let current = key(item);
        if previous
            .as_ref()
            .is_some_and(|previous| current <= *previous)
        {
            return Err(CoveError::BadCovi);
        }
        previous = Some(current);
    }
    Ok(())
}

fn validate_covi_posting_payload(
    posting: &CoviPostingsHeaderV2,
    payload: &[u8],
    row_ordinal_sets: &[CoviRowOrdinalSetHeaderV2],
) -> Result<(), CoveError> {
    match posting.representation {
        CoviPostingRepresentationV2::SortedFileRefs => {
            parse_fixed_postings(
                payload,
                CoviFileRefPostingV2::LEN,
                CoviFileRefPostingV2::parse,
                |item| item.file_ref,
            )?;
        }
        CoviPostingRepresentationV2::SortedSegmentRefs => {
            parse_fixed_postings(
                payload,
                CoviSegmentRefPostingV2::LEN,
                CoviSegmentRefPostingV2::parse,
                |item| (item.file_ref, item.table_id, item.segment_id),
            )?;
        }
        CoviPostingRepresentationV2::SortedPageRefs => {
            parse_fixed_postings(
                payload,
                CoviPageRefPostingV2::LEN,
                CoviPageRefPostingV2::parse,
                |item| {
                    (
                        item.file_ref,
                        item.table_id,
                        item.segment_id,
                        item.morsel_id,
                        item.page_ref,
                    )
                },
            )?;
        }
        CoviPostingRepresentationV2::SortedMorselRefs => {
            parse_fixed_postings(
                payload,
                CoviMorselRefPostingV2::LEN,
                CoviMorselRefPostingV2::parse,
                |item| {
                    (
                        item.file_ref,
                        item.table_id,
                        item.segment_id,
                        item.morsel_id,
                    )
                },
            )?;
        }
        CoviPostingRepresentationV2::RowRangeList => {
            parse_covi_row_range_postings(payload)?;
        }
        CoviPostingRepresentationV2::RowOrdinalBitmap
        | CoviPostingRepresentationV2::RowOrdinalDeltaVarint => {
            validate_row_ordinal_refs(posting, payload, row_ordinal_sets)?;
        }
        CoviPostingRepresentationV2::ByteRangeList => {
            let ranges = parse_fixed_postings(
                payload,
                CoviByteRangePostingV2::LEN,
                CoviByteRangePostingV2::parse,
                |item| (item.file_ref, item.section_id, item.offset),
            )?;
            let mut previous: Option<CoviByteRangePostingV2> = None;
            for range in ranges {
                if let Some(prev) = previous {
                    if prev.file_ref == range.file_ref && prev.section_id == range.section_id {
                        let prev_end = prev
                            .offset
                            .checked_add(prev.length)
                            .ok_or(CoveError::ArithOverflow)?;
                        if range.offset < prev_end {
                            return Err(CoveError::BadCovi);
                        }
                    }
                }
                previous = Some(range);
            }
        }
        CoviPostingRepresentationV2::ObjectPathRefs => {
            parse_fixed_postings(
                payload,
                CoviObjectPathPostingV2::LEN,
                CoviObjectPathPostingV2::parse,
                |item| {
                    (
                        item.file_ref,
                        item.object_type_id,
                        item.path_ref,
                        item.segment_id,
                        item.row_start,
                    )
                },
            )?;
        }
        CoviPostingRepresentationV2::DimensionalBucketRefs => {
            parse_fixed_postings(
                payload,
                CoviDimensionalBucketPostingV2::LEN,
                CoviDimensionalBucketPostingV2::parse,
                |item| {
                    (
                        item.dimensional_bucket_ref,
                        item.file_ref,
                        item.table_id,
                        item.segment_id,
                        item.morsel_id,
                    )
                },
            )?;
        }
        CoviPostingRepresentationV2::CoverageSetRef => {
            if !payload.is_empty()
                || posting.item_count != 1
                || posting.coverage_set_ref == ABSENT_U32
            {
                return Err(CoveError::BadCovi);
            }
        }
        CoviPostingRepresentationV2::Extension => return Err(CoveError::BadCovi),
    }
    Ok(())
}

fn validate_row_ordinal_refs(
    posting: &CoviPostingsHeaderV2,
    payload: &[u8],
    row_ordinal_sets: &[CoviRowOrdinalSetHeaderV2],
) -> Result<(), CoveError> {
    if payload.len() % 4 != 0 {
        return Err(CoveError::BadCovi);
    }
    let mut previous: Option<u32> = None;
    for chunk in payload.chunks_exact(4) {
        let row_ordinal_set_ref = u32::from_le_bytes(chunk.try_into().expect("chunk len checked"));
        if previous.is_some_and(|previous| row_ordinal_set_ref <= previous) {
            return Err(CoveError::BadCovi);
        }
        let Some(row_set) = row_ordinal_sets.get(row_ordinal_set_ref as usize) else {
            return Err(CoveError::BadCovi);
        };
        match posting.representation {
            CoviPostingRepresentationV2::RowOrdinalBitmap => match row_set.bitmap_kind {
                CoviBitmapKindV2::DenseBitsetLsb0
                | CoviBitmapKindV2::SortedU32
                | CoviBitmapKindV2::SortedU64 => {}
                _ => return Err(CoveError::BadCovi),
            },
            CoviPostingRepresentationV2::RowOrdinalDeltaVarint => {
                if row_set.bitmap_kind != CoviBitmapKindV2::DeltaVarintU32 {
                    return Err(CoveError::BadCovi);
                }
            }
            _ => return Err(CoveError::BadCovi),
        }
        previous = Some(row_ordinal_set_ref);
    }
    Ok(())
}

fn validate_row_ordinal_set_payload(
    row_set: &CoviRowOrdinalSetHeaderV2,
    payload: &[u8],
) -> Result<(), CoveError> {
    match row_set.bitmap_kind {
        CoviBitmapKindV2::DenseBitsetLsb0 => {
            let expected = usize::try_from((row_set.universe_row_count + 7) / 8)
                .map_err(|_| CoveError::ArithOverflow)?;
            if payload.len() != expected {
                return Err(CoveError::BadCovi);
            }
            if row_set.universe_row_count % 8 != 0 && !payload.is_empty() {
                let used_bits = (row_set.universe_row_count % 8) as u8;
                let high_mask = !((1u8 << used_bits) - 1);
                if payload[payload.len() - 1] & high_mask != 0 {
                    return Err(CoveError::BadCovi);
                }
            }
            let count = payload
                .iter()
                .map(|byte| byte.count_ones() as u64)
                .sum::<u64>();
            if count != row_set.set_row_count {
                return Err(CoveError::BadCovi);
            }
        }
        CoviBitmapKindV2::SortedU32 => {
            if payload.len() != row_set.set_row_count as usize * 4 {
                return Err(CoveError::BadCovi);
            }
            let mut previous: Option<u32> = None;
            for chunk in payload.chunks_exact(4) {
                let value = u32::from_le_bytes(chunk.try_into().expect("chunk len checked"));
                if u64::from(value) >= row_set.universe_row_count
                    || previous.is_some_and(|previous| value <= previous)
                {
                    return Err(CoveError::BadCovi);
                }
                previous = Some(value);
            }
        }
        CoviBitmapKindV2::SortedU64 => {
            if payload.len() != row_set.set_row_count as usize * 8 {
                return Err(CoveError::BadCovi);
            }
            let mut previous: Option<u64> = None;
            for chunk in payload.chunks_exact(8) {
                let value = u64::from_le_bytes(chunk.try_into().expect("chunk len checked"));
                if value >= row_set.universe_row_count
                    || previous.is_some_and(|previous| value <= previous)
                {
                    return Err(CoveError::BadCovi);
                }
                previous = Some(value);
            }
        }
        CoviBitmapKindV2::DeltaVarintU32 => {
            let mut index = 0usize;
            let mut previous: Option<u64> = None;
            for ordinal_index in 0..row_set.set_row_count {
                let delta = read_leb128_u64(payload, &mut index)?;
                if ordinal_index != 0 && delta == 0 {
                    return Err(CoveError::BadCovi);
                }
                let value = previous
                    .unwrap_or(0)
                    .checked_add(delta)
                    .ok_or(CoveError::ArithOverflow)?;
                if value >= row_set.universe_row_count {
                    return Err(CoveError::BadCovi);
                }
                previous = Some(value);
            }
            if index != payload.len() {
                return Err(CoveError::BadCovi);
            }
        }
        CoviBitmapKindV2::RegisteredRoaring32
        | CoviBitmapKindV2::RegisteredRoaring64
        | CoviBitmapKindV2::RangeList
        | CoviBitmapKindV2::Extension => return Err(CoveError::BadCovi),
    }
    Ok(())
}

fn read_leb128_u64(bytes: &[u8], index: &mut usize) -> Result<u64, CoveError> {
    let mut shift = 0u32;
    let mut value = 0u64;
    while *index < bytes.len() {
        let byte = bytes[*index];
        *index += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            return Err(CoveError::BadCovi);
        }
    }
    Err(CoveError::BufferTooShort)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviPostingsBlockV2 {
    pub header: CoviPostingsBlockHeaderV2,
    pub postings: Vec<CoviPostingsHeaderV2>,
    pub row_ordinal_sets: Vec<CoviRowOrdinalSetHeaderV2>,
    pub payload: Vec<u8>,
}

impl CoviPostingsBlockV2 {
    pub fn posting_payload(&self, posting: &CoviPostingsHeaderV2) -> Result<&[u8], CoveError> {
        let start = usize::try_from(posting.payload_offset).map_err(|_| CoveError::OffsetRange)?;
        let len = usize::try_from(posting.payload_length).map_err(|_| CoveError::OffsetRange)?;
        let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        if end > self.payload.len() {
            return Err(CoveError::OffsetRange);
        }
        Ok(&self.payload[start..end])
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = CoviPostingsBlockHeaderV2::parse(bytes)?;
        let postings_start =
            usize::try_from(header.postings_headers_offset).map_err(|_| CoveError::OffsetRange)?;
        let postings_len = usize::try_from(
            header
                .postings_count
                .checked_mul(CoviPostingsHeaderV2::LEN as u32)
                .ok_or(CoveError::ArithOverflow)?,
        )
        .map_err(|_| CoveError::ArithOverflow)?;
        let postings_end = postings_start
            .checked_add(postings_len)
            .ok_or(CoveError::ArithOverflow)?;
        if postings_start < CoviPostingsBlockHeaderV2::LEN || postings_end > bytes.len() {
            return Err(CoveError::BadCovi);
        }
        let postings = bytes[postings_start..postings_end]
            .chunks_exact(CoviPostingsHeaderV2::LEN)
            .map(CoviPostingsHeaderV2::parse)
            .collect::<Result<Vec<_>, _>>()?;

        let row_ordinal_sets = if header.row_ordinal_set_count == 0 {
            Vec::new()
        } else {
            let start = usize::try_from(header.row_ordinal_headers_offset)
                .map_err(|_| CoveError::OffsetRange)?;
            let len = usize::try_from(
                header
                    .row_ordinal_set_count
                    .checked_mul(CoviRowOrdinalSetHeaderV2::LEN as u32)
                    .ok_or(CoveError::ArithOverflow)?,
            )
            .map_err(|_| CoveError::ArithOverflow)?;
            let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
            if start < CoviPostingsBlockHeaderV2::LEN || end > bytes.len() {
                return Err(CoveError::BadCovi);
            }
            bytes[start..end]
                .chunks_exact(CoviRowOrdinalSetHeaderV2::LEN)
                .map(CoviRowOrdinalSetHeaderV2::parse)
                .collect::<Result<Vec<_>, _>>()?
        };

        let payload_start =
            usize::try_from(header.postings_payload_offset).map_err(|_| CoveError::OffsetRange)?;
        let payload_len =
            usize::try_from(header.postings_payload_length).map_err(|_| CoveError::OffsetRange)?;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or(CoveError::ArithOverflow)?;
        if payload_start < postings_end || payload_end != bytes.len() {
            return Err(CoveError::BadCovi);
        }
        for (index, posting) in postings.iter().enumerate() {
            if posting.postings_ref as usize != index {
                return Err(CoveError::BadCovi);
            }
            let relative_start =
                usize::try_from(posting.payload_offset).map_err(|_| CoveError::OffsetRange)?;
            let len =
                usize::try_from(posting.payload_length).map_err(|_| CoveError::OffsetRange)?;
            let relative_end = relative_start
                .checked_add(len)
                .ok_or(CoveError::ArithOverflow)?;
            if relative_end > payload_len {
                return Err(CoveError::BadCovi);
            }
            validate_covi_posting_payload(
                posting,
                &bytes[payload_start + relative_start..payload_start + relative_end],
                &row_ordinal_sets,
            )?;
        }
        for (index, row_set) in row_ordinal_sets.iter().enumerate() {
            if row_set.row_ordinal_set_ref as usize != index {
                return Err(CoveError::BadCovi);
            }
            let relative_start =
                usize::try_from(row_set.payload_offset).map_err(|_| CoveError::OffsetRange)?;
            let len =
                usize::try_from(row_set.payload_length).map_err(|_| CoveError::OffsetRange)?;
            let relative_end = relative_start
                .checked_add(len)
                .ok_or(CoveError::ArithOverflow)?;
            if relative_end > payload_len {
                return Err(CoveError::BadCovi);
            }
            validate_row_ordinal_set_payload(
                row_set,
                &bytes[payload_start + relative_start..payload_start + relative_end],
            )?;
        }

        Ok(Self {
            header,
            postings,
            row_ordinal_sets,
            payload: bytes[payload_start..payload_end].to_vec(),
        })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        for (index, posting) in self.postings.iter().enumerate() {
            if posting.postings_ref as usize != index {
                return Err(CoveError::BadCovi);
            }
        }
        let postings_headers_offset = CoviPostingsBlockHeaderV2::LEN as u64;
        let row_ordinal_headers_offset = if self.row_ordinal_sets.is_empty() {
            0
        } else {
            postings_headers_offset
                .checked_add(
                    u64::try_from(self.postings.len()).map_err(|_| CoveError::ArithOverflow)?
                        * CoviPostingsHeaderV2::LEN as u64,
                )
                .ok_or(CoveError::ArithOverflow)?
        };
        let postings_payload_offset = postings_headers_offset
            .checked_add(
                u64::try_from(self.postings.len()).map_err(|_| CoveError::ArithOverflow)?
                    * CoviPostingsHeaderV2::LEN as u64,
            )
            .and_then(|offset| {
                offset.checked_add(
                    u64::try_from(self.row_ordinal_sets.len()).ok()?
                        * CoviRowOrdinalSetHeaderV2::LEN as u64,
                )
            })
            .ok_or(CoveError::ArithOverflow)?;
        let mut header = self.header.clone();
        header.magic = CoviPostingsBlockHeaderV2::MAGIC;
        header.version_major = 2;
        header.header_len = CoviPostingsBlockHeaderV2::LEN as u16;
        header.postings_header_len = CoviPostingsHeaderV2::LEN as u16;
        header.postings_count =
            u32::try_from(self.postings.len()).map_err(|_| CoveError::ArithOverflow)?;
        header.row_ordinal_set_count =
            u32::try_from(self.row_ordinal_sets.len()).map_err(|_| CoveError::ArithOverflow)?;
        header.postings_headers_offset = postings_headers_offset;
        header.row_ordinal_headers_offset = row_ordinal_headers_offset;
        header.postings_payload_offset = postings_payload_offset;
        header.postings_payload_length =
            u64::try_from(self.payload.len()).map_err(|_| CoveError::ArithOverflow)?;

        let mut out = Vec::with_capacity(
            CoviPostingsBlockHeaderV2::LEN
                + self.postings.len() * CoviPostingsHeaderV2::LEN
                + self.row_ordinal_sets.len() * CoviRowOrdinalSetHeaderV2::LEN
                + self.payload.len(),
        );
        out.extend_from_slice(&header.serialize());
        for posting in &self.postings {
            out.extend_from_slice(&posting.serialize()?);
        }
        for row_ordinal_set in &self.row_ordinal_sets {
            out.extend_from_slice(&row_ordinal_set.serialize()?);
        }
        out.extend_from_slice(&self.payload);
        CoviPostingsBlockV2::parse(&out)?;
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviAggregateAnswerBlockHeaderV2 {
    pub magic: [u8; 4],
    pub version_major: u16,
    pub version_minor: u16,
    pub header_len: u16,
    pub aggregate_answer_len: u16,
    pub aggregate_block_id: u32,
    pub index_root_id: u32,
    pub aggregate_answer_count: u32,
    pub aggregate_answers_offset: u64,
    pub aggregate_payload_offset: u64,
    pub aggregate_payload_length: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl CoviAggregateAnswerBlockHeaderV2 {
    pub const LEN: usize = 56;
    pub const MAGIC: [u8; 4] = *b"CIA2";

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            magic: read_array(bytes, 0)?,
            version_major: read_u16(bytes, 4)?,
            version_minor: read_u16(bytes, 6)?,
            header_len: read_u16(bytes, 8)?,
            aggregate_answer_len: read_u16(bytes, 10)?,
            aggregate_block_id: read_u32(bytes, 12)?,
            index_root_id: read_u32(bytes, 16)?,
            aggregate_answer_count: read_u32(bytes, 20)?,
            aggregate_answers_offset: read_u64(bytes, 24)?,
            aggregate_payload_offset: read_u64(bytes, 32)?,
            aggregate_payload_length: read_u64(bytes, 40)?,
            flags: read_u32(bytes, 48)?,
            checksum: read_u32(bytes, 52)?,
        };
        if header.magic != Self::MAGIC {
            return Err(CoveError::BadMagic);
        }
        if header.version_major != 2
            || header.header_len as usize != Self::LEN
            || header.aggregate_answer_len as usize != CoviAggregateAnswerV2::LEN
        {
            return Err(CoveError::BadVersion);
        }
        verify_crc(&bytes[..Self::LEN], 52, header.checksum)?;
        checked_end(
            header.aggregate_answers_offset,
            header.aggregate_answer_count as u64 * CoviAggregateAnswerV2::LEN as u64,
        )?;
        checked_end(
            header.aggregate_payload_offset,
            header.aggregate_payload_length,
        )?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version_major.to_le_bytes());
        out[6..8].copy_from_slice(&self.version_minor.to_le_bytes());
        out[8..10].copy_from_slice(&self.header_len.to_le_bytes());
        out[10..12].copy_from_slice(&self.aggregate_answer_len.to_le_bytes());
        out[12..16].copy_from_slice(&self.aggregate_block_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.index_root_id.to_le_bytes());
        out[20..24].copy_from_slice(&self.aggregate_answer_count.to_le_bytes());
        out[24..32].copy_from_slice(&self.aggregate_answers_offset.to_le_bytes());
        out[32..40].copy_from_slice(&self.aggregate_payload_offset.to_le_bytes());
        out[40..48].copy_from_slice(&self.aggregate_payload_length.to_le_bytes());
        out[48..52].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[52..56].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviAggregateAnswerV2 {
    pub aggregate_answer_ref: u32,
    pub index_root_id: u32,
    pub aggregate_kind: u16,
    pub exactness: u8,
    pub null_semantics: u8,
    pub flags: u16,
    pub row_count: u64,
    pub null_count: u64,
    pub non_null_count: u64,
    pub value_ref: u32,
    pub predicate_form_ref: u32,
    pub snapshot_validity_ref: u32,
    pub checksum: u32,
}

impl CoviAggregateAnswerV2 {
    pub const LEN: usize = 54;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            aggregate_answer_ref: read_u32(bytes, 0)?,
            index_root_id: read_u32(bytes, 4)?,
            aggregate_kind: read_u16(bytes, 8)?,
            exactness: read_u8(bytes, 10)?,
            null_semantics: read_u8(bytes, 11)?,
            flags: read_u16(bytes, 12)?,
            row_count: read_u64(bytes, 14)?,
            null_count: read_u64(bytes, 22)?,
            non_null_count: read_u64(bytes, 30)?,
            value_ref: read_u32(bytes, 38)?,
            predicate_form_ref: read_u32(bytes, 42)?,
            snapshot_validity_ref: read_u32(bytes, 46)?,
            checksum: read_u32(bytes, 50)?,
        };
        verify_crc(&bytes[..Self::LEN], 50, item.checksum)?;
        if item.null_count > item.row_count || item.non_null_count > item.row_count {
            return Err(CoveError::BadCovi);
        }
        Ok(item)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.aggregate_answer_ref.to_le_bytes());
        out[4..8].copy_from_slice(&self.index_root_id.to_le_bytes());
        out[8..10].copy_from_slice(&self.aggregate_kind.to_le_bytes());
        out[10] = self.exactness;
        out[11] = self.null_semantics;
        out[12..14].copy_from_slice(&self.flags.to_le_bytes());
        out[14..22].copy_from_slice(&self.row_count.to_le_bytes());
        out[22..30].copy_from_slice(&self.null_count.to_le_bytes());
        out[30..38].copy_from_slice(&self.non_null_count.to_le_bytes());
        out[38..42].copy_from_slice(&self.value_ref.to_le_bytes());
        out[42..46].copy_from_slice(&self.predicate_form_ref.to_le_bytes());
        out[46..50].copy_from_slice(&self.snapshot_validity_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[50..54].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviAggregateAnswerBlockV2 {
    pub header: CoviAggregateAnswerBlockHeaderV2,
    pub answers: Vec<CoviAggregateAnswerV2>,
    pub payload: Vec<u8>,
}

impl CoviAggregateAnswerBlockV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = CoviAggregateAnswerBlockHeaderV2::parse(bytes)?;
        let answers_start =
            usize::try_from(header.aggregate_answers_offset).map_err(|_| CoveError::OffsetRange)?;
        let answers_len = usize::try_from(
            header
                .aggregate_answer_count
                .checked_mul(CoviAggregateAnswerV2::LEN as u32)
                .ok_or(CoveError::ArithOverflow)?,
        )
        .map_err(|_| CoveError::ArithOverflow)?;
        let answers_end = answers_start
            .checked_add(answers_len)
            .ok_or(CoveError::ArithOverflow)?;
        if answers_start < CoviAggregateAnswerBlockHeaderV2::LEN || answers_end > bytes.len() {
            return Err(CoveError::BadCovi);
        }
        let answers = bytes[answers_start..answers_end]
            .chunks_exact(CoviAggregateAnswerV2::LEN)
            .map(CoviAggregateAnswerV2::parse)
            .collect::<Result<Vec<_>, _>>()?;
        let mut refs = BTreeSet::new();
        for answer in &answers {
            if !refs.insert(answer.aggregate_answer_ref) {
                return Err(CoveError::BadCovi);
            }
        }

        let payload_start =
            usize::try_from(header.aggregate_payload_offset).map_err(|_| CoveError::OffsetRange)?;
        let payload_len =
            usize::try_from(header.aggregate_payload_length).map_err(|_| CoveError::OffsetRange)?;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or(CoveError::ArithOverflow)?;
        if payload_len == 0 {
            if payload_start != 0 && payload_start != answers_end {
                return Err(CoveError::BadCovi);
            }
            return Ok(Self {
                header,
                answers,
                payload: Vec::new(),
            });
        }
        if payload_start < answers_end || payload_end != bytes.len() {
            return Err(CoveError::BadCovi);
        }
        Ok(Self {
            header,
            answers,
            payload: bytes[payload_start..payload_end].to_vec(),
        })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let answers_offset = CoviAggregateAnswerBlockHeaderV2::LEN as u64;
        let aggregate_payload_offset = answers_offset
            .checked_add(
                u64::try_from(self.answers.len()).map_err(|_| CoveError::ArithOverflow)?
                    * CoviAggregateAnswerV2::LEN as u64,
            )
            .ok_or(CoveError::ArithOverflow)?;
        let mut header = self.header.clone();
        header.magic = CoviAggregateAnswerBlockHeaderV2::MAGIC;
        header.version_major = 2;
        header.header_len = CoviAggregateAnswerBlockHeaderV2::LEN as u16;
        header.aggregate_answer_len = CoviAggregateAnswerV2::LEN as u16;
        header.aggregate_answer_count =
            u32::try_from(self.answers.len()).map_err(|_| CoveError::ArithOverflow)?;
        header.aggregate_answers_offset = answers_offset;
        header.aggregate_payload_offset = if self.payload.is_empty() {
            0
        } else {
            aggregate_payload_offset
        };
        header.aggregate_payload_length =
            u64::try_from(self.payload.len()).map_err(|_| CoveError::ArithOverflow)?;

        let mut out = Vec::with_capacity(
            CoviAggregateAnswerBlockHeaderV2::LEN
                + self.answers.len() * CoviAggregateAnswerV2::LEN
                + self.payload.len(),
        );
        out.extend_from_slice(&header.serialize());
        for answer in &self.answers {
            out.extend_from_slice(&answer.serialize());
        }
        out.extend_from_slice(&self.payload);
        CoviAggregateAnswerBlockV2::parse(&out)?;
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviArtifactV2 {
    pub postscript: CoviPostscriptV2,
    pub header: CoviHeaderV2,
    pub sections: Vec<CoviSectionEntryV2>,
    pub referenced_files: Vec<CoviReferencedFileV2>,
    pub snapshot_validity: Vec<CoviSnapshotValidityV2>,
    pub index_roots: Vec<CoviIndexRootV2>,
    pub capabilities: Vec<IndexCapabilityV2>,
    pub key_blocks: Vec<CoviKeyBlockV2>,
    pub entry_blocks: Vec<CoviEntryBlockV2>,
    pub postings_blocks: Vec<CoviPostingsBlockV2>,
    pub aggregate_answer_blocks: Vec<CoviAggregateAnswerBlockV2>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviSectionPayloadV2 {
    pub section_id: u32,
    pub section_kind: CoviSectionKindV2,
    pub payload: Vec<u8>,
    pub item_count: u64,
    pub required_features: u64,
    pub optional_features: u64,
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
        let referenced_files = CoviReferencedFileV2::parse_many(parse_fixed_region(
            bytes,
            header.referenced_files_offset,
            header.referenced_file_count,
            CoviReferencedFileV2::LEN,
            postscript_offset,
        )?)?;
        let snapshot_validity = CoviSnapshotValidityV2::parse_many(parse_fixed_region(
            bytes,
            header.snapshot_validity_offset,
            header.snapshot_validity_count,
            CoviSnapshotValidityV2::LEN,
            postscript_offset,
        )?)?;
        let index_roots = CoviIndexRootV2::parse_many(parse_fixed_region(
            bytes,
            header.index_roots_offset,
            header.index_root_count,
            CoviIndexRootV2::LEN,
            postscript_offset,
        )?)?;
        let capabilities = IndexCapabilityV2::parse_many(parse_fixed_region(
            bytes,
            header.capabilities_offset,
            header.capability_count,
            IndexCapabilityV2::LEN,
            postscript_offset,
        )?)?;
        let mut key_blocks = Vec::new();
        let mut entry_blocks = Vec::new();
        let mut postings_blocks = Vec::new();
        let mut aggregate_answer_blocks = Vec::new();
        for section in &sections {
            let payload = covi_section_payload(bytes, section)?;
            match section.section_kind {
                CoviSectionKindV2::KeyBlock => key_blocks.push(CoviKeyBlockV2::parse(&payload)?),
                CoviSectionKindV2::EntryBlock => {
                    entry_blocks.push(CoviEntryBlockV2::parse(&payload)?)
                }
                CoviSectionKindV2::PostingsBlock => {
                    postings_blocks.push(CoviPostingsBlockV2::parse(&payload)?)
                }
                CoviSectionKindV2::AggregateAnswerBlock => {
                    aggregate_answer_blocks.push(CoviAggregateAnswerBlockV2::parse(&payload)?)
                }
                _ => {}
            }
        }
        Ok(Self {
            postscript,
            header,
            sections,
            referenced_files,
            snapshot_validity,
            index_roots,
            capabilities,
            key_blocks,
            entry_blocks,
            postings_blocks,
            aggregate_answer_blocks,
        })
    }

    pub fn section_payload_from_bytes<'a>(
        &self,
        bytes: &'a [u8],
        section_id: u32,
    ) -> Result<std::borrow::Cow<'a, [u8]>, CoveError> {
        let section = self
            .sections
            .iter()
            .find(|section| section.section_id == section_id)
            .ok_or(CoveError::BadCovi)?;
        covi_section_payload(bytes, section)
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
            referenced_files: Vec::new(),
            snapshot_validity: Vec::new(),
            index_roots: Vec::new(),
            capabilities: Vec::new(),
            key_blocks: Vec::new(),
            entry_blocks: Vec::new(),
            postings_blocks: Vec::new(),
            aggregate_answer_blocks: Vec::new(),
        }
    }

    pub fn serialize_empty(&self) -> Result<Vec<u8>, CoveError> {
        if !self.sections.is_empty()
            || !self.referenced_files.is_empty()
            || !self.snapshot_validity.is_empty()
            || !self.index_roots.is_empty()
            || !self.capabilities.is_empty()
            || !self.key_blocks.is_empty()
            || !self.entry_blocks.is_empty()
            || !self.postings_blocks.is_empty()
            || !self.aggregate_answer_blocks.is_empty()
            || self.header.section_count != 0
            || self.header.referenced_file_count != 0
            || self.header.snapshot_validity_count != 0
            || self.header.index_root_count != 0
            || self.header.capability_count != 0
        {
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

    pub fn serialize_metadata_only(
        dataset_id: [u8; 16],
        snapshot_id: [u8; 16],
        referenced_files: &[CoviReferencedFileV2],
        snapshot_validity: &[CoviSnapshotValidityV2],
        index_roots: &[CoviIndexRootV2],
        capabilities: &[IndexCapabilityV2],
    ) -> Result<Vec<u8>, CoveError> {
        Self::serialize_with_sections(
            dataset_id,
            snapshot_id,
            referenced_files,
            snapshot_validity,
            index_roots,
            capabilities,
            &[],
        )
    }

    pub fn serialize_with_sections(
        dataset_id: [u8; 16],
        snapshot_id: [u8; 16],
        referenced_files: &[CoviReferencedFileV2],
        snapshot_validity: &[CoviSnapshotValidityV2],
        index_roots: &[CoviIndexRootV2],
        capabilities: &[IndexCapabilityV2],
        section_payloads: &[CoviSectionPayloadV2],
    ) -> Result<Vec<u8>, CoveError> {
        validate_dense_ids(referenced_files, |item| item.file_ref)?;
        validate_dense_ids(snapshot_validity, |item| item.snapshot_validity_ref)?;
        validate_dense_ids(index_roots, |item| item.index_root_id)?;
        let mut previous_section_id = 0u32;
        for section in section_payloads {
            if section.section_id == 0 || section.section_id <= previous_section_id {
                return Err(CoveError::BadCovi);
            }
            previous_section_id = section.section_id;
        }

        let mut regions = Vec::new();
        let section_directory_length = u64::try_from(
            section_payloads
                .len()
                .checked_mul(COVI_SECTION_ENTRY_LEN)
                .ok_or(CoveError::ArithOverflow)?,
        )
        .map_err(|_| CoveError::ArithOverflow)?;
        let mut cursor = (COVI_HEADER_LEN as u64)
            .checked_add(section_directory_length)
            .ok_or(CoveError::ArithOverflow)?;
        let referenced_files_offset =
            append_region(&mut regions, &mut cursor, referenced_files, |item| {
                item.serialize().map(|bytes| bytes.to_vec())
            })?;
        let snapshot_validity_offset =
            append_region(&mut regions, &mut cursor, snapshot_validity, |item| {
                if item.valid_until_us < item.valid_from_us {
                    return Err(CoveError::BadCovi);
                }
                Ok(item.serialize().to_vec())
            })?;
        let index_roots_offset = append_region(&mut regions, &mut cursor, index_roots, |item| {
            item.serialize().map(|bytes| bytes.to_vec())
        })?;
        let capabilities_offset = append_region(&mut regions, &mut cursor, capabilities, |item| {
            item.serialize().map(|bytes| bytes.to_vec())
        })?;
        let mut sections = Vec::with_capacity(section_payloads.len());
        let mut section_bytes = Vec::new();
        for payload in section_payloads {
            let len = u64::try_from(payload.payload.len()).map_err(|_| CoveError::ArithOverflow)?;
            sections.push(CoviSectionEntryV2 {
                section_id: payload.section_id,
                section_kind: payload.section_kind,
                flags: 0,
                offset: cursor,
                length: len,
                uncompressed_length: len,
                item_count: payload.item_count,
                compression: CompressionCodec::None as u8,
                encryption: 0,
                alignment_log2: 0,
                reserved0: 0,
                required_features: payload.required_features,
                optional_features: payload.optional_features,
                crc32c: checksum::crc32c(&payload.payload),
                checksum: 0,
            });
            cursor = cursor.checked_add(len).ok_or(CoveError::ArithOverflow)?;
            section_bytes.extend_from_slice(&payload.payload);
        }
        let file_len = cursor
            .checked_add(COVI_TAIL_LEN as u64)
            .ok_or(CoveError::ArithOverflow)?;

        let string_table_section_ref = section_payloads
            .iter()
            .find(|section| section.section_kind == CoviSectionKindV2::StringTable)
            .map(|section| section.section_id)
            .unwrap_or(u32::MAX);
        let header = CoviHeaderV2 {
            magic: MAGIC_COVI,
            header_len: COVI_HEADER_LEN,
            version_major: VERSION_MAJOR_V1,
            version_minor: 0,
            flags: 0,
            index_artifact_id: [0u8; 16],
            dataset_id,
            snapshot_id,
            section_count: u32::try_from(section_payloads.len())
                .map_err(|_| CoveError::ArithOverflow)?,
            referenced_file_count: u32::try_from(referenced_files.len())
                .map_err(|_| CoveError::ArithOverflow)?,
            snapshot_validity_count: u32::try_from(snapshot_validity.len())
                .map_err(|_| CoveError::ArithOverflow)?,
            index_root_count: u32::try_from(index_roots.len())
                .map_err(|_| CoveError::ArithOverflow)?,
            capability_count: u32::try_from(capabilities.len())
                .map_err(|_| CoveError::ArithOverflow)?,
            section_directory_offset: COVI_HEADER_LEN as u64,
            section_directory_length,
            referenced_files_offset,
            snapshot_validity_offset,
            index_roots_offset,
            capabilities_offset,
            string_table_section_ref,
            created_at_us: 0,
            reserved: [0u8; 24],
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

        let mut out =
            Vec::with_capacity(usize::try_from(file_len).map_err(|_| CoveError::ArithOverflow)?);
        out.extend_from_slice(&header.serialize());
        for section in &sections {
            out.extend_from_slice(&section.serialize()?);
        }
        out.extend_from_slice(&regions);
        out.extend_from_slice(&section_bytes);
        out.extend_from_slice(&postscript.serialize());
        out.extend_from_slice(&POSTSCRIPT_VERSION_V1.to_le_bytes());
        out.extend_from_slice(&(COVI_POSTSCRIPT_LEN as u16).to_le_bytes());
        out.extend_from_slice(&MAGIC_COVI);
        CoviArtifactV2::parse(&out)?;
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

fn covi_section_payload<'a>(
    bytes: &'a [u8],
    entry: &CoviSectionEntryV2,
) -> Result<std::borrow::Cow<'a, [u8]>, CoveError> {
    let start = usize::try_from(entry.offset).map_err(|_| CoveError::OffsetRange)?;
    let end = usize::try_from(entry.end_offset()?).map_err(|_| CoveError::OffsetRange)?;
    if end > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    cove_core::compression::section_payload_from_raw(
        &bytes[start..end],
        entry.length,
        entry.uncompressed_length,
        entry.compression,
        entry.crc32c,
    )
}

fn append_region<T, F>(
    out: &mut Vec<u8>,
    cursor: &mut u64,
    items: &[T],
    mut serialize: F,
) -> Result<u64, CoveError>
where
    F: FnMut(&T) -> Result<Vec<u8>, CoveError>,
{
    if items.is_empty() {
        return Ok(0);
    }
    let offset = *cursor;
    for item in items {
        let bytes = serialize(item)?;
        *cursor = cursor
            .checked_add(u64::try_from(bytes.len()).map_err(|_| CoveError::ArithOverflow)?)
            .ok_or(CoveError::ArithOverflow)?;
        out.extend_from_slice(&bytes);
    }
    Ok(offset)
}

fn parse_fixed_region(
    bytes: &[u8],
    offset: u64,
    count: u32,
    item_len: usize,
    section_limit: usize,
) -> Result<&[u8], CoveError> {
    if count == 0 {
        if offset != 0 {
            return Err(CoveError::BadCovi);
        }
        return Ok(&[]);
    }
    if offset == 0 {
        return Err(CoveError::BadCovi);
    }
    let start = usize::try_from(offset).map_err(|_| CoveError::OffsetRange)?;
    let item_count = usize::try_from(count).map_err(|_| CoveError::ArithOverflow)?;
    let len = item_count
        .checked_mul(item_len)
        .ok_or(CoveError::ArithOverflow)?;
    let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > section_limit || end > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    Ok(&bytes[start..end])
}

fn validate_dense_ids<T, F>(items: &[T], id: F) -> Result<(), CoveError>
where
    F: Fn(&T) -> u32,
{
    for (index, item) in items.iter().enumerate() {
        if id(item) as usize != index {
            return Err(CoveError::BadCovi);
        }
    }
    Ok(())
}

fn parse_dense_many<T, F, Id>(
    bytes: &[u8],
    item_len: usize,
    parse: F,
    id: Id,
) -> Result<Vec<T>, CoveError>
where
    F: Fn(&[u8]) -> Result<T, CoveError>,
    Id: Fn(&T) -> u32,
{
    if bytes.len() % item_len != 0 {
        return Err(CoveError::BadCovi);
    }
    let mut out = Vec::new();
    for (index, chunk) in bytes.chunks_exact(item_len).enumerate() {
        let item = parse(chunk)?;
        if id(&item) as usize != index {
            return Err(CoveError::BadCovi);
        }
        out.push(item);
    }
    Ok(out)
}

fn validate_covi_entry_refs(entries: &[CoviIndexEntryV2]) -> Result<(), CoveError> {
    for (index, entry) in entries.iter().enumerate() {
        if entry.entry_ref as usize != index {
            return Err(CoveError::BadCovi);
        }
        if entry.next_duplicate_ref != ABSENT_U32
            && entry.next_duplicate_ref as usize >= entries.len()
        {
            return Err(CoveError::BadCovi);
        }
        let mut slow = entry.next_duplicate_ref;
        let mut fast = entry.next_duplicate_ref;
        while slow != ABSENT_U32 {
            slow = entries[slow as usize].next_duplicate_ref;
            fast = if fast == ABSENT_U32 {
                ABSENT_U32
            } else {
                entries[fast as usize].next_duplicate_ref
            };
            fast = if fast == ABSENT_U32 {
                ABSENT_U32
            } else {
                entries[fast as usize].next_duplicate_ref
            };
            if slow != ABSENT_U32 && slow == fast {
                return Err(CoveError::BadCovi);
            }
        }
    }
    Ok(())
}

fn validate_row_range_postings(rows: &[CoviRowRangePostingV2]) -> Result<(), CoveError> {
    let mut previous: Option<&CoviRowRangePostingV2> = None;
    for row in rows {
        checked_end(row.row_start, row.row_count)?;
        if row.row_count == 0 {
            return Err(CoveError::BadCovi);
        }
        if let Some(prev) = previous {
            let prev_scope = (
                prev.file_ref,
                prev.table_id,
                prev.segment_id,
                prev.morsel_id,
            );
            let scope = (row.file_ref, row.table_id, row.segment_id, row.morsel_id);
            if scope < prev_scope {
                return Err(CoveError::BadCovi);
            }
            if scope == prev_scope {
                let prev_end = prev
                    .row_start
                    .checked_add(prev.row_count)
                    .ok_or(CoveError::ArithOverflow)?;
                if row.row_start <= prev_end {
                    return Err(CoveError::BadCovi);
                }
            }
        }
        previous = Some(row);
    }
    Ok(())
}

fn checked_end(offset: u64, length: u64) -> Result<u64, CoveError> {
    offset.checked_add(length).ok_or(CoveError::ArithOverflow)
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

    fn referenced_file(file_ref: u32) -> CoviReferencedFileV2 {
        CoviReferencedFileV2 {
            file_ref,
            flags: 0,
            file_id: [3u8; 16],
            file_len: 128,
            footer_crc32c: 9,
            digest_algorithm: 0,
            digest_len: 0,
            digest_offset: 0,
            uri_ref: u32::MAX,
            schema_fingerprint_ref: u32::MAX,
            checksum: 0,
        }
    }

    fn snapshot_validity(snapshot_validity_ref: u32) -> CoviSnapshotValidityV2 {
        CoviSnapshotValidityV2 {
            snapshot_validity_ref,
            dataset_id: [1u8; 16],
            snapshot_id: [2u8; 16],
            schema_fingerprint_ref: u32::MAX,
            semantic_map_fingerprint_ref: u32::MAX,
            external_visibility_ref: u32::MAX,
            data_checksum_root_ref: u32::MAX,
            valid_from_us: 0,
            valid_until_us: i64::MAX,
            flags: 0,
            checksum: 0,
        }
    }

    fn index_root(index_root_id: u32) -> CoviIndexRootV2 {
        CoviIndexRootV2 {
            index_root_id,
            indexed_target_kind: CoviIndexedTargetKindV2::TableColumn,
            index_kind: CoviIndexKindV2::Sorted,
            coverage_granularity: cove_coverage::CoverageGranularityV2::Morsel as u8,
            proof_strength: CoverageProofStrengthV2::ExactConservative as u8,
            exactness: cove_coverage::CoverageExactnessV2::Exact as u8,
            flags: 0,
            table_id: 7,
            column_id: 8,
            object_type_id: u32::MAX,
            property_id: u32::MAX,
            path_ref: u32::MAX,
            semantic_dimension_ref: u32::MAX,
            logical_type: 1,
            physical_kind: 1,
            key_encoding_kind: CoviKeyEncodingKindV2::CanonicalValueBytes as u8,
            comparator_kind: CoviComparatorKindV2::CanonicalOrdering as u16,
            collation_id: 0,
            null_semantics: 0,
            sort_order: 0,
            value_count: 10,
            distinct_count: 0,
            null_count: 0,
            min_key_ref: u32::MAX,
            max_key_ref: u32::MAX,
            key_block_section_id: u32::MAX,
            entry_block_section_id: u32::MAX,
            postings_block_section_id: u32::MAX,
            aggregate_block_section_id: u32::MAX,
            coverage_set_ref: u32::MAX,
            capability_ref: 0,
            snapshot_validity_ref: 0,
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
    fn metadata_only_covi_round_trips_roots_and_capabilities() {
        let bytes = CoviArtifactV2::serialize_metadata_only(
            [1u8; 16],
            [2u8; 16],
            &[referenced_file(0)],
            &[snapshot_validity(0)],
            &[index_root(0)],
            &[index_capability(0)],
        )
        .unwrap();

        let parsed = CoviArtifactV2::parse(&bytes).unwrap();

        assert_eq!(parsed.referenced_files.len(), 1);
        assert_eq!(parsed.snapshot_validity.len(), 1);
        assert_eq!(parsed.index_roots.len(), 1);
        assert_eq!(parsed.capabilities.len(), 1);
        assert_eq!(parsed.index_roots[0].column_id, 8);
        assert_eq!(parsed.capabilities[0].supports_eq, 1);
    }

    #[test]
    fn full_covi_sections_round_trip_index_blocks() {
        let key_data = vec![2, 0, 4, 0, 0, 0, b't', b'e', b's', b't'];
        let key_block = CoviKeyBlockV2 {
            header: CoviKeyBlockHeaderV2 {
                magic: [0; 4],
                version_major: 0,
                version_minor: 0,
                header_len: 0,
                reserved0: 0,
                key_block_id: 0,
                index_root_id: 0,
                key_count: 1,
                encoding_kind: CoviKeyEncodingKindV2::CanonicalValueBytes,
                comparator_kind: CoviComparatorKindV2::CanonicalOrdering,
                flags: 0,
                key_data_offset: 0,
                key_data_length: 0,
                checksum: 0,
            },
            key_data: key_data.clone(),
        }
        .serialize()
        .unwrap();

        let entry_block = CoviEntryBlockV2 {
            header: CoviEntryBlockHeaderV2 {
                magic: [0; 4],
                version_major: 0,
                version_minor: 0,
                header_len: 0,
                entry_len: 0,
                entry_block_id: 0,
                index_root_id: 0,
                entry_count: 0,
                key_block_id: 0,
                postings_block_id: 0,
                aggregate_block_id: u32::MAX,
                entries_offset: 0,
                entries_length: 0,
                flags: 0,
                checksum: 0,
            },
            entries: vec![CoviIndexEntryV2 {
                entry_ref: 0,
                index_root_id: 0,
                entry_id: 0,
                key_kind: CoviKeyEncodingKindV2::CanonicalValueBytes,
                comparator_kind: CoviComparatorKindV2::CanonicalOrdering,
                flags: 0,
                key_offset: 0,
                key_length: key_data.len() as u32,
                key_hash64: 0x9e37_79b1_85eb_ca87,
                postings_ref: 0,
                coverage_set_ref: u32::MAX,
                aggregate_answer_ref: u32::MAX,
                next_duplicate_ref: u32::MAX,
                checksum: 0,
            }],
        }
        .serialize()
        .unwrap();

        let row_range = CoviRowRangePostingV2 {
            file_ref: 0,
            table_id: 7,
            segment_id: 11,
            morsel_id: 0,
            row_start: 3,
            row_count: 2,
            flags: 0,
            checksum: 0,
        }
        .serialize()
        .unwrap();
        let postings_block = CoviPostingsBlockV2 {
            header: CoviPostingsBlockHeaderV2 {
                magic: [0; 4],
                version_major: 0,
                version_minor: 0,
                header_len: 0,
                postings_header_len: 0,
                postings_block_id: 0,
                index_root_id: 0,
                postings_count: 0,
                row_ordinal_set_count: 0,
                postings_headers_offset: 0,
                row_ordinal_headers_offset: 0,
                postings_payload_offset: 0,
                postings_payload_length: 0,
                flags: 0,
                checksum: 0,
            },
            postings: vec![CoviPostingsHeaderV2 {
                postings_ref: 0,
                index_root_id: 0,
                representation: CoviPostingRepresentationV2::RowRangeList,
                target_granularity: cove_coverage::CoverageGranularityV2::Morsel as u8,
                flags: 0,
                item_count: 1,
                payload_offset: 0,
                payload_length: row_range.len() as u64,
                coverage_set_ref: u32::MAX,
                checksum: 0,
            }],
            row_ordinal_sets: Vec::new(),
            payload: row_range.to_vec(),
        }
        .serialize()
        .unwrap();

        let mut root = index_root(0);
        root.distinct_count = 1;
        root.value_count = 2;
        root.min_key_ref = 0;
        root.max_key_ref = 0;
        root.key_block_section_id = 1;
        root.entry_block_section_id = 2;
        root.postings_block_section_id = 3;
        root.capability_ref = 0;
        root.snapshot_validity_ref = 0;
        let mut capability = index_capability(0);
        capability.index_root_id = 0;
        capability.snapshot_validity_ref = 0;

        let bytes = CoviArtifactV2::serialize_with_sections(
            [1u8; 16],
            [2u8; 16],
            &[referenced_file(0)],
            &[snapshot_validity(0)],
            &[root],
            &[capability],
            &[
                CoviSectionPayloadV2 {
                    section_id: 1,
                    section_kind: CoviSectionKindV2::KeyBlock,
                    payload: key_block,
                    item_count: 1,
                    required_features: 0,
                    optional_features: 0,
                },
                CoviSectionPayloadV2 {
                    section_id: 2,
                    section_kind: CoviSectionKindV2::EntryBlock,
                    payload: entry_block,
                    item_count: 1,
                    required_features: 0,
                    optional_features: 0,
                },
                CoviSectionPayloadV2 {
                    section_id: 3,
                    section_kind: CoviSectionKindV2::PostingsBlock,
                    payload: postings_block,
                    item_count: 1,
                    required_features: 0,
                    optional_features: 0,
                },
            ],
        )
        .unwrap();

        let parsed = CoviArtifactV2::parse(&bytes).unwrap();
        assert_eq!(parsed.key_blocks.len(), 1);
        assert_eq!(parsed.entry_blocks.len(), 1);
        assert_eq!(parsed.postings_blocks.len(), 1);
        assert_eq!(parsed.key_blocks[0].key_data, key_data);
        assert_eq!(parsed.entry_blocks[0].entries[0].postings_ref, 0);
        assert_eq!(parsed.postings_blocks[0].postings[0].item_count, 1);
        let rows = parse_covi_row_range_postings(&parsed.postings_blocks[0].payload).unwrap();
        assert_eq!(rows[0].row_start, 3);
        assert_eq!(rows[0].row_count, 2);

        let validated = crate::execution::ValidatedCoviArtifactV2::validate(
            parsed,
            crate::execution::CoviValidationContextV2::for_file([3u8; 16], 128, 9),
        )
        .unwrap();
        let candidates = validated
            .lookup(&crate::execution::CoviLookupRequestV2::eq(
                7,
                8,
                crate::execution::CoviLookupKeyV2::CanonicalValueBytes(key_data),
            ))
            .unwrap();
        assert_eq!(candidates.row_ranges.len(), 1);
        assert_eq!(candidates.row_ranges[0].segment_id, 11);
    }

    #[test]
    fn key_block_checksum_covers_key_data() {
        let mut bytes = CoviKeyBlockV2 {
            header: CoviKeyBlockHeaderV2 {
                magic: [0; 4],
                version_major: 0,
                version_minor: 0,
                header_len: 0,
                reserved0: 0,
                key_block_id: 0,
                index_root_id: 0,
                key_count: 1,
                encoding_kind: CoviKeyEncodingKindV2::CanonicalValueBytes,
                comparator_kind: CoviComparatorKindV2::CanonicalOrdering,
                flags: 0,
                key_data_offset: 0,
                key_data_length: 0,
                checksum: 0,
            },
            key_data: vec![1, 2, 3],
        }
        .serialize()
        .unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(
            CoviKeyBlockV2::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        ));
    }

    #[test]
    fn row_ordinal_dense_bitset_rejects_unused_high_bits() {
        let block = CoviPostingsBlockV2 {
            header: CoviPostingsBlockHeaderV2 {
                magic: [0; 4],
                version_major: 0,
                version_minor: 0,
                header_len: 0,
                postings_header_len: 0,
                postings_block_id: 0,
                index_root_id: 0,
                postings_count: 0,
                row_ordinal_set_count: 0,
                postings_headers_offset: 0,
                row_ordinal_headers_offset: 0,
                postings_payload_offset: 0,
                postings_payload_length: 0,
                flags: 0,
                checksum: 0,
            },
            postings: vec![CoviPostingsHeaderV2 {
                postings_ref: 0,
                index_root_id: 0,
                representation: CoviPostingRepresentationV2::RowOrdinalBitmap,
                target_granularity: cove_coverage::CoverageGranularityV2::RowOrdinalSet as u8,
                flags: 0,
                item_count: 1,
                payload_offset: 1,
                payload_length: 4,
                coverage_set_ref: u32::MAX,
                checksum: 0,
            }],
            row_ordinal_sets: vec![CoviRowOrdinalSetHeaderV2 {
                row_ordinal_set_ref: 0,
                file_ref: 0,
                table_id: 7,
                segment_id: 11,
                morsel_id: 0,
                bitmap_kind: CoviBitmapKindV2::DenseBitsetLsb0,
                flags: 0,
                reserved: 0,
                universe_row_count: 5,
                set_row_count: 3,
                payload_offset: 0,
                payload_length: 1,
                checksum: 0,
            }],
            payload: vec![0b1000_0111, 0, 0, 0, 0],
        };
        assert!(matches!(block.serialize(), Err(CoveError::BadCovi)));
    }

    #[test]
    fn aggregate_answer_block_round_trips() {
        let block = CoviAggregateAnswerBlockV2 {
            header: CoviAggregateAnswerBlockHeaderV2 {
                magic: [0; 4],
                version_major: 0,
                version_minor: 0,
                header_len: 0,
                aggregate_answer_len: 0,
                aggregate_block_id: 0,
                index_root_id: 0,
                aggregate_answer_count: 0,
                aggregate_answers_offset: 0,
                aggregate_payload_offset: 0,
                aggregate_payload_length: 0,
                flags: 0,
                checksum: 0,
            },
            answers: vec![CoviAggregateAnswerV2 {
                aggregate_answer_ref: 0,
                index_root_id: 0,
                aggregate_kind: crate::execution::CoviAggregateKindV2::Count as u16,
                exactness: IndexCapabilityExactnessV2::Exact as u8,
                null_semantics: 0,
                flags: 0,
                row_count: 42,
                null_count: 2,
                non_null_count: 40,
                value_ref: u32::MAX,
                predicate_form_ref: u32::MAX,
                snapshot_validity_ref: 0,
                checksum: 0,
            }],
            payload: Vec::new(),
        };
        let bytes = block.serialize().unwrap();
        let parsed = CoviAggregateAnswerBlockV2::parse(&bytes).unwrap();
        assert_eq!(parsed.answers[0].row_count, 42);
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
