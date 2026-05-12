//! Cove Format (COVE) v1.0 — Extension registry and descriptor payloads
//! (Spec §§45-47).

use crate::{
    checksum, compression,
    constants::{CoveLogicalType, SectionKind, ValueTag},
    footer::{CoveFooter, CoveSectionEntryV1},
    CoveError,
};

/// Extension registry header length in bytes.
pub const EXTENSION_REGISTRY_HEADER_LEN: usize = 8;

/// Fixed portion of an `ExtensionLogicalTypeV1` excluding the Arrow extension
/// name bytes.
pub const EXTENSION_LOGICAL_TYPE_FIXED_LEN: usize = 16;

/// Encoded length of `ExtensionIndexDescriptorV1`.
pub const EXTENSION_INDEX_DESCRIPTOR_LEN: usize = 18;

/// Spec §45 `ExtensionKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
#[non_exhaustive]
pub enum ExtensionKind {
    LogicalType = 0,
    PhysicalKind = 1,
    Encoding = 2,
    CompressionCodec = 3,
    Index = 4,
    AggregateSynopsis = 5,
    PredicateKernel = 6,
    EngineProfile = 7,
    RedactionPolicy = 8,
    TrustPolicy = 9,
    SemanticMapping = 10,
    MappingFunction = 11,
    SourceConnector = 12,
    VendorMetadata = 255,
}

impl ExtensionKind {
    pub fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            0 => Self::LogicalType,
            1 => Self::PhysicalKind,
            2 => Self::Encoding,
            3 => Self::CompressionCodec,
            4 => Self::Index,
            5 => Self::AggregateSynopsis,
            6 => Self::PredicateKernel,
            7 => Self::EngineProfile,
            8 => Self::RedactionPolicy,
            9 => Self::TrustPolicy,
            10 => Self::SemanticMapping,
            11 => Self::MappingFunction,
            12 => Self::SourceConnector,
            255 => Self::VendorMetadata,
            _ => return None,
        })
    }
}

/// Spec §45 `ExtensionEntryV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionRegistryEntry {
    pub extension_id: u32,
    pub namespace: Vec<u8>,
    pub name: Vec<u8>,
    pub version_major: u16,
    pub version_minor: u16,
    pub extension_kind: ExtensionKind,
    pub required_feature_bit: u64,
    pub optional_feature_bit: u64,
    pub fallback_kind: u16,
    pub fallback_ref: u32,
    pub payload_ref: u32,
    pub checksum: u32,
}

/// Spec §45 `ExtensionRegistryHeaderV1` plus entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionRegistry {
    pub flags: u32,
    pub entries: Vec<ExtensionRegistryEntry>,
}

/// Spec §46 `ExtensionLogicalTypeV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionLogicalTypeV1 {
    pub extension_id: u32,
    pub base_logical_type: CoveLogicalType,
    pub canonical_value_tag: ValueTag,
    pub collation_id: u16,
    pub flags: u16,
    pub arrow_extension_name: String,
    pub metadata_payload_ref: u32,
}

/// Spec §47 proof capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum ExtensionProofCapability {
    None = 0,
    DefinitelyNo = 1,
    DefinitelyNoAndYes = 2,
}

impl ExtensionProofCapability {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::None,
            1 => Self::DefinitelyNo,
            2 => Self::DefinitelyNoAndYes,
            _ => return None,
        })
    }
}

/// Spec §47 false-negative policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum ExtensionFalseNegativePolicy {
    MustNotHaveFalseNegatives = 0,
    MayHaveFalseNegatives = 1,
}

impl ExtensionFalseNegativePolicy {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::MustNotHaveFalseNegatives,
            1 => Self::MayHaveFalseNegatives,
            _ => return None,
        })
    }
}

/// Spec §47 `ExtensionIndexDescriptorV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionIndexDescriptorV1 {
    pub extension_id: u32,
    pub index_kind: u16,
    pub key_column_count: u16,
    pub proof_capability: ExtensionProofCapability,
    pub false_negative_policy: ExtensionFalseNegativePolicy,
    pub flags: u32,
    pub payload_ref: u32,
}

/// Optional validation context available only when validating a full file.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExtensionValidationContext {
    /// Number of collation entries available in the file-local collation
    /// registry. `None` means no registry was present.
    pub collation_count: Option<usize>,
}

impl ExtensionRegistry {
    /// Parse an extension registry from raw section bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < EXTENSION_REGISTRY_HEADER_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let extension_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if flags != 0 {
            return Err(CoveError::ReservedNotZero);
        }

        let mut pos = EXTENSION_REGISTRY_HEADER_LEN;
        let mut entries = Vec::with_capacity(extension_count);
        for _ in 0..extension_count {
            let start = pos;
            let extension_id = read_u32(bytes, &mut pos)?;
            let namespace = read_len_prefixed_bytes(bytes, &mut pos)?;
            let name = read_len_prefixed_bytes(bytes, &mut pos)?;
            let version_major = read_u16(bytes, &mut pos)?;
            let version_minor = read_u16(bytes, &mut pos)?;
            let extension_kind_raw = read_u16(bytes, &mut pos)?;
            let extension_kind =
                ExtensionKind::from_u16(extension_kind_raw).ok_or(CoveError::BadExtension)?;
            let required_feature_bit = read_u64(bytes, &mut pos)?;
            let optional_feature_bit = read_u64(bytes, &mut pos)?;
            let fallback_kind = read_u16(bytes, &mut pos)?;
            let fallback_ref = read_u32(bytes, &mut pos)?;
            let payload_ref = read_u32(bytes, &mut pos)?;
            let checksum_pos = pos;
            let checksum_field = read_u32(bytes, &mut pos)?;
            let mut entry_bytes = bytes[start..pos].to_vec();
            entry_bytes[checksum_pos - start..checksum_pos - start + 4].fill(0);
            if checksum::crc32c(&entry_bytes) != checksum_field {
                return Err(CoveError::ChecksumMismatch);
            }

            entries.push(ExtensionRegistryEntry {
                extension_id,
                namespace,
                name,
                version_major,
                version_minor,
                extension_kind,
                required_feature_bit,
                optional_feature_bit,
                fallback_kind,
                fallback_ref,
                payload_ref,
                checksum: checksum_field,
            });
        }
        if pos != bytes.len() {
            return Err(CoveError::BadExtension);
        }

        Ok(Self { flags, entries })
    }

    /// Inverse of [`Self::parse`]; computes entry checksums canonically.
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        if self.flags != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        let count = u32::try_from(self.entries.len())
            .map_err(|_| CoveError::BadSection("too many extension entries".into()))?;
        let mut out = Vec::with_capacity(EXTENSION_REGISTRY_HEADER_LEN + self.entries.len() * 64);
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize()?);
        }
        Ok(out)
    }

    /// Full-file registry validation. Unknown required extensions reject;
    /// optional unknown extensions are ignored when policy allows them.
    pub fn validate_in_file(
        &self,
        data: &[u8],
        footer: &CoveFooter,
        allow_unknown_optional: bool,
        context: ExtensionValidationContext,
    ) -> Result<(), CoveError> {
        let mut seen_ids = std::collections::HashSet::new();
        for entry in &self.entries {
            if !seen_ids.insert(entry.extension_id) {
                return Err(CoveError::BadExtension);
            }
            if entry.fallback_ref != 0 {
                section_by_id(footer, entry.fallback_ref).ok_or(CoveError::BadExtension)?;
                if SectionKind::from_u16(entry.fallback_kind).is_none() {
                    return Err(CoveError::BadExtension);
                }
            }
            if requires_canonical_fallback(entry.extension_kind)
                && entry.required_feature_bit == 0
                && entry.fallback_ref == 0
            {
                return Err(CoveError::BadExtension);
            }
            if entry.payload_ref != 0 {
                let payload_entry =
                    section_by_id(footer, entry.payload_ref).ok_or(CoveError::BadExtension)?;
                let payload = compression::section_payload(data, payload_entry)?;
                match entry.extension_kind {
                    ExtensionKind::LogicalType => {
                        let descriptor = ExtensionLogicalTypeV1::parse(&payload)?;
                        descriptor.validate(context)?;
                    }
                    ExtensionKind::Index | ExtensionKind::AggregateSynopsis => {
                        let descriptor = ExtensionIndexDescriptorV1::parse(&payload)?;
                        descriptor.validate()?;
                    }
                    _ => {}
                }
            }

            let is_required = entry.required_feature_bit != 0;
            let is_known_payload_kind = matches!(
                entry.extension_kind,
                ExtensionKind::LogicalType
                    | ExtensionKind::Index
                    | ExtensionKind::AggregateSynopsis
            );
            if is_required && !is_known_payload_kind {
                return Err(CoveError::BadExtension);
            }
            if !is_required && !allow_unknown_optional && !is_known_payload_kind {
                return Err(CoveError::BadExtension);
            }
        }
        Ok(())
    }

    /// Compatibility helper for callers that only have a registry payload.
    pub fn validate_known(&self, allow_unknown_optional: bool) -> Result<(), CoveError> {
        for entry in &self.entries {
            if requires_canonical_fallback(entry.extension_kind)
                && entry.required_feature_bit == 0
                && entry.fallback_ref == 0
            {
                return Err(CoveError::BadExtension);
            }
            if entry.required_feature_bit != 0 {
                return Err(CoveError::BadExtension);
            }
            if !allow_unknown_optional
                && !matches!(
                    entry.extension_kind,
                    ExtensionKind::LogicalType
                        | ExtensionKind::Index
                        | ExtensionKind::AggregateSynopsis
                )
            {
                return Err(CoveError::BadExtension);
            }
        }
        Ok(())
    }
}

fn requires_canonical_fallback(kind: ExtensionKind) -> bool {
    matches!(
        kind,
        ExtensionKind::PhysicalKind | ExtensionKind::Encoding | ExtensionKind::CompressionCodec
    )
}

impl ExtensionRegistryEntry {
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.extension_id.to_le_bytes());
        write_len_prefixed_bytes(&mut out, &self.namespace, "extension namespace")?;
        write_len_prefixed_bytes(&mut out, &self.name, "extension name")?;
        out.extend_from_slice(&self.version_major.to_le_bytes());
        out.extend_from_slice(&self.version_minor.to_le_bytes());
        out.extend_from_slice(&(self.extension_kind as u16).to_le_bytes());
        out.extend_from_slice(&self.required_feature_bit.to_le_bytes());
        out.extend_from_slice(&self.optional_feature_bit.to_le_bytes());
        out.extend_from_slice(&self.fallback_kind.to_le_bytes());
        out.extend_from_slice(&self.fallback_ref.to_le_bytes());
        out.extend_from_slice(&self.payload_ref.to_le_bytes());
        let checksum_pos = out.len();
        out.extend_from_slice(&0u32.to_le_bytes());
        let checksum = checksum::crc32c(&out);
        out[checksum_pos..checksum_pos + 4].copy_from_slice(&checksum.to_le_bytes());
        Ok(out)
    }
}

impl ExtensionLogicalTypeV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < EXTENSION_LOGICAL_TYPE_FIXED_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let extension_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let base_logical_raw = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        let canonical_tag_raw = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        let collation_id = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        let flags = u16::from_le_bytes(bytes[10..12].try_into().unwrap());
        let name_len = u16::from_le_bytes(bytes[12..14].try_into().unwrap()) as usize;
        let name_start = 14usize;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or(CoveError::ArithOverflow)?;
        let metadata_ref_end = name_end.checked_add(4).ok_or(CoveError::ArithOverflow)?;
        if metadata_ref_end > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        if metadata_ref_end != bytes.len() {
            return Err(CoveError::BadExtension);
        }
        let arrow_extension_name = std::str::from_utf8(&bytes[name_start..name_end])
            .map_err(|_| CoveError::BadExtension)?
            .to_string();
        let metadata_payload_ref =
            u32::from_le_bytes(bytes[name_end..metadata_ref_end].try_into().unwrap());
        let base_logical_type =
            CoveLogicalType::from_u16(base_logical_raw).ok_or(CoveError::BadExtension)?;
        let canonical_value_tag =
            ValueTag::from_u16(canonical_tag_raw).ok_or(CoveError::BadExtension)?;
        Ok(Self {
            extension_id,
            base_logical_type,
            canonical_value_tag,
            collation_id,
            flags,
            arrow_extension_name,
            metadata_payload_ref,
        })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let name = self.arrow_extension_name.as_bytes();
        let name_len = u16::try_from(name.len())
            .map_err(|_| CoveError::BadSection("Arrow extension name exceeds u16".into()))?;
        let mut out = Vec::with_capacity(EXTENSION_LOGICAL_TYPE_FIXED_LEN + name.len());
        out.extend_from_slice(&self.extension_id.to_le_bytes());
        out.extend_from_slice(&(self.base_logical_type as u16).to_le_bytes());
        out.extend_from_slice(&(self.canonical_value_tag as u16).to_le_bytes());
        out.extend_from_slice(&self.collation_id.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&self.metadata_payload_ref.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self, context: ExtensionValidationContext) -> Result<(), CoveError> {
        if let Some(count) = context.collation_count {
            if self.collation_id != 0 && usize::from(self.collation_id) >= count {
                return Err(CoveError::BadExtension);
            }
        }
        Ok(())
    }
}

impl ExtensionIndexDescriptorV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < EXTENSION_INDEX_DESCRIPTOR_LEN {
            return Err(CoveError::BufferTooShort);
        }
        if bytes.len() != EXTENSION_INDEX_DESCRIPTOR_LEN {
            return Err(CoveError::BadExtension);
        }
        let proof_capability =
            ExtensionProofCapability::from_u8(bytes[8]).ok_or(CoveError::BadExtension)?;
        let false_negative_policy =
            ExtensionFalseNegativePolicy::from_u8(bytes[9]).ok_or(CoveError::BadExtension)?;
        Ok(Self {
            extension_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            index_kind: u16::from_le_bytes(bytes[4..6].try_into().unwrap()),
            key_column_count: u16::from_le_bytes(bytes[6..8].try_into().unwrap()),
            proof_capability,
            false_negative_policy,
            flags: u32::from_le_bytes(bytes[10..14].try_into().unwrap()),
            payload_ref: u32::from_le_bytes(bytes[14..18].try_into().unwrap()),
        })
    }

    pub fn serialize(&self) -> [u8; EXTENSION_INDEX_DESCRIPTOR_LEN] {
        let mut out = [0u8; EXTENSION_INDEX_DESCRIPTOR_LEN];
        out[0..4].copy_from_slice(&self.extension_id.to_le_bytes());
        out[4..6].copy_from_slice(&self.index_kind.to_le_bytes());
        out[6..8].copy_from_slice(&self.key_column_count.to_le_bytes());
        out[8] = self.proof_capability as u8;
        out[9] = self.false_negative_policy as u8;
        out[10..14].copy_from_slice(&self.flags.to_le_bytes());
        out[14..18].copy_from_slice(&self.payload_ref.to_le_bytes());
        out
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.key_column_count == 0 {
            return Err(CoveError::BadExtension);
        }
        if self.false_negative_policy == ExtensionFalseNegativePolicy::MayHaveFalseNegatives
            && self.proof_capability != ExtensionProofCapability::None
        {
            return Err(CoveError::BadExtension);
        }
        Ok(())
    }

    pub fn can_skip_data(&self) -> bool {
        self.false_negative_policy == ExtensionFalseNegativePolicy::MustNotHaveFalseNegatives
            && matches!(
                self.proof_capability,
                ExtensionProofCapability::DefinitelyNo
                    | ExtensionProofCapability::DefinitelyNoAndYes
            )
    }
}

fn section_by_id(footer: &CoveFooter, section_id: u32) -> Option<&CoveSectionEntryV1> {
    footer
        .sections
        .iter()
        .find(|entry| entry.section_id == section_id)
}

fn read_u16(bytes: &[u8], pos: &mut usize) -> Result<u16, CoveError> {
    let end = pos.checked_add(2).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let value = u16::from_le_bytes(bytes[*pos..end].try_into().unwrap());
    *pos = end;
    Ok(value)
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32, CoveError> {
    let end = pos.checked_add(4).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let value = u32::from_le_bytes(bytes[*pos..end].try_into().unwrap());
    *pos = end;
    Ok(value)
}

fn read_u64(bytes: &[u8], pos: &mut usize) -> Result<u64, CoveError> {
    let end = pos.checked_add(8).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let value = u64::from_le_bytes(bytes[*pos..end].try_into().unwrap());
    *pos = end;
    Ok(value)
}

fn read_len_prefixed_bytes(bytes: &[u8], pos: &mut usize) -> Result<Vec<u8>, CoveError> {
    let len = read_u16(bytes, pos)? as usize;
    let end = pos.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let out = bytes[*pos..end].to_vec();
    *pos = end;
    Ok(out)
}

fn write_len_prefixed_bytes(out: &mut Vec<u8>, bytes: &[u8], what: &str) -> Result<(), CoveError> {
    let len = u16::try_from(bytes.len())
        .map_err(|_| CoveError::BadSection(format!("{what} exceeds u16 length limit")))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: ExtensionKind, required: u64, payload_ref: u32) -> ExtensionRegistryEntry {
        ExtensionRegistryEntry {
            extension_id: 7,
            namespace: b"org.example".to_vec(),
            name: b"feature-x".to_vec(),
            version_major: 1,
            version_minor: 0,
            extension_kind: kind,
            required_feature_bit: required,
            optional_feature_bit: if required == 0 { 0x20_0000 } else { 0 },
            fallback_kind: 0,
            fallback_ref: 0,
            payload_ref,
            checksum: 0,
        }
    }

    #[test]
    fn registry_round_trips_with_computed_crc() {
        let reg = ExtensionRegistry {
            flags: 0,
            entries: vec![entry(ExtensionKind::VendorMetadata, 0, 0)],
        };
        let parsed = ExtensionRegistry::parse(&reg.serialize().unwrap()).unwrap();
        assert_eq!(parsed.entries[0].namespace, b"org.example");
        assert_ne!(parsed.entries[0].checksum, 0);
    }

    #[test]
    fn registry_rejects_bad_entry_crc() {
        let reg = ExtensionRegistry {
            flags: 0,
            entries: vec![entry(ExtensionKind::VendorMetadata, 0, 0)],
        };
        let mut bytes = reg.serialize().unwrap();
        *bytes.last_mut().unwrap() ^= 0xFF;
        assert_eq!(
            ExtensionRegistry::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        );
    }

    #[test]
    fn registry_rejects_reserved_flags() {
        let mut bytes = ExtensionRegistry {
            flags: 0,
            entries: vec![],
        }
        .serialize()
        .unwrap();
        bytes[4] = 1;
        assert_eq!(
            ExtensionRegistry::parse(&bytes),
            Err(CoveError::ReservedNotZero)
        );
    }

    #[test]
    fn registry_rejects_trailing_bytes() {
        let mut bytes = ExtensionRegistry {
            flags: 0,
            entries: vec![],
        }
        .serialize()
        .unwrap();
        bytes.push(0);
        assert_eq!(
            ExtensionRegistry::parse(&bytes),
            Err(CoveError::BadExtension)
        );
    }

    #[test]
    fn logical_type_descriptor_round_trips() {
        let descriptor = ExtensionLogicalTypeV1 {
            extension_id: 7,
            base_logical_type: CoveLogicalType::Utf8,
            canonical_value_tag: ValueTag::Utf8,
            collation_id: 0,
            flags: 0,
            arrow_extension_name: "org.example.patient-id".into(),
            metadata_payload_ref: 0,
        };
        assert_eq!(
            ExtensionLogicalTypeV1::parse(&descriptor.serialize().unwrap()).unwrap(),
            descriptor
        );
    }

    #[test]
    fn index_descriptor_false_negative_indexes_cannot_skip() {
        let descriptor = ExtensionIndexDescriptorV1 {
            extension_id: 7,
            index_kind: 100,
            key_column_count: 1,
            proof_capability: ExtensionProofCapability::None,
            false_negative_policy: ExtensionFalseNegativePolicy::MayHaveFalseNegatives,
            flags: 0,
            payload_ref: 0,
        };
        assert!(descriptor.validate().is_ok());
        assert!(!descriptor.can_skip_data());
    }

    #[test]
    fn index_descriptor_rejects_false_negative_proof_claim() {
        let descriptor = ExtensionIndexDescriptorV1 {
            extension_id: 7,
            index_kind: 100,
            key_column_count: 1,
            proof_capability: ExtensionProofCapability::DefinitelyNo,
            false_negative_policy: ExtensionFalseNegativePolicy::MayHaveFalseNegatives,
            flags: 0,
            payload_ref: 0,
        };
        assert_eq!(descriptor.validate(), Err(CoveError::BadExtension));
    }

    #[test]
    fn required_unknown_extension_rejected_by_compat_helper() {
        let reg = ExtensionRegistry {
            flags: 0,
            entries: vec![entry(ExtensionKind::VendorMetadata, 0x20_0000, 0)],
        };
        assert_eq!(reg.validate_known(true), Err(CoveError::BadExtension));
    }

    #[test]
    fn optional_custom_physical_encoding_requires_fallback() {
        let reg = ExtensionRegistry {
            flags: 0,
            entries: vec![entry(ExtensionKind::Encoding, 0, 0)],
        };
        assert_eq!(reg.validate_known(true), Err(CoveError::BadExtension));
    }
}
