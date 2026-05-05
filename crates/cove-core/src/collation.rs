//! Cove Format (COVE) v1.0 — Collation registry (Spec §22).
//!
//! Every COVE v1 reader MUST recognise the six minimum collations defined by the
//! spec. Each collation has a stable name and a deterministic comparison rule.
//! Comparisons are total orders so they can drive ColumnDomain rank maps,
//! min/max statistics, and ordered indexes safely.

use crate::CoveError;

/// Total order produced by comparing two values under a collation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ordering3 {
    Less,
    Equal,
    Greater,
}

impl From<std::cmp::Ordering> for Ordering3 {
    fn from(o: std::cmp::Ordering) -> Self {
        match o {
            std::cmp::Ordering::Less => Self::Less,
            std::cmp::Ordering::Equal => Self::Equal,
            std::cmp::Ordering::Greater => Self::Greater,
        }
    }
}

/// One of the six v1 collations, plus `None` for unspecified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollationKind {
    /// `none` — equality only, no ordering allowed.
    None,
    /// `utf8-bytewise` — byte-by-byte comparison of UTF-8 encoded text.
    Utf8Bytewise,
    /// `unsigned-fixed-bytes` — lexicographic on fixed-width unsigned bytes.
    UnsignedFixedBytes,
    /// `signed-numeric` — two's-complement signed integers.
    SignedNumeric,
    /// `unsigned-numeric` — unsigned integers.
    UnsignedNumeric,
    /// `timestamp-chronological` — chronological ordering on timestamps.
    TimestampChronological,
}

impl CollationKind {
    /// Stable spec name for this collation.
    pub const fn name(self) -> &'static str {
        match self {
            CollationKind::None => "none",
            CollationKind::Utf8Bytewise => "utf8-bytewise",
            CollationKind::UnsignedFixedBytes => "unsigned-fixed-bytes",
            CollationKind::SignedNumeric => "signed-numeric",
            CollationKind::UnsignedNumeric => "unsigned-numeric",
            CollationKind::TimestampChronological => "timestamp-chronological",
        }
    }

    /// Look up a collation by its spec name.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "none" => CollationKind::None,
            "utf8-bytewise" => CollationKind::Utf8Bytewise,
            "unsigned-fixed-bytes" => CollationKind::UnsignedFixedBytes,
            "signed-numeric" => CollationKind::SignedNumeric,
            "unsigned-numeric" => CollationKind::UnsignedNumeric,
            "timestamp-chronological" => CollationKind::TimestampChronological,
            _ => return None,
        })
    }

    /// Whether this collation defines a total order (i.e. supports min/max,
    /// ColumnDomain ranks, and ordered indexes). `None` does not.
    pub const fn supports_ordering(self) -> bool {
        !matches!(self, CollationKind::None)
    }

    /// Compare two values under this collation. Returns `BadStats` when the
    /// inputs do not match the expected width for the collation.
    pub fn compare(self, lhs: &[u8], rhs: &[u8]) -> Result<Ordering3, CoveError> {
        match self {
            CollationKind::None => {
                // Equality only; ordering is not defined.
                if lhs == rhs {
                    Ok(Ordering3::Equal)
                } else {
                    Err(CoveError::BadStats)
                }
            }
            CollationKind::Utf8Bytewise | CollationKind::UnsignedFixedBytes => {
                Ok(lhs.cmp(rhs).into())
            }
            CollationKind::UnsignedNumeric => unsigned_numeric(lhs, rhs),
            CollationKind::SignedNumeric => signed_numeric(lhs, rhs),
            CollationKind::TimestampChronological => signed_numeric(lhs, rhs),
        }
    }
}

fn check_same_len(lhs: &[u8], rhs: &[u8]) -> Result<(), CoveError> {
    if lhs.len() != rhs.len() || !matches!(lhs.len(), 1 | 2 | 4 | 8 | 16) {
        Err(CoveError::BadStats)
    } else {
        Ok(())
    }
}

fn unsigned_numeric(lhs: &[u8], rhs: &[u8]) -> Result<Ordering3, CoveError> {
    check_same_len(lhs, rhs)?;
    Ok(read_unsigned(lhs).cmp(&read_unsigned(rhs)).into())
}

fn signed_numeric(lhs: &[u8], rhs: &[u8]) -> Result<Ordering3, CoveError> {
    check_same_len(lhs, rhs)?;
    Ok(read_signed(lhs).cmp(&read_signed(rhs)).into())
}

fn read_unsigned(b: &[u8]) -> u128 {
    let mut buf = [0u8; 16];
    let n = b.len();
    buf[..n].copy_from_slice(b);
    u128::from_le_bytes(buf)
}

fn read_signed(b: &[u8]) -> i128 {
    match b.len() {
        1 => i8::from_le_bytes(b.try_into().unwrap()) as i128,
        2 => i16::from_le_bytes(b.try_into().unwrap()) as i128,
        4 => i32::from_le_bytes(b.try_into().unwrap()) as i128,
        8 => i64::from_le_bytes(b.try_into().unwrap()) as i128,
        16 => i128::from_le_bytes(b.try_into().unwrap()),
        _ => 0,
    }
}

/// The six v1 collations enumerated in spec order.
pub const V1_COLLATIONS: &[CollationKind] = &[
    CollationKind::None,
    CollationKind::Utf8Bytewise,
    CollationKind::UnsignedFixedBytes,
    CollationKind::SignedNumeric,
    CollationKind::UnsignedNumeric,
    CollationKind::TimestampChronological,
];

/// A collation entry, mapping a column or domain to a named collation.
#[derive(Debug, Clone)]
pub struct CollationEntry {
    /// Collation name (e.g. "utf8-bytewise").
    pub name: String,
    /// Optional vendor-defined metadata bytes.
    pub metadata: Vec<u8>,
    /// Resolved kind, if it matches a v1 collation.
    pub kind: Option<CollationKind>,
}

/// A parsed collation registry section.
#[derive(Debug, Clone, Default)]
pub struct CollationRegistry {
    pub entries: Vec<CollationEntry>,
}

impl CollationRegistry {
    /// Parse a collation registry section.
    ///
    /// Wire format: `u32` LE entry count, then entries of:
    /// `u16` LE `name_len`, name bytes (UTF-8), `u16` LE `meta_len`, meta bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 4 {
            return Err(CoveError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let mut entries = Vec::with_capacity(entry_count as usize);

        for _ in 0..entry_count {
            if pos + 2 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let name_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            let name_end = pos.checked_add(name_len).ok_or(CoveError::ArithOverflow)?;
            if name_end > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let name = std::str::from_utf8(&bytes[pos..name_end])
                .map_err(|_| CoveError::BadSection("collation name is not valid UTF-8".into()))?
                .to_string();
            pos = name_end;

            if pos + 2 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let meta_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            let meta_end = pos.checked_add(meta_len).ok_or(CoveError::ArithOverflow)?;
            if meta_end > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let metadata = bytes[pos..meta_end].to_vec();
            pos = meta_end;

            let kind = CollationKind::from_name(&name);
            entries.push(CollationEntry {
                name,
                metadata,
                kind,
            });
        }
        Ok(Self { entries })
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.entries.len() * 8);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            let name_bytes = entry.name.as_bytes();
            out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(&(entry.metadata.len() as u16).to_le_bytes());
            out.extend_from_slice(&entry.metadata);
        }
        out
    }

    /// Returns true if every named collation is one of the six v1 collations.
    pub fn all_known(&self) -> bool {
        self.entries.iter().all(|e| e.kind.is_some())
    }

    /// Whether a given collation name is recognised by this v1 reader.
    pub fn is_known_collation(name: &str) -> bool {
        CollationKind::from_name(name).is_some()
    }
}

#[cfg(test)]
mod serialize_tests {
    use super::*;

    #[test]
    fn serialize_round_trip() {
        let reg = CollationRegistry {
            entries: vec![
                CollationEntry {
                    name: "utf8-bytewise".into(),
                    metadata: vec![],
                    kind: Some(CollationKind::Utf8Bytewise),
                },
                CollationEntry {
                    name: "vendor-x".into(),
                    metadata: vec![1, 2, 3, 4],
                    kind: None,
                },
            ],
        };
        let bytes = reg.serialize();
        let back = CollationRegistry::parse(&bytes).unwrap();
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.entries[0].name, "utf8-bytewise");
        assert_eq!(back.entries[1].metadata, vec![1, 2, 3, 4]);
    }

    #[test]
    fn serialize_empty() {
        let reg = CollationRegistry::default();
        let bytes = reg.serialize();
        assert_eq!(bytes, vec![0u8; 4]);
        assert!(CollationRegistry::parse(&bytes).unwrap().entries.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = (entries.len() as u32).to_le_bytes().to_vec();
        for (name, meta) in entries {
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&(meta.len() as u16).to_le_bytes());
            out.extend_from_slice(meta);
        }
        out
    }

    #[test]
    fn empty_registry_parses() {
        let reg = CollationRegistry::parse(&make_registry_bytes(&[])).unwrap();
        assert_eq!(reg.entries.len(), 0);
        assert!(reg.all_known());
    }

    #[test]
    fn spec_22_v1_collations_all_resolve() {
        for c in V1_COLLATIONS {
            assert_eq!(CollationKind::from_name(c.name()), Some(*c));
        }
    }

    #[test]
    fn spec_22_none_supports_only_equality() {
        let none = CollationKind::None;
        assert!(!none.supports_ordering());
        assert_eq!(none.compare(b"a", b"a"), Ok(Ordering3::Equal));
        assert!(matches!(none.compare(b"a", b"b"), Err(CoveError::BadStats)));
    }

    #[test]
    fn utf8_bytewise_orders_strings_lexicographically() {
        let c = CollationKind::Utf8Bytewise;
        assert_eq!(c.compare(b"abc", b"abd"), Ok(Ordering3::Less));
        assert_eq!(c.compare(b"abc", b"abc"), Ok(Ordering3::Equal));
    }

    #[test]
    fn unsigned_fixed_bytes_orders_by_byte_lex() {
        let c = CollationKind::UnsignedFixedBytes;
        assert_eq!(
            c.compare(&[0x01, 0x00], &[0x00, 0xff]),
            Ok(Ordering3::Greater)
        );
    }

    #[test]
    fn signed_numeric_handles_negative_values() {
        let c = CollationKind::SignedNumeric;
        // -1i32 vs 0i32: -1 < 0
        let neg_one = (-1i32).to_le_bytes();
        let zero = 0i32.to_le_bytes();
        assert_eq!(c.compare(&neg_one, &zero), Ok(Ordering3::Less));
    }

    #[test]
    fn unsigned_numeric_orders_by_value_not_bytes() {
        let c = CollationKind::UnsignedNumeric;
        // 0x0001 < 0x0100 numerically but byte-lex says 0x01 0x00 > 0x00 0x01
        let small = 1u16.to_le_bytes();
        let big = 256u16.to_le_bytes();
        assert_eq!(c.compare(&small, &big), Ok(Ordering3::Less));
    }

    #[test]
    fn timestamp_chronological_uses_signed_compare() {
        let c = CollationKind::TimestampChronological;
        let earlier = (-100i64).to_le_bytes();
        let later = 100i64.to_le_bytes();
        assert_eq!(c.compare(&earlier, &later), Ok(Ordering3::Less));
    }

    #[test]
    fn known_collation_check() {
        assert!(CollationRegistry::is_known_collation("utf8-bytewise"));
        assert!(!CollationRegistry::is_known_collation("utf8-icu"));
    }

    #[test]
    fn registry_with_v1_entries_resolves_all() {
        let bytes = make_registry_bytes(&[("utf8-bytewise", b""), ("signed-numeric", b"")]);
        let reg = CollationRegistry::parse(&bytes).unwrap();
        assert_eq!(reg.entries.len(), 2);
        assert!(reg.all_known());
    }

    #[test]
    fn registry_with_unknown_entry_is_not_all_known() {
        let bytes = make_registry_bytes(&[("vendor-magic", b"")]);
        let reg = CollationRegistry::parse(&bytes).unwrap();
        assert!(!reg.all_known());
    }
}
