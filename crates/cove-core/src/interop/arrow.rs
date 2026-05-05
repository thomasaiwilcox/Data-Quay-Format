//! Spec §49 — Arrow interop helpers.
//!
//! COVE stores nulls as a *null* bitmap (bit set ⇒ null), Arrow stores them as
//! a *validity* bitmap (bit set ⇒ valid). This module owns the bit inversion
//! and byte-aligned conversion required to bridge the two.

use std::{borrow::Cow, sync::Arc};

use arrow_array::{
    builder::{BinaryBuilder, StringBuilder},
    ArrayRef, BinaryArray, BooleanArray, Date32Array, Float32Array, Float64Array, Int16Array,
    Int32Array, Int64Array, Int8Array, RecordBatch, TimestampMicrosecondArray,
    TimestampNanosecondArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};

use crate::{
    array::{CoveArrayValue, EncodedArray},
    constants::CoveLogicalType,
    dictionary::DictionaryValue,
    wire, CoveError,
};

/// Invert a COVE null bitmap into an Arrow validity bitmap with the same byte
/// length. Per Spec §49.2, the row count MUST be preserved exactly.
pub fn cove_null_to_arrow_validity(
    cove_null: &[u8],
    row_count: usize,
) -> Result<Vec<u8>, CoveError> {
    let needed = (row_count + 7) / 8;
    if cove_null.len() < needed {
        return Err(CoveError::BufferTooShort);
    }
    let mut out = vec![0u8; needed];
    for row in 0..row_count {
        let byte = row / 8;
        let bit = 1u8 << (row % 8);
        let is_null = (cove_null[byte] & bit) != 0;
        if !is_null {
            out[byte] |= bit;
        }
    }
    Ok(out)
}

/// Invert an Arrow validity bitmap into a COVE null bitmap.
pub fn arrow_validity_to_cove_null(
    arrow_validity: &[u8],
    row_count: usize,
) -> Result<Vec<u8>, CoveError> {
    let needed = (row_count + 7) / 8;
    if arrow_validity.len() < needed {
        return Err(CoveError::BufferTooShort);
    }
    let mut out = vec![0u8; needed];
    for row in 0..row_count {
        let byte = row / 8;
        let bit = 1u8 << (row % 8);
        let is_valid = (arrow_validity[byte] & bit) != 0;
        if !is_valid {
            out[byte] |= bit;
        }
    }
    Ok(out)
}

/// Export one decoded COVE array view as an Arrow array.
pub fn encoded_array_to_arrow(array: &EncodedArray<'_>) -> Result<ArrayRef, CoveError> {
    let values = array.decode_all_rows()?;
    match arrow_data_type(array.logical)? {
        DataType::Boolean => Ok(Arc::new(BooleanArray::from(collect_bool(&values)?))),
        DataType::Int8 => Ok(Arc::new(Int8Array::from(collect_i64(array.logical, &values, |v| {
            i8::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?))),
        DataType::Int16 => Ok(Arc::new(Int16Array::from(collect_i64(array.logical, &values, |v| {
            i16::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?))),
        DataType::Int32 => Ok(Arc::new(Int32Array::from(collect_i64(array.logical, &values, |v| {
            i32::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?))),
        DataType::Int64 => Ok(Arc::new(Int64Array::from(collect_i64(array.logical, &values, Ok)?))),
        DataType::UInt8 => Ok(Arc::new(UInt8Array::from(collect_u64(array.logical, &values, |v| {
            u8::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?))),
        DataType::UInt16 => Ok(Arc::new(UInt16Array::from(collect_u64(array.logical, &values, |v| {
            u16::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?))),
        DataType::UInt32 => Ok(Arc::new(UInt32Array::from(collect_u64(array.logical, &values, |v| {
            u32::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?))),
        DataType::UInt64 => Ok(Arc::new(UInt64Array::from(collect_u64(array.logical, &values, Ok)?))),
        DataType::Float32 => Ok(Arc::new(Float32Array::from(collect_f32(&values)?))),
        DataType::Float64 => Ok(Arc::new(Float64Array::from(collect_f64(&values)?))),
        DataType::Date32 => Ok(Arc::new(Date32Array::from(collect_i64(array.logical, &values, |v| {
            i32::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?))),
        DataType::Timestamp(TimeUnit::Microsecond, None) => Ok(Arc::new(
            TimestampMicrosecondArray::from(collect_i64(array.logical, &values, Ok)?),
        )),
        DataType::Timestamp(TimeUnit::Nanosecond, None) => Ok(Arc::new(
            TimestampNanosecondArray::from(collect_i64(array.logical, &values, Ok)?),
        )),
        DataType::Utf8 => Ok(Arc::new(collect_utf8(array.logical, &values)?)),
        DataType::Binary => Ok(Arc::new(collect_binary(array.logical, &values)?)),
        other => Err(CoveError::UnsupportedEncoding(format!(
            "Arrow export for {other:?}"
        ))),
    }
}

/// Export named COVE array views as an Arrow [`RecordBatch`].
pub fn encoded_columns_to_record_batch(
    columns: &[(&str, &EncodedArray<'_>)],
) -> Result<RecordBatch, CoveError> {
    let mut fields = Vec::with_capacity(columns.len());
    let mut arrays = Vec::with_capacity(columns.len());
    for (name, array) in columns {
        let arrow_array = encoded_array_to_arrow(array)?;
        fields.push(Field::new(
            *name,
            arrow_array.data_type().clone(),
            array.validity.is_some() || array.logical == CoveLogicalType::Null,
        ));
        arrays.push(arrow_array);
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
        .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))
}

fn arrow_data_type(logical: CoveLogicalType) -> Result<DataType, CoveError> {
    match logical {
        CoveLogicalType::Bool => Ok(DataType::Boolean),
        CoveLogicalType::Int8 => Ok(DataType::Int8),
        CoveLogicalType::Int16 => Ok(DataType::Int16),
        CoveLogicalType::Int32 => Ok(DataType::Int32),
        CoveLogicalType::Int64 | CoveLogicalType::Decimal64 => Ok(DataType::Int64),
        CoveLogicalType::UInt8 => Ok(DataType::UInt8),
        CoveLogicalType::UInt16 => Ok(DataType::UInt16),
        CoveLogicalType::UInt32 => Ok(DataType::UInt32),
        CoveLogicalType::UInt64 => Ok(DataType::UInt64),
        CoveLogicalType::Float32 => Ok(DataType::Float32),
        CoveLogicalType::Float64 => Ok(DataType::Float64),
        CoveLogicalType::DateDays => Ok(DataType::Date32),
        CoveLogicalType::TimestampMicros => Ok(DataType::Timestamp(TimeUnit::Microsecond, None)),
        CoveLogicalType::TimestampNanos => Ok(DataType::Timestamp(TimeUnit::Nanosecond, None)),
        CoveLogicalType::Utf8 | CoveLogicalType::Json => Ok(DataType::Utf8),
        CoveLogicalType::Binary | CoveLogicalType::Uuid => Ok(DataType::Binary),
        other => Err(CoveError::UnsupportedEncoding(format!(
            "Arrow export for {:?}",
            other
        ))),
    }
}

fn collect_bool(values: &[CoveArrayValue<'_>]) -> Result<Vec<Option<bool>>, CoveError> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        out.push(match value {
            CoveArrayValue::Null => None,
            CoveArrayValue::Boolean(value) | CoveArrayValue::ValidityBit(value) => Some(*value),
            CoveArrayValue::Bytes(bytes) if bytes.len() == 1 => match bytes[0] {
                0 => Some(false),
                1 => Some(true),
                _ => return Err(CoveError::PageCorrupt),
            },
            other => return Err(unexpected_value("Boolean", other)),
        });
    }
    Ok(out)
}

fn collect_i64<T, F>(
    logical: CoveLogicalType,
    values: &[CoveArrayValue<'_>],
    cast: F,
) -> Result<Vec<Option<T>>, CoveError>
where
    F: Fn(i64) -> Result<T, CoveError>,
{
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        out.push(match value {
            CoveArrayValue::Null => None,
            _ => Some(cast(value_to_i64(logical, value)?)?),
        });
    }
    Ok(out)
}

fn collect_u64<T, F>(
    logical: CoveLogicalType,
    values: &[CoveArrayValue<'_>],
    cast: F,
) -> Result<Vec<Option<T>>, CoveError>
where
    F: Fn(u64) -> Result<T, CoveError>,
{
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        out.push(match value {
            CoveArrayValue::Null => None,
            _ => Some(cast(value_to_u64(logical, value)?)?),
        });
    }
    Ok(out)
}

fn collect_f32(values: &[CoveArrayValue<'_>]) -> Result<Vec<Option<f32>>, CoveError> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        out.push(match value {
            CoveArrayValue::Null => None,
            _ => Some(value_to_f32(value)?),
        });
    }
    Ok(out)
}

fn collect_f64(values: &[CoveArrayValue<'_>]) -> Result<Vec<Option<f64>>, CoveError> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        out.push(match value {
            CoveArrayValue::Null => None,
            _ => Some(value_to_f64(value)?),
        });
    }
    Ok(out)
}

fn collect_utf8(
    logical: CoveLogicalType,
    values: &[CoveArrayValue<'_>],
) -> Result<arrow_array::StringArray, CoveError> {
    let mut builder = StringBuilder::new();
    for value in values {
        match value {
            CoveArrayValue::Null => builder.append_null(),
            _ => {
                let bytes = value_to_bytes(logical, value)?;
                let text = std::str::from_utf8(bytes.as_ref()).map_err(|_| {
                    CoveError::BadSection("Arrow Utf8 export requires valid UTF-8".into())
                })?;
                builder.append_value(text);
            }
        }
    }
    Ok(builder.finish())
}

fn collect_binary(
    logical: CoveLogicalType,
    values: &[CoveArrayValue<'_>],
) -> Result<BinaryArray, CoveError> {
    let mut builder = BinaryBuilder::new();
    for value in values {
        match value {
            CoveArrayValue::Null => builder.append_null(),
            _ => {
                let bytes = value_to_bytes(logical, value)?;
                builder.append_value(bytes.as_ref());
            }
        }
    }
    Ok(builder.finish())
}

fn value_to_i64(logical: CoveLogicalType, value: &CoveArrayValue<'_>) -> Result<i64, CoveError> {
    match value {
        CoveArrayValue::Int64(value) => Ok(*value),
        CoveArrayValue::NumCode(value) | CoveArrayValue::Varint(value) => {
            i64::try_from(*value).map_err(|_| CoveError::PageCorrupt)
        }
        CoveArrayValue::FileCode(value) => Ok(i64::from(*value)),
        CoveArrayValue::Bytes(bytes) => signed_from_bytes(logical, bytes),
        CoveArrayValue::DictValue(DictionaryValue::RawBytes(bytes)) => {
            signed_from_bytes(logical, bytes)
        }
        other => Err(unexpected_value("signed integer", other)),
    }
}

fn value_to_u64(logical: CoveLogicalType, value: &CoveArrayValue<'_>) -> Result<u64, CoveError> {
    match value {
        CoveArrayValue::NumCode(value) | CoveArrayValue::Varint(value) => Ok(*value),
        CoveArrayValue::Int64(value) => u64::try_from(*value).map_err(|_| CoveError::PageCorrupt),
        CoveArrayValue::FileCode(value) => Ok(u64::from(*value)),
        CoveArrayValue::Bytes(bytes) => unsigned_from_bytes(logical, bytes),
        CoveArrayValue::DictValue(DictionaryValue::RawBytes(bytes)) => {
            unsigned_from_bytes(logical, bytes)
        }
        other => Err(unexpected_value("unsigned integer", other)),
    }
}

fn value_to_f32(value: &CoveArrayValue<'_>) -> Result<f32, CoveError> {
    let bytes = plain_bytes(value)?;
    if bytes.len() != 4 {
        return Err(CoveError::PageCorrupt);
    }
    Ok(f32::from_bits(u32::from_le_bytes(
        bytes.try_into().unwrap(),
    )))
}

fn value_to_f64(value: &CoveArrayValue<'_>) -> Result<f64, CoveError> {
    let bytes = plain_bytes(value)?;
    if bytes.len() != 8 {
        return Err(CoveError::PageCorrupt);
    }
    Ok(f64::from_bits(u64::from_le_bytes(
        bytes.try_into().unwrap(),
    )))
}

fn value_to_bytes<'a>(
    logical: CoveLogicalType,
    value: &'a CoveArrayValue<'a>,
) -> Result<Cow<'a, [u8]>, CoveError> {
    match value {
        CoveArrayValue::Bytes(bytes) => Ok(Cow::Borrowed(bytes)),
        CoveArrayValue::OwnedBytes(bytes) => Ok(Cow::Borrowed(bytes)),
        CoveArrayValue::DictValue(DictionaryValue::RawBytes(bytes)) => {
            canonical_payload_bytes(logical, bytes)
        }
        CoveArrayValue::DictValue(DictionaryValue::RedactedPresent) => {
            Err(CoveError::RedactionPolicy)
        }
        other => Err(unexpected_value("bytes", other)),
    }
}

fn plain_bytes<'a>(value: &'a CoveArrayValue<'a>) -> Result<&'a [u8], CoveError> {
    match value {
        CoveArrayValue::Bytes(bytes) => Ok(bytes),
        CoveArrayValue::OwnedBytes(bytes) => Ok(bytes),
        CoveArrayValue::DictValue(DictionaryValue::RawBytes(bytes)) => Ok(bytes),
        CoveArrayValue::DictValue(DictionaryValue::RedactedPresent) => {
            Err(CoveError::RedactionPolicy)
        }
        other => Err(unexpected_value("plain bytes", other)),
    }
}

fn canonical_payload_bytes<'a>(
    logical: CoveLogicalType,
    bytes: &'a [u8],
) -> Result<Cow<'a, [u8]>, CoveError> {
    match logical {
        CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json => {
            let (len, consumed) = wire::decode_u64_leb128(bytes)?;
            let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
            let start = consumed;
            let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
            if end != bytes.len() {
                return Err(CoveError::PageCorrupt);
            }
            Ok(Cow::Borrowed(&bytes[start..end]))
        }
        _ => Ok(Cow::Borrowed(bytes)),
    }
}

fn signed_from_bytes(logical: CoveLogicalType, bytes: &[u8]) -> Result<i64, CoveError> {
    match logical {
        CoveLogicalType::Int8 => exact_bytes::<1>(bytes).map(|raw| i8::from_le_bytes(raw) as i64),
        CoveLogicalType::Int16 => exact_bytes::<2>(bytes).map(|raw| i16::from_le_bytes(raw) as i64),
        CoveLogicalType::Int32 | CoveLogicalType::DateDays => {
            exact_bytes::<4>(bytes).map(|raw| i32::from_le_bytes(raw) as i64)
        }
        CoveLogicalType::Int64
        | CoveLogicalType::Decimal64
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos => exact_bytes::<8>(bytes).map(i64::from_le_bytes),
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "signed Arrow export from {:?}",
            logical
        ))),
    }
}

fn unsigned_from_bytes(logical: CoveLogicalType, bytes: &[u8]) -> Result<u64, CoveError> {
    match logical {
        CoveLogicalType::UInt8 => exact_bytes::<1>(bytes).map(|raw| u8::from_le_bytes(raw) as u64),
        CoveLogicalType::UInt16 => {
            exact_bytes::<2>(bytes).map(|raw| u16::from_le_bytes(raw) as u64)
        }
        CoveLogicalType::UInt32 => {
            exact_bytes::<4>(bytes).map(|raw| u32::from_le_bytes(raw) as u64)
        }
        CoveLogicalType::UInt64 => exact_bytes::<8>(bytes).map(u64::from_le_bytes),
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "unsigned Arrow export from {:?}",
            logical
        ))),
    }
}

fn exact_bytes<const N: usize>(bytes: &[u8]) -> Result<[u8; N], CoveError> {
    if bytes.len() != N {
        return Err(CoveError::PageCorrupt);
    }
    Ok(bytes.try_into().unwrap())
}

fn unexpected_value(expected: &str, value: &CoveArrayValue<'_>) -> CoveError {
    CoveError::UnsupportedEncoding(format!("cannot export {value:?} as Arrow {expected}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Array;

    use crate::{
        array::EncodedArray,
        constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind, StorageClass, ValueTag},
        dictionary::{FileDictionary, FileDictionaryHeaderV1, FileDictionaryIndexEntryV1},
        validity::ValidityBitmapBuilder,
    };

    #[test]
    fn round_trip_inversion_preserves_payload() {
        let cove = vec![0b0000_1010u8]; // rows 1 and 3 are null
        let arrow = cove_null_to_arrow_validity(&cove, 8).unwrap();
        // Arrow: bits 1 and 3 should be 0 (invalid), others 1 (valid).
        assert_eq!(arrow[0], !cove[0]);
        let back = arrow_validity_to_cove_null(&arrow, 8).unwrap();
        assert_eq!(back, cove);
    }

    #[test]
    fn partial_byte_only_iterates_row_count() {
        let cove = vec![0b1111_0000u8];
        let arrow = cove_null_to_arrow_validity(&cove, 4).unwrap();
        // Only the lower 4 bits of byte 0 are touched; high bits stay 0.
        assert_eq!(arrow[0] & 0b0000_1111, 0b0000_1111);
        assert_eq!(arrow[0] & 0b1111_0000, 0);
    }

    #[test]
    fn rejects_short_cove_null_bitmap() {
        assert_eq!(
            cove_null_to_arrow_validity(&[], 1),
            Err(CoveError::BufferTooShort)
        );
    }

    #[test]
    fn rejects_short_arrow_validity_bitmap() {
        assert_eq!(
            arrow_validity_to_cove_null(&[], 1),
            Err(CoveError::BufferTooShort)
        );
    }

    #[test]
    fn exports_plain_int32_array_with_nulls() {
        let mut values = Vec::new();
        values.extend_from_slice(&10i32.to_le_bytes());
        values.extend_from_slice(&20i32.to_le_bytes());
        let mut validity = ValidityBitmapBuilder::new(2).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 2);
        let cove = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            2,
            CoveEncodingKind::PlainFixed,
            Some(bitmap),
            &values,
            None,
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let ints = arrow.as_any().downcast_ref::<Int32Array>().unwrap();
        assert_eq!(ints.len(), 2);
        assert_eq!(ints.value(0), 10);
        assert!(ints.is_null(1));
    }

    #[test]
    fn exports_utf8_varbytes_array() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        values.extend_from_slice(&5u32.to_le_bytes());
        values.extend_from_slice(b"there");
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let strings = arrow
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(strings.value(0), "hi");
        assert_eq!(strings.value(1), "there");
    }

    #[test]
    fn exports_filecode_dictionary_values_as_logical_utf8() {
        let dictionary = FileDictionary {
            header: FileDictionaryHeaderV1 {
                entry_count: 2,
                flags: 0,
                index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
                value_hash_algorithm: 0,
                payload_length: 0,
                reserved: [0; 24],
            },
            entries: vec![inline_utf8_entry("red"), inline_utf8_entry("blue")],
            payload: Vec::new(),
        };
        let mut codes = Vec::new();
        codes.extend_from_slice(&1u32.to_le_bytes());
        codes.extend_from_slice(&0u32.to_le_bytes());
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::FileCode,
            2,
            CoveEncodingKind::FileCode,
            None,
            &codes,
            Some(&dictionary),
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let strings = arrow
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(strings.value(0), "blue");
        assert_eq!(strings.value(1), "red");
    }

    #[test]
    fn exports_record_batch_from_named_columns() {
        let ids = [1u64.to_le_bytes(), 2u64.to_le_bytes()].concat();
        let id_array = EncodedArray::new(
            CoveLogicalType::UInt64,
            CovePhysicalKind::NumCode,
            2,
            CoveEncodingKind::NumCode,
            None,
            &ids,
            None,
        );
        let mut names = Vec::new();
        names.extend_from_slice(&1u32.to_le_bytes());
        names.extend_from_slice(b"a");
        names.extend_from_slice(&1u32.to_le_bytes());
        names.extend_from_slice(b"b");
        let name_array = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            &names,
            None,
        );

        let batch =
            encoded_columns_to_record_batch(&[("id", &id_array), ("name", &name_array)]).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 2);
        assert_eq!(batch.schema().field(0).name(), "id");
    }

    fn inline_utf8_entry(value: &str) -> FileDictionaryIndexEntryV1 {
        let mut canonical = wire::encode_u64_leb128(value.len() as u64);
        canonical.extend_from_slice(value.as_bytes());
        let mut inline_data = [0u8; 16];
        inline_data[..canonical.len()].copy_from_slice(&canonical);
        FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: canonical.len() as u8,
            reserved0: [0; 3],
            inline_data,
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        }
    }
}
