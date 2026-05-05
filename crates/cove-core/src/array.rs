//! Cove Format (COVE) v1.0 — Encoded array decoding.
//!
//! This module provides access to individual rows within an encoded column
//! array, supporting the encodings described in Section 20 of the specification.

use crate::{
    constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
    dictionary::{DictionaryValue, FileDictionary},
    validity::ValidityBitmap,
    wire, CoveError,
};

/// A decoded value from an encoded array row.
#[derive(Debug, Clone, PartialEq)]
pub enum CoveArrayValue<'a> {
    /// The row is null.
    Null,
    /// Raw bytes (PlainFixed, Constant, VarBytes).
    Bytes(&'a [u8]),
    /// A decoded LEB128 varint (PlainVarint).
    Varint(u64),
    /// A raw FileCode before dictionary resolution.
    FileCode(u32),
    /// A resolved dictionary value.
    DictValue(DictionaryValue),
    /// A raw NumCode.
    NumCode(u64),
    /// The validity bit for this row (Validity-encoded columns).
    ValidityBit(bool),
}

/// A view over an encoded column array.
pub struct EncodedArray<'a> {
    /// Logical type of the column.
    pub logical: CoveLogicalType,
    /// Physical kind of the column.
    pub physical: CovePhysicalKind,
    /// Number of logical rows.
    pub row_count: u64,
    /// Encoding applied to the data buffer.
    pub encoding: CoveEncodingKind,
    /// Optional validity (null) bitmap.
    pub validity: Option<ValidityBitmap<'a>>,
    /// Raw encoded data bytes.
    pub data: &'a [u8],
    /// Optional file dictionary for FileCode resolution.
    pub dictionary: Option<&'a FileDictionary>,
}

impl<'a> EncodedArray<'a> {
    /// Constructs a new `EncodedArray`.
    pub fn new(
        logical: CoveLogicalType,
        physical: CovePhysicalKind,
        row_count: u64,
        encoding: CoveEncodingKind,
        validity: Option<ValidityBitmap<'a>>,
        data: &'a [u8],
        dictionary: Option<&'a FileDictionary>,
    ) -> Self {
        Self {
            logical,
            physical,
            row_count,
            encoding,
            validity,
            data,
            dictionary,
        }
    }

    /// Returns `true` if the given row is null.
    ///
    /// If no validity bitmap is present, all rows are non-null.
    ///
    /// # Errors
    ///
    /// Returns [`CoveError::OffsetRange`] if `row >= row_count`.
    pub fn is_null(&self, row: u64) -> Result<bool, CoveError> {
        if row >= self.row_count {
            return Err(CoveError::OffsetRange);
        }
        match &self.validity {
            Some(bm) => bm.is_null(row),
            None => Ok(false),
        }
    }

    /// Decodes the value at `row`.
    ///
    /// Returns [`CoveArrayValue::Null`] if the row is null.
    /// Returns [`CoveError::OffsetRange`] if `row >= row_count`.
    /// Returns [`CoveError::UnsupportedEncoding`] for encodings not yet implemented.
    pub fn decode_row(&self, row: u64) -> Result<CoveArrayValue<'_>, CoveError> {
        if row >= self.row_count {
            return Err(CoveError::OffsetRange);
        }
        if self.is_null(row)? {
            return Ok(CoveArrayValue::Null);
        }
        match self.encoding {
            CoveEncodingKind::Validity => {
                let byte_idx = (row / 8) as usize;
                let bit_idx = (row % 8) as u32;
                let byte = self
                    .data
                    .get(byte_idx)
                    .copied()
                    .ok_or(CoveError::OffsetRange)?;
                let bit = (byte >> bit_idx) & 1 == 1;
                Ok(CoveArrayValue::ValidityBit(bit))
            }
            CoveEncodingKind::Constant => {
                let w = logical_type_fixed_width(self.logical).ok_or_else(|| {
                    CoveError::UnsupportedEncoding(format!(
                        "Constant encoding requires fixed-width logical type, got {:?}",
                        self.logical
                    ))
                })?;
                let slice = wire::read_range_checked(self.data, 0, w)?;
                Ok(CoveArrayValue::Bytes(slice))
            }
            CoveEncodingKind::FileCode => {
                let code = read_u32_le(
                    self.data,
                    (row as usize)
                        .checked_mul(4)
                        .ok_or(CoveError::ArithOverflow)?,
                )?;
                match self.dictionary {
                    Some(dict) => {
                        let val = dict.decode_value(code)?;
                        Ok(CoveArrayValue::DictValue(val))
                    }
                    None => Ok(CoveArrayValue::FileCode(code)),
                }
            }
            CoveEncodingKind::NumCode => {
                let code = read_u64_le(
                    self.data,
                    (row as usize)
                        .checked_mul(8)
                        .ok_or(CoveError::ArithOverflow)?,
                )?;
                Ok(CoveArrayValue::NumCode(code))
            }
            CoveEncodingKind::PlainFixed => {
                let w = logical_type_fixed_width(self.logical).ok_or_else(|| {
                    CoveError::UnsupportedEncoding(format!(
                        "PlainFixed encoding requires fixed-width logical type, got {:?}",
                        self.logical
                    ))
                })?;
                let offset = (row as usize)
                    .checked_mul(w)
                    .ok_or(CoveError::ArithOverflow)?;
                let slice = wire::read_range_checked(self.data, offset, w)?;
                Ok(CoveArrayValue::Bytes(slice))
            }
            CoveEncodingKind::PlainVarint => {
                // O(n): variable-width encoding requires scanning all preceding rows.
                let mut pos = 0usize;
                for _ in 0..row {
                    if pos >= self.data.len() {
                        return Err(CoveError::OffsetRange);
                    }
                    let (_val, consumed) = wire::decode_u64_leb128(&self.data[pos..])?;
                    pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
                }
                if pos >= self.data.len() {
                    return Err(CoveError::OffsetRange);
                }
                let (val, _consumed) = wire::decode_u64_leb128(&self.data[pos..])?;
                Ok(CoveArrayValue::Varint(val))
            }
            CoveEncodingKind::VarBytes => {
                // O(n): variable-width encoding requires scanning all preceding rows.
                let mut pos = 0usize;
                for _ in 0..row {
                    let len = read_u32_le(self.data, pos)? as usize;
                    pos = pos
                        .checked_add(4)
                        .and_then(|p| p.checked_add(len))
                        .ok_or(CoveError::ArithOverflow)?;
                    if pos > self.data.len() {
                        return Err(CoveError::OffsetRange);
                    }
                }
                let len = read_u32_le(self.data, pos)? as usize;
                let data_start = pos.checked_add(4).ok_or(CoveError::ArithOverflow)?;
                let slice = wire::read_range_checked(self.data, data_start, len)?;
                Ok(CoveArrayValue::Bytes(slice))
            }
            CoveEncodingKind::LocalCodebook
            | CoveEncodingKind::Rle
            | CoveEncodingKind::RunEnd
            | CoveEncodingKind::BitPacked
            | CoveEncodingKind::Delta
            | CoveEncodingKind::FrameOfReference
            | CoveEncodingKind::PatchedBase
            | CoveEncodingKind::Sparse
            | CoveEncodingKind::Sequence
            | CoveEncodingKind::Lz4Block
            | CoveEncodingKind::ZstdBlock
            | CoveEncodingKind::Canonical => Err(CoveError::UnsupportedEncoding(format!(
                "{:?}",
                self.encoding
            ))),
        }
    }
}

/// Returns the fixed byte width for a logical type, or `None` for variable-width types.
pub fn logical_type_fixed_width(logical: CoveLogicalType) -> Option<usize> {
    match logical {
        CoveLogicalType::Bool | CoveLogicalType::Int8 | CoveLogicalType::UInt8 => Some(1),
        CoveLogicalType::Int16 | CoveLogicalType::UInt16 => Some(2),
        CoveLogicalType::Int32
        | CoveLogicalType::UInt32
        | CoveLogicalType::Float32
        | CoveLogicalType::DateDays => Some(4),
        CoveLogicalType::Int64
        | CoveLogicalType::UInt64
        | CoveLogicalType::Float64
        | CoveLogicalType::Decimal64
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos => Some(8),
        CoveLogicalType::Decimal128 | CoveLogicalType::Uuid => Some(16),
        _ => None,
    }
}

/// Reads a little-endian `u32` from `bytes` at `offset`.
fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, CoveError> {
    let slice = wire::read_range_checked(bytes, offset, 4)?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

/// Reads a little-endian `u64` from `bytes` at `offset`.
fn read_u64_le(bytes: &[u8], offset: usize) -> Result<u64, CoveError> {
    let slice = wire::read_range_checked(bytes, offset, 8)?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
        dictionary::{FileDictionary, FileDictionaryHeaderV1, FileDictionaryIndexEntryV1},
        validity::ValidityBitmapBuilder,
    };

    fn make_array<'a>(
        logical: CoveLogicalType,
        encoding: CoveEncodingKind,
        row_count: u64,
        data: &'a [u8],
        validity: Option<ValidityBitmap<'a>>,
    ) -> EncodedArray<'a> {
        EncodedArray::new(
            logical,
            CovePhysicalKind::FixedBytes,
            row_count,
            encoding,
            validity,
            data,
            None,
        )
    }

    #[test]
    fn filecode_zero_decodes_as_raw_filecode_without_dictionary() {
        // FileCode 0 stored as LE u32 = [0,0,0,0]
        let data = 0u32.to_le_bytes();
        let arr = EncodedArray::new(
            CoveLogicalType::UInt64,
            CovePhysicalKind::FileCode,
            1,
            CoveEncodingKind::FileCode,
            None,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), CoveArrayValue::FileCode(0));
    }

    #[test]
    fn numcode_zero_decodes_as_ordinary_value() {
        let data = 0u64.to_le_bytes();
        let arr = EncodedArray::new(
            CoveLogicalType::UInt64,
            CovePhysicalKind::NumCode,
            1,
            CoveEncodingKind::NumCode,
            None,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), CoveArrayValue::NumCode(0));
    }

    #[test]
    fn null_row_returns_null() {
        let data = 42i32.to_le_bytes();
        let mut builder = ValidityBitmapBuilder::new(1).unwrap();
        builder.set_null(0).unwrap();
        let bitmap_bytes = builder.into_bytes();
        let bm = ValidityBitmap::new(&bitmap_bytes, 1);
        let arr = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            1,
            CoveEncodingKind::PlainFixed,
            Some(bm),
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), CoveArrayValue::Null);
    }

    #[test]
    fn out_of_range_row_returns_error() {
        let data = 0i32.to_le_bytes();
        let arr = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::PlainFixed,
            1,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(1), Err(CoveError::OffsetRange));
    }

    #[test]
    fn plain_fixed_i32_roundtrip() {
        let v0: i32 = -42;
        let v1: i32 = 1337;
        let mut data = Vec::new();
        data.extend_from_slice(&v0.to_le_bytes());
        data.extend_from_slice(&v1.to_le_bytes());
        let arr = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::PlainFixed,
            2,
            &data,
            None,
        );
        assert_eq!(
            arr.decode_row(0).unwrap(),
            CoveArrayValue::Bytes(&v0.to_le_bytes())
        );
        assert_eq!(
            arr.decode_row(1).unwrap(),
            CoveArrayValue::Bytes(&v1.to_le_bytes())
        );
    }

    #[test]
    fn varbytes_decodes_byte_slices() {
        let a = b"hello";
        let b = b"world!!";
        let mut data = Vec::new();
        data.extend_from_slice(&(a.len() as u32).to_le_bytes());
        data.extend_from_slice(a);
        data.extend_from_slice(&(b.len() as u32).to_le_bytes());
        data.extend_from_slice(b);
        let arr = make_array(
            CoveLogicalType::Binary,
            CoveEncodingKind::VarBytes,
            2,
            &data,
            None,
        );
        assert_eq!(
            arr.decode_row(0).unwrap(),
            CoveArrayValue::Bytes(a.as_ref())
        );
        assert_eq!(
            arr.decode_row(1).unwrap(),
            CoveArrayValue::Bytes(b.as_ref())
        );
    }

    #[test]
    fn constant_returns_same_value_for_all_rows() {
        let val: i32 = 99;
        let data = val.to_le_bytes();
        let arr = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::Constant,
            5,
            &data,
            None,
        );
        for row in 0..5 {
            assert_eq!(
                arr.decode_row(row).unwrap(),
                CoveArrayValue::Bytes(&val.to_le_bytes())
            );
        }
    }

    #[test]
    fn unsupported_encoding_returns_error() {
        let data = [0u8; 1];
        let arr = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::Canonical,
            1,
            &data,
            None,
        );
        assert!(matches!(
            arr.decode_row(0),
            Err(CoveError::UnsupportedEncoding(_))
        ));
        let arr2 = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::LocalCodebook,
            1,
            &data,
            None,
        );
        assert!(matches!(
            arr2.decode_row(0),
            Err(CoveError::UnsupportedEncoding(_))
        ));
    }

    #[test]
    fn filecode_out_of_range_fails() {
        // Build a minimal dictionary with 1 entry (FileCode 0 only).
        // Then try to decode FileCode 1 — should fail with BadFileCode.
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: 12,    // Utf8
            storage_class: 0, // Inline
            flags: 0,
            inline_len: 4,
            reserved0: [0; 3],
            inline_data: {
                let mut d = [0u8; 16];
                // Canonical UTF-8 encoding: varint length prefix + bytes.
                d[..4].copy_from_slice(&[0x03, b'f', b'o', b'o']);
                d
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
        let mut index_bytes = header.serialize().to_vec();
        index_bytes.extend_from_slice(&entry.serialize());
        let dict = FileDictionary::parse(&index_bytes, &[]).unwrap();

        // Two rows: [0, 1] as u32 LE
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());

        let arr = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::FileCode,
            2,
            CoveEncodingKind::FileCode,
            None,
            &data,
            Some(&dict),
        );
        // FileCode 0 is valid
        assert!(arr.decode_row(0).is_ok());
        // FileCode 1 is out of range
        assert_eq!(arr.decode_row(1), Err(CoveError::BadFileCode));
    }
}
