//! Spec §49 — Arrow interop helpers.
//!
//! COVE stores nulls as a *null* bitmap (bit set ⇒ null), Arrow stores them as
//! a *validity* bitmap (bit set ⇒ valid). This module owns the bit inversion
//! and byte-aligned conversion required to bridge the two.

use std::{borrow::Cow, collections::HashMap, sync::Arc};

use arrow_array::{
    builder::{BinaryBuilder, StringBuilder},
    types::UInt32Type,
    Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, DictionaryArray,
    FixedSizeBinaryArray, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array,
    Int8Array, ListArray, MapArray, RecordBatch, StructArray, TimestampMicrosecondArray,
    TimestampNanosecondArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_buffer::{NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow_schema::{DataType, Field, Fields, Schema, TimeUnit};

use crate::{
    array::{CoveArrayValue, EncodedArray},
    constants::CoveLogicalType,
    dictionary::DictionaryValue,
    encoding::nested::{ListLayoutPayload, MapLayoutPayload, StructLayoutPayload},
    validity::ValidityBitmap,
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

/// Policy for exporting FileCode-backed scalar columns to Arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowDictionaryPolicy {
    /// Decode FileCodes to their logical values before building the Arrow array.
    DecodeValues,
    /// Export FileCodes as Arrow dictionary keys when values are representable.
    DictionaryKeys,
}

impl Default for ArrowDictionaryPolicy {
    fn default() -> Self {
        Self::DictionaryKeys
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrowDecimalContext {
    pub precision: u8,
    pub scale: i8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrowFidelitySeverity {
    Informational,
    Lossy,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrowFidelityIssue {
    pub field: Option<String>,
    pub logical_type: CoveLogicalType,
    pub severity: ArrowFidelitySeverity,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArrowExportReport {
    pub issues: Vec<ArrowFidelityIssue>,
}

impl ArrowExportReport {
    pub fn has_lossy_or_unsupported(&self) -> bool {
        self.issues.iter().any(|issue| {
            matches!(
                issue.severity,
                ArrowFidelitySeverity::Lossy | ArrowFidelitySeverity::Unsupported
            )
        })
    }

    fn push(
        &mut self,
        field: Option<&str>,
        logical_type: CoveLogicalType,
        severity: ArrowFidelitySeverity,
        message: impl Into<String>,
    ) {
        self.issues.push(ArrowFidelityIssue {
            field: field.map(ToOwned::to_owned),
            logical_type,
            severity,
            message: message.into(),
        });
    }

    fn extend_with_field(&mut self, field: &str, mut other: ArrowExportReport) {
        for issue in &mut other.issues {
            if issue.field.is_none() {
                issue.field = Some(field.to_string());
            }
        }
        self.issues.extend(other.issues);
    }
}

pub struct ArrowExportResult<T> {
    pub value: T,
    pub report: ArrowExportReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrowExportOptions {
    pub dictionary_policy: ArrowDictionaryPolicy,
    pub decimal: Option<ArrowDecimalContext>,
    pub emit_uuid_extension_metadata: bool,
    pub emit_json_extension_metadata: bool,
}

impl Default for ArrowExportOptions {
    fn default() -> Self {
        Self {
            dictionary_policy: ArrowDictionaryPolicy::DecodeValues,
            decimal: None,
            emit_uuid_extension_metadata: false,
            emit_json_extension_metadata: false,
        }
    }
}

/// A named top-level Arrow export column.
pub struct ArrowExportColumn<'a> {
    pub name: &'a str,
    pub node: ArrowExportNode<'a>,
    pub nullable: bool,
}

impl<'a> ArrowExportColumn<'a> {
    pub fn scalar(name: &'a str, array: &'a EncodedArray<'a>) -> Self {
        Self {
            name,
            node: ArrowExportNode::scalar(array),
            nullable: array.validity.is_some() || array.logical == CoveLogicalType::Null,
        }
    }
}

/// A layout-aware Arrow export node.
pub enum ArrowExportNode<'a> {
    Scalar {
        array: &'a EncodedArray<'a>,
        dictionary_policy: ArrowDictionaryPolicy,
    },
    List {
        layout: &'a ListLayoutPayload,
        child: Box<ArrowExportNode<'a>>,
        validity: Option<ValidityBitmap<'a>>,
    },
    Struct {
        layout: &'a StructLayoutPayload,
        fields: Vec<ArrowExportColumn<'a>>,
        validity: Option<ValidityBitmap<'a>>,
    },
    Map {
        layout: &'a MapLayoutPayload,
        keys: Box<ArrowExportNode<'a>>,
        values: Box<ArrowExportNode<'a>>,
        validity: Option<ValidityBitmap<'a>>,
        ordered: bool,
    },
}

impl<'a> ArrowExportNode<'a> {
    pub fn scalar(array: &'a EncodedArray<'a>) -> Self {
        Self::Scalar {
            array,
            dictionary_policy: ArrowDictionaryPolicy::default(),
        }
    }

    pub fn scalar_with_policy(
        array: &'a EncodedArray<'a>,
        dictionary_policy: ArrowDictionaryPolicy,
    ) -> Self {
        Self::Scalar {
            array,
            dictionary_policy,
        }
    }
}

/// Export one layout-aware COVE node as an Arrow array.
pub fn arrow_export_node_to_array(node: &ArrowExportNode<'_>) -> Result<ArrayRef, CoveError> {
    match node {
        ArrowExportNode::Scalar {
            array,
            dictionary_policy,
        } => encoded_array_to_arrow_with_policy(array, *dictionary_policy),
        ArrowExportNode::List {
            layout,
            child,
            validity,
        } => {
            layout.validate()?;
            let offsets = arrow_i32_offsets(&layout.layout.offsets)?;
            let child_array = arrow_export_node_to_array(child)?;
            if child_array.len()
                != usize::try_from(layout.child_row_count).map_err(|_| CoveError::ArithOverflow)?
            {
                return Err(CoveError::PageCorrupt);
            }
            let row_count = layout.layout.row_count();
            let nulls = arrow_null_buffer(*validity, row_count)?;
            let field = Arc::new(Field::new(
                "item",
                child_array.data_type().clone(),
                arrow_node_nullable(child),
            ));
            ListArray::try_new(field, offsets, child_array, nulls)
                .map(|array| Arc::new(array) as ArrayRef)
                .map_err(|err| CoveError::BadSection(format!("Arrow ListArray: {err}")))
        }
        ArrowExportNode::Struct {
            layout,
            fields,
            validity,
        } => {
            let row_count = usize::try_from(layout.layout.row_count()?)
                .map_err(|_| CoveError::ArithOverflow)?;
            layout.validate(row_count as u64)?;
            if fields.len() != layout.layout.field_row_counts.len() {
                return Err(CoveError::PageCorrupt);
            }

            let mut arrow_fields = Vec::with_capacity(fields.len());
            let mut arrays = Vec::with_capacity(fields.len());
            for (index, column) in fields.iter().enumerate() {
                let array = arrow_export_node_to_array(&column.node)?;
                let expected = usize::try_from(layout.layout.field_row_counts[index])
                    .map_err(|_| CoveError::ArithOverflow)?;
                if array.len() != expected || expected != row_count {
                    return Err(CoveError::PageCorrupt);
                }
                arrow_fields.push(Field::new(
                    column.name,
                    array.data_type().clone(),
                    column.nullable || arrow_node_nullable(&column.node),
                ));
                arrays.push(array);
            }
            let nulls = arrow_null_buffer(*validity, row_count)?;
            StructArray::try_new(Fields::from(arrow_fields), arrays, nulls)
                .map(|array| Arc::new(array) as ArrayRef)
                .map_err(|err| CoveError::BadSection(format!("Arrow StructArray: {err}")))
        }
        ArrowExportNode::Map {
            layout,
            keys,
            values,
            validity,
            ordered,
        } => {
            layout.validate()?;
            let offsets = arrow_i32_offsets(&layout.layout.offsets)?;
            if !matches!(keys.as_ref(), ArrowExportNode::Scalar { .. }) {
                return Err(CoveError::PageCorrupt);
            }
            let key_array = arrow_export_node_to_array(keys)?;
            if key_array.null_count() != 0 {
                return Err(CoveError::UnsupportedEncoding(
                    "Arrow map export requires non-null map keys".into(),
                ));
            }
            let value_array = arrow_export_node_to_array(values)?;
            let key_count = usize::try_from(layout.layout.key_row_count)
                .map_err(|_| CoveError::ArithOverflow)?;
            let value_count = usize::try_from(layout.layout.value_row_count)
                .map_err(|_| CoveError::ArithOverflow)?;
            if key_array.len() != key_count || value_array.len() != value_count {
                return Err(CoveError::PageCorrupt);
            }
            let entry_fields = Fields::from(vec![
                Field::new("key", key_array.data_type().clone(), false),
                Field::new(
                    "value",
                    value_array.data_type().clone(),
                    arrow_node_nullable(values),
                ),
            ]);
            let entries = StructArray::try_new(entry_fields, vec![key_array, value_array], None)
                .map_err(|err| CoveError::BadSection(format!("Arrow Map entries: {err}")))?;
            let row_count = layout.layout.row_count();
            let nulls = arrow_null_buffer(*validity, row_count)?;
            let entries_field = Arc::new(Field::new("entries", entries.data_type().clone(), false));
            MapArray::try_new(entries_field, offsets, entries, nulls, *ordered)
                .map(|array| Arc::new(array) as ArrayRef)
                .map_err(|err| CoveError::BadSection(format!("Arrow MapArray: {err}")))
        }
    }
}

/// Export layout-aware COVE columns as an Arrow [`RecordBatch`].
pub fn arrow_export_columns_to_record_batch(
    columns: &[ArrowExportColumn<'_>],
) -> Result<RecordBatch, CoveError> {
    let mut fields = Vec::with_capacity(columns.len());
    let mut arrays = Vec::with_capacity(columns.len());
    for column in columns {
        let arrow_array = arrow_export_node_to_array(&column.node)?;
        fields.push(Field::new(
            column.name,
            arrow_array.data_type().clone(),
            column.nullable || arrow_node_nullable(&column.node),
        ));
        arrays.push(arrow_array);
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
        .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))
}

/// Export one decoded COVE array view as an Arrow array.
pub fn encoded_array_to_arrow(array: &EncodedArray<'_>) -> Result<ArrayRef, CoveError> {
    let result = encoded_array_to_arrow_with_options(array, ArrowExportOptions::default())?;
    if result.report.has_lossy_or_unsupported() {
        return Err(CoveError::UnsupportedEncoding(format!(
            "Arrow export for {:?} requires explicit fidelity reporting",
            array.logical
        )));
    }
    Ok(result.value)
}

/// Export one scalar COVE array with explicit dictionary handling.
pub fn encoded_array_to_arrow_with_policy(
    array: &EncodedArray<'_>,
    dictionary_policy: ArrowDictionaryPolicy,
) -> Result<ArrayRef, CoveError> {
    let result = encoded_array_to_arrow_with_options(
        array,
        ArrowExportOptions {
            dictionary_policy,
            ..ArrowExportOptions::default()
        },
    )?;
    if result.report.has_lossy_or_unsupported() {
        return Err(CoveError::UnsupportedEncoding(format!(
            "Arrow export for {:?} requires explicit fidelity reporting",
            array.logical
        )));
    }
    Ok(result.value)
}

/// Export one scalar COVE array and return representation-fidelity diagnostics.
pub fn encoded_array_to_arrow_with_report(
    array: &EncodedArray<'_>,
) -> Result<ArrowExportResult<ArrayRef>, CoveError> {
    encoded_array_to_arrow_with_options(array, ArrowExportOptions::default())
}

/// Export one scalar COVE array with explicit Arrow export options and diagnostics.
pub fn encoded_array_to_arrow_with_options(
    array: &EncodedArray<'_>,
    options: ArrowExportOptions,
) -> Result<ArrowExportResult<ArrayRef>, CoveError> {
    let mut report = ArrowExportReport::default();
    if options.dictionary_policy == ArrowDictionaryPolicy::DictionaryKeys {
        if let Some(dictionary_array) = try_filecode_dictionary_array(array)? {
            report.push(
                None,
                array.logical,
                ArrowFidelitySeverity::Informational,
                "FileCode values exported as Arrow dictionary keys",
            );
            return Ok(ArrowExportResult {
                value: dictionary_array,
                report,
            });
        }
    }
    let values = array.decode_all_rows()?;
    let array_ref = match arrow_data_type_with_report(array.logical, &options, &mut report)? {
        DataType::Boolean => Arc::new(BooleanArray::from(collect_bool(&values)?)) as ArrayRef,
        DataType::Int8 => Arc::new(Int8Array::from(collect_i64(array.logical, &values, |v| {
            i8::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?)) as ArrayRef,
        DataType::Int16 => Arc::new(Int16Array::from(collect_i64(
            array.logical,
            &values,
            |v| i16::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?)) as ArrayRef,
        DataType::Int32 => Arc::new(Int32Array::from(collect_i64(
            array.logical,
            &values,
            |v| i32::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?)) as ArrayRef,
        DataType::Int64 => {
            Arc::new(Int64Array::from(collect_i64(array.logical, &values, Ok)?)) as ArrayRef
        }
        DataType::UInt8 => Arc::new(UInt8Array::from(collect_u64(
            array.logical,
            &values,
            |v| u8::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?)) as ArrayRef,
        DataType::UInt16 => Arc::new(UInt16Array::from(collect_u64(
            array.logical,
            &values,
            |v| u16::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?)) as ArrayRef,
        DataType::UInt32 => Arc::new(UInt32Array::from(collect_u64(
            array.logical,
            &values,
            |v| u32::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?)) as ArrayRef,
        DataType::UInt64 => {
            Arc::new(UInt64Array::from(collect_u64(array.logical, &values, Ok)?)) as ArrayRef
        }
        DataType::Float32 => Arc::new(Float32Array::from(collect_f32(&values)?)) as ArrayRef,
        DataType::Float64 => Arc::new(Float64Array::from(collect_f64(&values)?)) as ArrayRef,
        DataType::Date32 => Arc::new(Date32Array::from(collect_i64(
            array.logical,
            &values,
            |v| i32::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?)) as ArrayRef,
        DataType::Timestamp(TimeUnit::Microsecond, None) => Arc::new(
            TimestampMicrosecondArray::from(collect_i64(array.logical, &values, Ok)?),
        ) as ArrayRef,
        DataType::Timestamp(TimeUnit::Nanosecond, None) => Arc::new(TimestampNanosecondArray::from(
            collect_i64(array.logical, &values, Ok)?,
        )) as ArrayRef,
        DataType::Utf8 => Arc::new(collect_utf8(array.logical, &values)?) as ArrayRef,
        DataType::Binary => Arc::new(collect_binary(array.logical, &values)?) as ArrayRef,
        DataType::FixedSizeBinary(size) => {
            Arc::new(collect_fixed_size_binary(array.logical, &values, size)?) as ArrayRef
        }
        DataType::Decimal128(precision, scale) => Arc::new(
            Decimal128Array::from(collect_i128(array.logical, &values)?)
                .with_precision_and_scale(precision, scale)
                .map_err(|err| CoveError::BadSection(format!("Arrow Decimal128: {err}")))?,
        ) as ArrayRef,
        other => {
            return Err(CoveError::UnsupportedEncoding(format!(
                "Arrow export for {other:?}"
            )));
        }
    };
    Ok(ArrowExportResult {
        value: array_ref,
        report,
    })
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

/// Export named COVE array views as an Arrow [`RecordBatch`] with fidelity diagnostics.
pub fn encoded_columns_to_record_batch_with_report(
    columns: &[(&str, &EncodedArray<'_>)],
) -> Result<ArrowExportResult<RecordBatch>, CoveError> {
    encoded_columns_to_record_batch_with_options(columns, ArrowExportOptions::default())
}

/// Export named COVE array views with explicit Arrow export options and diagnostics.
pub fn encoded_columns_to_record_batch_with_options(
    columns: &[(&str, &EncodedArray<'_>)],
    options: ArrowExportOptions,
) -> Result<ArrowExportResult<RecordBatch>, CoveError> {
    let mut fields = Vec::with_capacity(columns.len());
    let mut arrays = Vec::with_capacity(columns.len());
    let mut report = ArrowExportReport::default();
    for (name, array) in columns {
        let result = encoded_array_to_arrow_with_options(array, options)?;
        report.extend_with_field(name, result.report);
        fields.push(arrow_field_for_cove(
            *name,
            result.value.data_type().clone(),
            array.validity.is_some() || array.logical == CoveLogicalType::Null,
            array.logical,
            options,
        ));
        arrays.push(result.value);
    }
    let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
        .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))?;
    Ok(ArrowExportResult {
        value: batch,
        report,
    })
}

fn arrow_node_nullable(node: &ArrowExportNode<'_>) -> bool {
    match node {
        ArrowExportNode::Scalar { array, .. } => {
            array.validity.is_some() || array.logical == CoveLogicalType::Null
        }
        ArrowExportNode::List { validity, .. }
        | ArrowExportNode::Struct { validity, .. }
        | ArrowExportNode::Map { validity, .. } => validity.is_some(),
    }
}

fn arrow_field_for_cove(
    name: &str,
    data_type: DataType,
    nullable: bool,
    logical: CoveLogicalType,
    options: ArrowExportOptions,
) -> Field {
    let field = Field::new(name, data_type, nullable);
    let metadata = arrow_extension_metadata(logical, options);
    if metadata.is_empty() {
        field
    } else {
        field.with_metadata(metadata)
    }
}

fn arrow_extension_metadata(
    logical: CoveLogicalType,
    options: ArrowExportOptions,
) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    match logical {
        CoveLogicalType::Uuid if options.emit_uuid_extension_metadata => {
            metadata.insert("ARROW:extension:name".into(), "cove.uuid".into());
            metadata.insert(
                "ARROW:extension:metadata".into(),
                r#"{"storage":"fixed_size_binary[16]"}"#.into(),
            );
        }
        CoveLogicalType::Json if options.emit_json_extension_metadata => {
            metadata.insert("ARROW:extension:name".into(), "cove.json".into());
            metadata.insert(
                "ARROW:extension:metadata".into(),
                r#"{"storage":"utf8"}"#.into(),
            );
        }
        _ => {}
    }
    metadata
}

fn arrow_i32_offsets(offsets: &[u32]) -> Result<OffsetBuffer<i32>, CoveError> {
    let mut converted = Vec::with_capacity(offsets.len());
    for &offset in offsets {
        converted.push(i32::try_from(offset).map_err(|_| {
            CoveError::UnsupportedEncoding(
                "Arrow ListArray/MapArray export requires i32 offsets; chunk the column first"
                    .into(),
            )
        })?);
    }
    Ok(OffsetBuffer::new(ScalarBuffer::from(converted)))
}

fn arrow_null_buffer(
    validity: Option<ValidityBitmap<'_>>,
    row_count: usize,
) -> Result<Option<NullBuffer>, CoveError> {
    let Some(validity) = validity else {
        return Ok(None);
    };
    let row_count_u64 = u64::try_from(row_count).map_err(|_| CoveError::ArithOverflow)?;
    validity.validate_len(row_count_u64)?;
    let mut any_null = false;
    let mut bits = Vec::with_capacity(row_count);
    for row in 0..row_count_u64 {
        let valid = validity.is_valid(row)?;
        any_null |= !valid;
        bits.push(valid);
    }
    if any_null {
        Ok(Some(NullBuffer::from(bits)))
    } else {
        Ok(None)
    }
}

fn try_filecode_dictionary_array(array: &EncodedArray<'_>) -> Result<Option<ArrayRef>, CoveError> {
    if array.encoding != crate::constants::CoveEncodingKind::FileCode {
        return Ok(None);
    }
    let Some(dictionary) = array.dictionary else {
        return Ok(None);
    };
    let values = file_dictionary_values_to_arrow(array.logical, dictionary)?;
    let mut keys = Vec::with_capacity(usize::try_from(array.row_count).map_err(|_| {
        CoveError::UnsupportedEncoding("Arrow export row count exceeds usize".into())
    })?);
    for row in 0..array.row_count {
        if array.is_null(row)? {
            keys.push(None);
            continue;
        }
        let offset = usize::try_from(row)
            .map_err(|_| CoveError::ArithOverflow)?
            .checked_mul(4)
            .ok_or(CoveError::ArithOverflow)?;
        keys.push(Some(read_u32_le(array.data, offset)?));
    }
    let keys = UInt32Array::from(keys);
    DictionaryArray::<UInt32Type>::try_new(keys, values)
        .map(|array| Some(Arc::new(array) as ArrayRef))
        .map_err(|err| CoveError::BadSection(format!("Arrow DictionaryArray: {err}")))
}

fn file_dictionary_values_to_arrow(
    logical: CoveLogicalType,
    dictionary: &crate::dictionary::FileDictionary,
) -> Result<ArrayRef, CoveError> {
    let mut values = Vec::with_capacity(dictionary.entries.len());
    for code in 0..dictionary.len() {
        values.push(CoveArrayValue::DictValue(dictionary.decode_value(code)?));
    }
    match arrow_data_type(logical)? {
        DataType::Boolean => Ok(Arc::new(BooleanArray::from(collect_bool(&values)?))),
        DataType::Int8 => Ok(Arc::new(Int8Array::from(collect_i64(
            logical,
            &values,
            |v| i8::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?))),
        DataType::Int16 => Ok(Arc::new(Int16Array::from(collect_i64(
            logical,
            &values,
            |v| i16::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?))),
        DataType::Int32 => Ok(Arc::new(Int32Array::from(collect_i64(
            logical,
            &values,
            |v| i32::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?))),
        DataType::Date32 => Ok(Arc::new(Date32Array::from(collect_i64(
            logical,
            &values,
            |v| i32::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?))),
        DataType::Int64 => Ok(Arc::new(Int64Array::from(collect_i64(
            logical, &values, Ok,
        )?))),
        DataType::UInt8 => Ok(Arc::new(UInt8Array::from(collect_u64(
            logical,
            &values,
            |v| u8::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?))),
        DataType::UInt16 => Ok(Arc::new(UInt16Array::from(collect_u64(
            logical,
            &values,
            |v| u16::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?))),
        DataType::UInt32 => Ok(Arc::new(UInt32Array::from(collect_u64(
            logical,
            &values,
            |v| u32::try_from(v).map_err(|_| CoveError::PageCorrupt),
        )?))),
        DataType::UInt64 => Ok(Arc::new(UInt64Array::from(collect_u64(
            logical, &values, Ok,
        )?))),
        DataType::Float32 => Ok(Arc::new(Float32Array::from(collect_f32(&values)?))),
        DataType::Float64 => Ok(Arc::new(Float64Array::from(collect_f64(&values)?))),
        DataType::Timestamp(TimeUnit::Microsecond, None) => Ok(Arc::new(
            TimestampMicrosecondArray::from(collect_i64(logical, &values, Ok)?),
        )),
        DataType::Timestamp(TimeUnit::Nanosecond, None) => Ok(Arc::new(
            TimestampNanosecondArray::from(collect_i64(logical, &values, Ok)?),
        )),
        DataType::Utf8 => Ok(Arc::new(collect_utf8(logical, &values)?)),
        DataType::Binary => Ok(Arc::new(collect_binary(logical, &values)?)),
        DataType::FixedSizeBinary(size) => {
            Ok(Arc::new(collect_fixed_size_binary(logical, &values, size)?))
        }
        other => Err(CoveError::UnsupportedEncoding(format!(
            "Arrow dictionary export for {other:?}"
        ))),
    }
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, CoveError> {
    let slice = wire::read_range_checked(bytes, offset, 4)?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

fn arrow_data_type(logical: CoveLogicalType) -> Result<DataType, CoveError> {
    match logical {
        CoveLogicalType::Bool => Ok(DataType::Boolean),
        CoveLogicalType::Int8 => Ok(DataType::Int8),
        CoveLogicalType::Int16 => Ok(DataType::Int16),
        CoveLogicalType::Int32 => Ok(DataType::Int32),
        CoveLogicalType::Int64 => Ok(DataType::Int64),
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
        CoveLogicalType::Binary => Ok(DataType::Binary),
        CoveLogicalType::Uuid => Ok(DataType::FixedSizeBinary(16)),
        other => Err(CoveError::UnsupportedEncoding(format!(
            "Arrow export for {:?}",
            other
        ))),
    }
}

fn arrow_data_type_with_report(
    logical: CoveLogicalType,
    options: &ArrowExportOptions,
    report: &mut ArrowExportReport,
) -> Result<DataType, CoveError> {
    match logical {
        CoveLogicalType::Decimal64 => match options.decimal {
            Some(decimal) => Ok(DataType::Decimal128(decimal.precision, decimal.scale)),
            None => {
                report.push(
                    None,
                    logical,
                    ArrowFidelitySeverity::Lossy,
                    "Decimal64 exported as Int64 because no Arrow decimal precision/scale context was supplied",
                );
                Ok(DataType::Int64)
            }
        },
        CoveLogicalType::Decimal128 => match options.decimal {
            Some(decimal) => Ok(DataType::Decimal128(decimal.precision, decimal.scale)),
            None => {
                report.push(
                    None,
                    logical,
                    ArrowFidelitySeverity::Lossy,
                    "Decimal128 exported as FixedSizeBinary(16) because no Arrow decimal precision/scale context was supplied",
                );
                Ok(DataType::FixedSizeBinary(16))
            }
        },
        CoveLogicalType::Uuid => {
            if !options.emit_uuid_extension_metadata {
                report.push(
                    None,
                    logical,
                    ArrowFidelitySeverity::Informational,
                    "Uuid exported as FixedSizeBinary(16) without Arrow extension metadata",
                );
            }
            Ok(DataType::FixedSizeBinary(16))
        }
        CoveLogicalType::Json => {
            if !options.emit_json_extension_metadata {
                report.push(
                    None,
                    logical,
                    ArrowFidelitySeverity::Informational,
                    "Json exported as Utf8 without Arrow extension metadata",
                );
            }
            Ok(DataType::Utf8)
        }
        other => arrow_data_type(other),
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

fn collect_fixed_size_binary(
    logical: CoveLogicalType,
    values: &[CoveArrayValue<'_>],
    size: i32,
) -> Result<FixedSizeBinaryArray, CoveError> {
    let mut out = Vec::with_capacity(values.len());
    let expected = usize::try_from(size).map_err(|_| CoveError::PageCorrupt)?;
    for value in values {
        match value {
            CoveArrayValue::Null => out.push(None),
            _ => {
                let bytes = value_to_bytes(logical, value)?;
                if bytes.len() != expected {
                    return Err(CoveError::PageCorrupt);
                }
                out.push(Some(bytes.into_owned()));
            }
        }
    }
    FixedSizeBinaryArray::try_from_sparse_iter_with_size(out.into_iter(), size)
        .map_err(|err| CoveError::BadSection(format!("Arrow FixedSizeBinary: {err}")))
}

fn collect_i128(
    logical: CoveLogicalType,
    values: &[CoveArrayValue<'_>],
) -> Result<Vec<Option<i128>>, CoveError> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        out.push(match value {
            CoveArrayValue::Null => None,
            _ => Some(value_to_i128(logical, value)?),
        });
    }
    Ok(out)
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

fn value_to_i128(logical: CoveLogicalType, value: &CoveArrayValue<'_>) -> Result<i128, CoveError> {
    match logical {
        CoveLogicalType::Decimal64 => value_to_i64(logical, value).map(i128::from),
        CoveLogicalType::Decimal128 => {
            let bytes = plain_bytes(value)?;
            exact_bytes::<16>(bytes).map(i128::from_le_bytes)
        }
        _ => Err(unexpected_value("decimal", value)),
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
        encoding::nested::{
            ListLayout, ListLayoutPayload, MapLayout, MapLayoutPayload, StructLayout,
            StructLayoutPayload,
        },
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
    fn strict_export_rejects_decimal_without_precision_scale() {
        let mut values = Vec::new();
        values.extend_from_slice(&123u64.to_le_bytes());
        let cove = EncodedArray::new(
            CoveLogicalType::Decimal64,
            CovePhysicalKind::NumCode,
            1,
            CoveEncodingKind::NumCode,
            None,
            &values,
            None,
        );

        assert!(matches!(
            encoded_array_to_arrow(&cove),
            Err(CoveError::UnsupportedEncoding(_))
        ));
        let result = encoded_array_to_arrow_with_report(&cove).unwrap();
        assert!(result.report.has_lossy_or_unsupported());
        assert_eq!(result.value.data_type(), &DataType::Int64);
    }

    #[test]
    fn exports_decimal128_with_explicit_context() {
        let mut values = Vec::new();
        values.extend_from_slice(&12345i128.to_le_bytes());
        let cove = EncodedArray::new(
            CoveLogicalType::Decimal128,
            CovePhysicalKind::FixedBytes,
            1,
            CoveEncodingKind::PlainFixed,
            None,
            &values,
            None,
        );

        let result = encoded_array_to_arrow_with_options(
            &cove,
            ArrowExportOptions {
                decimal: Some(ArrowDecimalContext {
                    precision: 10,
                    scale: 2,
                }),
                ..ArrowExportOptions::default()
            },
        )
        .unwrap();
        assert!(result.report.issues.is_empty());
        assert_eq!(result.value.data_type(), &DataType::Decimal128(10, 2));
    }

    #[test]
    fn exports_uuid_as_fixed_size_binary() {
        let values = [7u8; 16];
        let cove = EncodedArray::new(
            CoveLogicalType::Uuid,
            CovePhysicalKind::FixedBytes,
            1,
            CoveEncodingKind::PlainFixed,
            None,
            &values,
            None,
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        assert_eq!(arrow.data_type(), &DataType::FixedSizeBinary(16));
        let uuids = arrow
            .as_any()
            .downcast_ref::<FixedSizeBinaryArray>()
            .unwrap();
        assert_eq!(uuids.value(0), &[7u8; 16]);
    }

    #[test]
    fn record_batch_emits_uuid_extension_metadata_when_requested() {
        let values = [7u8; 16];
        let cove = EncodedArray::new(
            CoveLogicalType::Uuid,
            CovePhysicalKind::FixedBytes,
            1,
            CoveEncodingKind::PlainFixed,
            None,
            &values,
            None,
        );

        let result = encoded_columns_to_record_batch_with_options(
            &[("id", &cove)],
            ArrowExportOptions {
                emit_uuid_extension_metadata: true,
                ..ArrowExportOptions::default()
            },
        )
        .unwrap();
        let schema = result.value.schema();
        let field = schema.field(0);
        assert_eq!(
            field.metadata().get("ARROW:extension:name"),
            Some(&"cove.uuid".to_string())
        );
        assert!(result.report.issues.is_empty());
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
    fn exports_filecode_dictionary_keys_when_requested() {
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

        let arrow = arrow_export_node_to_array(&ArrowExportNode::scalar(&cove)).unwrap();
        let dictionary = arrow
            .as_any()
            .downcast_ref::<DictionaryArray<UInt32Type>>()
            .unwrap();
        assert_eq!(dictionary.keys().value(0), 1);
        assert_eq!(dictionary.keys().value(1), 0);
        let values = dictionary
            .values()
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(values.value(0), "red");
        assert_eq!(values.value(1), "blue");
    }

    #[test]
    fn exports_list_of_int32() {
        let values = [1i32.to_le_bytes(), 2i32.to_le_bytes(), 3i32.to_le_bytes()].concat();
        let child = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            3,
            CoveEncodingKind::PlainFixed,
            None,
            &values,
            None,
        );
        let layout = ListLayoutPayload {
            layout: ListLayout {
                offsets: vec![0, 2, 3],
            },
            child_row_count: 3,
        };
        let node = ArrowExportNode::List {
            layout: &layout,
            child: Box::new(ArrowExportNode::scalar(&child)),
            validity: None,
        };

        let arrow = arrow_export_node_to_array(&node).unwrap();
        let lists = arrow.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(lists.len(), 2);
        assert_eq!(lists.value(0).len(), 2);
        assert_eq!(lists.value(1).len(), 1);
    }

    #[test]
    fn exports_nullable_list() {
        let values = [1i32.to_le_bytes(), 2i32.to_le_bytes()].concat();
        let child = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            2,
            CoveEncodingKind::PlainFixed,
            None,
            &values,
            None,
        );
        let layout = ListLayoutPayload {
            layout: ListLayout {
                offsets: vec![0, 2, 2],
            },
            child_row_count: 2,
        };
        let mut validity = ValidityBitmapBuilder::new(2).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 2);
        let node = ArrowExportNode::List {
            layout: &layout,
            child: Box::new(ArrowExportNode::scalar(&child)),
            validity: Some(bitmap),
        };

        let arrow = arrow_export_node_to_array(&node).unwrap();
        let lists = arrow.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(lists.len(), 2);
        assert!(lists.is_null(1));
    }

    #[test]
    fn exports_struct_with_nullable_parent() {
        let ids = [10i32.to_le_bytes(), 20i32.to_le_bytes()].concat();
        let id_array = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            2,
            CoveEncodingKind::PlainFixed,
            None,
            &ids,
            None,
        );
        let flags = [1u8, 0u8];
        let flag_array = EncodedArray::new(
            CoveLogicalType::Bool,
            CovePhysicalKind::FixedBytes,
            2,
            CoveEncodingKind::PlainFixed,
            None,
            &flags,
            None,
        );
        let layout = StructLayoutPayload {
            layout: StructLayout {
                field_row_counts: vec![2, 2],
            },
            parent_null_handling_declared: true,
        };
        let mut validity = ValidityBitmapBuilder::new(2).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 2);
        let node = ArrowExportNode::Struct {
            layout: &layout,
            fields: vec![
                ArrowExportColumn::scalar("id", &id_array),
                ArrowExportColumn::scalar("flag", &flag_array),
            ],
            validity: Some(bitmap),
        };

        let arrow = arrow_export_node_to_array(&node).unwrap();
        let structs = arrow.as_any().downcast_ref::<StructArray>().unwrap();
        assert_eq!(structs.len(), 2);
        assert!(structs.is_null(1));
    }

    #[test]
    fn exports_map_with_scalar_keys() {
        let mut keys_data = Vec::new();
        keys_data.extend_from_slice(&1u32.to_le_bytes());
        keys_data.extend_from_slice(b"a");
        keys_data.extend_from_slice(&1u32.to_le_bytes());
        keys_data.extend_from_slice(b"b");
        let keys = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            &keys_data,
            None,
        );
        let values_data = [7i32.to_le_bytes(), 9i32.to_le_bytes()].concat();
        let values = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            2,
            CoveEncodingKind::PlainFixed,
            None,
            &values_data,
            None,
        );
        let layout = MapLayoutPayload {
            layout: MapLayout {
                offsets: vec![0, 2],
                key_row_count: 2,
                value_row_count: 2,
                keys_are_scalar: true,
                allow_duplicate_keys: false,
                canonical_keys: vec![b"a".to_vec(), b"b".to_vec()],
            },
        };
        let node = ArrowExportNode::Map {
            layout: &layout,
            keys: Box::new(ArrowExportNode::scalar(&keys)),
            values: Box::new(ArrowExportNode::scalar(&values)),
            validity: None,
            ordered: false,
        };

        let arrow = arrow_export_node_to_array(&node).unwrap();
        let map = arrow.as_any().downcast_ref::<MapArray>().unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map.entries().len(), 2);
    }

    #[test]
    fn rejects_duplicate_map_keys() {
        let key_bytes = [0u8; 10];
        let keys = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            &key_bytes,
            None,
        );
        let value_bytes = [0u8; 8];
        let values = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            2,
            CoveEncodingKind::PlainFixed,
            None,
            &value_bytes,
            None,
        );
        let layout = MapLayoutPayload {
            layout: MapLayout {
                offsets: vec![0, 2],
                key_row_count: 2,
                value_row_count: 2,
                keys_are_scalar: true,
                allow_duplicate_keys: false,
                canonical_keys: vec![b"a".to_vec(), b"a".to_vec()],
            },
        };
        let node = ArrowExportNode::Map {
            layout: &layout,
            keys: Box::new(ArrowExportNode::scalar(&keys)),
            values: Box::new(ArrowExportNode::scalar(&values)),
            validity: None,
            ordered: false,
        };

        assert!(matches!(
            arrow_export_node_to_array(&node),
            Err(CoveError::PageCorrupt)
        ));
    }

    #[test]
    fn rejects_null_map_key_arrow_export() {
        let mut keys_data = Vec::new();
        keys_data.extend_from_slice(&1u32.to_le_bytes());
        keys_data.extend_from_slice(b"a");
        let mut validity = ValidityBitmapBuilder::new(1).unwrap();
        validity.set_null(0).unwrap();
        let validity_bytes = validity.into_bytes();
        let key_bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 1);
        let keys = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            1,
            CoveEncodingKind::VarBytes,
            Some(key_bitmap),
            &keys_data,
            None,
        );
        let values_data = [7i32.to_le_bytes()].concat();
        let values = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            1,
            CoveEncodingKind::PlainFixed,
            None,
            &values_data,
            None,
        );
        let layout = MapLayoutPayload {
            layout: MapLayout {
                offsets: vec![0, 1],
                key_row_count: 1,
                value_row_count: 1,
                keys_are_scalar: true,
                allow_duplicate_keys: false,
                canonical_keys: vec![b"a".to_vec()],
            },
        };
        let node = ArrowExportNode::Map {
            layout: &layout,
            keys: Box::new(ArrowExportNode::scalar(&keys)),
            values: Box::new(ArrowExportNode::scalar(&values)),
            validity: None,
            ordered: false,
        };

        assert!(matches!(
            arrow_export_node_to_array(&node),
            Err(CoveError::UnsupportedEncoding(_))
        ));
    }

    #[test]
    fn rejects_oversized_arrow_offsets() {
        let values = [1i32.to_le_bytes()].concat();
        let child = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            1,
            CoveEncodingKind::PlainFixed,
            None,
            &values,
            None,
        );
        let layout = ListLayoutPayload {
            layout: ListLayout {
                offsets: vec![0, i32::MAX as u32 + 1],
            },
            child_row_count: i32::MAX as u32 + 1,
        };
        let node = ArrowExportNode::List {
            layout: &layout,
            child: Box::new(ArrowExportNode::scalar(&child)),
            validity: None,
        };

        assert!(matches!(
            arrow_export_node_to_array(&node),
            Err(CoveError::UnsupportedEncoding(_))
        ));
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
