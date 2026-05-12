//! COVE v2 section-level extended feature bindings.

use std::collections::BTreeSet;

use crate::{checksum, CoveError};

const MAGIC_SECTION_FEATURE_BINDING: [u8; 4] = *b"SFB2";
const ABSENT_U32: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FeatureScopeV2 {
    FileRequired = 0,
    SectionRequired = 1,
    PageRequired = 2,
    ProfileRequired = 3,
    OperationRequired = 4,
    AdvisoryOnly = 5,
}

impl FeatureScopeV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::FileRequired),
            1 => Some(Self::SectionRequired),
            2 => Some(Self::PageRequired),
            3 => Some(Self::ProfileRequired),
            4 => Some(Self::OperationRequired),
            5 => Some(Self::AdvisoryOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum SectionFeatureBindingPayloadKindV2 {
    None = 0,
    ProfileRequirement = 1,
    OperationRequirement = 2,
    PageRequirement = 3,
    ExtensionRequirement = 4,
    CodecRequirement = 5,
    CoverageRequirement = 6,
    IndexRequirement = 7,
    RuntimeRequirement = 8,
    VendorDefined = 255,
}

impl SectionFeatureBindingPayloadKindV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::None),
            1 => Some(Self::ProfileRequirement),
            2 => Some(Self::OperationRequirement),
            3 => Some(Self::PageRequirement),
            4 => Some(Self::ExtensionRequirement),
            5 => Some(Self::CodecRequirement),
            6 => Some(Self::CoverageRequirement),
            7 => Some(Self::IndexRequirement),
            8 => Some(Self::RuntimeRequirement),
            255 => Some(Self::VendorDefined),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum OperationKindV2 {
    None = 0,
    OrdinaryTableScan = 1,
    ObjectReconstruction = 2,
    MappingReplay = 3,
    MappingExplanation = 4,
    ProjectionReadback = 5,
    TrustVerification = 6,
    RedactionPolicyEvaluation = 7,
    HarborMount = 8,
    EngineExecutionMapping = 9,
    IndexOnlyAnswer = 10,
    CoveragePlanning = 11,
    ZeroCopyExport = 12,
    RuntimeAdapterSelection = 13,
    VendorDefined = 255,
}

impl OperationKindV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::None),
            1 => Some(Self::OrdinaryTableScan),
            2 => Some(Self::ObjectReconstruction),
            3 => Some(Self::MappingReplay),
            4 => Some(Self::MappingExplanation),
            5 => Some(Self::ProjectionReadback),
            6 => Some(Self::TrustVerification),
            7 => Some(Self::RedactionPolicyEvaluation),
            8 => Some(Self::HarborMount),
            9 => Some(Self::EngineExecutionMapping),
            10 => Some(Self::IndexOnlyAnswer),
            11 => Some(Self::CoveragePlanning),
            12 => Some(Self::ZeroCopyExport),
            13 => Some(Self::RuntimeAdapterSelection),
            255 => Some(Self::VendorDefined),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionFeatureBindingSectionHeaderV2 {
    pub magic: [u8; 4],
    pub version_major: u16,
    pub version_minor: u16,
    pub header_len: u16,
    pub entry_len: u16,
    pub binding_count: u32,
    pub payload_ref_count: u32,
    pub feature_word_count: u32,
    pub bindings_offset: u64,
    pub payload_refs_offset: u64,
    pub feature_words_offset: u64,
    pub payload_data_offset: u64,
    pub payload_data_length: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl SectionFeatureBindingSectionHeaderV2 {
    pub const LEN: usize = 72;

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
            binding_count: read_u32(bytes, 12)?,
            payload_ref_count: read_u32(bytes, 16)?,
            feature_word_count: read_u32(bytes, 20)?,
            bindings_offset: read_u64(bytes, 24)?,
            payload_refs_offset: read_u64(bytes, 32)?,
            feature_words_offset: read_u64(bytes, 40)?,
            payload_data_offset: read_u64(bytes, 48)?,
            payload_data_length: read_u64(bytes, 56)?,
            flags: read_u32(bytes, 64)?,
            checksum: read_u32(bytes, 68)?,
        };
        if header.magic != MAGIC_SECTION_FEATURE_BINDING {
            return Err(CoveError::BadMagic);
        }
        if header.version_major != 2 {
            return Err(CoveError::BadVersion);
        }
        if header.header_len as usize != Self::LEN {
            return Err(CoveError::BadSection(
                "SECTION_FEATURE_BINDING header_len mismatch".into(),
            ));
        }
        if header.entry_len as usize != SectionFeatureBindingV2::LEN {
            return Err(CoveError::BadSection(
                "SECTION_FEATURE_BINDING entry_len mismatch".into(),
            ));
        }
        verify_crc(&bytes[..Self::LEN], 68, header.checksum)?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version_major.to_le_bytes());
        out[6..8].copy_from_slice(&self.version_minor.to_le_bytes());
        out[8..10].copy_from_slice(&self.header_len.to_le_bytes());
        out[10..12].copy_from_slice(&self.entry_len.to_le_bytes());
        out[12..16].copy_from_slice(&self.binding_count.to_le_bytes());
        out[16..20].copy_from_slice(&self.payload_ref_count.to_le_bytes());
        out[20..24].copy_from_slice(&self.feature_word_count.to_le_bytes());
        out[24..32].copy_from_slice(&self.bindings_offset.to_le_bytes());
        out[32..40].copy_from_slice(&self.payload_refs_offset.to_le_bytes());
        out[40..48].copy_from_slice(&self.feature_words_offset.to_le_bytes());
        out[48..56].copy_from_slice(&self.payload_data_offset.to_le_bytes());
        out[56..64].copy_from_slice(&self.payload_data_length.to_le_bytes());
        out[64..68].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[68..72].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionFeatureBindingPayloadRefV2 {
    pub binding_payload_ref: u32,
    pub payload_kind: SectionFeatureBindingPayloadKindV2,
    pub operation_kind: OperationKindV2,
    pub profile: u8,
    pub flags: u8,
    pub reserved: u16,
    pub payload_offset: u64,
    pub payload_length: u64,
    pub checksum: u32,
}

impl SectionFeatureBindingPayloadRefV2 {
    pub const LEN: usize = 32;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let payload_kind = SectionFeatureBindingPayloadKindV2::from_u16(read_u16(bytes, 4)?)
            .ok_or_else(|| {
                CoveError::BadSection("unknown SECTION_FEATURE_BINDING payload kind".into())
            })?;
        let operation_kind = OperationKindV2::from_u16(read_u16(bytes, 6)?).ok_or_else(|| {
            CoveError::BadSection("unknown SECTION_FEATURE_BINDING operation kind".into())
        })?;
        let payload_ref = Self {
            binding_payload_ref: read_u32(bytes, 0)?,
            payload_kind,
            operation_kind,
            profile: read_u8(bytes, 8)?,
            flags: read_u8(bytes, 9)?,
            reserved: read_u16(bytes, 10)?,
            payload_offset: read_u64(bytes, 12)?,
            payload_length: read_u64(bytes, 20)?,
            checksum: read_u32(bytes, 28)?,
        };
        if payload_ref.reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        verify_crc(&bytes[..Self::LEN], 28, payload_ref.checksum)?;
        Ok(payload_ref)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        if self.reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.binding_payload_ref.to_le_bytes());
        out[4..6].copy_from_slice(&(self.payload_kind as u16).to_le_bytes());
        out[6..8].copy_from_slice(&(self.operation_kind as u16).to_le_bytes());
        out[8] = self.profile;
        out[9] = self.flags;
        out[10..12].copy_from_slice(&self.reserved.to_le_bytes());
        out[12..20].copy_from_slice(&self.payload_offset.to_le_bytes());
        out[20..28].copy_from_slice(&self.payload_length.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[28..32].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionFeatureBindingV2 {
    pub binding_id: u32,
    pub section_id: u32,
    pub scope: FeatureScopeV2,
    pub profile: u8,
    pub operation_kind: OperationKindV2,
    pub required_word_count: u32,
    pub optional_word_count: u32,
    pub required_feature_word_index: u32,
    pub optional_feature_word_index: u32,
    pub required_first_feature_word_number: u32,
    pub optional_first_feature_word_number: u32,
    pub binding_payload_ref: u32,
    pub target_local_ref: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl SectionFeatureBindingV2 {
    pub const LEN: usize = 56;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let scope = FeatureScopeV2::from_u8(read_u8(bytes, 8)?)
            .ok_or_else(|| CoveError::BadSection("unknown feature binding scope".into()))?;
        let operation_kind = OperationKindV2::from_u16(read_u16(bytes, 10)?)
            .ok_or_else(|| CoveError::BadSection("unknown operation kind".into()))?;
        let binding = Self {
            binding_id: read_u32(bytes, 0)?,
            section_id: read_u32(bytes, 4)?,
            scope,
            profile: read_u8(bytes, 9)?,
            operation_kind,
            required_word_count: read_u32(bytes, 12)?,
            optional_word_count: read_u32(bytes, 16)?,
            required_feature_word_index: read_u32(bytes, 20)?,
            optional_feature_word_index: read_u32(bytes, 24)?,
            required_first_feature_word_number: read_u32(bytes, 28)?,
            optional_first_feature_word_number: read_u32(bytes, 32)?,
            binding_payload_ref: read_u32(bytes, 36)?,
            target_local_ref: read_u64(bytes, 40)?,
            flags: read_u32(bytes, 48)?,
            checksum: read_u32(bytes, 52)?,
        };
        verify_crc(&bytes[..Self::LEN], 52, binding.checksum)?;
        Ok(binding)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.binding_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.section_id.to_le_bytes());
        out[8] = self.scope as u8;
        out[9] = self.profile;
        out[10..12].copy_from_slice(&(self.operation_kind as u16).to_le_bytes());
        out[12..16].copy_from_slice(&self.required_word_count.to_le_bytes());
        out[16..20].copy_from_slice(&self.optional_word_count.to_le_bytes());
        out[20..24].copy_from_slice(&self.required_feature_word_index.to_le_bytes());
        out[24..28].copy_from_slice(&self.optional_feature_word_index.to_le_bytes());
        out[28..32].copy_from_slice(&self.required_first_feature_word_number.to_le_bytes());
        out[32..36].copy_from_slice(&self.optional_first_feature_word_number.to_le_bytes());
        out[36..40].copy_from_slice(&self.binding_payload_ref.to_le_bytes());
        out[40..48].copy_from_slice(&self.target_local_ref.to_le_bytes());
        out[48..52].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[52..56].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionFeatureBindingSectionV2 {
    pub header: SectionFeatureBindingSectionHeaderV2,
    pub bindings: Vec<SectionFeatureBindingV2>,
    pub payload_refs: Vec<SectionFeatureBindingPayloadRefV2>,
    pub feature_words: Vec<u64>,
    pub payload_data: Vec<u8>,
}

impl SectionFeatureBindingSectionV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = SectionFeatureBindingSectionHeaderV2::parse(bytes)?;
        let ranges = section_ranges(&header, bytes.len())?;
        ensure_non_overlapping(&ranges)?;

        let binding_range = ranges[0];
        let payload_ref_range = ranges[1];
        let feature_word_range = ranges[2];
        let payload_data_range = ranges[3];

        let mut bindings = Vec::with_capacity(header.binding_count as usize);
        for chunk in
            bytes[binding_range.0..binding_range.1].chunks_exact(SectionFeatureBindingV2::LEN)
        {
            bindings.push(SectionFeatureBindingV2::parse(chunk)?);
        }

        let mut payload_refs = Vec::with_capacity(header.payload_ref_count as usize);
        for chunk in bytes[payload_ref_range.0..payload_ref_range.1]
            .chunks_exact(SectionFeatureBindingPayloadRefV2::LEN)
        {
            payload_refs.push(SectionFeatureBindingPayloadRefV2::parse(chunk)?);
        }

        let mut feature_words = Vec::with_capacity(header.feature_word_count as usize);
        for chunk in bytes[feature_word_range.0..feature_word_range.1].chunks_exact(8) {
            feature_words.push(u64::from_le_bytes(chunk.try_into().unwrap()));
        }
        let payload_data = bytes[payload_data_range.0..payload_data_range.1].to_vec();

        let section = Self {
            header,
            bindings,
            payload_refs,
            feature_words,
            payload_data,
        };
        section.validate()?;
        Ok(section)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        self.validate()?;
        let bindings_offset = SectionFeatureBindingSectionHeaderV2::LEN;
        let payload_refs_offset =
            bindings_offset + self.bindings.len() * SectionFeatureBindingV2::LEN;
        let feature_words_offset =
            payload_refs_offset + self.payload_refs.len() * SectionFeatureBindingPayloadRefV2::LEN;
        let payload_data_offset = feature_words_offset + self.feature_words.len() * 8;

        let header = SectionFeatureBindingSectionHeaderV2 {
            magic: MAGIC_SECTION_FEATURE_BINDING,
            version_major: 2,
            version_minor: 0,
            header_len: SectionFeatureBindingSectionHeaderV2::LEN as u16,
            entry_len: SectionFeatureBindingV2::LEN as u16,
            binding_count: self.bindings.len() as u32,
            payload_ref_count: self.payload_refs.len() as u32,
            feature_word_count: self.feature_words.len() as u32,
            bindings_offset: bindings_offset as u64,
            payload_refs_offset: if self.payload_refs.is_empty() {
                0
            } else {
                payload_refs_offset as u64
            },
            feature_words_offset: if self.feature_words.is_empty() {
                0
            } else {
                feature_words_offset as u64
            },
            payload_data_offset: if self.payload_data.is_empty() {
                0
            } else {
                payload_data_offset as u64
            },
            payload_data_length: self.payload_data.len() as u64,
            flags: self.header.flags,
            checksum: 0,
        };

        let mut out = Vec::new();
        out.extend_from_slice(&header.serialize());
        for binding in &self.bindings {
            out.extend_from_slice(&binding.serialize()?);
        }
        for payload_ref in &self.payload_refs {
            out.extend_from_slice(&payload_ref.serialize()?);
        }
        for word in &self.feature_words {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&self.payload_data);
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.header.binding_count as usize != self.bindings.len()
            || self.header.payload_ref_count as usize != self.payload_refs.len()
            || self.header.feature_word_count as usize != self.feature_words.len()
        {
            return Err(CoveError::BadSection(
                "SECTION_FEATURE_BINDING count mismatch".into(),
            ));
        }

        let mut binding_ids = BTreeSet::new();
        for (index, binding) in self.bindings.iter().enumerate() {
            if binding.binding_id as usize != index || !binding_ids.insert(binding.binding_id) {
                return Err(CoveError::BadSection(
                    "SECTION_FEATURE_BINDING binding_id is not dense and unique".into(),
                ));
            }
            validate_binding(binding, self.payload_refs.len(), self.feature_words.len())?;
        }

        let mut payload_ref_ids = BTreeSet::new();
        for payload_ref in &self.payload_refs {
            if payload_ref.binding_payload_ref == 0
                || payload_ref.binding_payload_ref as usize > self.payload_refs.len()
                || !payload_ref_ids.insert(payload_ref.binding_payload_ref)
            {
                return Err(CoveError::BadSection(
                    "SECTION_FEATURE_BINDING payload refs are not dense and unique".into(),
                ));
            }
            let end = payload_ref
                .payload_offset
                .checked_add(payload_ref.payload_length)
                .ok_or(CoveError::ArithOverflow)?;
            if end > self.payload_data.len() as u64 {
                return Err(CoveError::OffsetRange);
            }
            if payload_ref.operation_kind != OperationKindV2::None
                && payload_ref.payload_kind
                    != SectionFeatureBindingPayloadKindV2::OperationRequirement
            {
                return Err(CoveError::BadSection(
                    "operation_kind requires an operation payload kind".into(),
                ));
            }
        }
        Ok(())
    }
}

fn validate_binding(
    binding: &SectionFeatureBindingV2,
    payload_ref_count: usize,
    feature_word_count: usize,
) -> Result<(), CoveError> {
    if binding.scope != FeatureScopeV2::OperationRequired
        && binding.operation_kind != OperationKindV2::None
    {
        return Err(CoveError::BadSection(
            "operation_kind is only valid for OperationRequired bindings".into(),
        ));
    }
    if binding.scope == FeatureScopeV2::OperationRequired
        && binding.operation_kind == OperationKindV2::None
    {
        return Err(CoveError::BadSection(
            "OperationRequired binding requires operation_kind".into(),
        ));
    }
    if binding.binding_payload_ref as usize > payload_ref_count {
        return Err(CoveError::BadSection(
            "SECTION_FEATURE_BINDING binding_payload_ref out of range".into(),
        ));
    }
    validate_word_range(
        binding.required_word_count,
        binding.required_feature_word_index,
        binding.required_first_feature_word_number,
        feature_word_count,
    )?;
    validate_word_range(
        binding.optional_word_count,
        binding.optional_feature_word_index,
        binding.optional_first_feature_word_number,
        feature_word_count,
    )?;
    Ok(())
}

fn validate_word_range(
    word_count: u32,
    local_index: u32,
    first_word_number: u32,
    feature_word_count: usize,
) -> Result<(), CoveError> {
    if word_count == 0 {
        if local_index != ABSENT_U32 || first_word_number != ABSENT_U32 {
            return Err(CoveError::BadSection(
                "empty feature-word binding must use absent sentinels".into(),
            ));
        }
        return Ok(());
    }
    if local_index == ABSENT_U32 || first_word_number == ABSENT_U32 {
        return Err(CoveError::BadSection(
            "non-empty feature-word binding missing local/global word reference".into(),
        ));
    }
    if first_word_number == 0 {
        return Err(CoveError::BadSection(
            "SECTION_FEATURE_BINDING must not bind global feature word 0".into(),
        ));
    }
    let end = local_index
        .checked_add(word_count)
        .ok_or(CoveError::ArithOverflow)?;
    if end as usize > feature_word_count {
        return Err(CoveError::OffsetRange);
    }
    Ok(())
}

fn section_ranges(
    header: &SectionFeatureBindingSectionHeaderV2,
    len: usize,
) -> Result<[(usize, usize); 4], CoveError> {
    let binding_len = header
        .binding_count
        .checked_mul(SectionFeatureBindingV2::LEN as u32)
        .ok_or(CoveError::ArithOverflow)? as u64;
    let payload_ref_len = header
        .payload_ref_count
        .checked_mul(SectionFeatureBindingPayloadRefV2::LEN as u32)
        .ok_or(CoveError::ArithOverflow)? as u64;
    let feature_word_len = header
        .feature_word_count
        .checked_mul(8)
        .ok_or(CoveError::ArithOverflow)? as u64;

    Ok([
        checked_range(header.bindings_offset, binding_len, len)?,
        optional_range(header.payload_refs_offset, payload_ref_len, len)?,
        optional_range(header.feature_words_offset, feature_word_len, len)?,
        optional_range(header.payload_data_offset, header.payload_data_length, len)?,
    ])
}

fn optional_range(offset: u64, length: u64, len: usize) -> Result<(usize, usize), CoveError> {
    if length == 0 {
        if offset != 0 {
            return Err(CoveError::OffsetRange);
        }
        Ok((0, 0))
    } else {
        checked_range(offset, length, len)
    }
}

fn checked_range(offset: u64, length: u64, len: usize) -> Result<(usize, usize), CoveError> {
    let start = usize::try_from(offset).map_err(|_| CoveError::OffsetRange)?;
    let length = usize::try_from(length).map_err(|_| CoveError::OffsetRange)?;
    let end = start.checked_add(length).ok_or(CoveError::ArithOverflow)?;
    if start < SectionFeatureBindingSectionHeaderV2::LEN || end > len {
        return Err(CoveError::OffsetRange);
    }
    Ok((start, end))
}

fn ensure_non_overlapping(ranges: &[(usize, usize); 4]) -> Result<(), CoveError> {
    let mut non_empty = ranges
        .iter()
        .copied()
        .filter(|(start, end)| start != end)
        .collect::<Vec<_>>();
    non_empty.sort_unstable();
    for pair in non_empty.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(CoveError::OffsetRange);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_section() -> SectionFeatureBindingSectionV2 {
        SectionFeatureBindingSectionV2 {
            header: SectionFeatureBindingSectionHeaderV2 {
                magic: MAGIC_SECTION_FEATURE_BINDING,
                version_major: 2,
                version_minor: 0,
                header_len: SectionFeatureBindingSectionHeaderV2::LEN as u16,
                entry_len: SectionFeatureBindingV2::LEN as u16,
                binding_count: 1,
                payload_ref_count: 1,
                feature_word_count: 1,
                bindings_offset: 0,
                payload_refs_offset: 0,
                feature_words_offset: 0,
                payload_data_offset: 0,
                payload_data_length: 0,
                flags: 0,
                checksum: 0,
            },
            bindings: vec![SectionFeatureBindingV2 {
                binding_id: 0,
                section_id: 0,
                scope: FeatureScopeV2::OperationRequired,
                profile: 2,
                operation_kind: OperationKindV2::CoveragePlanning,
                required_word_count: 1,
                optional_word_count: 0,
                required_feature_word_index: 0,
                optional_feature_word_index: ABSENT_U32,
                required_first_feature_word_number: 1,
                optional_first_feature_word_number: ABSENT_U32,
                binding_payload_ref: 1,
                target_local_ref: u64::MAX,
                flags: 0,
                checksum: 0,
            }],
            payload_refs: vec![SectionFeatureBindingPayloadRefV2 {
                binding_payload_ref: 1,
                payload_kind: SectionFeatureBindingPayloadKindV2::OperationRequirement,
                operation_kind: OperationKindV2::CoveragePlanning,
                profile: 2,
                flags: 0,
                reserved: 0,
                payload_offset: 0,
                payload_length: 0,
                checksum: 0,
            }],
            feature_words: vec![1],
            payload_data: Vec::new(),
        }
    }

    #[test]
    fn feature_binding_round_trips() {
        let bytes = valid_section().serialize().unwrap();
        let parsed = SectionFeatureBindingSectionV2::parse(&bytes).unwrap();
        assert_eq!(parsed.bindings.len(), 1);
        assert_eq!(
            parsed.bindings[0].operation_kind,
            OperationKindV2::CoveragePlanning
        );
    }

    #[test]
    fn invalid_payload_ref_is_rejected() {
        let mut section = valid_section();
        section.bindings[0].binding_payload_ref = 2;
        assert!(matches!(section.serialize(), Err(CoveError::BadSection(_))));
    }

    #[test]
    fn global_word_zero_binding_is_rejected() {
        let mut section = valid_section();
        section.bindings[0].required_first_feature_word_number = 0;
        assert!(matches!(section.serialize(), Err(CoveError::BadSection(_))));
    }
}
