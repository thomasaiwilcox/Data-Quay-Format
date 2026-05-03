//! Quay Format (QF) v1.0 — Extension registry skeleton.

use crate::QfError;

/// Determines whether an unknown extension is acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionRequirement {
    /// Extension is optional; unknown extensions are ignored.
    Optional,
    /// Extension is required; unknown required extensions MUST fail.
    Required,
}

/// An entry in the extension registry.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionRegistryEntry {
    /// Opaque identifier for the extension (e.g. a UUID or short tag).
    pub id: Vec<u8>,
    /// Whether the extension is required to decode the file correctly.
    pub requirement: ExtensionRequirement,
    /// Raw metadata bytes from the registry entry.
    pub metadata: Vec<u8>,
}

/// A parsed extension registry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtensionRegistry {
    /// All entries in the registry.
    pub entries: Vec<ExtensionRegistryEntry>,
}

impl ExtensionRegistry {
    /// Parse an extension registry from raw section bytes.
    ///
    /// Wire format: `u32` LE entry count, then entries of:
    /// `u8` requirement (0=Optional, 1=Required), `u16` LE `id_len`, id bytes,
    /// `u16` LE `meta_len`, meta bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 {
            return Err(QfError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let mut entries = Vec::with_capacity(entry_count as usize);

        for _ in 0..entry_count {
            // u8 requirement
            if pos >= bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let req_byte = bytes[pos];
            pos = pos.checked_add(1).ok_or(QfError::ArithOverflow)?;
            let requirement = match req_byte {
                0 => ExtensionRequirement::Optional,
                1 => ExtensionRequirement::Required,
                other => {
                    return Err(QfError::BadSection(format!(
                        "unknown extension requirement byte {other}"
                    )))
                }
            };

            // u16 id_len
            if pos.checked_add(2).ok_or(QfError::ArithOverflow)? > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let id_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos = pos.checked_add(2).ok_or(QfError::ArithOverflow)?;

            // id bytes
            let id_end = pos.checked_add(id_len).ok_or(QfError::ArithOverflow)?;
            if id_end > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let id = bytes[pos..id_end].to_vec();
            pos = id_end;

            // u16 meta_len
            if pos.checked_add(2).ok_or(QfError::ArithOverflow)? > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let meta_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos = pos.checked_add(2).ok_or(QfError::ArithOverflow)?;

            // meta bytes
            let meta_end = pos.checked_add(meta_len).ok_or(QfError::ArithOverflow)?;
            if meta_end > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let metadata = bytes[pos..meta_end].to_vec();
            pos = meta_end;

            entries.push(ExtensionRegistryEntry {
                id,
                requirement,
                metadata,
            });
        }

        Ok(Self { entries })
    }

    /// Validate entries against known extensions.
    ///
    /// This skeleton implementation knows no extensions. Required unknown
    /// extensions return [`QfError::BadExtension`]. Optional unknown extensions
    /// are silently ignored when `allow_unknown_optional` is `true`.
    pub fn validate_known(&self, allow_unknown_optional: bool) -> Result<(), QfError> {
        for entry in &self.entries {
            match entry.requirement {
                ExtensionRequirement::Required => return Err(QfError::BadExtension),
                ExtensionRequirement::Optional => {
                    if !allow_unknown_optional {
                        return Err(QfError::BadExtension);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry_bytes(entries: &[(u8, &[u8], &[u8])]) -> Vec<u8> {
        let mut out = (entries.len() as u32).to_le_bytes().to_vec();
        for (req, id, meta) in entries {
            out.push(*req);
            out.extend_from_slice(&(id.len() as u16).to_le_bytes());
            out.extend_from_slice(id);
            out.extend_from_slice(&(meta.len() as u16).to_le_bytes());
            out.extend_from_slice(meta);
        }
        out
    }

    #[test]
    fn empty_registry_parses_and_validates() {
        let bytes = make_registry_bytes(&[]);
        let reg = ExtensionRegistry::parse(&bytes).unwrap();
        assert_eq!(reg.entries.len(), 0);
        assert!(reg.validate_known(true).is_ok());
    }

    #[test]
    fn optional_unknown_extension_accepted() {
        let bytes = make_registry_bytes(&[(0, b"my-ext", b"")]);
        let reg = ExtensionRegistry::parse(&bytes).unwrap();
        assert_eq!(reg.entries.len(), 1);
        assert!(reg.validate_known(true).is_ok());
    }

    #[test]
    fn required_unknown_extension_rejected() {
        let bytes = make_registry_bytes(&[(1, b"must-have-ext", b"meta")]);
        let reg = ExtensionRegistry::parse(&bytes).unwrap();
        assert_eq!(reg.validate_known(true), Err(QfError::BadExtension));
    }

    #[test]
    fn truncated_registry_rejected() {
        // 1 entry declared but no data follows
        let bytes = 1u32.to_le_bytes().to_vec();
        assert_eq!(
            ExtensionRegistry::parse(&bytes),
            Err(QfError::BufferTooShort)
        );
    }
}
