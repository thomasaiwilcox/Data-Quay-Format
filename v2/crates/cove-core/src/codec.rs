//! COVE-CX descriptor, envelope, and registered-page materialization helpers.

use std::collections::BTreeSet;

use crate::{
    checksum,
    constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
    page::ColumnPageIndexEntryV1,
    page_payload::ColumnPagePayloadV1,
    wire, CoveError,
};

pub const ABSENT_REF: u32 = u32::MAX;

pub const FSST_UTF8_CODEC_IDENTITY: (&str, &str, u16, u16) =
    ("org.coveformat.codec", "fsst-utf8", 2, 0);
pub const ALP_FLOAT_CODEC_IDENTITY: (&str, &str, u16, u16) =
    ("org.coveformat.codec", "alp-float", 2, 0);
pub const FASTLANES_INTEGER_CODEC_IDENTITY: (&str, &str, u16, u16) =
    ("org.coveformat.codec", "fastlanes-integer", 2, 0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalPage {
    pub values: Vec<Option<Vec<u8>>>,
}

pub trait RegisteredCodecResolver {
    fn decode_registered_page(
        &self,
        descriptor: &CodecExtensionDescriptorV2,
        envelope: &RegisteredEncodingEnvelopeV2,
        encoded_payload: &[u8],
    ) -> Result<LogicalPage, CoveError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoRegisteredCodecResolver;

impl RegisteredCodecResolver for NoRegisteredCodecResolver {
    fn decode_registered_page(
        &self,
        _descriptor: &CodecExtensionDescriptorV2,
        _envelope: &RegisteredEncodingEnvelopeV2,
        _encoded_payload: &[u8],
    ) -> Result<LogicalPage, CoveError> {
        Err(CoveError::CodecUnsupported)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StableRegisteredCodecResolver;

impl RegisteredCodecResolver for StableRegisteredCodecResolver {
    fn decode_registered_page(
        &self,
        descriptor: &CodecExtensionDescriptorV2,
        envelope: &RegisteredEncodingEnvelopeV2,
        encoded_payload: &[u8],
    ) -> Result<LogicalPage, CoveError> {
        let codec =
            stable_codec_for_descriptor(descriptor, envelope).ok_or(CoveError::CodecUnsupported)?;
        codec.decode(encoded_payload)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StableRegisteredCodec {
    FsstUtf8,
    AlpFloat,
    FastLanesInteger,
}

impl StableRegisteredCodec {
    fn descriptor_identity(self) -> (&'static str, &'static str, u16, u16) {
        match self {
            Self::FsstUtf8 => FSST_UTF8_CODEC_IDENTITY,
            Self::AlpFloat => ALP_FLOAT_CODEC_IDENTITY,
            Self::FastLanesInteger => FASTLANES_INTEGER_CODEC_IDENTITY,
        }
    }

    fn decode(self, payload: &[u8]) -> Result<LogicalPage, CoveError> {
        let expected_magic = match self {
            Self::FsstUtf8 => b"CFS2",
            Self::AlpFloat => b"CAF2",
            Self::FastLanesInteger => b"CFI2",
        };
        let page = decode_row_bytes(expected_magic, payload)?;
        if self == Self::FsstUtf8 {
            for value in page.values.iter().flatten() {
                std::str::from_utf8(value).map_err(|_| CoveError::BadCodecExtension)?;
            }
        }
        Ok(page)
    }
}

fn stable_codec_for_descriptor(
    descriptor: &CodecExtensionDescriptorV2,
    envelope: &RegisteredEncodingEnvelopeV2,
) -> Option<StableRegisteredCodec> {
    [
        StableRegisteredCodec::FsstUtf8,
        StableRegisteredCodec::AlpFloat,
        StableRegisteredCodec::FastLanesInteger,
    ]
    .into_iter()
    .find(|codec| {
        let (namespace, name, major, minor) = codec.descriptor_identity();
        descriptor.codec_id == envelope.codec_id
            && descriptor.version_major == envelope.codec_version_major
            && descriptor.version_minor == envelope.codec_version_minor
            && descriptor.namespace == namespace
            && descriptor.name == name
            && descriptor.version_major == major
            && descriptor.version_minor == minor
            && descriptor.specification_status == CodecSpecificationStatusV2::StableRegistered
            && descriptor.spec_digest == stable_spec_digest(*codec)
    })
}

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

        let requirement =
            CodecRequirementV2::from_u8(requirement_raw).ok_or(CoveError::BadCodecExtension)?;
        let fallback_policy = CodecFallbackPolicyV2::from_u8(fallback_policy_raw)
            .ok_or(CoveError::BadCodecExtension)?;
        let specification_status =
            CodecSpecificationStatusV2::from_u8(status_raw).ok_or(CoveError::BadCodecExtension)?;

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
        if self.flags != 0 {
            return Err(CoveError::BadCodecExtension);
        }
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

#[derive(Debug, Clone)]
pub struct RegisteredPageMaterialization {
    pub payload: ColumnPagePayloadV1,
    pub used_fallback: bool,
}

pub fn materialize_registered_page_payload<R: RegisteredCodecResolver + ?Sized>(
    payload: &ColumnPagePayloadV1,
    page: &ColumnPageIndexEntryV1,
    logical_type: CoveLogicalType,
    physical_kind: CovePhysicalKind,
    descriptors: &[CodecExtensionDescriptorV2],
    resolver: &R,
    dictionary_len: Option<u32>,
) -> Result<Option<RegisteredPageMaterialization>, CoveError> {
    let root = payload.root_node()?;
    if root.encoding_kind != CoveEncodingKind::RegisteredEncoding {
        return Ok(None);
    }
    if root.logical_type != logical_type
        || root.physical_kind != physical_kind
        || root.logical_len != page.row_count
        || page.encoding_root != CoveEncodingKind::RegisteredEncoding as u32
    {
        return Err(CoveError::PageCorrupt);
    }
    let envelope = registered_envelope_from_root(payload)?;
    validate_envelope_against_page(&envelope, page)?;
    let encoded_payload = envelope_range(
        payload,
        envelope.encoded_payload_offset,
        envelope.encoded_payload_length,
    )?;
    let fallback_payload = if envelope.fallback_payload_length == 0 {
        None
    } else {
        Some(parse_fallback_payload(
            payload,
            &envelope,
            page,
            logical_type,
            physical_kind,
        )?)
    };
    let fallback_logical = if let Some(fallback) = fallback_payload.as_ref() {
        Some(column_payload_to_logical_page(
            fallback,
            page,
            dictionary_len,
        )?)
    } else {
        None
    };
    let descriptor = descriptors.iter().find(|descriptor| {
        descriptor.codec_id == envelope.codec_id
            && descriptor.version_major == envelope.codec_version_major
            && descriptor.version_minor == envelope.codec_version_minor
    });

    if let Some(descriptor) = descriptor {
        match resolver.decode_registered_page(descriptor, &envelope, encoded_payload) {
            Ok(decoded) => {
                let materialized = logical_page_to_column_payload(
                    &decoded,
                    logical_type,
                    physical_kind,
                    dictionary_len,
                )?;
                if let Some(fallback_logical) = fallback_logical.as_ref() {
                    if &decoded != fallback_logical {
                        return Err(CoveError::BadCodecExtension);
                    }
                }
                return Ok(Some(RegisteredPageMaterialization {
                    payload: materialized,
                    used_fallback: false,
                }));
            }
            Err(CoveError::CodecUnsupported) => {}
            Err(_) => return Err(CoveError::BadCodecExtension),
        }
    }

    if let Some(fallback) = fallback_payload {
        return Ok(Some(RegisteredPageMaterialization {
            payload: fallback,
            used_fallback: true,
        }));
    }
    Err(CoveError::CodecUnsupported)
}

pub fn registered_page_has_fallback(payload: &[u8]) -> Result<bool, CoveError> {
    let payload = ColumnPagePayloadV1::parse(payload)?;
    let root = payload.root_node()?;
    if root.encoding_kind != CoveEncodingKind::RegisteredEncoding {
        return Ok(false);
    }
    Ok(registered_envelope_from_root(&payload)?.fallback_payload_length != 0)
}

pub fn registered_envelope_from_root(
    payload: &ColumnPagePayloadV1,
) -> Result<RegisteredEncodingEnvelopeV2, CoveError> {
    let root = payload.root_node()?;
    if root.params_length as usize != RegisteredEncodingEnvelopeV2::LEN {
        return Err(CoveError::BadCodecExtension);
    }
    let params = wire::read_range_checked(
        payload.data.as_slice(),
        root.params_offset as usize,
        root.params_length as usize,
    )?;
    let envelope = RegisteredEncodingEnvelopeV2::parse(params)?;
    if envelope.params_offset != root.params_offset || envelope.params_length != root.params_length
    {
        return Err(CoveError::BadCodecExtension);
    }
    Ok(envelope)
}

fn validate_envelope_against_page(
    envelope: &RegisteredEncodingEnvelopeV2,
    page: &ColumnPageIndexEntryV1,
) -> Result<(), CoveError> {
    if envelope.logical_len != page.row_count || envelope.non_null_count != page.non_null_count {
        return Err(CoveError::BadCodecExtension);
    }
    Ok(())
}

fn parse_fallback_payload(
    registered_payload: &ColumnPagePayloadV1,
    envelope: &RegisteredEncodingEnvelopeV2,
    page: &ColumnPageIndexEntryV1,
    logical_type: CoveLogicalType,
    physical_kind: CovePhysicalKind,
) -> Result<ColumnPagePayloadV1, CoveError> {
    let fallback = envelope_range(
        registered_payload,
        envelope.fallback_payload_offset,
        envelope.fallback_payload_length,
    )?;
    let fallback = ColumnPagePayloadV1::parse(fallback)?;
    let fallback_root = fallback.root_node()?;
    if fallback_root.encoding_kind == CoveEncodingKind::RegisteredEncoding
        || fallback_root.logical_type != logical_type
        || fallback_root.physical_kind != physical_kind
        || fallback_root.logical_len != page.row_count
    {
        return Err(CoveError::BadCodecExtension);
    }
    Ok(fallback)
}

fn envelope_range(
    payload: &ColumnPagePayloadV1,
    offset: u64,
    length: u64,
) -> Result<&[u8], CoveError> {
    let start = usize::try_from(offset).map_err(|_| CoveError::OffsetRange)?;
    let len = usize::try_from(length).map_err(|_| CoveError::OffsetRange)?;
    wire::read_range_checked(payload.data.as_slice(), start, len)
}

pub fn logical_page_to_column_payload(
    page: &LogicalPage,
    logical_type: CoveLogicalType,
    physical_kind: CovePhysicalKind,
    dictionary_len: Option<u32>,
) -> Result<ColumnPagePayloadV1, CoveError> {
    let row_count = u32::try_from(page.values.len()).map_err(|_| CoveError::ArithOverflow)?;
    let mut null_bitmap = vec![0u8; (page.values.len() + 7) / 8];
    let mut null_count = 0usize;
    let mut values = Vec::new();
    for (index, value) in page.values.iter().enumerate() {
        let Some(bytes) = value else {
            null_count += 1;
            null_bitmap[index >> 3] |= 1 << (index & 7);
            append_null_placeholder(logical_type, physical_kind, &mut values)?;
            continue;
        };
        append_non_null_value(
            logical_type,
            physical_kind,
            bytes,
            &mut values,
            dictionary_len,
        )?;
    }
    let bytes = ColumnPagePayloadV1::build_single_node(
        row_count,
        fallback_encoding_kind(physical_kind),
        logical_type,
        physical_kind,
        (null_count != 0).then_some(null_bitmap),
        values,
    )?;
    ColumnPagePayloadV1::parse(&bytes)
}

fn column_payload_to_logical_page(
    payload: &ColumnPagePayloadV1,
    page: &ColumnPageIndexEntryV1,
    dictionary_len: Option<u32>,
) -> Result<LogicalPage, CoveError> {
    let root = payload.root_node()?;
    let null_bitmap = payload.buffer_bytes(crate::page_payload::PageBufferKind::NullBitmap)?;
    let values = payload
        .buffer_bytes(crate::page_payload::PageBufferKind::Values)?
        .unwrap_or(&[]);
    let array = crate::array::EncodedArray::new(
        root.logical_type,
        root.physical_kind,
        page.row_count as u64,
        root.encoding_kind,
        null_bitmap
            .map(|bitmap| crate::validity::ValidityBitmap::new(bitmap, page.row_count as u64)),
        values,
        None,
    );
    let decoded = array.decode_all_rows()?;
    let mut out = Vec::with_capacity(decoded.len());
    for value in decoded {
        out.push(match value {
            crate::array::CoveArrayValue::Null => None,
            crate::array::CoveArrayValue::Bytes(bytes) => Some(bytes.to_vec()),
            crate::array::CoveArrayValue::OwnedBytes(bytes) => Some(bytes),
            crate::array::CoveArrayValue::Varint(value) => Some(value.to_le_bytes().to_vec()),
            crate::array::CoveArrayValue::Int64(value) => Some(value.to_le_bytes().to_vec()),
            crate::array::CoveArrayValue::FileCode(value) => {
                if let Some(dictionary_len) = dictionary_len {
                    if value >= dictionary_len {
                        return Err(CoveError::BadFileCode);
                    }
                }
                Some(value.to_le_bytes().to_vec())
            }
            crate::array::CoveArrayValue::NumCode(value) => Some(value.to_le_bytes().to_vec()),
            crate::array::CoveArrayValue::Boolean(value) => Some(vec![u8::from(value)]),
            crate::array::CoveArrayValue::ValidityBit(value) => Some(vec![u8::from(value)]),
            crate::array::CoveArrayValue::DictValue(_) => return Err(CoveError::BadCodecExtension),
        });
    }
    Ok(LogicalPage { values: out })
}

fn append_non_null_value(
    logical_type: CoveLogicalType,
    physical_kind: CovePhysicalKind,
    bytes: &[u8],
    out: &mut Vec<u8>,
    dictionary_len: Option<u32>,
) -> Result<(), CoveError> {
    match physical_kind {
        CovePhysicalKind::Boolean => {
            if bytes.len() != 1 || bytes[0] > 1 {
                return Err(CoveError::BadCodecExtension);
            }
            out.push(bytes[0]);
        }
        CovePhysicalKind::NumCode => {
            if bytes.len() != 8 {
                return Err(CoveError::BadCodecExtension);
            }
            out.extend_from_slice(bytes);
        }
        CovePhysicalKind::FileCode => {
            if bytes.len() != 4 {
                return Err(CoveError::BadCodecExtension);
            }
            if let Some(dictionary_len) = dictionary_len {
                let code = u32::from_le_bytes(bytes.try_into().unwrap());
                if code >= dictionary_len {
                    return Err(CoveError::BadFileCode);
                }
            }
            out.extend_from_slice(bytes);
        }
        CovePhysicalKind::FixedBytes => {
            let expected = fixed_width_for_logical_type(logical_type)?;
            if bytes.len() != expected {
                return Err(CoveError::BadCodecExtension);
            }
            out.extend_from_slice(bytes);
        }
        CovePhysicalKind::VarBytes => {
            let len = u32::try_from(bytes.len()).map_err(|_| CoveError::ArithOverflow)?;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(bytes);
        }
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            return Err(CoveError::BadCodecExtension);
        }
    }
    Ok(())
}

fn append_null_placeholder(
    logical_type: CoveLogicalType,
    physical_kind: CovePhysicalKind,
    out: &mut Vec<u8>,
) -> Result<(), CoveError> {
    match physical_kind {
        CovePhysicalKind::Boolean => out.push(0),
        CovePhysicalKind::NumCode => out.extend_from_slice(&0u64.to_le_bytes()),
        CovePhysicalKind::FileCode => out.extend_from_slice(&0u32.to_le_bytes()),
        CovePhysicalKind::FixedBytes => {
            out.resize(out.len() + fixed_width_for_logical_type(logical_type)?, 0)
        }
        CovePhysicalKind::VarBytes => out.extend_from_slice(&0u32.to_le_bytes()),
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            return Err(CoveError::BadCodecExtension);
        }
    }
    Ok(())
}

fn fixed_width_for_logical_type(logical_type: CoveLogicalType) -> Result<usize, CoveError> {
    match logical_type {
        CoveLogicalType::Bool => Ok(1),
        CoveLogicalType::Int8 | CoveLogicalType::UInt8 => Ok(1),
        CoveLogicalType::Int16 | CoveLogicalType::UInt16 => Ok(2),
        CoveLogicalType::Int32
        | CoveLogicalType::UInt32
        | CoveLogicalType::Float32
        | CoveLogicalType::DateDays => Ok(4),
        CoveLogicalType::Int64
        | CoveLogicalType::UInt64
        | CoveLogicalType::Float64
        | CoveLogicalType::Decimal64
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos => Ok(8),
        CoveLogicalType::Decimal128 | CoveLogicalType::Uuid => Ok(16),
        _ => Err(CoveError::BadCodecExtension),
    }
}

fn fallback_encoding_kind(physical_kind: CovePhysicalKind) -> CoveEncodingKind {
    match physical_kind {
        CovePhysicalKind::FileCode => CoveEncodingKind::FileCode,
        CovePhysicalKind::NumCode => CoveEncodingKind::NumCode,
        CovePhysicalKind::Boolean | CovePhysicalKind::FixedBytes => CoveEncodingKind::PlainFixed,
        CovePhysicalKind::VarBytes => CoveEncodingKind::VarBytes,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            CoveEncodingKind::Canonical
        }
    }
}

fn stable_spec_digest(codec: StableRegisteredCodec) -> Vec<u8> {
    match codec {
        StableRegisteredCodec::FsstUtf8 => b"COVE-FSST-UTF8-V2-SPEC-DIGEST".to_vec(),
        StableRegisteredCodec::AlpFloat => b"COVE-ALP-FLOAT-V2-SPEC-DIGEST".to_vec(),
        StableRegisteredCodec::FastLanesInteger => b"COVE-FASTLANES-I-V2-SPEC-DIGEST".to_vec(),
    }
}

fn parse_utf8(bytes: &[u8], field: &str) -> Result<String, CoveError> {
    std::str::from_utf8(bytes)
        .map(|value| value.to_string())
        .map_err(|_| CoveError::BadSection(format!("{field} is not valid UTF-8")))
}

fn checked_end(offset: u64, length: u64) -> Result<u64, CoveError> {
    offset.checked_add(length).ok_or(CoveError::ArithOverflow)
}

fn decode_row_bytes(expected_magic: &[u8; 4], bytes: &[u8]) -> Result<LogicalPage, CoveError> {
    if bytes.len() < 16 || &bytes[0..4] != expected_magic {
        return Err(CoveError::BadCodecExtension);
    }
    let row_count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let null_bitmap_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let offsets_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if null_bitmap_len != (row_count + 7) / 8 || offsets_len != (row_count + 1) * 4 {
        return Err(CoveError::BadCodecExtension);
    }
    let bitmap_start = 16usize;
    let offsets_start = bitmap_start
        .checked_add(null_bitmap_len)
        .ok_or(CoveError::ArithOverflow)?;
    let payload_start = offsets_start
        .checked_add(offsets_len)
        .ok_or(CoveError::ArithOverflow)?;
    if payload_start > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let null_bitmap = &bytes[bitmap_start..offsets_start];
    let mut offsets = Vec::with_capacity(row_count + 1);
    for chunk in bytes[offsets_start..payload_start].chunks_exact(4) {
        offsets.push(u32::from_le_bytes(chunk.try_into().unwrap()) as usize);
    }
    if offsets.first() != Some(&0) {
        return Err(CoveError::BadCodecExtension);
    }
    let payload = &bytes[payload_start..];
    let mut values = Vec::with_capacity(row_count);
    for index in 0..row_count {
        let start = offsets[index];
        let end = offsets[index + 1];
        if start > end || end > payload.len() {
            return Err(CoveError::BadCodecExtension);
        }
        let is_null = (null_bitmap[index >> 3] & (1 << (index & 7))) != 0;
        values.push((!is_null).then(|| payload[start..end].to_vec()));
    }
    if row_count % 8 != 0 && !null_bitmap.is_empty() {
        let unused_mask = !((1u8 << (row_count % 8)) - 1);
        if null_bitmap[null_bitmap.len() - 1] & unused_mask != 0 {
            return Err(CoveError::BadCodecExtension);
        }
    }
    Ok(LogicalPage { values })
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
    use crate::page_payload::{
        COLUMN_PAGE_PAYLOAD_HEADER_LEN, COVE_ENCODING_NODE_LEN, PAGE_BUFFER_DESCRIPTOR_LEN,
    };

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

    fn stable_fsst_descriptor() -> CodecExtensionDescriptorV2 {
        CodecExtensionDescriptorV2 {
            codec_id: 1,
            namespace: FSST_UTF8_CODEC_IDENTITY.0.into(),
            name: FSST_UTF8_CODEC_IDENTITY.1.into(),
            version_major: FSST_UTF8_CODEC_IDENTITY.2,
            version_minor: FSST_UTF8_CODEC_IDENTITY.3,
            codec_family: 1,
            logical_type_mask: u64::MAX,
            physical_kind_mask: u64::MAX,
            requirement: CodecRequirementV2::OptionalWithFallback,
            fallback_policy: CodecFallbackPolicyV2::CoreEncodingPayloadPresent,
            parameter_schema_kind: 0,
            flags: 0,
            specification_status: CodecSpecificationStatusV2::StableRegistered,
            required_feature_bit: 0,
            optional_feature_bit: 0,
            spec_digest_algorithm: 1,
            spec_digest: stable_spec_digest(StableRegisteredCodec::FsstUtf8),
            conformance_vector_ref: ABSENT_REF,
            fallback_ref: 0,
            private_payload_ref: ABSENT_REF,
            checksum: 0,
        }
    }

    fn utf8_logical_page() -> LogicalPage {
        LogicalPage {
            values: vec![Some(b"alpha".to_vec()), None, Some(b"omega".to_vec())],
        }
    }

    fn fallback_payload(page: &LogicalPage) -> Vec<u8> {
        logical_page_to_column_payload(
            page,
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            None,
        )
        .unwrap()
        .serialize()
        .unwrap()
    }

    fn registered_payload(logical: &LogicalPage, fallback: Option<Vec<u8>>) -> ColumnPagePayloadV1 {
        let encoded = encode_registered_row_bytes(b"CFS2", logical);
        let bytes = ColumnPagePayloadV1::build_registered_single_node(
            3,
            2,
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            1,
            2,
            0,
            encoded,
            fallback,
        )
        .unwrap();
        ColumnPagePayloadV1::parse(&bytes).unwrap()
    }

    fn encode_registered_row_bytes(magic: &[u8; 4], page: &LogicalPage) -> Vec<u8> {
        let mut value_bytes = Vec::new();
        let mut offsets = Vec::with_capacity(page.values.len() + 1);
        offsets.push(0u32);
        for value in &page.values {
            if let Some(value) = value {
                let next = offsets.last().copied().unwrap() + value.len() as u32;
                offsets.push(next);
                value_bytes.extend_from_slice(value);
            } else {
                offsets.push(*offsets.last().unwrap());
            }
        }
        let mut null_bitmap = vec![0u8; page.values.len().div_ceil(8)];
        for (index, value) in page.values.iter().enumerate() {
            if value.is_none() {
                null_bitmap[index / 8] |= 1u8 << (index % 8);
            }
        }
        let offsets_len = offsets.len() * 4;
        let mut out = Vec::new();
        out.extend_from_slice(magic);
        out.extend_from_slice(&(page.values.len() as u32).to_le_bytes());
        out.extend_from_slice(&(null_bitmap.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offsets_len as u32).to_le_bytes());
        out.extend_from_slice(&null_bitmap);
        for offset in offsets {
            out.extend_from_slice(&offset.to_le_bytes());
        }
        out.extend_from_slice(&value_bytes);
        out
    }

    fn registered_page() -> ColumnPageIndexEntryV1 {
        ColumnPageIndexEntryV1 {
            column_id: 7,
            morsel_id: 0,
            row_count: 3,
            non_null_count: 2,
            null_count: 1,
            encoding_root: CoveEncodingKind::RegisteredEncoding as u32,
            page_offset: 0,
            page_length: 0,
            uncompressed_length: 0,
            stats_ref: 0,
            flags: 0,
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
    fn registered_encoding_envelope_rejects_malformed_fallback() {
        let mut item = RegisteredEncodingEnvelopeV2 {
            codec_id: 1,
            codec_version_major: 1,
            codec_version_minor: 0,
            logical_len: 4,
            non_null_count: 3,
            params_offset: 72,
            params_length: 8,
            encoded_payload_offset: 80,
            encoded_payload_length: 32,
            fallback_payload_offset: 0,
            fallback_payload_length: 16,
            decoded_uncompressed_length: 64,
            flags: 0,
            checksum: 0,
        };
        assert!(matches!(
            item.serialize(),
            Err(CoveError::BadCodecExtension)
        ));
        item.fallback_payload_length = 0;
        assert!(item.serialize().is_ok());
    }

    #[test]
    fn registered_page_uses_valid_fallback_when_codec_is_unsupported() {
        let logical = utf8_logical_page();
        let payload = registered_payload(&logical, Some(fallback_payload(&logical)));
        let materialized = materialize_registered_page_payload(
            &payload,
            &registered_page(),
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            &[],
            &NoRegisteredCodecResolver,
            None,
        )
        .unwrap()
        .unwrap();
        assert!(materialized.used_fallback);
        assert_eq!(
            materialized.payload.root_node().unwrap().encoding_kind,
            CoveEncodingKind::VarBytes
        );
    }

    #[test]
    fn registered_page_without_fallback_returns_codec_unsupported() {
        let logical = utf8_logical_page();
        let payload = registered_payload(&logical, None);
        let err = materialize_registered_page_payload(
            &payload,
            &registered_page(),
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            &[],
            &NoRegisteredCodecResolver,
            None,
        )
        .unwrap_err();
        assert_eq!(err, CoveError::CodecUnsupported);
    }

    #[test]
    fn registered_page_rejects_supported_decode_fallback_mismatch() {
        let logical = utf8_logical_page();
        let fallback = LogicalPage {
            values: vec![Some(b"alpha".to_vec()), None, Some(b"wrong".to_vec())],
        };
        let payload = registered_payload(&logical, Some(fallback_payload(&fallback)));
        let err = materialize_registered_page_payload(
            &payload,
            &registered_page(),
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            &[stable_fsst_descriptor()],
            &StableRegisteredCodecResolver,
            None,
        )
        .unwrap_err();
        assert_eq!(err, CoveError::BadCodecExtension);
    }

    #[test]
    fn registered_page_validates_embedded_envelope_checksum() {
        let logical = utf8_logical_page();
        let mut bytes = registered_payload(&logical, Some(fallback_payload(&logical)))
            .serialize()
            .unwrap();
        let descriptor_offset = COLUMN_PAGE_PAYLOAD_HEADER_LEN + COVE_ENCODING_NODE_LEN;
        let other_offset = u64::from_le_bytes(
            bytes[descriptor_offset + 8..descriptor_offset + 16]
                .try_into()
                .unwrap(),
        ) as usize;
        let other_length = u64::from_le_bytes(
            bytes[descriptor_offset + 16..descriptor_offset + 24]
                .try_into()
                .unwrap(),
        ) as usize;
        bytes[other_offset] ^= 1;
        let checksum = checksum::crc32c(&bytes[other_offset..other_offset + other_length]);
        bytes[descriptor_offset + 24..descriptor_offset + 28]
            .copy_from_slice(&checksum.to_le_bytes());
        let payload = ColumnPagePayloadV1::parse(&bytes).unwrap();
        assert_eq!(
            registered_envelope_from_root(&payload).unwrap_err(),
            CoveError::ChecksumMismatch
        );
        assert_eq!(
            descriptor_offset + PAGE_BUFFER_DESCRIPTOR_LEN,
            payload.header.buffers_offset as usize
        );
    }
}
