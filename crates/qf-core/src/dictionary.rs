//! Quay Format (QF) v1.0 — File dictionary structures.
//!
//! Corresponds to Section 16 of the QF v1.0 specification.
//!
//! The file dictionary maps dense file-local FileCodes (zero-based ordinals)
//! to canonical logical values.  It is split across two sections:
//!
//! - `FILE_DICTIONARY_INDEX`   — fixed-size index entries, one per dictionary entry.
//! - `FILE_DICTIONARY_PAYLOAD` — variable-length value bytes for inline-overflow
//!   and payload-class values.

use crate::{
    constants::{StorageClass, ValueTag},
    error::QfError,
};

// ── FileDictionaryHeaderV1 ────────────────────────────────────────────────────

/// Serialised size of the dictionary header in bytes.
/// `entry_count`(4) + `flags`(4) + `index_entry_len`(2) + `value_hash_algorithm`(2)
/// + `payload_length`(8) + `reserved`(24) = 44.
pub const DICT_HEADER_SIZE: usize = 44;

/// Header that precedes the array of [`FileDictionaryIndexEntryV1`] records.
///
/// Corresponds to `FileDictionaryHeaderV1` in Section 16.1 of the specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDictionaryHeaderV1 {
    /// Total number of dictionary entries (== maximum valid FileCode + 1).
    pub entry_count: u32,
    /// Dictionary-level flags.
    pub flags: u32,
    /// Byte length of each index entry (fixed at 40 for v1).
    pub index_entry_len: u16,
    /// Hash algorithm used for `canonical_hash64`.
    /// 0 = None, 1 = xxh3_64, 2 = sha256_truncated64.
    pub value_hash_algorithm: u16,
    /// Total byte length of the FILE_DICTIONARY_PAYLOAD section.
    pub payload_length: u64,
    /// Reserved — MUST be zero.
    pub reserved: [u8; 24],
}

impl FileDictionaryHeaderV1 {
    /// Fixed byte length of each dictionary index entry in v1.
    ///
    /// Field breakdown: `value_tag`(2) + `storage_class`(1) + `flags`(1) +
    /// `inline_len`(1) + `reserved0`(3) + `inline_data`(16) +
    /// `payload_offset`(8) + `payload_length`(4) + `canonical_hash64`(8) +
    /// `reserved1`(4) = 48 bytes.
    pub const INDEX_ENTRY_LEN: u16 = 48;

    /// Serialise to a 44-byte wire buffer.
    pub fn serialize(&self) -> [u8; DICT_HEADER_SIZE] {
        let mut buf = [0u8; DICT_HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.entry_count.to_le_bytes());
        buf[4..8].copy_from_slice(&self.flags.to_le_bytes());
        buf[8..10].copy_from_slice(&self.index_entry_len.to_le_bytes());
        buf[10..12].copy_from_slice(&self.value_hash_algorithm.to_le_bytes());
        buf[12..20].copy_from_slice(&self.payload_length.to_le_bytes());
        buf[20..44].copy_from_slice(&self.reserved);
        buf
    }

    /// Parse from a byte slice.
    pub fn parse(buf: &[u8]) -> Result<Self, QfError> {
        if buf.len() < DICT_HEADER_SIZE {
            return Err(QfError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let flags = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        let index_entry_len = u16::from_le_bytes(buf[8..10].try_into().unwrap());
        let value_hash_algorithm = u16::from_le_bytes(buf[10..12].try_into().unwrap());
        let payload_length = u64::from_le_bytes(buf[12..20].try_into().unwrap());
        let mut reserved = [0u8; 24];
        reserved.copy_from_slice(&buf[20..44]);
        if reserved.iter().any(|&b| b != 0) {
            return Err(QfError::ReservedNotZero);
        }
        Ok(Self {
            entry_count,
            flags,
            index_entry_len,
            value_hash_algorithm,
            payload_length,
            reserved,
        })
    }
}

// ── FileDictionaryIndexEntryV1 ────────────────────────────────────────────────

/// Serialised size of one dictionary index entry in bytes.
/// `value_tag`(2) + `storage_class`(1) + `flags`(1) + `inline_len`(1) +
/// `reserved0`(3) + `inline_data`(16) + `payload_offset`(8) +
/// `payload_length`(4) + `canonical_hash64`(8) + `reserved1`(4) = 48.
pub const DICT_INDEX_ENTRY_SIZE: usize = 48;

/// One index entry in the file dictionary.
///
/// The FileCode for this entry is its zero-based ordinal position in the
/// index array (i.e., the first entry is FileCode 0, the second is FileCode 1,
/// and so on).
///
/// Corresponds to `FileDictionaryIndexEntryV1` in Section 16.2 of the
/// specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDictionaryIndexEntryV1 {
    /// Canonical type tag for the value.
    pub value_tag: u16,
    /// Storage class — `Inline`, `Payload`, or `Redacted`.
    pub storage_class: u8,
    /// Entry-level flags.
    pub flags: u8,
    /// Number of inline bytes used (≤ 16).
    pub inline_len: u8,
    /// Reserved — MUST be zero.
    pub reserved0: [u8; 3],
    /// Inline canonical value bytes (up to 16 bytes).
    pub inline_data: [u8; 16],
    /// Byte offset into the `FILE_DICTIONARY_PAYLOAD` section.
    pub payload_offset: u64,
    /// Byte length within the `FILE_DICTIONARY_PAYLOAD` section.
    pub payload_length: u32,
    /// 64-bit canonical hash (acceleration hint — NOT a proof of equality).
    pub canonical_hash64: u64,
    /// Reserved — MUST be zero.
    pub reserved1: u32,
}

impl FileDictionaryIndexEntryV1 {
    /// Serialise to a 48-byte wire buffer.
    pub fn serialize(&self) -> [u8; DICT_INDEX_ENTRY_SIZE] {
        let mut buf = [0u8; DICT_INDEX_ENTRY_SIZE];
        buf[0..2].copy_from_slice(&self.value_tag.to_le_bytes());
        buf[2] = self.storage_class;
        buf[3] = self.flags;
        buf[4] = self.inline_len;
        buf[5..8].copy_from_slice(&self.reserved0);
        buf[8..24].copy_from_slice(&self.inline_data);
        buf[24..32].copy_from_slice(&self.payload_offset.to_le_bytes());
        buf[32..36].copy_from_slice(&self.payload_length.to_le_bytes());
        buf[36..44].copy_from_slice(&self.canonical_hash64.to_le_bytes());
        buf[44..48].copy_from_slice(&self.reserved1.to_le_bytes());
        buf
    }

    /// Parse from a byte slice (must be at least 48 bytes).
    pub fn parse(buf: &[u8]) -> Result<Self, QfError> {
        if buf.len() < DICT_INDEX_ENTRY_SIZE {
            return Err(QfError::BufferTooShort);
        }
        let value_tag = u16::from_le_bytes(buf[0..2].try_into().unwrap());
        let storage_class = buf[2];
        let flags = buf[3];
        let inline_len = buf[4];

        let mut reserved0 = [0u8; 3];
        reserved0.copy_from_slice(&buf[5..8]);
        if reserved0.iter().any(|&b| b != 0) {
            return Err(QfError::ReservedNotZero);
        }

        let mut inline_data = [0u8; 16];
        inline_data.copy_from_slice(&buf[8..24]);

        let payload_offset = u64::from_le_bytes(buf[24..32].try_into().unwrap());
        let payload_length = u32::from_le_bytes(buf[32..36].try_into().unwrap());
        let canonical_hash64 = u64::from_le_bytes(buf[36..44].try_into().unwrap());
        let reserved1 = u32::from_le_bytes(buf[44..48].try_into().unwrap());

        if reserved1 != 0 {
            return Err(QfError::ReservedNotZero);
        }
        if ValueTag::from_u16(value_tag).is_none() {
            return Err(QfError::BadSection(format!(
                "unknown value_tag {value_tag} in dictionary index entry"
            )));
        }
        if StorageClass::from_u8(storage_class).is_none() {
            return Err(QfError::BadSection(format!(
                "unknown storage_class {storage_class} in dictionary index entry"
            )));
        }
        if inline_len as usize > inline_data.len() {
            return Err(QfError::BadSection(format!(
                "inline_len {inline_len} exceeds inline_data capacity (16)"
            )));
        }

        Ok(Self {
            value_tag,
            storage_class,
            flags,
            inline_len,
            reserved0,
            inline_data,
            payload_offset,
            payload_length,
            canonical_hash64,
            reserved1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_header_roundtrip() {
        let hdr = FileDictionaryHeaderV1 {
            entry_count: 3,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0u8; 24],
        };
        let bytes = hdr.serialize();
        assert_eq!(bytes.len(), DICT_HEADER_SIZE);
        let parsed = FileDictionaryHeaderV1::parse(&bytes).expect("parse should succeed");
        assert_eq!(parsed.entry_count, 3);
        assert_eq!(
            parsed.index_entry_len,
            FileDictionaryHeaderV1::INDEX_ENTRY_LEN
        );
    }

    #[test]
    fn dict_index_entry_roundtrip() {
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: 6,
            reserved0: [0; 3],
            inline_data: {
                let mut d = [0u8; 16];
                d[..6].copy_from_slice(b"active");
                d
            },
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0xcafe_babe_0000_0001,
            reserved1: 0,
        };
        let bytes = entry.serialize();
        let parsed = FileDictionaryIndexEntryV1::parse(&bytes).expect("parse should succeed");
        assert_eq!(parsed.value_tag, ValueTag::Utf8 as u16);
        assert_eq!(parsed.inline_len, 6);
        assert_eq!(&parsed.inline_data[..6], b"active");
    }

    #[test]
    fn dict_header_reserved_nonzero_rejected() {
        let hdr = FileDictionaryHeaderV1 {
            entry_count: 1,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0u8; 24],
        };
        let mut bytes = hdr.serialize();
        bytes[20] = 1; // first byte of reserved field
        assert_eq!(
            FileDictionaryHeaderV1::parse(&bytes),
            Err(QfError::ReservedNotZero)
        );
    }

    #[test]
    fn dict_index_entry_payload_storage_class() {
        // Payload storage class: inline_len may be 0; payload_offset/length point elsewhere.
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Binary as u16,
            storage_class: StorageClass::Payload as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0u8; 16],
            payload_offset: 1024,
            payload_length: 256,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let bytes = entry.serialize();
        let parsed = FileDictionaryIndexEntryV1::parse(&bytes).expect("parse should succeed");
        assert_eq!(parsed.storage_class, StorageClass::Payload as u8);
        assert_eq!(parsed.payload_offset, 1024);
        assert_eq!(parsed.payload_length, 256);
    }

    #[test]
    fn dict_index_entry_redacted_storage_class() {
        // Redacted storage class: value is present but access-restricted.
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Redacted as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0u8; 16],
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let bytes = entry.serialize();
        let parsed = FileDictionaryIndexEntryV1::parse(&bytes).expect("parse should succeed");
        assert_eq!(parsed.storage_class, StorageClass::Redacted as u8);
    }

    #[test]
    fn dict_index_entry_unknown_value_tag_rejected() {
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0u8; 16],
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let mut bytes = entry.serialize();
        // Overwrite value_tag with an unknown value.
        bytes[0..2].copy_from_slice(&999u16.to_le_bytes());
        assert!(matches!(
            FileDictionaryIndexEntryV1::parse(&bytes),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn dict_index_entry_unknown_storage_class_rejected() {
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0u8; 16],
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let mut bytes = entry.serialize();
        // Overwrite storage_class with an unknown value.
        bytes[2] = 99;
        assert!(matches!(
            FileDictionaryIndexEntryV1::parse(&bytes),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn dict_index_entry_inline_len_too_large_rejected() {
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0u8; 16],
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let mut bytes = entry.serialize();
        // Overwrite inline_len with 17 (> 16 capacity).
        bytes[4] = 17;
        assert!(matches!(
            FileDictionaryIndexEntryV1::parse(&bytes),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn dict_index_entry_reserved1_nonzero_rejected() {
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Int64 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0u8; 16],
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let mut bytes = entry.serialize();
        // Overwrite reserved1 with a non-zero value (bytes [44..48]).
        bytes[44..48].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            FileDictionaryIndexEntryV1::parse(&bytes),
            Err(QfError::ReservedNotZero)
        );
    }
}
