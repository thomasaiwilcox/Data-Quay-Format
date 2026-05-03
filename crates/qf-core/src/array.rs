//! Quay Format (QF) v1.0 — Encoded array decoding.
//!
//! This module provides access to individual rows within an encoded column
//! array, supporting the encodings described in Section 20 of the specification.

use crate::{
    constants::{QfEncodingKind, QfLogicalType, QfPhysicalKind},
    dictionary::{DictionaryValue, FileDictionary},
    validity::ValidityBitmap,
    wire, QfError,
};

/// A decoded value from an encoded array row.
#[derive(Debug, Clone, PartialEq)]
pub enum QfArrayValue<'a> {
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
    pub logical: QfLogicalType,
    /// Physical kind of the column.
    pub physical: QfPhysicalKind,
    /// Number of logical rows.
    pub row_count: u64,
    /// Encoding applied to the data buffer.
    pub encoding: QfEncodingKind,
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
        logical: QfLogicalType,
        physical: QfPhysicalKind,
        row_count: u64,
        encoding: QfEncodingKind,
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
    /// Returns [`QfError::OffsetRange`] if `row >= row_count`.
    pub fn is_null(&self, row: u64) -> Result<bool, QfError> {
        if row >= self.row_count {
            return Err(QfError::OffsetRange);
        }
        match &self.validity {
            Some(bm) => bm.is_null(row),
            None => Ok(false),
        }
    }

    /// Decodes the value at `row`.
    ///
    /// Returns [`QfArrayValue::Null`] if the row is null.
    /// Returns [`QfError::OffsetRange`] if `row >= row_count`.
    /// Returns [`QfError::UnsupportedEncoding`] for encodings not yet implemented.
    pub fn decode_row(&self, row: u64) -> Result<QfArrayValue<'_>, QfError> {
        if row >= self.row_count {
            return Err(QfError::OffsetRange);
        }
        if self.is_null(row)? {
            return Ok(QfArrayValue::Null);
        }
        match self.encoding {
            QfEncodingKind::Validity => {
                let byte_idx = (row / 8) as usize;
                let bit_idx = (row % 8) as u32;
                let byte = self
                    .data
                    .get(byte_idx)
                    .copied()
                    .ok_or(QfError::OffsetRange)?;
                let bit = (byte >> bit_idx) & 1 == 1;
                Ok(QfArrayValue::ValidityBit(bit))
            }
            QfEncodingKind::Constant => {
                let w = logical_type_fixed_width(self.logical).ok_or_else(|| {
                    QfError::UnsupportedEncoding(format!(
                        "Constant encoding requires fixed-width logical type, got {:?}",
                        self.logical
                    ))
                })?;
                let slice = wire::read_range_checked(self.data, 0, w)?;
                Ok(QfArrayValue::Bytes(slice))
            }
            QfEncodingKind::FileCode => {
                let code = read_u32_le(
                    self.data,
                    (row as usize)
                        .checked_mul(4)
                        .ok_or(QfError::ArithOverflow)?,
                )?;
                match self.dictionary {
                    Some(dict) => {
                        let val = dict.decode_value(code)?;
                        Ok(QfArrayValue::DictValue(val))
                    }
                    None => Ok(QfArrayValue::FileCode(code)),
                }
            }
            QfEncodingKind::NumCode => {
                let code = read_u64_le(
                    self.data,
                    (row as usize)
                        .checked_mul(8)
                        .ok_or(QfError::ArithOverflow)?,
                )?;
                Ok(QfArrayValue::NumCode(code))
            }
            QfEncodingKind::PlainFixed => {
                let w = logical_type_fixed_width(self.logical).ok_or_else(|| {
                    QfError::UnsupportedEncoding(format!(
                        "PlainFixed encoding requires fixed-width logical type, got {:?}",
                        self.logical
                    ))
                })?;
                let offset = (row as usize)
                    .checked_mul(w)
                    .ok_or(QfError::ArithOverflow)?;
                let slice = wire::read_range_checked(self.data, offset, w)?;
                Ok(QfArrayValue::Bytes(slice))
            }
            QfEncodingKind::PlainVarint => {
                // O(n): variable-width encoding requires scanning all preceding rows.
                let mut pos = 0usize;
                for _ in 0..row {
                    if pos >= self.data.len() {
                        return Err(QfError::OffsetRange);
                    }
                    let (_val, consumed) = wire::decode_u64_leb128(&self.data[pos..])?;
                    pos = pos.checked_add(consumed).ok_or(QfError::ArithOverflow)?;
                }
                if pos >= self.data.len() {
                    return Err(QfError::OffsetRange);
                }
                let (val, _consumed) = wire::decode_u64_leb128(&self.data[pos..])?;
                Ok(QfArrayValue::Varint(val))
            }
            QfEncodingKind::VarBytes => {
                // O(n): variable-width encoding requires scanning all preceding rows.
                let mut pos = 0usize;
                for _ in 0..row {
                    let len = read_u32_le(self.data, pos)? as usize;
                    pos = pos
                        .checked_add(4)
                        .and_then(|p| p.checked_add(len))
                        .ok_or(QfError::ArithOverflow)?;
                    if pos > self.data.len() {
                        return Err(QfError::OffsetRange);
                    }
                }
                let len = read_u32_le(self.data, pos)? as usize;
                let data_start = pos.checked_add(4).ok_or(QfError::ArithOverflow)?;
                let slice = wire::read_range_checked(self.data, data_start, len)?;
                Ok(QfArrayValue::Bytes(slice))
            }
            QfEncodingKind::LocalCodebook
            | QfEncodingKind::Rle
            | QfEncodingKind::RunEnd
            | QfEncodingKind::BitPacked
            | QfEncodingKind::Delta
            | QfEncodingKind::FrameOfReference
            | QfEncodingKind::PatchedBase
            | QfEncodingKind::Sparse
            | QfEncodingKind::Sequence
            | QfEncodingKind::Lz4Block
            | QfEncodingKind::ZstdBlock
            | QfEncodingKind::Canonical => {
                Err(QfError::UnsupportedEncoding(format!("{:?}", self.encoding)))
            }
        }
    }
}

/// Returns the fixed byte width for a logical type, or `None` for variable-width types.
pub fn logical_type_fixed_width(logical: QfLogicalType) -> Option<usize> {
    match logical {
        QfLogicalType::Bool | QfLogicalType::Int8 | QfLogicalType::UInt8 => Some(1),
        QfLogicalType::Int16 | QfLogicalType::UInt16 => Some(2),
        QfLogicalType::Int32
        | QfLogicalType::UInt32
        | QfLogicalType::Float32
        | QfLogicalType::DateDays => Some(4),
        QfLogicalType::Int64
        | QfLogicalType::UInt64
        | QfLogicalType::Float64
        | QfLogicalType::Decimal64
        | QfLogicalType::TimestampMicros
        | QfLogicalType::TimestampNanos => Some(8),
        QfLogicalType::Decimal128 | QfLogicalType::Uuid => Some(16),
        _ => None,
    }
}

/// Reads a little-endian `u32` from `bytes` at `offset`.
fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, QfError> {
    let slice = wire::read_range_checked(bytes, offset, 4)?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

/// Reads a little-endian `u64` from `bytes` at `offset`.
fn read_u64_le(bytes: &[u8], offset: usize) -> Result<u64, QfError> {
    let slice = wire::read_range_checked(bytes, offset, 8)?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::{QfEncodingKind, QfLogicalType, QfPhysicalKind},
        dictionary::{FileDictionary, FileDictionaryHeaderV1, FileDictionaryIndexEntryV1},
        validity::ValidityBitmapBuilder,
    };

    fn make_array<'a>(
        logical: QfLogicalType,
        encoding: QfEncodingKind,
        row_count: u64,
        data: &'a [u8],
        validity: Option<ValidityBitmap<'a>>,
    ) -> EncodedArray<'a> {
        EncodedArray::new(
            logical,
            QfPhysicalKind::FixedBytes,
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
            QfLogicalType::UInt64,
            QfPhysicalKind::FileCode,
            1,
            QfEncodingKind::FileCode,
            None,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), QfArrayValue::FileCode(0));
    }

    #[test]
    fn numcode_zero_decodes_as_ordinary_value() {
        let data = 0u64.to_le_bytes();
        let arr = EncodedArray::new(
            QfLogicalType::UInt64,
            QfPhysicalKind::NumCode,
            1,
            QfEncodingKind::NumCode,
            None,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), QfArrayValue::NumCode(0));
    }

    #[test]
    fn null_row_returns_null() {
        let data = 42i32.to_le_bytes();
        let mut builder = ValidityBitmapBuilder::new(1).unwrap();
        builder.set_null(0).unwrap();
        let bitmap_bytes = builder.into_bytes();
        let bm = ValidityBitmap::new(&bitmap_bytes, 1);
        let arr = EncodedArray::new(
            QfLogicalType::Int32,
            QfPhysicalKind::FixedBytes,
            1,
            QfEncodingKind::PlainFixed,
            Some(bm),
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), QfArrayValue::Null);
    }

    #[test]
    fn out_of_range_row_returns_error() {
        let data = 0i32.to_le_bytes();
        let arr = make_array(
            QfLogicalType::Int32,
            QfEncodingKind::PlainFixed,
            1,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(1), Err(QfError::OffsetRange));
    }

    #[test]
    fn plain_fixed_i32_roundtrip() {
        let v0: i32 = -42;
        let v1: i32 = 1337;
        let mut data = Vec::new();
        data.extend_from_slice(&v0.to_le_bytes());
        data.extend_from_slice(&v1.to_le_bytes());
        let arr = make_array(
            QfLogicalType::Int32,
            QfEncodingKind::PlainFixed,
            2,
            &data,
            None,
        );
        assert_eq!(
            arr.decode_row(0).unwrap(),
            QfArrayValue::Bytes(&v0.to_le_bytes())
        );
        assert_eq!(
            arr.decode_row(1).unwrap(),
            QfArrayValue::Bytes(&v1.to_le_bytes())
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
            QfLogicalType::Binary,
            QfEncodingKind::VarBytes,
            2,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), QfArrayValue::Bytes(a.as_ref()));
        assert_eq!(arr.decode_row(1).unwrap(), QfArrayValue::Bytes(b.as_ref()));
    }

    #[test]
    fn constant_returns_same_value_for_all_rows() {
        let val: i32 = 99;
        let data = val.to_le_bytes();
        let arr = make_array(
            QfLogicalType::Int32,
            QfEncodingKind::Constant,
            5,
            &data,
            None,
        );
        for row in 0..5 {
            assert_eq!(
                arr.decode_row(row).unwrap(),
                QfArrayValue::Bytes(&val.to_le_bytes())
            );
        }
    }

    #[test]
    fn unsupported_encoding_returns_error() {
        let data = [0u8; 1];
        let arr = make_array(
            QfLogicalType::Int32,
            QfEncodingKind::Canonical,
            1,
            &data,
            None,
        );
        assert!(matches!(
            arr.decode_row(0),
            Err(QfError::UnsupportedEncoding(_))
        ));
        let arr2 = make_array(
            QfLogicalType::Int32,
            QfEncodingKind::LocalCodebook,
            1,
            &data,
            None,
        );
        assert!(matches!(
            arr2.decode_row(0),
            Err(QfError::UnsupportedEncoding(_))
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
            inline_len: 3,
            reserved0: [0; 3],
            inline_data: {
                let mut d = [0u8; 16];
                d[..3].copy_from_slice(b"foo");
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
            QfLogicalType::Utf8,
            QfPhysicalKind::FileCode,
            2,
            QfEncodingKind::FileCode,
            None,
            &data,
            Some(&dict),
        );
        // FileCode 0 is valid
        assert!(arr.decode_row(0).is_ok());
        // FileCode 1 is out of range
        assert_eq!(arr.decode_row(1), Err(QfError::BadFileCode));
    }
}
