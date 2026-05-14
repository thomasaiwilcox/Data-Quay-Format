//! COVE-R runtime compatibility hints for COVE v2.

use std::collections::BTreeSet;

use cove_core::{checksum, CoveError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
pub enum RuntimeHintKindV2 {
    CodecRegistry = 0,
    LayoutRegistry = 1,
    PredicateKernel = 2,
    EngineAdapter = 3,
    FfiSurface = 4,
    LanguageBinding = 5,
    WasmOrExternalKernelPackage = 6,
}

impl RuntimeHintKindV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::CodecRegistry),
            1 => Some(Self::LayoutRegistry),
            2 => Some(Self::PredicateKernel),
            3 => Some(Self::EngineAdapter),
            4 => Some(Self::FfiSurface),
            5 => Some(Self::LanguageBinding),
            6 => Some(Self::WasmOrExternalKernelPackage),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCompatibilityHintV2 {
    pub hint_id: u32,
    pub hint_kind: RuntimeHintKindV2,
    pub required: bool,
    pub flags: u8,
    pub namespace: String,
    pub name: String,
    pub version_major: u16,
    pub version_minor: u16,
    pub payload_ref: u32,
    pub checksum: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuntimeCapability {
    pub kind: RuntimeHintKindV2,
    pub namespace: String,
    pub name: String,
    pub version_major: u16,
    pub version_minor: u16,
}

impl RuntimeCapability {
    pub fn new(
        kind: RuntimeHintKindV2,
        namespace: impl Into<String>,
        name: impl Into<String>,
        version_major: u16,
        version_minor: u16,
    ) -> Self {
        Self {
            kind,
            namespace: namespace.into(),
            name: name.into(),
            version_major,
            version_minor,
        }
    }

    fn matches_hint(&self, hint: &RuntimeCompatibilityHintV2) -> bool {
        self.kind == hint.hint_kind
            && self.namespace == hint.namespace
            && self.name == hint.name
            && self.version_major == hint.version_major
            && self.version_minor == hint.version_minor
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeCapabilityRegistry {
    capabilities: BTreeSet<RuntimeCapability>,
}

impl RuntimeCapabilityRegistry {
    pub fn register(&mut self, capability: RuntimeCapability) -> Result<(), CoveError> {
        if capability.namespace.is_empty() || capability.name.is_empty() {
            return Err(CoveError::RuntimeHintUnsupported);
        }
        self.capabilities.insert(capability);
        Ok(())
    }

    pub fn supports(
        &self,
        kind: RuntimeHintKindV2,
        namespace: &str,
        name: &str,
        version_major: u16,
        version_minor: u16,
    ) -> bool {
        self.capabilities.contains(&RuntimeCapability::new(
            kind,
            namespace,
            name,
            version_major,
            version_minor,
        ))
    }

    pub fn supports_hint(&self, hint: &RuntimeCompatibilityHintV2) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability.matches_hint(hint))
    }

    pub fn capabilities(&self) -> impl Iterator<Item = &RuntimeCapability> {
        self.capabilities.iter()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodecRegistry(RuntimeCapabilityRegistry);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutPlanRegistry(RuntimeCapabilityRegistry);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PredicateKernelRegistry(RuntimeCapabilityRegistry);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MappingFunctionRegistry(RuntimeCapabilityRegistry);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineProfileRuntimeRegistry(RuntimeCapabilityRegistry);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FfiAdapterRegistry(RuntimeCapabilityRegistry);

macro_rules! runtime_registry_impl {
    ($ty:ty, $kind:expr) => {
        impl $ty {
            pub fn register(
                &mut self,
                namespace: impl Into<String>,
                name: impl Into<String>,
                version_major: u16,
                version_minor: u16,
            ) -> Result<(), CoveError> {
                self.0.register(RuntimeCapability::new(
                    $kind,
                    namespace,
                    name,
                    version_major,
                    version_minor,
                ))
            }

            pub fn supports(
                &self,
                namespace: &str,
                name: &str,
                version_major: u16,
                version_minor: u16,
            ) -> bool {
                self.0
                    .supports($kind, namespace, name, version_major, version_minor)
            }

            pub fn capabilities(&self) -> impl Iterator<Item = &RuntimeCapability> {
                self.0.capabilities()
            }
        }
    };
}

runtime_registry_impl!(CodecRegistry, RuntimeHintKindV2::CodecRegistry);
runtime_registry_impl!(LayoutPlanRegistry, RuntimeHintKindV2::LayoutRegistry);
runtime_registry_impl!(PredicateKernelRegistry, RuntimeHintKindV2::PredicateKernel);
runtime_registry_impl!(MappingFunctionRegistry, RuntimeHintKindV2::LanguageBinding);
runtime_registry_impl!(
    EngineProfileRuntimeRegistry,
    RuntimeHintKindV2::EngineAdapter
);
runtime_registry_impl!(FfiAdapterRegistry, RuntimeHintKindV2::FfiSurface);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeSession {
    pub codecs: CodecRegistry,
    pub layout_plans: LayoutPlanRegistry,
    pub predicate_kernels: PredicateKernelRegistry,
    pub mapping_functions: MappingFunctionRegistry,
    pub engine_profiles: EngineProfileRuntimeRegistry,
    pub ffi_adapters: FfiAdapterRegistry,
}

impl RuntimeSession {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn default_builtins() -> Self {
        let mut session = Self::empty();
        session
            .codecs
            .register("org.cove", "core-codecs", 1, 0)
            .expect("builtin runtime capability is valid");
        session
            .layout_plans
            .register("org.cove", "scan-plan-v1", 1, 0)
            .expect("builtin runtime capability is valid");
        session
            .predicate_kernels
            .register("org.cove", "metadata-pruning", 1, 0)
            .expect("builtin runtime capability is valid");
        session
            .mapping_functions
            .register("org.cove", "filecode-canonical", 1, 0)
            .expect("builtin runtime capability is valid");
        session
            .engine_profiles
            .register("org.cove", "datafusion", 1, 0)
            .expect("builtin runtime capability is valid");
        session
    }

    pub fn supports_hint(&self, hint: &RuntimeCompatibilityHintV2) -> bool {
        match hint.hint_kind {
            RuntimeHintKindV2::CodecRegistry => self.codecs.0.supports_hint(hint),
            RuntimeHintKindV2::LayoutRegistry => self.layout_plans.0.supports_hint(hint),
            RuntimeHintKindV2::PredicateKernel => self.predicate_kernels.0.supports_hint(hint),
            RuntimeHintKindV2::EngineAdapter => self.engine_profiles.0.supports_hint(hint),
            RuntimeHintKindV2::FfiSurface => self.ffi_adapters.0.supports_hint(hint),
            RuntimeHintKindV2::LanguageBinding | RuntimeHintKindV2::WasmOrExternalKernelPackage => {
                self.mapping_functions.0.supports_hint(hint)
            }
        }
    }

    pub fn unsupported_required_hints<'a>(
        &self,
        hints: &'a [RuntimeCompatibilityHintV2],
    ) -> Vec<&'a RuntimeCompatibilityHintV2> {
        hints
            .iter()
            .filter(|hint| hint.required && !self.supports_hint(hint))
            .collect()
    }
}

impl RuntimeCompatibilityHintV2 {
    pub fn parse_one(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        let mut cursor = Cursor::new(bytes);
        let hint_id = cursor.u32()?;
        let hint_kind_raw = cursor.u16()?;
        let required_raw = cursor.u8()?;
        let flags = cursor.u8()?;
        let namespace_len = cursor.u16()? as usize;
        let namespace = parse_utf8(cursor.bytes(namespace_len)?, "runtime hint namespace")?;
        let name_len = cursor.u16()? as usize;
        let name = parse_utf8(cursor.bytes(name_len)?, "runtime hint name")?;
        let version_major = cursor.u16()?;
        let version_minor = cursor.u16()?;
        let payload_ref = cursor.u32()?;
        let checksum_offset = cursor.position;
        let checksum = cursor.u32()?;
        let consumed = cursor.position;

        let mut check = bytes[..consumed].to_vec();
        check[checksum_offset..checksum_offset + 4].fill(0);
        if checksum::crc32c(&check) != checksum {
            return Err(CoveError::ChecksumMismatch);
        }

        let hint_kind =
            RuntimeHintKindV2::from_u16(hint_kind_raw).ok_or(CoveError::RuntimeHintUnsupported)?;
        let required = match required_raw {
            0 => false,
            1 => true,
            _ => return Err(CoveError::RuntimeHintUnsupported),
        };
        let hint = Self {
            hint_id,
            hint_kind,
            required,
            flags,
            namespace,
            name,
            version_major,
            version_minor,
            payload_ref,
            checksum,
        };
        hint.validate()?;
        Ok((hint, consumed))
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        let mut hints = Vec::new();
        let mut offset = 0usize;
        while offset < bytes.len() {
            let (hint, consumed) = Self::parse_one(&bytes[offset..])?;
            hints.push(hint);
            offset = offset
                .checked_add(consumed)
                .ok_or(CoveError::ArithOverflow)?;
        }
        validate_hints(&hints)?;
        Ok(hints)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        self.validate()?;
        if self.namespace.len() > u16::MAX as usize || self.name.len() > u16::MAX as usize {
            return Err(CoveError::RuntimeHintUnsupported);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&self.hint_id.to_le_bytes());
        out.extend_from_slice(&(self.hint_kind as u16).to_le_bytes());
        out.push(u8::from(self.required));
        out.push(self.flags);
        out.extend_from_slice(&(self.namespace.len() as u16).to_le_bytes());
        out.extend_from_slice(self.namespace.as_bytes());
        out.extend_from_slice(&(self.name.len() as u16).to_le_bytes());
        out.extend_from_slice(self.name.as_bytes());
        out.extend_from_slice(&self.version_major.to_le_bytes());
        out.extend_from_slice(&self.version_minor.to_le_bytes());
        out.extend_from_slice(&self.payload_ref.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        let crc = checksum::crc32c(&out);
        let checksum_offset = out.len() - 4;
        out[checksum_offset..].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.namespace.is_empty() || self.name.is_empty() {
            return Err(CoveError::RuntimeHintUnsupported);
        }
        Ok(())
    }
}

pub fn validate_hints(hints: &[RuntimeCompatibilityHintV2]) -> Result<(), CoveError> {
    let mut ids = BTreeSet::new();
    let mut identities = BTreeSet::new();
    for hint in hints {
        hint.validate()?;
        if !ids.insert(hint.hint_id) {
            return Err(CoveError::RuntimeHintUnsupported);
        }
        let identity = (
            hint.hint_kind as u16,
            hint.namespace.as_str(),
            hint.name.as_str(),
            hint.version_major,
            hint.version_minor,
        );
        if !identities.insert(identity) {
            return Err(CoveError::RuntimeHintUnsupported);
        }
    }
    Ok(())
}

pub fn unsupported_required_hints<'a, I>(
    hints: &'a [RuntimeCompatibilityHintV2],
    supported: I,
) -> Vec<&'a RuntimeCompatibilityHintV2>
where
    I: IntoIterator<Item = (RuntimeHintKindV2, &'a str, &'a str, u16, u16)>,
{
    let supported = supported
        .into_iter()
        .map(|(kind, namespace, name, major, minor)| (kind as u16, namespace, name, major, minor))
        .collect::<BTreeSet<_>>();
    hints
        .iter()
        .filter(|hint| {
            hint.required
                && !supported.contains(&(
                    hint.hint_kind as u16,
                    hint.namespace.as_str(),
                    hint.name.as_str(),
                    hint.version_major,
                    hint.version_minor,
                ))
        })
        .collect()
}

fn parse_utf8(bytes: &[u8], field: &str) -> Result<String, CoveError> {
    std::str::from_utf8(bytes)
        .map(|value| value.to_string())
        .map_err(|_| CoveError::BadSection(format!("{field} is not valid UTF-8")))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(hint_id: u32) -> RuntimeCompatibilityHintV2 {
        RuntimeCompatibilityHintV2 {
            hint_id,
            hint_kind: RuntimeHintKindV2::EngineAdapter,
            required: true,
            flags: 0,
            namespace: "org.cove".into(),
            name: format!("adapter-{hint_id}"),
            version_major: 1,
            version_minor: 0,
            payload_ref: u32::MAX,
            checksum: 0,
        }
    }

    #[test]
    fn hints_round_trip() {
        let mut bytes = hint(1).serialize().unwrap();
        bytes.extend_from_slice(&hint(2).serialize().unwrap());
        let parsed = RuntimeCompatibilityHintV2::parse_many(&bytes).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].namespace, "org.cove");
    }

    #[test]
    fn unsupported_required_hints_are_operation_scoped() {
        let hints = vec![hint(1)];
        let unsupported = unsupported_required_hints(&hints, []);
        assert_eq!(unsupported.len(), 1);
        let supported = unsupported_required_hints(
            &hints,
            [(
                RuntimeHintKindV2::EngineAdapter,
                "org.cove",
                "adapter-1",
                1,
                0,
            )],
        );
        assert!(supported.is_empty());
    }

    #[test]
    fn runtime_session_discovers_default_builtins() {
        let session = RuntimeSession::default_builtins();
        assert!(session
            .engine_profiles
            .supports("org.cove", "datafusion", 1, 0));
        assert!(session
            .predicate_kernels
            .supports("org.cove", "metadata-pruning", 1, 0));
    }

    #[test]
    fn runtime_session_reports_unsupported_required_hints() {
        let mut required = hint(7);
        required.name = "datafusion".into();
        let mut missing = hint(8);
        missing.hint_kind = RuntimeHintKindV2::PredicateKernel;
        missing.name = "missing-kernel".into();
        let optional = RuntimeCompatibilityHintV2 {
            required: false,
            ..missing.clone()
        };
        let hints = vec![required, missing, optional];
        let unsupported = RuntimeSession::default_builtins().unsupported_required_hints(&hints);
        assert_eq!(unsupported.len(), 1);
        assert_eq!(unsupported[0].hint_id, 8);
    }
}
