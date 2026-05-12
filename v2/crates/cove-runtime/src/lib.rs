//! COVE-R runtime compatibility hints for COVE v2.

use std::collections::BTreeSet;

use cove_core::{checksum, CoveError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}
