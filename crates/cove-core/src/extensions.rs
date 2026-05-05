//! Cove Format (COVE) v1.0 — Extension registry (Spec §45).

use crate::CoveError;

/// An entry in the extension registry (Spec §45 `ExtensionEntryV1`).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionRegistryEntry {
    /// Opaque numeric identifier assigned to this extension in the registry.
    pub extension_id: u32,
    /// Extension namespace (e.g. reverse-DNS org name).
    pub namespace: Vec<u8>,
    /// Extension name within the namespace.
    pub name: Vec<u8>,
    /// Major version of the extension.
    pub version_major: u16,
    /// Minor version of the extension.
    pub version_minor: u16,
    /// Extension kind tag (see `ExtensionKind` in the spec).
    pub extension_kind: u16,
    /// Feature bit that, when non-zero, marks this extension as required.
    /// Readers that do not understand a required extension MUST reject the file.
    pub required_feature_bit: u64,
    /// Feature bit that, when non-zero, marks this extension as optional.
    /// Readers that do not understand an optional extension MUST ignore it.
    pub optional_feature_bit: u64,
    /// Fallback kind tag (0 = none).
    pub fallback_kind: u16,
    /// Section-directory reference for the fallback section (0 = none).
    pub fallback_ref: u32,
    /// Section-directory reference for the extension payload (0 = none).
    pub payload_ref: u32,
    /// CRC32C of this entry's preceding fields.
    pub checksum: u32,
}

/// A parsed extension registry (Spec §45 `ExtensionRegistryHeaderV1` + entries).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtensionRegistry {
    /// All entries in the registry.
    pub entries: Vec<ExtensionRegistryEntry>,
}

impl ExtensionRegistry {
    /// Parse an extension registry from raw section bytes.
    ///
    /// Wire format (Spec §45):
    /// ```text
    /// ExtensionRegistryHeaderV1 { extension_count: u32, flags: u32 }
    /// ExtensionEntryV1 × extension_count {
    ///     extension_id: u32,
    ///     namespace_len: u16, namespace: [u8],
    ///     name_len: u16, name: [u8],
    ///     version_major: u16, version_minor: u16,
    ///     extension_kind: u16,
    ///     required_feature_bit: u64, optional_feature_bit: u64,
    ///     fallback_kind: u16, fallback_ref: u32,
    ///     payload_ref: u32, checksum: u32,
    /// }
    /// ```
    /// All integers are little-endian.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        // Header: extension_count (u32) + flags (u32) = 8 bytes
        if bytes.len() < 8 {
            return Err(CoveError::BufferTooShort);
        }
        let extension_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        // flags (bytes[4..8]) reserved for future use — read and discard
        let mut pos = 8usize;
        let mut entries = Vec::with_capacity(extension_count);

        for _ in 0..extension_count {
            macro_rules! read_u16 {
                () => {{
                    let end = pos.checked_add(2).ok_or(CoveError::ArithOverflow)?;
                    if end > bytes.len() {
                        return Err(CoveError::BufferTooShort);
                    }
                    let v = u16::from_le_bytes(bytes[pos..end].try_into().unwrap());
                    pos = end;
                    v
                }};
            }
            macro_rules! read_u32 {
                () => {{
                    let end = pos.checked_add(4).ok_or(CoveError::ArithOverflow)?;
                    if end > bytes.len() {
                        return Err(CoveError::BufferTooShort);
                    }
                    let v = u32::from_le_bytes(bytes[pos..end].try_into().unwrap());
                    pos = end;
                    v
                }};
            }
            macro_rules! read_u64 {
                () => {{
                    let end = pos.checked_add(8).ok_or(CoveError::ArithOverflow)?;
                    if end > bytes.len() {
                        return Err(CoveError::BufferTooShort);
                    }
                    let v = u64::from_le_bytes(bytes[pos..end].try_into().unwrap());
                    pos = end;
                    v
                }};
            }
            macro_rules! read_bytes {
                ($len:expr) => {{
                    let len = $len as usize;
                    let end = pos.checked_add(len).ok_or(CoveError::ArithOverflow)?;
                    if end > bytes.len() {
                        return Err(CoveError::BufferTooShort);
                    }
                    let v = bytes[pos..end].to_vec();
                    pos = end;
                    v
                }};
            }

            let extension_id = read_u32!();
            let namespace_len = read_u16!();
            let namespace = read_bytes!(namespace_len);
            let name_len = read_u16!();
            let name = read_bytes!(name_len);
            let version_major = read_u16!();
            let version_minor = read_u16!();
            let extension_kind = read_u16!();
            let required_feature_bit = read_u64!();
            let optional_feature_bit = read_u64!();
            let fallback_kind = read_u16!();
            let fallback_ref = read_u32!();
            let payload_ref = read_u32!();
            let checksum = read_u32!();

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
                checksum,
            });
        }

        Ok(Self { entries })
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    /// Header `flags` field is emitted as zero (reserved in v1).
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let mut out = Vec::with_capacity(8 + self.entries.len() * 64);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // flags reserved
        for e in &self.entries {
            let namespace_len = u16::try_from(e.namespace.len()).map_err(|_| {
                CoveError::BadSection("extension namespace exceeds u16 length limit".into())
            })?;
            let name_len = u16::try_from(e.name.len()).map_err(|_| {
                CoveError::BadSection("extension name exceeds u16 length limit".into())
            })?;
            out.extend_from_slice(&e.extension_id.to_le_bytes());
            out.extend_from_slice(&namespace_len.to_le_bytes());
            out.extend_from_slice(&e.namespace);
            out.extend_from_slice(&name_len.to_le_bytes());
            out.extend_from_slice(&e.name);
            out.extend_from_slice(&e.version_major.to_le_bytes());
            out.extend_from_slice(&e.version_minor.to_le_bytes());
            out.extend_from_slice(&e.extension_kind.to_le_bytes());
            out.extend_from_slice(&e.required_feature_bit.to_le_bytes());
            out.extend_from_slice(&e.optional_feature_bit.to_le_bytes());
            out.extend_from_slice(&e.fallback_kind.to_le_bytes());
            out.extend_from_slice(&e.fallback_ref.to_le_bytes());
            out.extend_from_slice(&e.payload_ref.to_le_bytes());
            out.extend_from_slice(&e.checksum.to_le_bytes());
        }
        Ok(out)
    }

    /// Validate entries against known extensions.
    ///
    /// This skeleton implementation knows no extensions. Entries with a
    /// non-zero `required_feature_bit` are treated as required and return
    /// [`CoveError::BadExtension`]. Entries with a zero `required_feature_bit`
    /// are treated as optional and are silently ignored when
    /// `allow_unknown_optional` is `true`.
    pub fn validate_known(&self, allow_unknown_optional: bool) -> Result<(), CoveError> {
        for entry in &self.entries {
            if entry.required_feature_bit != 0 {
                return Err(CoveError::BadExtension);
            } else if !allow_unknown_optional {
                return Err(CoveError::BadExtension);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a Spec §45 EXTENSION_REGISTRY section payload for testing.
    ///
    /// Each entry tuple: `(required_feature_bit, optional_feature_bit, namespace, name)`.
    /// Other fields (`extension_id`, `version_major`, etc.) are set to fixed
    /// placeholder values sufficient for parse/validate round-trip tests.
    fn make_registry_bytes(entries: &[(u64, u64, &[u8], &[u8])]) -> Vec<u8> {
        // Header: extension_count (u32) + flags (u32)
        let mut out = (entries.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&0u32.to_le_bytes()); // flags = 0
        for (req_bit, opt_bit, ns, nm) in entries {
            out.extend_from_slice(&1u32.to_le_bytes()); // extension_id
            out.extend_from_slice(&(ns.len() as u16).to_le_bytes());
            out.extend_from_slice(ns);
            out.extend_from_slice(&(nm.len() as u16).to_le_bytes());
            out.extend_from_slice(nm);
            out.extend_from_slice(&1u16.to_le_bytes()); // version_major
            out.extend_from_slice(&0u16.to_le_bytes()); // version_minor
            out.extend_from_slice(&0u16.to_le_bytes()); // extension_kind
            out.extend_from_slice(&req_bit.to_le_bytes()); // required_feature_bit
            out.extend_from_slice(&opt_bit.to_le_bytes()); // optional_feature_bit
            out.extend_from_slice(&0u16.to_le_bytes()); // fallback_kind
            out.extend_from_slice(&0u32.to_le_bytes()); // fallback_ref
            out.extend_from_slice(&0u32.to_le_bytes()); // payload_ref
            out.extend_from_slice(&0u32.to_le_bytes()); // checksum
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
        // required_feature_bit = 0 → optional extension
        let bytes = make_registry_bytes(&[(0, 0x0020_0000, b"com.example", b"my-ext")]);
        let reg = ExtensionRegistry::parse(&bytes).unwrap();
        assert_eq!(reg.entries.len(), 1);
        assert_eq!(reg.entries[0].required_feature_bit, 0);
        assert!(reg.validate_known(true).is_ok());
    }

    #[test]
    fn required_unknown_extension_rejected() {
        // required_feature_bit != 0 → required extension; skeleton knows none
        let bytes = make_registry_bytes(&[(0x0020_0000, 0, b"com.example", b"must-have")]);
        let reg = ExtensionRegistry::parse(&bytes).unwrap();
        assert_eq!(reg.validate_known(true), Err(CoveError::BadExtension));
    }

    #[test]
    fn optional_extension_rejected_when_strict() {
        // allow_unknown_optional = false → even optional extensions fail
        let bytes = make_registry_bytes(&[(0, 0x0020_0000, b"com.example", b"opt-ext")]);
        let reg = ExtensionRegistry::parse(&bytes).unwrap();
        assert_eq!(reg.validate_known(false), Err(CoveError::BadExtension));
    }

    #[test]
    fn truncated_registry_header_rejected() {
        // Fewer than 8 header bytes
        let bytes = 1u32.to_le_bytes().to_vec(); // only 4 bytes
        assert_eq!(
            ExtensionRegistry::parse(&bytes),
            Err(CoveError::BufferTooShort)
        );
    }

    #[test]
    fn truncated_registry_entry_rejected() {
        // Header declares 1 entry but no entry data follows
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes()); // flags
        assert_eq!(
            ExtensionRegistry::parse(&bytes),
            Err(CoveError::BufferTooShort)
        );
    }
}

#[cfg(test)]
mod serialize_tests {
    use super::*;

    #[test]
    fn serialize_round_trip() {
        let reg = ExtensionRegistry {
            entries: vec![ExtensionRegistryEntry {
                extension_id: 9,
                namespace: b"org.example".to_vec(),
                name: b"feature-x".to_vec(),
                version_major: 1,
                version_minor: 2,
                extension_kind: 3,
                required_feature_bit: 0x1000,
                optional_feature_bit: 0,
                fallback_kind: 0,
                fallback_ref: 0,
                payload_ref: 11,
                checksum: 0xDEADBEEF,
            }],
        };
        let bytes = reg.serialize().unwrap();
        assert_eq!(ExtensionRegistry::parse(&bytes).unwrap(), reg);
    }

    #[test]
    fn serialize_rejects_namespace_longer_than_u16() {
        let reg = ExtensionRegistry {
            entries: vec![ExtensionRegistryEntry {
                extension_id: 9,
                namespace: vec![b'a'; usize::from(u16::MAX) + 1],
                name: b"feature-x".to_vec(),
                version_major: 1,
                version_minor: 2,
                extension_kind: 3,
                required_feature_bit: 0x1000,
                optional_feature_bit: 0,
                fallback_kind: 0,
                fallback_ref: 0,
                payload_ref: 11,
                checksum: 0xDEADBEEF,
            }],
        };

        assert!(matches!(reg.serialize(), Err(CoveError::BadSection(_))));
    }
}
