//! Spec §38–§43 — QF-E generic engine profile.
//!
//! QF-E metadata maps QF physical values into engine-local execution codes
//! without changing QF logical semantics. These parsers deliberately validate
//! only the on-disk descriptor contracts; whether an unknown or corrupt
//! engine profile is fatal is operation-scoped and decided by the reader.

use crate::{checksum, QfError};

pub const EXECUTION_CODE_DESCRIPTOR_LEN: usize = 28;
pub const ENGINE_MOUNT_POLICY_LEN: usize = 32;

// ── Engine profile registry (§39) ────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineProfileRegistry {
    pub flags: u32,
    pub profiles: Vec<EngineProfileEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineProfileEntryV1 {
    pub profile_id: u32,
    pub namespace: String,
    pub profile_name: String,
    pub version_major: u16,
    pub version_minor: u16,
    pub required_features: u64,
    pub optional_features: u64,
    pub execution_descriptor_ref: u32,
    pub mount_policy_ref: u32,
    pub private_payload_ref: u32,
    pub checksum: u32,
}

impl EngineProfileRegistry {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 8 {
            return Err(QfError::BufferTooShort);
        }
        let profile_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let mut pos = 8usize;
        let mut profiles = Vec::with_capacity(profile_count);
        for _ in 0..profile_count {
            let (entry, used) = EngineProfileEntryV1::parse(&bytes[pos..])?;
            pos = pos.checked_add(used).ok_or(QfError::ArithOverflow)?;
            profiles.push(entry);
        }
        let registry = Self { flags, profiles };
        registry.validate()?;
        Ok(registry)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        let count = u32::try_from(self.profiles.len()).map_err(|_| QfError::BadEngineProfile)?;
        let mut out = Vec::new();
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for profile in &self.profiles {
            out.extend_from_slice(&profile.serialize()?);
        }
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), QfError> {
        let mut namespaces = std::collections::HashSet::new();
        for profile in &self.profiles {
            if !namespaces.insert(profile.namespace.as_str()) {
                return Err(QfError::BadEngineProfile);
            }
        }
        Ok(())
    }
}

impl EngineProfileEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), QfError> {
        if bytes.len() < 4 + 2 {
            return Err(QfError::BufferTooShort);
        }
        let profile_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let namespace = read_str(bytes, &mut pos, "engine profile namespace")?;
        let profile_name = read_str(bytes, &mut pos, "engine profile name")?;
        if bytes.len() < pos + 2 + 2 + 8 + 8 + 4 + 4 + 4 + 4 {
            return Err(QfError::BufferTooShort);
        }
        let version_major = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let version_minor = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let required_features = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let optional_features = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let execution_descriptor_ref = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let mount_policy_ref = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let private_payload_ref = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let checksum_pos = pos;
        let checksum_field = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;

        let mut for_crc = bytes[..pos].to_vec();
        for_crc[checksum_pos..checksum_pos + 4].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }

        Ok((
            Self {
                profile_id,
                namespace,
                profile_name,
                version_major,
                version_minor,
                required_features,
                optional_features,
                execution_descriptor_ref,
                mount_policy_ref,
                private_payload_ref,
                checksum: checksum_field,
            },
            pos,
        ))
    }

    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.profile_id.to_le_bytes());
        write_str(&mut out, &self.namespace, "engine profile namespace")?;
        write_str(&mut out, &self.profile_name, "engine profile name")?;
        out.extend_from_slice(&self.version_major.to_le_bytes());
        out.extend_from_slice(&self.version_minor.to_le_bytes());
        out.extend_from_slice(&self.required_features.to_le_bytes());
        out.extend_from_slice(&self.optional_features.to_le_bytes());
        out.extend_from_slice(&self.execution_descriptor_ref.to_le_bytes());
        out.extend_from_slice(&self.mount_policy_ref.to_le_bytes());
        out.extend_from_slice(&self.private_payload_ref.to_le_bytes());
        let checksum_pos = out.len();
        out.extend_from_slice(&0u32.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[checksum_pos..checksum_pos + 4].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}

// ── ExecutionCode Descriptor (§40) ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExecutionCodeKind {
    UnsignedInteger = 0,
    SignedInteger = 1,
    OpaqueBytes = 2,
    DictionaryKey = 3,
    EnginePrivate = 255,
}

impl ExecutionCodeKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::UnsignedInteger),
            1 => Some(Self::SignedInteger),
            2 => Some(Self::OpaqueBytes),
            3 => Some(Self::DictionaryKey),
            255 => Some(Self::EnginePrivate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExecutionCodeLifetime {
    Query = 0,
    Scan = 1,
    Session = 2,
    Mount = 3,
    LeaseEpoch = 4,
    PersistentEngineLocal = 5,
}

impl ExecutionCodeLifetime {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Query),
            1 => Some(Self::Scan),
            2 => Some(Self::Session),
            3 => Some(Self::Mount),
            4 => Some(Self::LeaseEpoch),
            5 => Some(Self::PersistentEngineLocal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExecutionCodeComparisonScope {
    NotComparable = 0,
    File = 1,
    Dataset = 2,
    Catalog = 3,
    Scope = 4,
    EngineGlobal = 5,
}

impl ExecutionCodeComparisonScope {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::NotComparable),
            1 => Some(Self::File),
            2 => Some(Self::Dataset),
            3 => Some(Self::Catalog),
            4 => Some(Self::Scope),
            5 => Some(Self::EngineGlobal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExecutionCodeCanonicality {
    Transient = 0,
    Leased = 1,
    CanonicalWithinScope = 2,
    EnginePrivate = 255,
}

impl ExecutionCodeCanonicality {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Transient),
            1 => Some(Self::Leased),
            2 => Some(Self::CanonicalWithinScope),
            255 => Some(Self::EnginePrivate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NullCodePolicy {
    NoNullCode = 0,
    EngineDefinesNullCode = 1,
    NullBitmapOnly = 2,
}

impl NullCodePolicy {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::NoNullCode),
            1 => Some(Self::EngineDefinesNullCode),
            2 => Some(Self::NullBitmapOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionCodeDescriptorV1 {
    pub descriptor_id: u32,
    pub code_kind: ExecutionCodeKind,
    pub code_width_bits: u16,
    pub byte_order: u8,
    pub lifetime: ExecutionCodeLifetime,
    pub comparison_scope: ExecutionCodeComparisonScope,
    pub canonicality: ExecutionCodeCanonicality,
    pub null_code_policy: NullCodePolicy,
    pub flags: u32,
    pub scope_ref: u32,
    pub code_space_ref: u32,
    pub checksum: u32,
}

impl ExecutionCodeDescriptorV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < EXECUTION_CODE_DESCRIPTOR_LEN {
            return Err(QfError::BufferTooShort);
        }
        let descriptor_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let code_kind = ExecutionCodeKind::from_u8(bytes[4]).ok_or(QfError::BadEngineProfile)?;
        let code_width_bits = u16::from_le_bytes(bytes[5..7].try_into().unwrap());
        let byte_order = bytes[7];
        let lifetime = ExecutionCodeLifetime::from_u8(bytes[8]).ok_or(QfError::BadEngineProfile)?;
        let comparison_scope =
            ExecutionCodeComparisonScope::from_u8(bytes[9]).ok_or(QfError::BadEngineProfile)?;
        let canonicality =
            ExecutionCodeCanonicality::from_u8(bytes[10]).ok_or(QfError::BadEngineProfile)?;
        let null_code_policy =
            NullCodePolicy::from_u8(bytes[11]).ok_or(QfError::BadEngineProfile)?;
        let flags = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let scope_ref = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let code_space_ref = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let mut for_crc = [0u8; EXECUTION_CODE_DESCRIPTOR_LEN];
        for_crc.copy_from_slice(&bytes[..EXECUTION_CODE_DESCRIPTOR_LEN]);
        for_crc[24..28].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }
        Ok(Self {
            descriptor_id,
            code_kind,
            code_width_bits,
            byte_order,
            lifetime,
            comparison_scope,
            canonicality,
            null_code_policy,
            flags,
            scope_ref,
            code_space_ref,
            checksum: checksum_field,
        })
    }

    pub fn serialize(&self) -> [u8; EXECUTION_CODE_DESCRIPTOR_LEN] {
        let mut buf = [0u8; EXECUTION_CODE_DESCRIPTOR_LEN];
        buf[0..4].copy_from_slice(&self.descriptor_id.to_le_bytes());
        buf[4] = self.code_kind as u8;
        buf[5..7].copy_from_slice(&self.code_width_bits.to_le_bytes());
        buf[7] = self.byte_order;
        buf[8] = self.lifetime as u8;
        buf[9] = self.comparison_scope as u8;
        buf[10] = self.canonicality as u8;
        buf[11] = self.null_code_policy as u8;
        buf[12..16].copy_from_slice(&self.flags.to_le_bytes());
        buf[16..20].copy_from_slice(&self.scope_ref.to_le_bytes());
        buf[20..24].copy_from_slice(&self.code_space_ref.to_le_bytes());
        let crc = checksum::crc32c(&buf);
        buf[24..28].copy_from_slice(&crc.to_le_bytes());
        buf
    }
}

// ── Execution scope and code-space descriptors (§41–§42) ────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ExecutionScopeKind {
    None = 0,
    Tenant = 1,
    Account = 2,
    Organisation = 3,
    Workspace = 4,
    Catalog = 5,
    Dataset = 6,
    EngineSpecific = 255,
}

impl ExecutionScopeKind {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Tenant),
            2 => Some(Self::Account),
            3 => Some(Self::Organisation),
            4 => Some(Self::Workspace),
            5 => Some(Self::Catalog),
            6 => Some(Self::Dataset),
            255 => Some(Self::EngineSpecific),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionScopeDescriptorV1 {
    pub scope_id: u32,
    pub scope_kind: ExecutionScopeKind,
    pub flags: u16,
    pub stable_id: Vec<u8>,
    pub display_name: String,
    pub private_payload_ref: u32,
}

impl ExecutionScopeDescriptorV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 + 2 + 2 + 2 {
            return Err(QfError::BufferTooShort);
        }
        let scope_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let raw_scope_kind = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        let scope_kind =
            ExecutionScopeKind::from_u16(raw_scope_kind).ok_or(QfError::BadEngineProfile)?;
        let flags = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        let mut pos = 8usize;
        let stable_id = read_bytes(bytes, &mut pos)?;
        let display_name = read_str(bytes, &mut pos, "execution scope display name")?;
        if bytes.len() < pos + 4 {
            return Err(QfError::BufferTooShort);
        }
        let private_payload_ref = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        Ok(Self {
            scope_id,
            scope_kind,
            flags,
            stable_id,
            display_name,
            private_payload_ref,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSpaceDescriptorV1 {
    pub code_space_id: u32,
    pub namespace: String,
    pub stable_id: Vec<u8>,
    pub epoch: u64,
    pub flags: u32,
    pub private_payload_ref: u32,
}

impl CodeSpaceDescriptorV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 + 2 {
            return Err(QfError::BufferTooShort);
        }
        let code_space_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let namespace = read_str(bytes, &mut pos, "code-space namespace")?;
        let stable_id = read_bytes(bytes, &mut pos)?;
        if bytes.len() < pos + 8 + 4 + 4 {
            return Err(QfError::BufferTooShort);
        }
        let epoch = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let flags = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let private_payload_ref = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        Ok(Self {
            code_space_id,
            namespace,
            stable_id,
            epoch,
            flags,
            private_payload_ref,
        })
    }
}

// ── Engine mount policy (§43) ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FileCodeMappingKind {
    DecodeToValue = 0,
    MapToExecutionCode = 1,
    MapToArrowDictionary = 2,
    EnginePrivate = 255,
}

impl FileCodeMappingKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::DecodeToValue),
            1 => Some(Self::MapToExecutionCode),
            2 => Some(Self::MapToArrowDictionary),
            255 => Some(Self::EnginePrivate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MissingValuePolicy {
    Error = 0,
    DecodeValueOnly = 1,
    RequestLeaseOrIntern = 2,
    ReturnUnmapped = 3,
}

impl MissingValuePolicy {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Error),
            1 => Some(Self::DecodeValueOnly),
            2 => Some(Self::RequestLeaseOrIntern),
            3 => Some(Self::ReturnUnmapped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StaleMappingPolicy {
    Rebuild = 0,
    Reject = 1,
    IgnoreIfOptional = 2,
}

impl StaleMappingPolicy {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Rebuild),
            1 => Some(Self::Reject),
            2 => Some(Self::IgnoreIfOptional),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReverseLookupPolicy {
    NotAvailable = 0,
    BuildFromDictionary = 1,
    EngineProvided = 2,
    CachedExternal = 3,
}

impl ReverseLookupPolicy {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::NotAvailable),
            1 => Some(Self::BuildFromDictionary),
            2 => Some(Self::EngineProvided),
            3 => Some(Self::CachedExternal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineMountPolicyV1 {
    pub policy_id: u32,
    pub filecode_mapping_kind: FileCodeMappingKind,
    pub missing_value_policy: MissingValuePolicy,
    pub stale_mapping_policy: StaleMappingPolicy,
    pub reverse_lookup_policy: ReverseLookupPolicy,
    pub flags: u32,
    pub dictionary_digest_ref: u32,
    pub code_space_ref: u32,
    pub cache_key_ref: u32,
    pub private_payload_ref: u32,
    pub checksum: u32,
}

impl EngineMountPolicyV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < ENGINE_MOUNT_POLICY_LEN {
            return Err(QfError::BufferTooShort);
        }
        let policy_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let filecode_mapping_kind =
            FileCodeMappingKind::from_u8(bytes[4]).ok_or(QfError::BadEngineProfile)?;
        let missing_value_policy =
            MissingValuePolicy::from_u8(bytes[5]).ok_or(QfError::BadEngineProfile)?;
        let stale_mapping_policy =
            StaleMappingPolicy::from_u8(bytes[6]).ok_or(QfError::BadEngineProfile)?;
        let reverse_lookup_policy =
            ReverseLookupPolicy::from_u8(bytes[7]).ok_or(QfError::BadEngineProfile)?;
        let flags = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let dictionary_digest_ref = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let code_space_ref = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let cache_key_ref = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let private_payload_ref = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let mut for_crc = [0u8; ENGINE_MOUNT_POLICY_LEN];
        for_crc.copy_from_slice(&bytes[..ENGINE_MOUNT_POLICY_LEN]);
        for_crc[28..32].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }
        Ok(Self {
            policy_id,
            filecode_mapping_kind,
            missing_value_policy,
            stale_mapping_policy,
            reverse_lookup_policy,
            flags,
            dictionary_digest_ref,
            code_space_ref,
            cache_key_ref,
            private_payload_ref,
            checksum: checksum_field,
        })
    }

    pub fn serialize(&self) -> [u8; ENGINE_MOUNT_POLICY_LEN] {
        let mut buf = [0u8; ENGINE_MOUNT_POLICY_LEN];
        buf[0..4].copy_from_slice(&self.policy_id.to_le_bytes());
        buf[4] = self.filecode_mapping_kind as u8;
        buf[5] = self.missing_value_policy as u8;
        buf[6] = self.stale_mapping_policy as u8;
        buf[7] = self.reverse_lookup_policy as u8;
        buf[8..12].copy_from_slice(&self.flags.to_le_bytes());
        buf[12..16].copy_from_slice(&self.dictionary_digest_ref.to_le_bytes());
        buf[16..20].copy_from_slice(&self.code_space_ref.to_le_bytes());
        buf[20..24].copy_from_slice(&self.cache_key_ref.to_le_bytes());
        buf[24..28].copy_from_slice(&self.private_payload_ref.to_le_bytes());
        let crc = checksum::crc32c(&buf);
        buf[28..32].copy_from_slice(&crc.to_le_bytes());
        buf
    }
}

fn read_str(bytes: &[u8], pos: &mut usize, what: &str) -> Result<String, QfError> {
    let raw = read_bytes(bytes, pos)?;
    std::str::from_utf8(&raw)
        .map(|s| s.to_string())
        .map_err(|_| QfError::BadEngineProfile)
        .map_err(|_| QfError::BadSection(format!("{what} is not valid UTF-8")))
}

fn read_bytes(bytes: &[u8], pos: &mut usize) -> Result<Vec<u8>, QfError> {
    if *pos + 2 > bytes.len() {
        return Err(QfError::BufferTooShort);
    }
    let len = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap()) as usize;
    *pos += 2;
    let end = pos.checked_add(len).ok_or(QfError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(QfError::BufferTooShort);
    }
    let out = bytes[*pos..end].to_vec();
    *pos = end;
    Ok(out)
}

fn write_str(out: &mut Vec<u8>, s: &str, what: &str) -> Result<(), QfError> {
    let len = u16::try_from(s.len())
        .map_err(|_| QfError::BadSection(format!("{what} exceeds u16::MAX")))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(namespace: &str) -> EngineProfileEntryV1 {
        EngineProfileEntryV1 {
            profile_id: 1,
            namespace: namespace.into(),
            profile_name: "engine-dictionary-code".into(),
            version_major: 1,
            version_minor: 0,
            required_features: 0,
            optional_features: 0,
            execution_descriptor_ref: 2,
            mount_policy_ref: 3,
            private_payload_ref: 0,
            checksum: 0,
        }
    }

    #[test]
    fn engine_profile_registry_roundtrip() {
        let reg = EngineProfileRegistry {
            flags: 0,
            profiles: vec![profile("org.example")],
        };
        let parsed = EngineProfileRegistry::parse(&reg.serialize().unwrap()).unwrap();
        assert_eq!(parsed.profiles[0].namespace, "org.example");
    }

    #[test]
    fn engine_profile_registry_rejects_duplicate_namespace() {
        let reg = EngineProfileRegistry {
            flags: 0,
            profiles: vec![profile("org.example"), profile("org.example")],
        };
        assert_eq!(
            EngineProfileRegistry::parse(&reg.serialize().unwrap()),
            Err(QfError::BadEngineProfile)
        );
    }

    #[test]
    fn execution_code_descriptor_roundtrip() {
        let desc = ExecutionCodeDescriptorV1 {
            descriptor_id: 7,
            code_kind: ExecutionCodeKind::DictionaryKey,
            code_width_bits: 32,
            byte_order: 0,
            lifetime: ExecutionCodeLifetime::Scan,
            comparison_scope: ExecutionCodeComparisonScope::File,
            canonicality: ExecutionCodeCanonicality::Transient,
            null_code_policy: NullCodePolicy::NullBitmapOnly,
            flags: 0,
            scope_ref: 1,
            code_space_ref: 2,
            checksum: 0,
        };
        let parsed = ExecutionCodeDescriptorV1::parse(&desc.serialize()).unwrap();
        assert_eq!(parsed.descriptor_id, desc.descriptor_id);
        assert_eq!(parsed.code_kind, desc.code_kind);
        assert_eq!(parsed.null_code_policy, desc.null_code_policy);
    }

    #[test]
    fn execution_code_descriptor_rejects_bad_enum() {
        let mut bytes = ExecutionCodeDescriptorV1 {
            descriptor_id: 1,
            code_kind: ExecutionCodeKind::DictionaryKey,
            code_width_bits: 32,
            byte_order: 0,
            lifetime: ExecutionCodeLifetime::Scan,
            comparison_scope: ExecutionCodeComparisonScope::File,
            canonicality: ExecutionCodeCanonicality::Transient,
            null_code_policy: NullCodePolicy::NullBitmapOnly,
            flags: 0,
            scope_ref: 0,
            code_space_ref: 0,
            checksum: 0,
        }
        .serialize();
        bytes[4] = 42;
        assert_eq!(
            ExecutionCodeDescriptorV1::parse(&bytes),
            Err(QfError::BadEngineProfile)
        );
    }

    #[test]
    fn engine_mount_policy_roundtrip() {
        let policy = EngineMountPolicyV1 {
            policy_id: 1,
            filecode_mapping_kind: FileCodeMappingKind::MapToExecutionCode,
            missing_value_policy: MissingValuePolicy::DecodeValueOnly,
            stale_mapping_policy: StaleMappingPolicy::IgnoreIfOptional,
            reverse_lookup_policy: ReverseLookupPolicy::BuildFromDictionary,
            flags: 0,
            dictionary_digest_ref: 0,
            code_space_ref: 2,
            cache_key_ref: 0,
            private_payload_ref: 0,
            checksum: 0,
        };
        let parsed = EngineMountPolicyV1::parse(&policy.serialize()).unwrap();
        assert_eq!(parsed.policy_id, policy.policy_id);
        assert_eq!(parsed.filecode_mapping_kind, policy.filecode_mapping_kind);
        assert_eq!(parsed.stale_mapping_policy, policy.stale_mapping_policy);
    }
}
