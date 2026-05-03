//! Quay Format (QF) v1.0 — Collation registry skeleton.

use crate::QfError;

/// A collation entry, mapping a column or domain to a named collation.
#[derive(Debug, Clone)]
pub struct CollationEntry {
    /// Collation name (e.g. "byte-equality", "utf8-ci").
    pub name: String,
    /// Raw collation metadata bytes.
    pub metadata: Vec<u8>,
}

/// The default collation: byte-for-byte equality comparison.
pub const DEFAULT_COLLATION: &str = "byte-equality";

/// A parsed collation registry.
#[derive(Debug, Clone, Default)]
pub struct CollationRegistry {
    /// All entries in the registry.
    pub entries: Vec<CollationEntry>,
}

impl CollationRegistry {
    /// Parse a collation registry from raw section bytes.
    ///
    /// Wire format: `u32` LE entry count, then entries of:
    /// `u16` LE `name_len`, name bytes (UTF-8), `u16` LE `meta_len`, meta bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 {
            return Err(QfError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let mut entries = Vec::with_capacity(entry_count as usize);

        for _ in 0..entry_count {
            // u16 name_len
            if pos.checked_add(2).ok_or(QfError::ArithOverflow)? > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let name_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos = pos.checked_add(2).ok_or(QfError::ArithOverflow)?;

            // name bytes
            let name_end = pos.checked_add(name_len).ok_or(QfError::ArithOverflow)?;
            if name_end > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let name = std::str::from_utf8(&bytes[pos..name_end])
                .map_err(|_| QfError::BadSection("collation name is not valid UTF-8".into()))?
                .to_string();
            pos = name_end;

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

            entries.push(CollationEntry { name, metadata });
        }

        Ok(Self { entries })
    }

    /// Returns `true` if all entries use the default byte-equality collation.
    pub fn is_all_byte_equality(&self) -> bool {
        self.entries.iter().all(|e| e.name == DEFAULT_COLLATION)
    }

    /// Returns `true` if the named collation is safe to use.
    ///
    /// Only `"byte-equality"` is currently supported.
    pub fn is_safe_collation(name: &str) -> bool {
        name == DEFAULT_COLLATION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = (entries.len() as u32).to_le_bytes().to_vec();
        for (name, meta) in entries {
            let name_bytes = name.as_bytes();
            out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(&(meta.len() as u16).to_le_bytes());
            out.extend_from_slice(meta);
        }
        out
    }

    #[test]
    fn empty_registry_parses() {
        let bytes = make_registry_bytes(&[]);
        let reg = CollationRegistry::parse(&bytes).unwrap();
        assert_eq!(reg.entries.len(), 0);
        assert!(reg.is_all_byte_equality());
    }

    #[test]
    fn byte_equality_is_safe() {
        assert!(CollationRegistry::is_safe_collation("byte-equality"));
    }

    #[test]
    fn unknown_collation_is_not_safe() {
        assert!(!CollationRegistry::is_safe_collation("utf8-icu"));
    }

    #[test]
    fn registry_with_byte_equality_entry() {
        let bytes = make_registry_bytes(&[("byte-equality", b"")]);
        let reg = CollationRegistry::parse(&bytes).unwrap();
        assert_eq!(reg.entries.len(), 1);
        assert_eq!(reg.entries[0].name, "byte-equality");
        assert!(reg.is_all_byte_equality());
    }
}
