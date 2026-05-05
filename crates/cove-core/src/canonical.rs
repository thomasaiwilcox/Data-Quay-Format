//! Cove Format (COVE) v1.0 — Canonical value encoding (Spec §17).
//!
//! The canonical encoding is the byte-level identity of a value in COVE. Two
//! values are equal in a dictionary if and only if their `(value_tag,
//! canonical_bytes)` pairs are equal (Spec §6.6, §16). All scalar canonical
//! encodings are little-endian and length-prefixed when variable. Nested
//! canonical encoding is defined recursively for `List`, `Struct`, and `Map`.

use crate::{constants::ValueTag, wire, CoveError};

#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalField<'a> {
    pub field_id: u64,
    pub value: CanonicalValue<'a>,
}

/// A logical canonical value, suitable for dictionary equality and trust-chain
/// hashing (Spec §63).
#[derive(Debug, Clone, PartialEq)]
pub enum CanonicalValue<'a> {
    /// Empty payload.
    Null,
    /// Tag-only payload per Spec §17.
    Bool(bool),
    /// Signed integers canonicalize to the Int64 tag and an i64 payload.
    Int {
        width: u8,
        value: i128,
    },
    /// Unsigned integers canonicalize to the UInt64 tag and a u64 payload.
    Uint {
        width: u8,
        value: u128,
    },
    /// IEEE 754 float, raw bits LE.
    Float32(f32),
    Float64(f64),
    Decimal64(i64),
    Decimal128(i128),
    DateDays(i32),
    TimestampMicros(i64),
    TimestampNanos(i64),
    Uuid([u8; 16]),
    Utf8(&'a str),
    /// Variable-length opaque bytes.
    Bytes(&'a [u8]),
    Json(&'a str),
    List(Vec<CanonicalValue<'a>>),
    Struct(Vec<CanonicalField<'a>>),
    Map(Vec<(CanonicalValue<'a>, CanonicalValue<'a>)>),
}

impl<'a> CanonicalValue<'a> {
    /// Tag describing the value's logical kind (used for dictionary equality
    /// per Spec §16). Smaller integer widths share the [`ValueTag::Int64`] /
    /// [`ValueTag::UInt64`] tags after sign-extension.
    pub fn value_tag(&self) -> ValueTag {
        match self {
            CanonicalValue::Null => ValueTag::Null,
            CanonicalValue::Bool(false) => ValueTag::BoolFalse,
            CanonicalValue::Bool(true) => ValueTag::BoolTrue,
            CanonicalValue::Int { .. } => ValueTag::Int64,
            CanonicalValue::Uint { .. } => ValueTag::UInt64,
            CanonicalValue::Float32(_) => ValueTag::Float32Bits,
            CanonicalValue::Float64(_) => ValueTag::Float64Bits,
            CanonicalValue::Decimal64(_) => ValueTag::Decimal64,
            CanonicalValue::Decimal128(_) => ValueTag::Decimal128,
            CanonicalValue::DateDays(_) => ValueTag::DateDays,
            CanonicalValue::TimestampMicros(_) => ValueTag::TimestampMicros,
            CanonicalValue::TimestampNanos(_) => ValueTag::TimestampNanos,
            CanonicalValue::Uuid(_) => ValueTag::Uuid,
            CanonicalValue::Utf8(_) => ValueTag::Utf8,
            CanonicalValue::Bytes(_) => ValueTag::Binary,
            CanonicalValue::Json(_) => ValueTag::Json,
            CanonicalValue::List(_) => ValueTag::List,
            CanonicalValue::Struct(_) => ValueTag::Struct,
            CanonicalValue::Map(_) => ValueTag::Map,
        }
    }

    pub fn is_scalar_key(&self) -> bool {
        !matches!(self, Self::List(_) | Self::Struct(_) | Self::Map(_))
    }

    /// Encode the value to its canonical byte form (Spec §17). The output is
    /// the exact representation hashed by the trust chain and used as
    /// dictionary equality input.
    pub fn encode(&self) -> Result<Vec<u8>, CoveError> {
        match self {
            CanonicalValue::Null | CanonicalValue::Bool(_) => Ok(Vec::new()),
            CanonicalValue::Int { width, value } => encode_i64_width(*width, *value),
            CanonicalValue::Uint { width, value } => encode_u64_width(*width, *value),
            CanonicalValue::Float32(v) => Ok(v.to_bits().to_le_bytes().to_vec()),
            CanonicalValue::Float64(v) => Ok(v.to_bits().to_le_bytes().to_vec()),
            CanonicalValue::Decimal64(v) => Ok(v.to_le_bytes().to_vec()),
            CanonicalValue::Decimal128(v) => Ok(v.to_le_bytes().to_vec()),
            CanonicalValue::DateDays(v) => Ok(v.to_le_bytes().to_vec()),
            CanonicalValue::TimestampMicros(v) | CanonicalValue::TimestampNanos(v) => {
                Ok(v.to_le_bytes().to_vec())
            }
            CanonicalValue::Uuid(uuid) => Ok(uuid.to_vec()),
            CanonicalValue::Utf8(s) | CanonicalValue::Json(s) => Ok(length_prefixed(s.as_bytes())),
            CanonicalValue::Bytes(b) => Ok(length_prefixed(b)),
            CanonicalValue::List(elements) => canonicalize_list(elements),
            CanonicalValue::Struct(fields) => canonicalize_struct(fields),
            CanonicalValue::Map(entries) => canonicalize_map(entries),
        }
    }
}

fn encode_i64_width(width: u8, value: i128) -> Result<Vec<u8>, CoveError> {
    let min = match width {
        1 => i8::MIN as i128,
        2 => i16::MIN as i128,
        4 => i32::MIN as i128,
        8 => i64::MIN as i128,
        _ => {
            return Err(CoveError::BadSection(format!(
                "invalid signed integer width {width}"
            )))
        }
    };
    let max = match width {
        1 => i8::MAX as i128,
        2 => i16::MAX as i128,
        4 => i32::MAX as i128,
        8 => i64::MAX as i128,
        _ => unreachable!(),
    };
    if !(min..=max).contains(&value) {
        return Err(CoveError::BadSection(
            "signed integer canonical value out of range".into(),
        ));
    }
    Ok((value as i64).to_le_bytes().to_vec())
}

fn encode_u64_width(width: u8, value: u128) -> Result<Vec<u8>, CoveError> {
    let max = match width {
        1 => u8::MAX as u128,
        2 => u16::MAX as u128,
        4 => u32::MAX as u128,
        8 => u64::MAX as u128,
        _ => {
            return Err(CoveError::BadSection(format!(
                "invalid unsigned integer width {width}"
            )))
        }
    };
    if value > max {
        return Err(CoveError::BadSection(
            "unsigned integer canonical value out of range".into(),
        ));
    }
    Ok((value as u64).to_le_bytes().to_vec())
}

fn length_prefixed(bytes: &[u8]) -> Vec<u8> {
    let mut out = wire::encode_u64_leb128(bytes.len() as u64);
    out.extend_from_slice(bytes);
    out
}

fn encode_tagged(value: &CanonicalValue<'_>) -> Result<Vec<u8>, CoveError> {
    let mut out = wire::encode_u64_leb128(value.value_tag() as u64);
    out.extend_from_slice(&value.encode()?);
    Ok(out)
}

/// Canonical representation of a map: a list of canonical key/value pairs.
///
/// Spec §17.6 requires:
/// 1. Keys are scalar canonical-typed (no nested key types).
/// 2. Duplicate keys are rejected.
/// 3. Pairs are emitted in ascending key order under the column collation.
pub fn canonicalize_map_entries(
    entries: &[(CanonicalValue<'_>, CanonicalValue<'_>)],
) -> Result<Vec<(Vec<u8>, Vec<u8>)>, CoveError> {
    let mut sorted = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        if !key.is_scalar_key() {
            return Err(CoveError::BadSchema(
                "canonical map key must be scalar".into(),
            ));
        }
        sorted.push((encode_tagged(key)?, encode_tagged(value)?));
    }
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for w in sorted.windows(2) {
        if w[0].0 == w[1].0 {
            return Err(CoveError::BadSchema(
                "duplicate key in canonical map encoding (Spec §17.6)".into(),
            ));
        }
    }
    Ok(sorted)
}

/// Canonical encoding of a list (Spec §17): varint element count followed by
/// each element's value tag and payload.
pub fn canonicalize_list(elements: &[CanonicalValue<'_>]) -> Result<Vec<u8>, CoveError> {
    let mut out = wire::encode_u64_leb128(elements.len() as u64);
    for e in elements {
        out.extend_from_slice(&encode_tagged(e)?);
    }
    Ok(out)
}

pub fn canonicalize_struct(fields: &[CanonicalField<'_>]) -> Result<Vec<u8>, CoveError> {
    let mut sorted = fields.to_vec();
    sorted.sort_by_key(|field| field.field_id);
    for pair in sorted.windows(2) {
        if pair[0].field_id == pair[1].field_id {
            return Err(CoveError::BadSchema(
                "duplicate field_id in canonical struct".into(),
            ));
        }
    }

    let mut out = wire::encode_u64_leb128(sorted.len() as u64);
    for field in &sorted {
        out.extend_from_slice(&wire::encode_u64_leb128(field.field_id));
        out.extend_from_slice(&encode_tagged(&field.value)?);
    }
    Ok(out)
}

pub fn canonicalize_map(
    entries: &[(CanonicalValue<'_>, CanonicalValue<'_>)],
) -> Result<Vec<u8>, CoveError> {
    let sorted = canonicalize_map_entries(entries)?;
    let mut out = wire::encode_u64_leb128(sorted.len() as u64);
    for (key, value) in sorted {
        out.extend_from_slice(&key);
        out.extend_from_slice(&value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_17_bool_has_no_payload() {
        assert_eq!(
            CanonicalValue::Bool(true).encode().unwrap(),
            Vec::<u8>::new()
        );
        assert_eq!(
            CanonicalValue::Bool(false).encode().unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn spec_17_signed_int_le_round_trip() {
        let v = CanonicalValue::Int {
            width: 4,
            value: -1,
        };
        assert_eq!(v.encode().unwrap(), (-1i64).to_le_bytes().to_vec());
    }

    #[test]
    fn spec_17_unsigned_int_le_round_trip() {
        let v = CanonicalValue::Uint {
            width: 8,
            value: 42,
        };
        assert_eq!(v.encode().unwrap(), 42u64.to_le_bytes().to_vec());
    }

    #[test]
    fn spec_17_float_uses_raw_bits() {
        let v = CanonicalValue::Float32(1.0_f32);
        assert_eq!(
            v.encode().unwrap(),
            1.0_f32.to_bits().to_le_bytes().to_vec()
        );
    }

    #[test]
    fn spec_17_utf8_is_varint_length_prefixed() {
        assert_eq!(
            CanonicalValue::Utf8("abc").encode().unwrap(),
            b"\x03abc".to_vec()
        );
    }

    #[test]
    fn spec_17_list_uses_tags_and_varint_count() {
        let l =
            canonicalize_list(&[CanonicalValue::Bool(true), CanonicalValue::Utf8("a")]).unwrap();
        assert_eq!(
            l,
            vec![2, ValueTag::BoolTrue as u8, ValueTag::Utf8 as u8, 1, b'a']
        );
    }

    #[test]
    fn spec_17_6_map_rejects_duplicate_keys() {
        let entries = vec![
            (CanonicalValue::Utf8("k"), CanonicalValue::Utf8("v1")),
            (CanonicalValue::Utf8("k"), CanonicalValue::Utf8("v2")),
        ];
        assert!(matches!(
            canonicalize_map_entries(&entries),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn spec_17_6_map_keys_emitted_in_sorted_order() {
        let entries = vec![
            (CanonicalValue::Utf8("b"), CanonicalValue::Utf8("1")),
            (CanonicalValue::Utf8("a"), CanonicalValue::Utf8("2")),
        ];
        let result = canonicalize_map_entries(&entries).unwrap();
        assert_eq!(result[0].0, vec![ValueTag::Utf8 as u8, 1, b'a']);
        assert_eq!(result[1].0, vec![ValueTag::Utf8 as u8, 1, b'b']);
    }

    #[test]
    fn spec_17_struct_fields_are_sorted_by_id() {
        let encoded = CanonicalValue::Struct(vec![
            CanonicalField {
                field_id: 7,
                value: CanonicalValue::Bool(false),
            },
            CanonicalField {
                field_id: 1,
                value: CanonicalValue::Int { width: 8, value: 9 },
            },
        ])
        .encode()
        .unwrap();
        assert_eq!(encoded[0], 2);
        assert_eq!(encoded[1], 1);
    }

    #[test]
    fn value_tag_is_consistent_with_kind() {
        assert_eq!(CanonicalValue::Bool(false).value_tag(), ValueTag::BoolFalse);
        assert_eq!(CanonicalValue::Utf8("x").value_tag(), ValueTag::Utf8);
        assert_eq!(
            CanonicalValue::Int { width: 4, value: 0 }.value_tag(),
            ValueTag::Int64
        );
    }
}
