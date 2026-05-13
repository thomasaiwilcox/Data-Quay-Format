//! Cove Format (COVE) v2.0 — File dictionary structures.
//!
//! Corresponds to Section 16 of the COVE v2.0 specification.
//!
//! The file dictionary maps dense file-local FileCodes (zero-based ordinals)
//! to canonical logical values.  It is split across two sections:
//!
//! - `FILE_DICTIONARY_INDEX`   — fixed-size index entries, one per dictionary entry.
//! - `FILE_DICTIONARY_PAYLOAD` — variable-length value bytes for inline-overflow
//!   and payload-class values.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use crate::{
    canonical::{self, CanonicalValue},
    constants::{StorageClass, ValueTag},
    error::CoveError,
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
    /// Byte length of each index entry (fixed at 48 for v2).
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
    /// Fixed byte length of each dictionary index entry in v2.
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
    pub fn parse(buf: &[u8]) -> Result<Self, CoveError> {
        if buf.len() < DICT_HEADER_SIZE {
            return Err(CoveError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let flags = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        let index_entry_len = u16::from_le_bytes(buf[8..10].try_into().unwrap());
        if index_entry_len != Self::INDEX_ENTRY_LEN {
            return Err(CoveError::BadSection(format!(
                "index_entry_len is {index_entry_len}, expected {}",
                Self::INDEX_ENTRY_LEN
            )));
        }
        let value_hash_algorithm = u16::from_le_bytes(buf[10..12].try_into().unwrap());
        if value_hash_algorithm > 2 {
            return Err(CoveError::BadSection(format!(
                "unknown value_hash_algorithm {value_hash_algorithm}"
            )));
        }
        let payload_length = u64::from_le_bytes(buf[12..20].try_into().unwrap());
        let mut reserved = [0u8; 24];
        reserved.copy_from_slice(&buf[20..44]);
        if reserved.iter().any(|&b| b != 0) {
            return Err(CoveError::ReservedNotZero);
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
    pub fn parse(buf: &[u8]) -> Result<Self, CoveError> {
        if buf.len() < DICT_INDEX_ENTRY_SIZE {
            return Err(CoveError::BufferTooShort);
        }
        let value_tag = u16::from_le_bytes(buf[0..2].try_into().unwrap());
        let storage_class = buf[2];
        let flags = buf[3];
        let inline_len = buf[4];

        let mut reserved0 = [0u8; 3];
        reserved0.copy_from_slice(&buf[5..8]);
        if reserved0.iter().any(|&b| b != 0) {
            return Err(CoveError::ReservedNotZero);
        }

        let mut inline_data = [0u8; 16];
        inline_data.copy_from_slice(&buf[8..24]);

        let payload_offset = u64::from_le_bytes(buf[24..32].try_into().unwrap());
        let payload_length = u32::from_le_bytes(buf[32..36].try_into().unwrap());
        let canonical_hash64 = u64::from_le_bytes(buf[36..44].try_into().unwrap());
        let reserved1 = u32::from_le_bytes(buf[44..48].try_into().unwrap());

        if reserved1 != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        if ValueTag::from_u16(value_tag).is_none() {
            return Err(CoveError::BadSection(format!(
                "unknown value_tag {value_tag} in dictionary index entry"
            )));
        }
        if StorageClass::from_u8(storage_class).is_none() {
            return Err(CoveError::BadSection(format!(
                "unknown storage_class {storage_class} in dictionary index entry"
            )));
        }
        if inline_len as usize > inline_data.len() {
            return Err(CoveError::BadSection(format!(
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

    /// Validate payload offset/length against the dictionary payload section size.
    pub fn validate_payload_bounds(&self, payload_total_len: u64) -> Result<(), CoveError> {
        let end = self
            .payload_offset
            .checked_add(self.payload_length as u64)
            .ok_or(CoveError::ArithOverflow)?;
        if end > payload_total_len {
            return Err(CoveError::OffsetRange);
        }
        Ok(())
    }
}

/// A resolved dictionary value returned by [`FileDictionary::decode_value`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DictionaryValue {
    /// Raw value bytes copied from inline or payload storage.
    RawBytes(Vec<u8>),
    /// Value is present but access-restricted by the file's redaction policy.
    RedactedPresent,
}

/// Borrowed or owned view over file dictionary sections.
///
/// The view validates the dictionary header and section lengths up front, then
/// parses individual index entries only when requested. Call [`Self::validate_all`]
/// before using the view for conformance validation; lazy access must not
/// weaken Spec §16 dictionary invariants.
#[derive(Debug, Clone)]
pub struct FileDictionaryView<'a> {
    /// Parsed dictionary header.
    pub header: FileDictionaryHeaderV1,
    index_section: Cow<'a, [u8]>,
    payload_section: Cow<'a, [u8]>,
}

impl<'a> FileDictionaryView<'a> {
    /// Parses a lazy dictionary view from raw dictionary sections.
    pub fn parse(
        index_section: Cow<'a, [u8]>,
        payload_section: Cow<'a, [u8]>,
    ) -> Result<Self, CoveError> {
        if index_section.len() < DICT_HEADER_SIZE {
            return Err(CoveError::BufferTooShort);
        }
        let header = FileDictionaryHeaderV1::parse(&index_section[..DICT_HEADER_SIZE])?;
        let entries_bytes = usize::try_from(header.entry_count)
            .map_err(|_| CoveError::ArithOverflow)?
            .checked_mul(header.index_entry_len as usize)
            .ok_or(CoveError::ArithOverflow)?;
        let expected = DICT_HEADER_SIZE
            .checked_add(entries_bytes)
            .ok_or(CoveError::ArithOverflow)?;
        if index_section.len() != expected {
            return Err(CoveError::BadSection(
                "dictionary index section length mismatch".into(),
            ));
        }
        if payload_section.len()
            != usize::try_from(header.payload_length).map_err(|_| CoveError::ArithOverflow)?
        {
            return Err(CoveError::BadSection(
                "dictionary payload section length mismatch".into(),
            ));
        }
        Ok(Self {
            header,
            index_section,
            payload_section,
        })
    }

    /// Parses a lazy dictionary view over borrowed dictionary sections.
    pub fn borrowed(index_section: &'a [u8], payload_section: &'a [u8]) -> Result<Self, CoveError> {
        Self::parse(Cow::Borrowed(index_section), Cow::Borrowed(payload_section))
    }

    /// Returns the number of entries in the dictionary.
    pub fn len(&self) -> u32 {
        self.header.entry_count
    }

    /// Returns `true` if the dictionary contains no entries.
    pub fn is_empty(&self) -> bool {
        self.header.entry_count == 0
    }

    /// Returns the raw dictionary payload section.
    pub fn payload(&self) -> &[u8] {
        &self.payload_section
    }

    /// Returns the parsed index entry for `file_code`.
    pub fn get_entry(&self, file_code: u32) -> Result<FileDictionaryIndexEntryV1, CoveError> {
        if file_code >= self.header.entry_count {
            return Err(CoveError::BadFileCode);
        }
        let i = usize::try_from(file_code).map_err(|_| CoveError::ArithOverflow)?;
        let off = DICT_HEADER_SIZE
            .checked_add(
                i.checked_mul(DICT_INDEX_ENTRY_SIZE)
                    .ok_or(CoveError::ArithOverflow)?,
            )
            .ok_or(CoveError::ArithOverflow)?;
        FileDictionaryIndexEntryV1::parse(&self.index_section[off..off + DICT_INDEX_ENTRY_SIZE])
    }

    /// Validates every dictionary entry and payload reference.
    pub fn validate_all(&self) -> Result<(), CoveError> {
        for file_code in 0..self.header.entry_count {
            let entry = self.get_entry(file_code)?;
            validate_dictionary_entry(&entry, self.payload(), self.header.payload_length)?;
        }
        Ok(())
    }

    /// Resolves the raw value bytes for a given `file_code`.
    pub fn decode_value(&self, file_code: u32) -> Result<DictionaryValue, CoveError> {
        let entry = self.get_entry(file_code)?;
        decode_dictionary_entry(&entry, self.payload())
    }
}

/// A fully parsed file dictionary, combining the index section and payload
/// section into a queryable structure.
///
/// Corresponds to the `FILE_DICTIONARY_INDEX` and `FILE_DICTIONARY_PAYLOAD`
/// sections described in Section 16 of the COVE v2.0 specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDictionary {
    /// Parsed dictionary header.
    pub header: FileDictionaryHeaderV1,
    /// All index entries, indexed by FileCode.
    pub entries: Vec<FileDictionaryIndexEntryV1>,
    /// Raw bytes of the payload section.
    pub payload: Vec<u8>,
}

impl FileDictionary {
    /// Parses a [`FileDictionary`] from the raw `index_section` and
    /// `payload_section` byte slices.
    ///
    /// Validates section lengths, entry storage-class constraints, and payload
    /// bounds for every entry.  Returns [`CoveError::BufferTooShort`] if the
    /// index section is too small, [`CoveError::BadSection`] on structural
    /// violations, and [`CoveError::ArithOverflow`] or [`CoveError::OffsetRange`]
    /// on arithmetic or bounds failures.
    pub fn parse(index_section: &[u8], payload_section: &[u8]) -> Result<Self, CoveError> {
        let view = FileDictionaryView::borrowed(index_section, payload_section)?;
        view.validate_all()?;
        let mut entries = Vec::with_capacity(view.header.entry_count as usize);
        for file_code in 0..view.header.entry_count {
            entries.push(view.get_entry(file_code)?);
        }
        Ok(Self {
            header: view.header,
            entries,
            payload: payload_section.to_vec(),
        })
    }

    /// Returns the number of entries in the dictionary.
    pub fn len(&self) -> u32 {
        self.header.entry_count
    }

    /// Returns `true` if the dictionary contains no entries.
    pub fn is_empty(&self) -> bool {
        self.header.entry_count == 0
    }

    /// Returns the index entry for the given `file_code`, or
    /// [`CoveError::BadFileCode`] if it is out of range.
    pub fn get_entry(&self, file_code: u32) -> Result<&FileDictionaryIndexEntryV1, CoveError> {
        self.entries
            .get(file_code as usize)
            .ok_or(CoveError::BadFileCode)
    }

    /// Resolves the raw value bytes for a given `file_code`.
    ///
    /// For inline entries the bytes are copied from the `inline_data` field;
    /// for payload entries they are read from the payload section using checked
    /// arithmetic.  Redacted entries return [`DictionaryValue::RedactedPresent`]
    /// without exposing any bytes.
    pub fn decode_value(&self, file_code: u32) -> Result<DictionaryValue, CoveError> {
        let entry = self.get_entry(file_code)?;
        decode_dictionary_entry(entry, &self.payload)
    }
}

/// Canonical key used when synthesising a file dictionary from logical values.
///
/// INVARIANT: ordering is over `(value_tag, canonical_bytes)`, so generated
/// FileCodes are deterministic and independent of source row order or source
/// engine dictionary codes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileDictionaryKey {
    pub value_tag: u16,
    pub canonical: Vec<u8>,
}

impl FileDictionaryKey {
    /// Build a dictionary key for COVE-Core's repeated byte-compatible
    /// dictionary path.
    pub fn from_logical_bytes(
        logical: crate::constants::CoveLogicalType,
        value: &[u8],
    ) -> Result<Self, CoveError> {
        let canonical_value = match logical {
            crate::constants::CoveLogicalType::Utf8 => {
                CanonicalValue::Utf8(std::str::from_utf8(value).map_err(|error| {
                    CoveError::BadSection(format!("invalid UTF-8 dictionary value: {error}"))
                })?)
            }
            crate::constants::CoveLogicalType::Binary => CanonicalValue::Bytes(value),
            other => {
                return Err(CoveError::BadSchema(format!(
                    "logical type {other:?} is not eligible for FileCode dictionary synthesis"
                )));
            }
        };
        Ok(Self {
            value_tag: canonical_value.value_tag() as u16,
            canonical: canonical_value.encode()?,
        })
    }
}

/// Deterministic file dictionary plus reverse assignments for writer-side
/// FileCode page synthesis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDictionaryEncoding {
    pub dictionary: FileDictionary,
    assignments: BTreeMap<FileDictionaryKey, u32>,
}

impl FileDictionaryEncoding {
    /// Build a validated file dictionary from an arbitrary set of canonical
    /// keys. Duplicate keys are collapsed before FileCodes are assigned.
    pub fn from_keys<I>(keys: I) -> Result<Self, CoveError>
    where
        I: IntoIterator<Item = FileDictionaryKey>,
    {
        let unique = keys.into_iter().collect::<BTreeSet<_>>();
        let mut assignments = BTreeMap::new();
        for (code, key) in unique.into_iter().enumerate() {
            let code = u32::try_from(code).map_err(|_| {
                CoveError::BadSchema("dictionary entry count exceeds u32::MAX".into())
            })?;
            assignments.insert(key, code);
        }
        let dictionary = build_file_dictionary(&assignments)?;
        Ok(Self {
            dictionary,
            assignments,
        })
    }

    pub fn file_code_for_key(&self, key: &FileDictionaryKey) -> Result<u32, CoveError> {
        self.assignments
            .get(key)
            .copied()
            .ok_or(CoveError::BadFileCode)
    }

    pub fn file_code_for_logical_bytes(
        &self,
        logical: crate::constants::CoveLogicalType,
        value: &[u8],
    ) -> Result<u32, CoveError> {
        let key = FileDictionaryKey::from_logical_bytes(logical, value)?;
        self.file_code_for_key(&key)
    }

    pub fn assignments(&self) -> &BTreeMap<FileDictionaryKey, u32> {
        &self.assignments
    }
}

/// Estimate the stored bytes for a FileCode dictionary candidate.
///
/// This intentionally models the v1 file dictionary index/payload plus one
/// little-endian `u32` key per logical row. It does not include section header
/// or compression effects, so callers should use it as a conservative local
/// choice against plain page payload bytes.
pub fn file_dictionary_candidate_len(
    unique: &BTreeSet<FileDictionaryKey>,
    row_count: usize,
) -> Result<usize, CoveError> {
    let index_len = unique
        .len()
        .checked_mul(DICT_INDEX_ENTRY_SIZE)
        .ok_or(CoveError::ArithOverflow)?;
    let key_len = row_count.checked_mul(4).ok_or(CoveError::ArithOverflow)?;
    let payload_len = unique.iter().try_fold(0usize, |total, key| {
        if key.canonical.len() <= 16 {
            Ok(total)
        } else {
            total
                .checked_add(key.canonical.len())
                .ok_or(CoveError::ArithOverflow)
        }
    })?;
    DICT_HEADER_SIZE
        .checked_add(index_len)
        .and_then(|total| total.checked_add(payload_len))
        .and_then(|total| total.checked_add(key_len))
        .ok_or(CoveError::ArithOverflow)
}

fn build_file_dictionary(
    assignments: &BTreeMap<FileDictionaryKey, u32>,
) -> Result<FileDictionary, CoveError> {
    let entry_count = u32::try_from(assignments.len())
        .map_err(|_| CoveError::BadSchema("dictionary entry count exceeds u32::MAX".into()))?;
    let mut entries = Vec::with_capacity(assignments.len());
    let mut payload = Vec::new();

    for (expected_code, (key, assigned_code)) in assignments.iter().enumerate() {
        if *assigned_code != u32::try_from(expected_code).map_err(|_| CoveError::ArithOverflow)? {
            return Err(CoveError::BadFileCode);
        }
        let mut inline_data = [0u8; 16];
        let (storage_class, inline_len, payload_offset, payload_length) =
            if key.canonical.len() <= 16 {
                inline_data[..key.canonical.len()].copy_from_slice(&key.canonical);
                (StorageClass::Inline as u8, key.canonical.len() as u8, 0, 0)
            } else {
                let offset = u64::try_from(payload.len()).map_err(|_| CoveError::ArithOverflow)?;
                let length =
                    u32::try_from(key.canonical.len()).map_err(|_| CoveError::ArithOverflow)?;
                payload.extend_from_slice(&key.canonical);
                (StorageClass::Payload as u8, 0, offset, length)
            };
        entries.push(FileDictionaryIndexEntryV1 {
            value_tag: key.value_tag,
            storage_class,
            flags: 0,
            inline_len,
            reserved0: [0; 3],
            inline_data,
            payload_offset,
            payload_length,
            canonical_hash64: 0,
            reserved1: 0,
        });
    }

    let dictionary = FileDictionary {
        header: FileDictionaryHeaderV1 {
            entry_count,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: payload.len() as u64,
            reserved: [0; 24],
        },
        entries,
        payload,
    };
    let mut index = Vec::new();
    index.extend_from_slice(&dictionary.header.serialize());
    for entry in &dictionary.entries {
        index.extend_from_slice(&entry.serialize());
    }
    FileDictionary::parse(&index, &dictionary.payload)?;
    Ok(dictionary)
}

fn validate_dictionary_entry(
    entry: &FileDictionaryIndexEntryV1,
    payload_section: &[u8],
    payload_total_len: u64,
) -> Result<(), CoveError> {
    entry.validate_payload_bounds(payload_total_len)?;
    match StorageClass::from_u8(entry.storage_class)
        .ok_or_else(|| CoveError::BadSection("invalid storage class".into()))?
    {
        StorageClass::Inline => {
            if entry.payload_length != 0 {
                return Err(CoveError::BadSection(
                    "inline storage must not use payload bytes".into(),
                ));
            }
            let bytes = &entry.inline_data[..entry.inline_len as usize];
            validate_canonical_value_bytes(
                ValueTag::from_u16(entry.value_tag)
                    .ok_or_else(|| CoveError::BadSection("invalid value tag".into()))?,
                bytes,
            )?;
        }
        StorageClass::Payload => {
            if entry.inline_len != 0 {
                return Err(CoveError::BadSection(
                    "payload storage must not use inline bytes".into(),
                ));
            }
            let start =
                usize::try_from(entry.payload_offset).map_err(|_| CoveError::ArithOverflow)?;
            let bytes = crate::wire::read_range_checked(
                payload_section,
                start,
                entry.payload_length as usize,
            )?;
            validate_canonical_value_bytes(
                ValueTag::from_u16(entry.value_tag)
                    .ok_or_else(|| CoveError::BadSection("invalid value tag".into()))?,
                bytes,
            )?;
        }
        StorageClass::Redacted => {
            if ValueTag::from_u16(entry.value_tag) == Some(ValueTag::Null) {
                return Err(CoveError::BadSection(
                    "redacted value must not be null".into(),
                ));
            }
        }
    }
    Ok(())
}

fn decode_dictionary_entry(
    entry: &FileDictionaryIndexEntryV1,
    payload_section: &[u8],
) -> Result<DictionaryValue, CoveError> {
    match StorageClass::from_u8(entry.storage_class)
        .ok_or_else(|| CoveError::BadSection("invalid storage class".into()))?
    {
        StorageClass::Redacted => Ok(DictionaryValue::RedactedPresent),
        StorageClass::Inline => Ok(DictionaryValue::RawBytes(
            entry.inline_data[..entry.inline_len as usize].to_vec(),
        )),
        StorageClass::Payload => {
            let start =
                usize::try_from(entry.payload_offset).map_err(|_| CoveError::ArithOverflow)?;
            let payload = crate::wire::read_range_checked(
                payload_section,
                start,
                entry.payload_length as usize,
            )?;
            Ok(DictionaryValue::RawBytes(payload.to_vec()))
        }
    }
}

fn validate_canonical_value_bytes(value_tag: ValueTag, bytes: &[u8]) -> Result<(), CoveError> {
    canonical::validate_canonical_payload(value_tag, bytes)
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
            Err(CoveError::ReservedNotZero)
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
            Err(CoveError::BadSection(_))
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
            Err(CoveError::BadSection(_))
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
            Err(CoveError::BadSection(_))
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
            Err(CoveError::ReservedNotZero)
        );
    }

    #[test]
    fn file_dictionary_inline_raw_bytes_and_filecode_zero_works() {
        let mut inline = [0u8; 16];
        inline[..4].copy_from_slice(&[3, b'a', b'b', b'c']);
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: 4,
            reserved0: [0; 3],
            inline_data: inline,
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let hdr = FileDictionaryHeaderV1 {
            entry_count: 1,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        };
        let mut idx = Vec::new();
        idx.extend_from_slice(&hdr.serialize());
        idx.extend_from_slice(&entry.serialize());
        let dict = FileDictionary::parse(&idx, &[]).unwrap();
        assert_eq!(dict.len(), 1);
        assert_eq!(
            dict.decode_value(0).unwrap(),
            DictionaryValue::RawBytes(vec![3, b'a', b'b', b'c'])
        );
    }

    #[test]
    fn dictionary_encoding_assigns_deterministic_filecodes() {
        let mut keys = BTreeSet::new();
        keys.insert(
            FileDictionaryKey::from_logical_bytes(
                crate::constants::CoveLogicalType::Utf8,
                b"bravo",
            )
            .unwrap(),
        );
        keys.insert(
            FileDictionaryKey::from_logical_bytes(
                crate::constants::CoveLogicalType::Utf8,
                b"alpha",
            )
            .unwrap(),
        );
        keys.insert(
            FileDictionaryKey::from_logical_bytes(
                crate::constants::CoveLogicalType::Utf8,
                b"alpha",
            )
            .unwrap(),
        );

        let encoding = FileDictionaryEncoding::from_keys(keys).unwrap();
        assert_eq!(encoding.dictionary.len(), 2);
        let alpha = FileDictionaryKey::from_logical_bytes(
            crate::constants::CoveLogicalType::Utf8,
            b"alpha",
        )
        .unwrap();
        let bravo = FileDictionaryKey::from_logical_bytes(
            crate::constants::CoveLogicalType::Utf8,
            b"bravo",
        )
        .unwrap();
        assert_eq!(encoding.file_code_for_key(&alpha).unwrap(), 0);
        assert_eq!(encoding.file_code_for_key(&bravo).unwrap(), 1);
        assert_eq!(
            encoding.dictionary.decode_value(0).unwrap(),
            DictionaryValue::RawBytes(alpha.canonical)
        );
    }

    #[test]
    fn dictionary_candidate_len_counts_keys_payload_and_codes() {
        let keys = [
            FileDictionaryKey::from_logical_bytes(crate::constants::CoveLogicalType::Utf8, b"a")
                .unwrap(),
            FileDictionaryKey::from_logical_bytes(
                crate::constants::CoveLogicalType::Utf8,
                b"this-value-is-longer-than-inline",
            )
            .unwrap(),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        let expected_payload = keys
            .iter()
            .filter(|key| key.canonical.len() > 16)
            .map(|key| key.canonical.len())
            .sum::<usize>();
        assert_eq!(
            file_dictionary_candidate_len(&keys, 10).unwrap(),
            DICT_HEADER_SIZE + 2 * DICT_INDEX_ENTRY_SIZE + expected_payload + 10 * 4
        );
    }

    #[test]
    fn dictionary_key_rejects_invalid_utf8() {
        assert!(matches!(
            FileDictionaryKey::from_logical_bytes(crate::constants::CoveLogicalType::Utf8, &[0xff]),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn file_dictionary_payload_raw_bytes_returned() {
        let payload = vec![4u8, 1, 2, 3, 4];
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Binary as u16,
            storage_class: StorageClass::Payload as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0; 16],
            payload_offset: 0,
            payload_length: 5,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let hdr = FileDictionaryHeaderV1 {
            entry_count: 1,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 5,
            reserved: [0; 24],
        };
        let mut idx = Vec::new();
        idx.extend_from_slice(&hdr.serialize());
        idx.extend_from_slice(&entry.serialize());
        let dict = FileDictionary::parse(&idx, &payload).unwrap();
        assert_eq!(
            dict.decode_value(0).unwrap(),
            DictionaryValue::RawBytes(vec![4, 1, 2, 3, 4])
        );
    }

    #[test]
    fn file_dictionary_out_of_range_filecode_rejected() {
        let hdr = FileDictionaryHeaderV1 {
            entry_count: 0,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        };
        let idx = hdr.serialize().to_vec();
        let dict = FileDictionary::parse(&idx, &[]).unwrap();
        assert_eq!(dict.get_entry(0), Err(CoveError::BadFileCode));
    }

    #[test]
    fn file_dictionary_redacted_returns_present_not_null() {
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Redacted as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0; 16],
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let hdr = FileDictionaryHeaderV1 {
            entry_count: 1,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        };
        let mut idx = Vec::new();
        idx.extend_from_slice(&hdr.serialize());
        idx.extend_from_slice(&entry.serialize());
        let dict = FileDictionary::parse(&idx, &[]).unwrap();
        assert_eq!(
            dict.decode_value(0).unwrap(),
            DictionaryValue::RedactedPresent
        );
    }

    #[test]
    fn file_dictionary_view_decodes_on_demand_but_full_validation_checks_all_entries() {
        let mut inline_data = [0u8; 16];
        inline_data[..2].copy_from_slice(b"ok");
        let good_entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: 2,
            reserved0: [0; 3],
            inline_data,
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let bad_later_entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0; 16],
            payload_offset: 0,
            payload_length: 1,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let header = FileDictionaryHeaderV1 {
            entry_count: 2,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        };
        let mut index = Vec::new();
        index.extend_from_slice(&header.serialize());
        index.extend_from_slice(&good_entry.serialize());
        index.extend_from_slice(&bad_later_entry.serialize());

        let view = FileDictionaryView::borrowed(&index, &[]).unwrap();
        assert_eq!(
            view.decode_value(0).unwrap(),
            DictionaryValue::RawBytes(b"ok".to_vec())
        );
        assert!(view.validate_all().is_err());
    }

    #[test]
    fn file_dictionary_payload_overflow_rejected() {
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Binary as u16,
            storage_class: StorageClass::Payload as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0; 16],
            payload_offset: 1,
            payload_length: 4,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let hdr = FileDictionaryHeaderV1 {
            entry_count: 1,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 4,
            reserved: [0; 24],
        };
        let mut idx = Vec::new();
        idx.extend_from_slice(&hdr.serialize());
        idx.extend_from_slice(&entry.serialize());
        assert_eq!(
            FileDictionary::parse(&idx, &[1, 2, 3, 4]),
            Err(CoveError::OffsetRange)
        );
    }

    #[test]
    fn canonical_utf8_payload_must_be_valid_utf8() {
        let mut entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: 1,
            reserved0: [0; 3],
            inline_data: [0; 16],
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        entry.inline_data[0] = 0xff;
        let header = FileDictionaryHeaderV1 {
            entry_count: 1,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        };
        let mut index = header.serialize().to_vec();
        index.extend_from_slice(&entry.serialize());
        assert!(matches!(
            FileDictionary::parse(&index, &[]),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn canonical_json_payload_must_be_valid_json() {
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Json as u16,
            storage_class: StorageClass::Payload as u8,
            flags: 0,
            inline_len: 0,
            reserved0: [0; 3],
            inline_data: [0; 16],
            payload_offset: 0,
            payload_length: 3,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let header = FileDictionaryHeaderV1 {
            entry_count: 1,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 4,
            reserved: [0; 24],
        };
        let mut index = header.serialize().to_vec();
        index.extend_from_slice(&entry.serialize());
        assert!(matches!(
            FileDictionary::parse(&index, &[3, b'{', b'x', b'}']),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn canonical_date_days_payload_uses_4_bytes() {
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::DateDays as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: 4,
            reserved0: [0; 3],
            inline_data: {
                let mut inline = [0u8; 16];
                inline[..4].copy_from_slice(&12i32.to_le_bytes());
                inline
            },
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let header = FileDictionaryHeaderV1 {
            entry_count: 1,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        };
        let mut index = header.serialize().to_vec();
        index.extend_from_slice(&entry.serialize());
        FileDictionary::parse(&index, &[]).unwrap();
    }
}
