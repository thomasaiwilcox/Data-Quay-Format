//! COVE v2 extended feature words and scoped requiredness metadata.

use std::collections::BTreeSet;

use crate::{
    checksum,
    constants::KNOWN_FEATURE_BITS_MASK,
    feature_binding::{FeatureScopeV2, OperationKindV2, SectionFeatureBindingSectionV2},
    footer::{CoveFooter, CoveSectionEntryV1},
    header::CoveHeaderV1,
    CoveError,
};

const MAGIC_PROFILE_CAPABILITY_MATRIX: [u8; 4] = *b"PCM2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendedFeatureSetHeaderV2 {
    pub word_count: u32,
    pub required_word_count: u32,
    pub optional_word_count: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl ExtendedFeatureSetHeaderV2 {
    pub const LEN: usize = 20;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            word_count: read_u32(bytes, 0)?,
            required_word_count: read_u32(bytes, 4)?,
            optional_word_count: read_u32(bytes, 8)?,
            flags: read_u32(bytes, 12)?,
            checksum: read_u32(bytes, 16)?,
        };
        verify_crc(&bytes[..Self::LEN], 16, header.checksum)?;
        header.validate_counts()?;
        Ok(header)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate_counts()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.word_count.to_le_bytes());
        out[4..8].copy_from_slice(&self.required_word_count.to_le_bytes());
        out[8..12].copy_from_slice(&self.optional_word_count.to_le_bytes());
        out[12..16].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[16..20].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    fn validate_counts(&self) -> Result<(), CoveError> {
        if self.word_count == 0
            || self.required_word_count > self.word_count
            || self.optional_word_count > self.word_count
        {
            return Err(CoveError::BadSection(
                "EXTENDED_FEATURE_SET word counts are invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendedFeatureSetV2 {
    pub header: ExtendedFeatureSetHeaderV2,
    pub required_feature_words: Vec<u64>,
    pub optional_feature_words: Vec<u64>,
}

impl ExtendedFeatureSetV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = ExtendedFeatureSetHeaderV2::parse(bytes)?;
        let required_len = word_bytes_len(header.required_word_count)?;
        let optional_len = word_bytes_len(header.optional_word_count)?;
        let required_start = ExtendedFeatureSetHeaderV2::LEN;
        let optional_start = required_start
            .checked_add(required_len)
            .ok_or(CoveError::ArithOverflow)?;
        let end = optional_start
            .checked_add(optional_len)
            .ok_or(CoveError::ArithOverflow)?;
        if bytes.len() != end {
            return Err(CoveError::BadSection(
                "EXTENDED_FEATURE_SET payload length mismatch".into(),
            ));
        }
        let required_feature_words = read_words(&bytes[required_start..optional_start])?;
        let optional_feature_words = read_words(&bytes[optional_start..end])?;
        let set = Self {
            header,
            required_feature_words,
            optional_feature_words,
        };
        set.validate()?;
        Ok(set)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        self.validate()?;
        let mut header = self.header.clone();
        header.required_word_count = self.required_feature_words.len() as u32;
        header.optional_word_count = self.optional_feature_words.len() as u32;
        header.word_count = header
            .word_count
            .max(header.required_word_count)
            .max(header.optional_word_count);
        let mut out = Vec::with_capacity(
            ExtendedFeatureSetHeaderV2::LEN
                + self.required_feature_words.len() * 8
                + self.optional_feature_words.len() * 8,
        );
        out.extend_from_slice(&header.serialize()?);
        for word in &self.required_feature_words {
            out.extend_from_slice(&word.to_le_bytes());
        }
        for word in &self.optional_feature_words {
            out.extend_from_slice(&word.to_le_bytes());
        }
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        self.header.validate_counts()?;
        if self.required_feature_words.len() != self.header.required_word_count as usize
            || self.optional_feature_words.len() != self.header.optional_word_count as usize
        {
            return Err(CoveError::BadSection(
                "EXTENDED_FEATURE_SET vector count mismatch".into(),
            ));
        }
        Ok(())
    }

    pub fn validate_against_low_words(
        &self,
        required_features: u64,
        optional_features: u64,
    ) -> Result<(), CoveError> {
        if self
            .required_feature_words
            .first()
            .copied()
            .unwrap_or_default()
            != required_features
        {
            return Err(CoveError::BadSection(
                "EXTENDED_FEATURE_SET required word 0 does not match header/postscript".into(),
            ));
        }
        if self
            .optional_feature_words
            .first()
            .copied()
            .unwrap_or_default()
            != optional_features
        {
            return Err(CoveError::BadSection(
                "EXTENDED_FEATURE_SET optional word 0 does not match header/postscript".into(),
            ));
        }
        Ok(())
    }

    pub fn validate_binding_horizon(
        &self,
        binding: &SectionFeatureBindingSectionV2,
    ) -> Result<(), CoveError> {
        for item in &binding.bindings {
            validate_binding_word_horizon(
                item.required_first_feature_word_number,
                item.required_word_count,
                self.header.word_count,
            )?;
            validate_binding_word_horizon(
                item.optional_first_feature_word_number,
                item.optional_word_count,
                self.header.word_count,
            )?;
        }
        Ok(())
    }

    pub fn required_word(&self, word_index: u32) -> u64 {
        self.required_feature_words
            .get(word_index as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn optional_word(&self, word_index: u32) -> u64 {
        self.optional_feature_words
            .get(word_index as usize)
            .copied()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileCapabilityMatrixHeaderV2 {
    pub magic: [u8; 4],
    pub version_major: u16,
    pub header_len: u16,
    pub entry_len: u16,
    pub reserved: u16,
    pub entry_count: u32,
    pub flags: u32,
    pub entries_offset: u64,
    pub entries_length: u64,
    pub checksum: u32,
}

impl ProfileCapabilityMatrixHeaderV2 {
    pub const LEN: usize = 40;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            magic: read_array(bytes, 0)?,
            version_major: read_u16(bytes, 4)?,
            header_len: read_u16(bytes, 6)?,
            entry_len: read_u16(bytes, 8)?,
            reserved: read_u16(bytes, 10)?,
            entry_count: read_u32(bytes, 12)?,
            flags: read_u32(bytes, 16)?,
            entries_offset: read_u64(bytes, 20)?,
            entries_length: read_u64(bytes, 28)?,
            checksum: read_u32(bytes, 36)?,
        };
        if header.magic != MAGIC_PROFILE_CAPABILITY_MATRIX {
            return Err(CoveError::BadMagic);
        }
        if header.version_major != 2 {
            return Err(CoveError::BadVersion);
        }
        if header.header_len as usize != Self::LEN
            || header.entry_len as usize != ProfileCapabilityEntryV2::LEN
        {
            return Err(CoveError::BadSection(
                "PROFILE_CAPABILITY_MATRIX length field mismatch".into(),
            ));
        }
        if header.reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        verify_crc(&bytes[..Self::LEN], 36, header.checksum)?;
        Ok(header)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        if self.reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version_major.to_le_bytes());
        out[6..8].copy_from_slice(&self.header_len.to_le_bytes());
        out[8..10].copy_from_slice(&self.entry_len.to_le_bytes());
        out[10..12].copy_from_slice(&self.reserved.to_le_bytes());
        out[12..16].copy_from_slice(&self.entry_count.to_le_bytes());
        out[16..20].copy_from_slice(&self.flags.to_le_bytes());
        out[20..28].copy_from_slice(&self.entries_offset.to_le_bytes());
        out[28..36].copy_from_slice(&self.entries_length.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[36..40].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileCapabilityEntryV2 {
    pub profile: u8,
    pub scope: FeatureScopeV2,
    pub operation_kind: OperationKindV2,
    pub global_feature_word_index: u32,
    pub required_mask: u64,
    pub optional_mask: u64,
    pub section_id: u32,
    pub target_local_ref: u64,
    pub flags: u32,
    pub reserved: u32,
    pub checksum: u32,
}

impl ProfileCapabilityEntryV2 {
    pub const LEN: usize = 48;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let entry = Self {
            profile: read_u8(bytes, 0)?,
            scope: FeatureScopeV2::from_u8(read_u8(bytes, 1)?).ok_or_else(|| {
                CoveError::BadSection("PROFILE_CAPABILITY_MATRIX has unknown scope".into())
            })?,
            operation_kind: OperationKindV2::from_u16(read_u16(bytes, 2)?).ok_or_else(|| {
                CoveError::BadSection("PROFILE_CAPABILITY_MATRIX has unknown operation".into())
            })?,
            global_feature_word_index: read_u32(bytes, 4)?,
            required_mask: read_u64(bytes, 8)?,
            optional_mask: read_u64(bytes, 16)?,
            section_id: read_u32(bytes, 24)?,
            target_local_ref: read_u64(bytes, 28)?,
            flags: read_u32(bytes, 36)?,
            reserved: read_u32(bytes, 40)?,
            checksum: read_u32(bytes, 44)?,
        };
        verify_crc(&bytes[..Self::LEN], 44, entry.checksum)?;
        entry.validate()?;
        Ok(entry)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0] = self.profile;
        out[1] = self.scope as u8;
        out[2..4].copy_from_slice(&(self.operation_kind as u16).to_le_bytes());
        out[4..8].copy_from_slice(&self.global_feature_word_index.to_le_bytes());
        out[8..16].copy_from_slice(&self.required_mask.to_le_bytes());
        out[16..24].copy_from_slice(&self.optional_mask.to_le_bytes());
        out[24..28].copy_from_slice(&self.section_id.to_le_bytes());
        out[28..36].copy_from_slice(&self.target_local_ref.to_le_bytes());
        out[36..40].copy_from_slice(&self.flags.to_le_bytes());
        out[40..44].copy_from_slice(&self.reserved.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[44..48].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        if self.scope == FeatureScopeV2::OperationRequired
            && self.operation_kind == OperationKindV2::None
        {
            return Err(CoveError::BadSection(
                "operation-scoped profile capability requires operation_kind".into(),
            ));
        }
        if self.scope != FeatureScopeV2::OperationRequired
            && self.operation_kind != OperationKindV2::None
        {
            return Err(CoveError::BadSection(
                "operation_kind is only valid for operation-scoped profile capability".into(),
            ));
        }
        if self.required_mask == 0 && self.optional_mask == 0 {
            return Err(CoveError::BadSection(
                "PROFILE_CAPABILITY_MATRIX entry has no feature bits".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileCapabilityMatrixV2 {
    pub header: ProfileCapabilityMatrixHeaderV2,
    pub entries: Vec<ProfileCapabilityEntryV2>,
}

impl ProfileCapabilityMatrixV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = ProfileCapabilityMatrixHeaderV2::parse(bytes)?;
        let start = usize::try_from(header.entries_offset).map_err(|_| CoveError::OffsetRange)?;
        let len = usize::try_from(header.entries_length).map_err(|_| CoveError::OffsetRange)?;
        let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        let expected = header
            .entry_count
            .checked_mul(ProfileCapabilityEntryV2::LEN as u32)
            .ok_or(CoveError::ArithOverflow)? as usize;
        if start < ProfileCapabilityMatrixHeaderV2::LEN || end != bytes.len() || len != expected {
            return Err(CoveError::BadSection(
                "PROFILE_CAPABILITY_MATRIX entry range mismatch".into(),
            ));
        }
        let entries = bytes[start..end]
            .chunks_exact(ProfileCapabilityEntryV2::LEN)
            .map(ProfileCapabilityEntryV2::parse)
            .collect::<Result<Vec<_>, _>>()?;
        let matrix = Self { header, entries };
        matrix.validate()?;
        Ok(matrix)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        self.validate()?;
        let mut header = self.header.clone();
        header.magic = MAGIC_PROFILE_CAPABILITY_MATRIX;
        header.version_major = 2;
        header.header_len = ProfileCapabilityMatrixHeaderV2::LEN as u16;
        header.entry_len = ProfileCapabilityEntryV2::LEN as u16;
        header.entry_count = self.entries.len() as u32;
        header.entries_offset = ProfileCapabilityMatrixHeaderV2::LEN as u64;
        header.entries_length = (self.entries.len() * ProfileCapabilityEntryV2::LEN) as u64;
        let mut out = Vec::with_capacity(
            ProfileCapabilityMatrixHeaderV2::LEN
                + self.entries.len() * ProfileCapabilityEntryV2::LEN,
        );
        out.extend_from_slice(&header.serialize()?);
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize()?);
        }
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.header.entry_count as usize != self.entries.len() {
            return Err(CoveError::BadSection(
                "PROFILE_CAPABILITY_MATRIX count mismatch".into(),
            ));
        }
        let mut prev = None;
        let mut seen = BTreeSet::new();
        for entry in &self.entries {
            entry.validate()?;
            let key = (
                entry.profile,
                entry.scope as u8,
                entry.operation_kind as u16,
                entry.global_feature_word_index,
                entry.section_id,
                entry.target_local_ref,
            );
            if let Some(previous) = prev {
                if key <= previous {
                    return Err(CoveError::BadSection(
                        "PROFILE_CAPABILITY_MATRIX entries are not sorted".into(),
                    ));
                }
            }
            if !seen.insert(key) {
                return Err(CoveError::BadSection(
                    "PROFILE_CAPABILITY_MATRIX duplicate entry".into(),
                ));
            }
            prev = Some(key);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureScopeEntry {
    pub scope: FeatureScopeV2,
    pub profile: u8,
    pub operation_kind: OperationKindV2,
    pub section_id: u32,
    pub target_local_ref: u64,
    pub global_feature_word_index: u32,
    pub required_mask: u64,
    pub optional_mask: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FeatureTargetRefV2 {
    pub section_id: u32,
    pub target_local_ref: u64,
}

impl FeatureTargetRefV2 {
    pub fn new(section_id: u32, target_local_ref: u64) -> Self {
        Self {
            section_id,
            target_local_ref,
        }
    }

    pub fn cove_t_column_page(section_id: u32, column_id: u32, morsel_id: u32) -> Self {
        Self::new(
            section_id,
            cove_column_page_target_ref(column_id, morsel_id),
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureUseRequestV2 {
    pub requested_profile: Option<u8>,
    pub requested_operation: Option<OperationKindV2>,
    pub needed_section_ids: BTreeSet<u32>,
    pub needed_page_refs: BTreeSet<FeatureTargetRefV2>,
}

impl FeatureUseRequestV2 {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_profile(mut self, profile: u8) -> Self {
        self.requested_profile = Some(profile);
        self
    }

    pub fn with_operation(mut self, operation: OperationKindV2) -> Self {
        self.requested_operation = Some(operation);
        self
    }

    pub fn with_section(mut self, section_id: u32) -> Self {
        self.needed_section_ids.insert(section_id);
        self
    }

    pub fn with_page_ref(mut self, section_id: u32, target_local_ref: u64) -> Self {
        self.needed_page_refs
            .insert(FeatureTargetRefV2::new(section_id, target_local_ref));
        self
    }

    pub fn with_target_ref(mut self, target: FeatureTargetRefV2) -> Self {
        self.needed_page_refs.insert(target);
        self
    }

    pub fn with_cove_t_column_page(self, section_id: u32, column_id: u32, morsel_id: u32) -> Self {
        self.with_target_ref(FeatureTargetRefV2::cove_t_column_page(
            section_id, column_id, morsel_id,
        ))
    }
}

pub fn cove_column_page_target_ref(column_id: u32, morsel_id: u32) -> u64 {
    (u64::from(column_id) << 32) | u64::from(morsel_id)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureScopeTable {
    pub entries: Vec<FeatureScopeEntry>,
}

impl FeatureScopeTable {
    pub fn build(
        header: &CoveHeaderV1,
        footer: &CoveFooter,
        extended: Option<&ExtendedFeatureSetV2>,
        profile_matrix: Option<&ProfileCapabilityMatrixV2>,
        section_binding: Option<&SectionFeatureBindingSectionV2>,
    ) -> Result<Self, CoveError> {
        match section_binding {
            Some(binding) => Self::build_many(
                header,
                footer,
                extended,
                profile_matrix,
                std::slice::from_ref(binding),
            ),
            None => Self::build_many(header, footer, extended, profile_matrix, &[]),
        }
    }

    pub fn build_many(
        header: &CoveHeaderV1,
        footer: &CoveFooter,
        extended: Option<&ExtendedFeatureSetV2>,
        profile_matrix: Option<&ProfileCapabilityMatrixV2>,
        section_bindings: &[SectionFeatureBindingSectionV2],
    ) -> Result<Self, CoveError> {
        validate_scoped_metadata_references(
            header,
            footer,
            extended,
            profile_matrix,
            section_bindings,
        )?;
        let scoped_required = scoped_required_masks(profile_matrix, section_bindings)?;
        let mut entries = Vec::new();
        entries.push(FeatureScopeEntry {
            scope: FeatureScopeV2::FileRequired,
            profile: header.primary_profile,
            operation_kind: OperationKindV2::None,
            section_id: 0,
            target_local_ref: u64::MAX,
            global_feature_word_index: 0,
            required_mask: header.required_features,
            optional_mask: 0,
        });
        if header.optional_features != 0 {
            entries.push(FeatureScopeEntry {
                scope: FeatureScopeV2::AdvisoryOnly,
                profile: header.primary_profile,
                operation_kind: OperationKindV2::None,
                section_id: 0,
                target_local_ref: u64::MAX,
                global_feature_word_index: 0,
                required_mask: 0,
                optional_mask: header.optional_features,
            });
        }
        for section in &footer.sections {
            if section.required_features != 0 || section.optional_features != 0 {
                entries.push(section_entry(section));
            }
        }
        if let Some(extended) = extended {
            for word_index in 1..extended.header.word_count {
                let scoped_mask = scoped_required
                    .get(&word_index)
                    .copied()
                    .unwrap_or_default();
                let required_mask = extended.required_word(word_index) & !scoped_mask;
                let optional_mask = extended.optional_word(word_index);
                if required_mask != 0 {
                    entries.push(FeatureScopeEntry {
                        scope: FeatureScopeV2::FileRequired,
                        profile: header.primary_profile,
                        operation_kind: OperationKindV2::None,
                        section_id: 0,
                        target_local_ref: u64::MAX,
                        global_feature_word_index: word_index,
                        required_mask,
                        optional_mask: 0,
                    });
                }
                if optional_mask != 0 {
                    entries.push(FeatureScopeEntry {
                        scope: FeatureScopeV2::AdvisoryOnly,
                        profile: header.primary_profile,
                        operation_kind: OperationKindV2::None,
                        section_id: 0,
                        target_local_ref: u64::MAX,
                        global_feature_word_index: word_index,
                        required_mask: 0,
                        optional_mask,
                    });
                }
            }
        }
        if let Some(matrix) = profile_matrix {
            entries.extend(matrix.entries.iter().map(|entry| FeatureScopeEntry {
                scope: entry.scope,
                profile: entry.profile,
                operation_kind: entry.operation_kind,
                section_id: entry.section_id,
                target_local_ref: entry.target_local_ref,
                global_feature_word_index: entry.global_feature_word_index,
                required_mask: entry.required_mask,
                optional_mask: entry.optional_mask,
            }));
        }
        for binding in section_bindings {
            for item in &binding.bindings {
                for idx in 0..item.required_word_count {
                    let local = item
                        .required_feature_word_index
                        .checked_add(idx)
                        .ok_or(CoveError::ArithOverflow)?;
                    let global = item
                        .required_first_feature_word_number
                        .checked_add(idx)
                        .ok_or(CoveError::ArithOverflow)?;
                    entries.push(FeatureScopeEntry {
                        scope: item.scope,
                        profile: item.profile,
                        operation_kind: item.operation_kind,
                        section_id: item.section_id,
                        target_local_ref: item.target_local_ref,
                        global_feature_word_index: global,
                        required_mask: binding.feature_words[local as usize],
                        optional_mask: 0,
                    });
                }
                for idx in 0..item.optional_word_count {
                    let local = item
                        .optional_feature_word_index
                        .checked_add(idx)
                        .ok_or(CoveError::ArithOverflow)?;
                    let global = item
                        .optional_first_feature_word_number
                        .checked_add(idx)
                        .ok_or(CoveError::ArithOverflow)?;
                    entries.push(FeatureScopeEntry {
                        scope: FeatureScopeV2::AdvisoryOnly,
                        profile: item.profile,
                        operation_kind: item.operation_kind,
                        section_id: item.section_id,
                        target_local_ref: item.target_local_ref,
                        global_feature_word_index: global,
                        required_mask: 0,
                        optional_mask: binding.feature_words[local as usize],
                    });
                }
            }
        }
        let mut table = Self { entries };
        table.normalize();
        Ok(table)
    }

    pub fn reject_file_required_unknowns(&self) -> Result<(), CoveError> {
        for entry in &self.entries {
            if entry.scope == FeatureScopeV2::FileRequired {
                let unknown = unknown_required_mask(entry);
                if unknown != 0 {
                    return Err(CoveError::UnknownRequiredFeature(unknown));
                }
            }
        }
        Ok(())
    }

    pub fn reject_unknowns_for_request(
        &self,
        request: &FeatureUseRequestV2,
    ) -> Result<(), CoveError> {
        for entry in &self.entries {
            let unknown = unknown_required_mask(entry);
            if unknown == 0 {
                continue;
            }
            let needed = match entry.scope {
                FeatureScopeV2::FileRequired => true,
                FeatureScopeV2::SectionRequired => {
                    request.needed_section_ids.contains(&entry.section_id)
                }
                FeatureScopeV2::PageRequired => request.needed_page_refs.contains(
                    &FeatureTargetRefV2::new(entry.section_id, entry.target_local_ref),
                ),
                FeatureScopeV2::ProfileRequired => request.requested_profile == Some(entry.profile),
                FeatureScopeV2::OperationRequired => {
                    request.requested_operation == Some(entry.operation_kind)
                        && match request.requested_profile {
                            Some(profile) => entry.profile == 0 || entry.profile == profile,
                            None => true,
                        }
                }
                FeatureScopeV2::AdvisoryOnly => false,
            };
            if needed {
                return Err(CoveError::UnknownRequiredFeature(unknown));
            }
        }
        Ok(())
    }

    pub fn reject_unknowns_for(
        &self,
        requested_profile: Option<u8>,
        requested_operation: Option<OperationKindV2>,
        needed_section_ids: &BTreeSet<u32>,
    ) -> Result<(), CoveError> {
        self.reject_unknowns_for_request(&FeatureUseRequestV2 {
            requested_profile,
            requested_operation,
            needed_section_ids: needed_section_ids.clone(),
            needed_page_refs: BTreeSet::new(),
        })
    }

    fn normalize(&mut self) {
        self.entries.sort_by_key(|entry| {
            (
                entry.global_feature_word_index,
                entry.scope as u8,
                entry.profile,
                entry.operation_kind as u16,
                entry.section_id,
                entry.target_local_ref,
            )
        });
    }
}

fn unknown_required_mask(entry: &FeatureScopeEntry) -> u64 {
    if entry.global_feature_word_index == 0 {
        entry.required_mask & !KNOWN_FEATURE_BITS_MASK
    } else {
        entry.required_mask
    }
}

fn section_entry(section: &CoveSectionEntryV1) -> FeatureScopeEntry {
    FeatureScopeEntry {
        scope: FeatureScopeV2::SectionRequired,
        profile: section.profile,
        operation_kind: OperationKindV2::None,
        section_id: section.section_id,
        target_local_ref: u64::MAX,
        global_feature_word_index: 0,
        required_mask: section.required_features,
        optional_mask: section.optional_features,
    }
}

fn validate_scoped_metadata_references(
    header: &CoveHeaderV1,
    footer: &CoveFooter,
    extended: Option<&ExtendedFeatureSetV2>,
    profile_matrix: Option<&ProfileCapabilityMatrixV2>,
    section_bindings: &[SectionFeatureBindingSectionV2],
) -> Result<(), CoveError> {
    if let Some(matrix) = profile_matrix {
        for entry in &matrix.entries {
            if entry.scope == FeatureScopeV2::AdvisoryOnly && entry.required_mask != 0 {
                return Err(CoveError::BadSection(
                    "AdvisoryOnly profile capability must not carry required bits".into(),
                ));
            }
            validate_scope_addressing(
                entry.scope,
                entry.operation_kind,
                entry.section_id,
                entry.target_local_ref,
                footer,
            )?;
            validate_feature_word_reference(
                entry.global_feature_word_index,
                entry.required_mask,
                entry.optional_mask,
                header,
                extended,
                "PROFILE_CAPABILITY_MATRIX",
            )?;
        }
    }
    for binding in section_bindings {
        for item in &binding.bindings {
            if item.scope == FeatureScopeV2::AdvisoryOnly && item.required_word_count != 0 {
                return Err(CoveError::BadSection(
                    "AdvisoryOnly section feature binding must not carry required bits".into(),
                ));
            }
            validate_scope_addressing(
                item.scope,
                item.operation_kind,
                item.section_id,
                item.target_local_ref,
                footer,
            )?;
            for idx in 0..item.required_word_count {
                let local = item
                    .required_feature_word_index
                    .checked_add(idx)
                    .ok_or(CoveError::ArithOverflow)?;
                let global = item
                    .required_first_feature_word_number
                    .checked_add(idx)
                    .ok_or(CoveError::ArithOverflow)?;
                validate_feature_word_reference(
                    global,
                    binding.feature_words[local as usize],
                    0,
                    header,
                    extended,
                    "SECTION_FEATURE_BINDING",
                )?;
            }
            for idx in 0..item.optional_word_count {
                let local = item
                    .optional_feature_word_index
                    .checked_add(idx)
                    .ok_or(CoveError::ArithOverflow)?;
                let global = item
                    .optional_first_feature_word_number
                    .checked_add(idx)
                    .ok_or(CoveError::ArithOverflow)?;
                validate_feature_word_reference(
                    global,
                    0,
                    binding.feature_words[local as usize],
                    header,
                    extended,
                    "SECTION_FEATURE_BINDING",
                )?;
            }
        }
    }
    Ok(())
}

fn validate_scope_addressing(
    scope: FeatureScopeV2,
    operation_kind: OperationKindV2,
    section_id: u32,
    target_local_ref: u64,
    footer: &CoveFooter,
) -> Result<(), CoveError> {
    if scope == FeatureScopeV2::AdvisoryOnly {
        return Ok(());
    }
    match scope {
        FeatureScopeV2::SectionRequired => {
            if section_id == 0 || !section_exists(footer, section_id) {
                return Err(CoveError::BadSection(
                    "SectionRequired feature scope references a missing section".into(),
                ));
            }
        }
        FeatureScopeV2::PageRequired => {
            if section_id == 0
                || target_local_ref == u64::MAX
                || !section_exists(footer, section_id)
            {
                return Err(CoveError::BadSection(
                    "PageRequired feature scope must reference an existing section and page target"
                        .into(),
                ));
            }
        }
        FeatureScopeV2::OperationRequired => {
            if operation_kind == OperationKindV2::None {
                return Err(CoveError::BadSection(
                    "OperationRequired feature scope requires operation_kind".into(),
                ));
            }
        }
        FeatureScopeV2::ProfileRequired => {
            if operation_kind != OperationKindV2::None {
                return Err(CoveError::BadSection(
                    "ProfileRequired feature scope must not set operation_kind".into(),
                ));
            }
        }
        FeatureScopeV2::FileRequired | FeatureScopeV2::AdvisoryOnly => {}
    }
    Ok(())
}

fn validate_feature_word_reference(
    word_index: u32,
    required_mask: u64,
    optional_mask: u64,
    header: &CoveHeaderV1,
    extended: Option<&ExtendedFeatureSetV2>,
    surface: &str,
) -> Result<(), CoveError> {
    if word_index == 0 {
        let advertised_required = header.required_features | header.optional_features;
        if required_mask & !advertised_required != 0
            || optional_mask & !header.optional_features != 0
        {
            return Err(CoveError::BadSection(format!(
                "{surface} references undeclared feature word 0 bits"
            )));
        }
        return Ok(());
    }
    let Some(extended) = extended else {
        return Err(CoveError::BadSection(format!(
            "{surface} references extended feature word without EXTENDED_FEATURE_SET"
        )));
    };
    if word_index >= extended.header.word_count {
        return Err(CoveError::BadSection(format!(
            "{surface} references a feature word beyond EXTENDED_FEATURE_SET horizon"
        )));
    }
    if required_mask & !extended.required_word(word_index) != 0 {
        return Err(CoveError::BadSection(format!(
            "{surface} required mask is not declared in EXTENDED_FEATURE_SET"
        )));
    }
    if optional_mask & !extended.optional_word(word_index) != 0 {
        return Err(CoveError::BadSection(format!(
            "{surface} optional mask is not declared in EXTENDED_FEATURE_SET"
        )));
    }
    Ok(())
}

fn scoped_required_masks(
    profile_matrix: Option<&ProfileCapabilityMatrixV2>,
    section_bindings: &[SectionFeatureBindingSectionV2],
) -> Result<std::collections::BTreeMap<u32, u64>, CoveError> {
    let mut scoped = std::collections::BTreeMap::<u32, u64>::new();
    if let Some(matrix) = profile_matrix {
        for entry in &matrix.entries {
            if entry.scope != FeatureScopeV2::FileRequired && entry.required_mask != 0 {
                *scoped.entry(entry.global_feature_word_index).or_default() |= entry.required_mask;
            }
        }
    }
    for binding in section_bindings {
        for item in &binding.bindings {
            if item.scope == FeatureScopeV2::FileRequired {
                continue;
            }
            for idx in 0..item.required_word_count {
                let local = item
                    .required_feature_word_index
                    .checked_add(idx)
                    .ok_or(CoveError::ArithOverflow)?;
                let global = item
                    .required_first_feature_word_number
                    .checked_add(idx)
                    .ok_or(CoveError::ArithOverflow)?;
                *scoped.entry(global).or_default() |= binding.feature_words[local as usize];
            }
        }
    }
    Ok(scoped)
}

fn section_exists(footer: &CoveFooter, section_id: u32) -> bool {
    footer
        .sections
        .iter()
        .any(|entry| entry.section_id == section_id)
}

fn validate_binding_word_horizon(
    first_word_number: u32,
    word_count: u32,
    horizon: u32,
) -> Result<(), CoveError> {
    if word_count == 0 {
        return Ok(());
    }
    let end = first_word_number
        .checked_add(word_count)
        .ok_or(CoveError::ArithOverflow)?;
    if first_word_number >= horizon || end > horizon {
        return Err(CoveError::BadSection(
            "SECTION_FEATURE_BINDING references a feature word beyond EXTENDED_FEATURE_SET horizon"
                .into(),
        ));
    }
    Ok(())
}

fn word_bytes_len(word_count: u32) -> Result<usize, CoveError> {
    usize::try_from(word_count)
        .map_err(|_| CoveError::ArithOverflow)?
        .checked_mul(8)
        .ok_or(CoveError::ArithOverflow)
}

fn read_words(bytes: &[u8]) -> Result<Vec<u64>, CoveError> {
    if !bytes.len().is_multiple_of(8) {
        return Err(CoveError::BadSection(
            "feature word byte length is not a multiple of 8".into(),
        ));
    }
    Ok(bytes
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()))
        .collect())
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
    use crate::{
        constants::{FEATURE_TABLE_PROFILE, MAGIC_COVE, VERSION_MAJOR_V1},
        footer::{CoveFooterHeaderV1, FOOTER_HEADER_SIZE},
    };

    fn header() -> CoveHeaderV1 {
        CoveHeaderV1 {
            magic: MAGIC_COVE,
            header_len: crate::constants::HEADER_LEN_V1,
            version_major: VERSION_MAJOR_V1,
            version_minor: 0,
            primary_profile: 2,
            endianness: crate::constants::ENDIANNESS_LITTLE,
            flags: 0,
            required_features: FEATURE_TABLE_PROFILE,
            optional_features: 0,
            file_id: [0; 16],
            producer_scope_id: [0; 16],
            producer_scope_kind: 0,
            reserved_scope_flags: 0,
            created_at_us: 0,
            feature_set_section_id: 0,
            profile_capability_section_id: 0,
            fast_metadata_section_id: 0,
            v2_flags: 0,
            reserved: [0; 64],
            checksum: 0,
        }
    }

    #[test]
    fn extended_feature_set_round_trips_and_checks_word_zero() {
        let set = ExtendedFeatureSetV2 {
            header: ExtendedFeatureSetHeaderV2 {
                word_count: 2,
                required_word_count: 2,
                optional_word_count: 1,
                flags: 0,
                checksum: 0,
            },
            required_feature_words: vec![FEATURE_TABLE_PROFILE, 1],
            optional_feature_words: vec![0],
        };
        let bytes = set.serialize().unwrap();
        let parsed = ExtendedFeatureSetV2::parse(&bytes).unwrap();
        parsed
            .validate_against_low_words(FEATURE_TABLE_PROFILE, 0)
            .unwrap();
        assert_eq!(parsed.required_word(1), 1);
    }

    #[test]
    fn profile_capability_matrix_requires_sorted_entries() {
        let mut first = ProfileCapabilityEntryV2 {
            profile: 2,
            scope: FeatureScopeV2::ProfileRequired,
            operation_kind: OperationKindV2::None,
            global_feature_word_index: 1,
            required_mask: 1,
            optional_mask: 0,
            section_id: 0,
            target_local_ref: u64::MAX,
            flags: 0,
            reserved: 0,
            checksum: 0,
        };
        let mut second = first.clone();
        first.global_feature_word_index = 2;
        second.global_feature_word_index = 1;
        let matrix = ProfileCapabilityMatrixV2 {
            header: ProfileCapabilityMatrixHeaderV2 {
                magic: MAGIC_PROFILE_CAPABILITY_MATRIX,
                version_major: 2,
                header_len: ProfileCapabilityMatrixHeaderV2::LEN as u16,
                entry_len: ProfileCapabilityEntryV2::LEN as u16,
                reserved: 0,
                entry_count: 2,
                flags: 0,
                entries_offset: ProfileCapabilityMatrixHeaderV2::LEN as u64,
                entries_length: (2 * ProfileCapabilityEntryV2::LEN) as u64,
                checksum: 0,
            },
            entries: vec![first, second],
        };
        assert!(matches!(matrix.serialize(), Err(CoveError::BadSection(_))));
    }

    #[test]
    fn feature_scope_table_rejects_file_required_extended_unknown() {
        let extended = ExtendedFeatureSetV2 {
            header: ExtendedFeatureSetHeaderV2 {
                word_count: 2,
                required_word_count: 2,
                optional_word_count: 1,
                flags: 0,
                checksum: 0,
            },
            required_feature_words: vec![FEATURE_TABLE_PROFILE, 0x10],
            optional_feature_words: vec![0],
        };
        let footer = CoveFooter {
            header: CoveFooterHeaderV1 {
                footer_magic: crate::constants::MAGIC_COVE_FOOTER,
                footer_version: crate::constants::FOOTER_VERSION_V1,
                header_len: FOOTER_HEADER_SIZE as u16,
                section_count: 0,
                section_entry_len: crate::constants::SECTION_ENTRY_LEN,
                flags: 0,
                metadata_len: 0,
                reserved: [0; 24],
            },
            sections: Vec::new(),
            metadata_json: Vec::new(),
        };
        let table =
            FeatureScopeTable::build(&header(), &footer, Some(&extended), None, None).unwrap();
        assert!(matches!(
            table.reject_file_required_unknowns(),
            Err(CoveError::UnknownRequiredFeature(0x10))
        ));
    }
}
