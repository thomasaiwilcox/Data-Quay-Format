//! COVE-CX registered codec descriptors and envelopes for COVE v2.

use std::collections::BTreeSet;

use cove_core::{checksum, CoveError};

const ABSENT_REF: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodecSpecificationStatusV2 {
    Candidate = 0,
    ProvisionalRegistered = 1,
    StableRegistered = 2,
    Deprecated = 3,
    VendorPrivate = 255,
}

impl CodecSpecificationStatusV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Candidate),
            1 => Some(Self::ProvisionalRegistered),
            2 => Some(Self::StableRegistered),
            3 => Some(Self::Deprecated),
            255 => Some(Self::VendorPrivate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodecRequirementV2 {
    OptionalWithFallback = 0,
    RequiredForDecode = 1,
    SidecarOnly = 2,
}

impl CodecRequirementV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::OptionalWithFallback),
            1 => Some(Self::RequiredForDecode),
            2 => Some(Self::SidecarOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodecFallbackPolicyV2 {
    NoFallback = 0,
    CoreEncodingPayloadPresent = 1,
    DictionaryOrCanonicalDecodePath = 2,
    ExternalRequiredExtension = 3,
}

impl CodecFallbackPolicyV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::NoFallback),
            1 => Some(Self::CoreEncodingPayloadPresent),
            2 => Some(Self::DictionaryOrCanonicalDecodePath),
            3 => Some(Self::ExternalRequiredExtension),
            _ => None,
        }
    }

    pub fn requires_fallback_ref(self) -> bool {
        !matches!(self, Self::NoFallback)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecExtensionDescriptorV2 {
    pub codec_id: u32,
    pub namespace: String,
    pub name: String,
    pub version_major: u16,
    pub version_minor: u16,
    pub codec_family: u16,
    pub logical_type_mask: u64,
    pub physical_kind_mask: u64,
    pub requirement: CodecRequirementV2,
    pub fallback_policy: CodecFallbackPolicyV2,
    pub parameter_schema_kind: u8,
    pub flags: u8,
    pub specification_status: CodecSpecificationStatusV2,
    pub required_feature_bit: u64,
    pub optional_feature_bit: u64,
    pub spec_digest_algorithm: u16,
    pub spec_digest: Vec<u8>,
    pub conformance_vector_ref: u32,
    pub fallback_ref: u32,
    pub private_payload_ref: u32,
    pub checksum: u32,
}

impl CodecExtensionDescriptorV2 {
    pub fn parse_one(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        let start = 0usize;
        let mut cursor = Cursor::new(bytes);
        let codec_id = cursor.u32()?;
        let namespace_len = cursor.u16()? as usize;
        let namespace_bytes = cursor.bytes(namespace_len)?;
        let namespace = parse_utf8(namespace_bytes, "codec namespace")?;
        let name_len = cursor.u16()? as usize;
        let name_bytes = cursor.bytes(name_len)?;
        let name = parse_utf8(name_bytes, "codec name")?;
        let version_major = cursor.u16()?;
        let version_minor = cursor.u16()?;
        let codec_family = cursor.u16()?;
        let logical_type_mask = cursor.u64()?;
        let physical_kind_mask = cursor.u64()?;
        let requirement_raw = cursor.u8()?;
        let fallback_policy_raw = cursor.u8()?;
        let parameter_schema_kind = cursor.u8()?;
        let flags = cursor.u8()?;
        let status_raw = cursor.u8()?;
        let reserved0 = cursor.bytes(3)?;
        if reserved0.iter().any(|byte| *byte != 0) {
            return Err(CoveError::ReservedNotZero);
        }
        let required_feature_bit = cursor.u64()?;
        let optional_feature_bit = cursor.u64()?;
        let spec_digest_algorithm = cursor.u16()?;
        let spec_digest_len = cursor.u16()? as usize;
        let spec_digest = cursor.bytes(spec_digest_len)?.to_vec();
        let conformance_vector_ref = cursor.u32()?;
        let fallback_ref = cursor.u32()?;
        let private_payload_ref = cursor.u32()?;
        let checksum_field_offset = cursor.position;
        let checksum = cursor.u32()?;
        let consumed = cursor.position;

        let descriptor_bytes = &bytes[start..consumed];
        let mut checksum_bytes = descriptor_bytes.to_vec();
        checksum_bytes[checksum_field_offset..checksum_field_offset + 4].fill(0);
        if checksum::crc32c(&checksum_bytes) != checksum {
            return Err(CoveError::ChecksumMismatch);
        }

        let requirement = CodecRequirementV2::from_u8(requirement_raw)
            .ok_or_else(|| CoveError::BadCodecExtension)?;
        let fallback_policy = CodecFallbackPolicyV2::from_u8(fallback_policy_raw)
            .ok_or_else(|| CoveError::BadCodecExtension)?;
        let specification_status = CodecSpecificationStatusV2::from_u8(status_raw)
            .ok_or_else(|| CoveError::BadCodecExtension)?;

        let descriptor = Self {
            codec_id,
            namespace,
            name,
            version_major,
            version_minor,
            codec_family,
            logical_type_mask,
            physical_kind_mask,
            requirement,
            fallback_policy,
            parameter_schema_kind,
            flags,
            specification_status,
            required_feature_bit,
            optional_feature_bit,
            spec_digest_algorithm,
            spec_digest,
            conformance_vector_ref,
            fallback_ref,
            private_payload_ref,
            checksum,
        };
        descriptor.validate()?;
        Ok((descriptor, consumed))
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        let mut descriptors = Vec::new();
        let mut offset = 0usize;
        while offset < bytes.len() {
            let (descriptor, consumed) = Self::parse_one(&bytes[offset..])?;
            if consumed == 0 {
                return Err(CoveError::BadCodecExtension);
            }
            descriptors.push(descriptor);
            offset = offset
                .checked_add(consumed)
                .ok_or(CoveError::ArithOverflow)?;
        }
        validate_descriptor_set(&descriptors)?;
        Ok(descriptors)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        if self.namespace.len() > u16::MAX as usize || self.name.len() > u16::MAX as usize {
            return Err(CoveError::BadCodecExtension);
        }
        if self.spec_digest.len() > u16::MAX as usize {
            return Err(CoveError::BadCodecExtension);
        }
        self.validate_without_checksum()?;
        let mut out = Vec::new();
        out.extend_from_slice(&self.codec_id.to_le_bytes());
        out.extend_from_slice(&(self.namespace.len() as u16).to_le_bytes());
        out.extend_from_slice(self.namespace.as_bytes());
        out.extend_from_slice(&(self.name.len() as u16).to_le_bytes());
        out.extend_from_slice(self.name.as_bytes());
        out.extend_from_slice(&self.version_major.to_le_bytes());
        out.extend_from_slice(&self.version_minor.to_le_bytes());
        out.extend_from_slice(&self.codec_family.to_le_bytes());
        out.extend_from_slice(&self.logical_type_mask.to_le_bytes());
        out.extend_from_slice(&self.physical_kind_mask.to_le_bytes());
        out.push(self.requirement as u8);
        out.push(self.fallback_policy as u8);
        out.push(self.parameter_schema_kind);
        out.push(self.flags);
        out.push(self.specification_status as u8);
        out.extend_from_slice(&[0, 0, 0]);
        out.extend_from_slice(&self.required_feature_bit.to_le_bytes());
        out.extend_from_slice(&self.optional_feature_bit.to_le_bytes());
        out.extend_from_slice(&self.spec_digest_algorithm.to_le_bytes());
        out.extend_from_slice(&(self.spec_digest.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.spec_digest);
        out.extend_from_slice(&self.conformance_vector_ref.to_le_bytes());
        out.extend_from_slice(&self.fallback_ref.to_le_bytes());
        out.extend_from_slice(&self.private_payload_ref.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        let checksum = checksum::crc32c(&out);
        let checksum_offset = out.len() - 4;
        out[checksum_offset..].copy_from_slice(&checksum.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        self.validate_without_checksum()
    }

    fn validate_without_checksum(&self) -> Result<(), CoveError> {
        if self.namespace.is_empty() || self.name.is_empty() {
            return Err(CoveError::BadCodecExtension);
        }
        if self.parameter_schema_kind > 3 {
            return Err(CoveError::BadCodecExtension);
        }
        if self.requirement == CodecRequirementV2::OptionalWithFallback
            && self.fallback_policy == CodecFallbackPolicyV2::NoFallback
        {
            return Err(CoveError::BadCodecExtension);
        }
        if self.fallback_policy.requires_fallback_ref() && self.fallback_ref == ABSENT_REF {
            return Err(CoveError::BadCodecExtension);
        }
        if self.specification_status == CodecSpecificationStatusV2::Candidate
            && self.requirement == CodecRequirementV2::RequiredForDecode
            && self.fallback_policy == CodecFallbackPolicyV2::NoFallback
        {
            return Err(CoveError::BadCodecExtension);
        }
        if self.specification_status == CodecSpecificationStatusV2::StableRegistered
            && self.requirement == CodecRequirementV2::RequiredForDecode
            && self.fallback_policy == CodecFallbackPolicyV2::NoFallback
            && self.required_feature_bit == 0
        {
            return Err(CoveError::BadCodecExtension);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredEncodingEnvelopeV2 {
    pub codec_id: u32,
    pub codec_version_major: u16,
    pub codec_version_minor: u16,
    pub logical_len: u32,
    pub non_null_count: u32,
    pub params_offset: u32,
    pub params_length: u32,
    pub encoded_payload_offset: u64,
    pub encoded_payload_length: u64,
    pub fallback_payload_offset: u64,
    pub fallback_payload_length: u64,
    pub decoded_uncompressed_length: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl RegisteredEncodingEnvelopeV2 {
    pub const LEN: usize = 72;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let mut cursor = Cursor::new(&bytes[..Self::LEN]);
        let envelope = Self {
            codec_id: cursor.u32()?,
            codec_version_major: cursor.u16()?,
            codec_version_minor: cursor.u16()?,
            logical_len: cursor.u32()?,
            non_null_count: cursor.u32()?,
            params_offset: cursor.u32()?,
            params_length: cursor.u32()?,
            encoded_payload_offset: cursor.u64()?,
            encoded_payload_length: cursor.u64()?,
            fallback_payload_offset: cursor.u64()?,
            fallback_payload_length: cursor.u64()?,
            decoded_uncompressed_length: cursor.u64()?,
            flags: cursor.u32()?,
            checksum: cursor.u32()?,
        };
        let mut check = bytes[..Self::LEN].to_vec();
        check[Self::LEN - 4..Self::LEN].fill(0);
        if checksum::crc32c(&check) != envelope.checksum {
            return Err(CoveError::ChecksumMismatch);
        }
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        if bytes.len() % Self::LEN != 0 {
            return Err(CoveError::BadCodecExtension);
        }
        bytes
            .chunks_exact(Self::LEN)
            .map(Self::parse)
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate_without_checksum()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.codec_id.to_le_bytes());
        out[4..6].copy_from_slice(&self.codec_version_major.to_le_bytes());
        out[6..8].copy_from_slice(&self.codec_version_minor.to_le_bytes());
        out[8..12].copy_from_slice(&self.logical_len.to_le_bytes());
        out[12..16].copy_from_slice(&self.non_null_count.to_le_bytes());
        out[16..20].copy_from_slice(&self.params_offset.to_le_bytes());
        out[20..24].copy_from_slice(&self.params_length.to_le_bytes());
        out[24..32].copy_from_slice(&self.encoded_payload_offset.to_le_bytes());
        out[32..40].copy_from_slice(&self.encoded_payload_length.to_le_bytes());
        out[40..48].copy_from_slice(&self.fallback_payload_offset.to_le_bytes());
        out[48..56].copy_from_slice(&self.fallback_payload_length.to_le_bytes());
        out[56..64].copy_from_slice(&self.decoded_uncompressed_length.to_le_bytes());
        out[64..68].copy_from_slice(&self.flags.to_le_bytes());
        let checksum = checksum::crc32c(&out);
        out[68..72].copy_from_slice(&checksum.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        self.validate_without_checksum()
    }

    fn validate_without_checksum(&self) -> Result<(), CoveError> {
        if self.non_null_count > self.logical_len {
            return Err(CoveError::BadCodecExtension);
        }
        if (self.fallback_payload_offset == 0) != (self.fallback_payload_length == 0) {
            return Err(CoveError::BadCodecExtension);
        }
        checked_end(self.params_offset as u64, self.params_length as u64)?;
        checked_end(self.encoded_payload_offset, self.encoded_payload_length)?;
        if self.fallback_payload_length != 0 {
            checked_end(self.fallback_payload_offset, self.fallback_payload_length)?;
        }
        Ok(())
    }
}

pub fn validate_descriptor_set(
    descriptors: &[CodecExtensionDescriptorV2],
) -> Result<(), CoveError> {
    let mut codec_ids = BTreeSet::new();
    let mut identities = BTreeSet::new();
    for descriptor in descriptors {
        descriptor.validate()?;
        if !codec_ids.insert(descriptor.codec_id) {
            return Err(CoveError::BadCodecExtension);
        }
        let identity = (
            descriptor.namespace.as_str(),
            descriptor.name.as_str(),
            descriptor.version_major,
            descriptor.version_minor,
        );
        if !identities.insert(identity) {
            return Err(CoveError::BadCodecExtension);
        }
    }
    Ok(())
}

fn parse_utf8(bytes: &[u8], field: &str) -> Result<String, CoveError> {
    std::str::from_utf8(bytes)
        .map(|value| value.to_string())
        .map_err(|_| CoveError::BadSection(format!("{field} is not valid UTF-8")))
}

fn checked_end(offset: u64, length: u64) -> Result<u64, CoveError> {
    offset.checked_add(length).ok_or(CoveError::ArithOverflow)
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

    fn u8(&mut self) -> Result<u8, CoveError> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, CoveError> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, CoveError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, CoveError> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(codec_id: u32) -> CodecExtensionDescriptorV2 {
        CodecExtensionDescriptorV2 {
            codec_id,
            namespace: "org.cove".into(),
            name: format!("codec-{codec_id}"),
            version_major: 1,
            version_minor: 0,
            codec_family: 3,
            logical_type_mask: 1,
            physical_kind_mask: 1,
            requirement: CodecRequirementV2::OptionalWithFallback,
            fallback_policy: CodecFallbackPolicyV2::CoreEncodingPayloadPresent,
            parameter_schema_kind: 0,
            flags: 0,
            specification_status: CodecSpecificationStatusV2::Candidate,
            required_feature_bit: 0,
            optional_feature_bit: 0,
            spec_digest_algorithm: 1,
            spec_digest: vec![1, 2, 3, 4],
            conformance_vector_ref: ABSENT_REF,
            fallback_ref: 42,
            private_payload_ref: ABSENT_REF,
            checksum: 0,
        }
    }

    #[test]
    fn descriptor_round_trips_and_validates_set() {
        let first = descriptor(1).serialize().unwrap();
        let second = descriptor(2).serialize().unwrap();
        let mut bytes = first;
        bytes.extend_from_slice(&second);
        let parsed = CodecExtensionDescriptorV2::parse_many(&bytes).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].namespace, "org.cove");
    }

    #[test]
    fn descriptor_rejects_bad_checksum() {
        let mut bytes = descriptor(1).serialize().unwrap();
        bytes[6] ^= 1;
        assert!(matches!(
            CodecExtensionDescriptorV2::parse_one(&bytes),
            Err(CoveError::ChecksumMismatch)
        ));
    }

    #[test]
    fn candidate_required_without_fallback_is_rejected() {
        let mut item = descriptor(1);
        item.requirement = CodecRequirementV2::RequiredForDecode;
        item.fallback_policy = CodecFallbackPolicyV2::NoFallback;
        item.fallback_ref = ABSENT_REF;
        assert!(matches!(
            item.serialize(),
            Err(CoveError::BadCodecExtension)
        ));
    }

    fn envelope() -> RegisteredEncodingEnvelopeV2 {
        RegisteredEncodingEnvelopeV2 {
            codec_id: 1,
            codec_version_major: 1,
            codec_version_minor: 0,
            logical_len: 4,
            non_null_count: 3,
            params_offset: 72,
            params_length: 8,
            encoded_payload_offset: 80,
            encoded_payload_length: 32,
            fallback_payload_offset: 112,
            fallback_payload_length: 16,
            decoded_uncompressed_length: 64,
            flags: 0,
            checksum: 0,
        }
    }

    #[test]
    fn registered_encoding_envelope_round_trips() {
        let first = envelope().serialize().unwrap();
        let mut second_item = envelope();
        second_item.codec_id = 2;
        let second = second_item.serialize().unwrap();
        let mut bytes = first.to_vec();
        bytes.extend_from_slice(&second);
        let parsed = RegisteredEncodingEnvelopeV2::parse_many(&bytes).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].decoded_uncompressed_length, 64);
    }

    #[test]
    fn registered_encoding_envelope_rejects_malformed_fallback() {
        let mut item = envelope();
        item.fallback_payload_offset = 0;
        item.fallback_payload_length = 16;
        assert!(matches!(
            item.serialize(),
            Err(CoveError::BadCodecExtension)
        ));
    }

    #[test]
    fn registered_encoding_envelope_rejects_bad_checksum() {
        let mut bytes = envelope().serialize().unwrap();
        bytes[8] ^= 1;
        assert!(matches!(
            RegisteredEncodingEnvelopeV2::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        ));
    }
}
