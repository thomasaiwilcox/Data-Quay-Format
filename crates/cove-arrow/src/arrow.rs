//! Spec §49 — Arrow interop helpers.
//!
//! COVE stores nulls as a *null* bitmap (bit set ⇒ null), Arrow stores them as
//! a *validity* bitmap (bit set ⇒ valid). This module owns the bit inversion
//! and byte-aligned conversion required to bridge the two.

use std::{borrow::Cow, collections::HashMap, sync::Arc};

use arrow_array::{
    builder::{BinaryBuilder, StringBuilder},
    types::{GenericBinaryType, GenericStringType, UInt32Type},
    Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, DictionaryArray,
    FixedSizeBinaryArray, Float32Array, Float64Array, GenericByteArray, Int16Array, Int32Array,
    Int64Array, Int8Array, ListArray, MapArray, RecordBatch, StructArray,
    TimestampMicrosecondArray, TimestampNanosecondArray, UInt16Array, UInt32Array, UInt64Array,
    UInt8Array,
};
use arrow_buffer::{Buffer, NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow_schema::{DataType, Field, Fields, Schema, TimeUnit};

use crate::{
    array::{CoveArrayValue, EncodedArray},
    constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
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
#[non_exhaustive]
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
#[non_exhaustive]
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

/// Page-local row selection for Arrow export.
///
/// INVARIANT: rows are COVE page ordinals. Bitsets must cover exactly the
/// source page length so dense predicate selections can cross the cove-arrow
/// boundary without first materialising a row-index vector.
#[derive(Debug, Clone, Copy)]
pub enum ArrowRowSelection<'a> {
    All,
    Rows(&'a [u32]),
    Bitset { words: &'a [u64], len: usize },
}

impl<'a> ArrowRowSelection<'a> {
    fn is_all_rows(self, row_count: u64) -> Result<bool, CoveError> {
        match self {
            Self::All => Ok(true),
            Self::Rows(rows) => Ok(selected_rows_are_all_rows(rows, row_count)),
            Self::Bitset { words, len } => {
                self.validate_for_row_count(row_count)?;
                Ok(count_bitset_rows(words, len)? == len)
            }
        }
    }

    fn selected_len(self, row_count: u64) -> Result<usize, CoveError> {
        match self {
            Self::All => usize::try_from(row_count).map_err(|_| CoveError::ArithOverflow),
            Self::Rows(rows) => Ok(rows.len()),
            Self::Bitset { words, len } => {
                self.validate_for_row_count(row_count)?;
                count_bitset_rows(words, len)
            }
        }
    }

    fn validate_for_row_count(self, row_count: u64) -> Result<(), CoveError> {
        match self {
            Self::All => usize::try_from(row_count)
                .map(|_| ())
                .map_err(|_| CoveError::ArithOverflow),
            Self::Rows(rows) => {
                for row in rows {
                    if u64::from(*row) >= row_count {
                        return Err(CoveError::OffsetRange);
                    }
                }
                Ok(())
            }
            Self::Bitset { words, len } => {
                if u64::try_from(len).map_err(|_| CoveError::ArithOverflow)? != row_count {
                    return Err(CoveError::OffsetRange);
                }
                let word_len = len.div_ceil(64);
                if words.len() < word_len {
                    return Err(CoveError::BufferTooShort);
                }
                Ok(())
            }
        }
    }

    fn for_each_row<F>(self, row_count: u64, mut visit: F) -> Result<(), CoveError>
    where
        F: FnMut(usize) -> Result<(), CoveError>,
    {
        match self {
            Self::All => {
                let row_count = usize::try_from(row_count).map_err(|_| CoveError::ArithOverflow)?;
                for row in 0..row_count {
                    visit(row)?;
                }
            }
            Self::Rows(rows) => {
                for row in rows {
                    if u64::from(*row) >= row_count {
                        return Err(CoveError::OffsetRange);
                    }
                    visit(usize::try_from(*row).map_err(|_| CoveError::ArithOverflow)?)?;
                }
            }
            Self::Bitset { words, len } => {
                self.validate_for_row_count(row_count)?;
                let word_len = len.div_ceil(64);
                for (word_index, raw_word) in words.iter().take(word_len).copied().enumerate() {
                    let mut word = if word_index + 1 == word_len {
                        mask_selection_tail(raw_word, len)
                    } else {
                        raw_word
                    };
                    while word != 0 {
                        let bit = word.trailing_zeros() as usize;
                        let row = word_index
                            .checked_mul(64)
                            .and_then(|base| base.checked_add(bit))
                            .ok_or(CoveError::ArithOverflow)?;
                        visit(row)?;
                        word &= word - 1;
                    }
                }
            }
        }
        Ok(())
    }

    fn to_rows(self, row_count: u64) -> Result<Vec<u32>, CoveError> {
        let mut rows = Vec::with_capacity(self.selected_len(row_count)?);
        self.for_each_row(row_count, |row| {
            rows.push(u32::try_from(row).map_err(|_| CoveError::ArithOverflow)?);
            Ok(())
        })?;
        Ok(rows)
    }
}

fn count_bitset_rows(words: &[u64], len: usize) -> Result<usize, CoveError> {
    let word_len = len.div_ceil(64);
    if words.len() < word_len {
        return Err(CoveError::BufferTooShort);
    }
    let mut count = 0usize;
    for (word_index, raw_word) in words.iter().take(word_len).copied().enumerate() {
        let word = if word_index + 1 == word_len {
            mask_selection_tail(raw_word, len)
        } else {
            raw_word
        };
        count = count
            .checked_add(word.count_ones() as usize)
            .ok_or(CoveError::ArithOverflow)?;
    }
    Ok(count)
}

fn mask_selection_tail(word: u64, len: usize) -> u64 {
    let tail_bits = len % 64;
    if tail_bits == 0 {
        word
    } else {
        word & ((1u64 << tail_bits) - 1)
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
#[non_exhaustive]
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
    let arrow_type = arrow_data_type_with_report(array.logical, &options, &mut report)?;
    if let Some(array_ref) = try_direct_byte_array(array, &arrow_type)? {
        return Ok(ArrowExportResult {
            value: array_ref,
            report,
        });
    }
    if let Some(array_ref) = try_direct_primitive_array(array, &arrow_type)? {
        return Ok(ArrowExportResult {
            value: array_ref,
            report,
        });
    }
    let values = array.decode_all_rows()?;
    let array_ref = match arrow_type {
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

/// Export selected rows from one scalar COVE array.
///
/// INVARIANT: `selected_rows` are page-local row ordinals. The function never
/// silently wraps or clamps an out-of-range ordinal because that would create
/// a wrong-row projection at the Arrow boundary.
pub fn encoded_array_to_arrow_selected_with_options(
    array: &EncodedArray<'_>,
    selected_rows: &[u32],
    options: ArrowExportOptions,
) -> Result<ArrowExportResult<ArrayRef>, CoveError> {
    encoded_array_to_arrow_with_row_selection_options(
        array,
        ArrowRowSelection::Rows(selected_rows),
        options,
    )
}

/// Export rows from one scalar COVE array using a page-local row selection.
pub fn encoded_array_to_arrow_with_row_selection_options(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    options: ArrowExportOptions,
) -> Result<ArrowExportResult<ArrayRef>, CoveError> {
    if selection.is_all_rows(array.row_count)? {
        return encoded_array_to_arrow_with_options(array, options);
    }
    let mut report = ArrowExportReport::default();
    if options.dictionary_policy == ArrowDictionaryPolicy::DictionaryKeys {
        if let Some(dictionary_array) =
            try_filecode_dictionary_array_for_selection(array, selection)?
        {
            report.push(
                None,
                array.logical,
                ArrowFidelitySeverity::Informational,
                "selected FileCode values exported as Arrow dictionary keys",
            );
            return Ok(ArrowExportResult {
                value: dictionary_array,
                report,
            });
        }
    }
    let arrow_type = arrow_data_type_with_report(array.logical, &options, &mut report)?;
    if let Some(array_ref) = try_direct_byte_array_for_selection(array, selection, &arrow_type)? {
        return Ok(ArrowExportResult {
            value: array_ref,
            report,
        });
    }
    if let Some(array_ref) =
        try_direct_primitive_array_for_selection(array, selection, &arrow_type)?
    {
        return Ok(ArrowExportResult {
            value: array_ref,
            report,
        });
    }
    let prepared = array.prepare()?;
    let selected_rows = selection.to_rows(array.row_count)?;
    let values = prepared.decode_selected_rows(&selected_rows)?;
    let array_ref = values_to_arrow_array_with_data_type(array.logical, &values, arrow_type)?;
    Ok(ArrowExportResult {
        value: array_ref,
        report,
    })
}

/// Export named COVE array views as an Arrow [`RecordBatch`] using a page-local
/// row selection.
pub fn encoded_columns_to_record_batch_selected_with_options(
    columns: &[(&str, &EncodedArray<'_>)],
    selected_rows: &[u32],
    options: ArrowExportOptions,
) -> Result<ArrowExportResult<RecordBatch>, CoveError> {
    let selection = ArrowRowSelection::Rows(selected_rows);
    let result = encoded_columns_to_arrow_arrays_with_options(columns, selection, options)?;
    let batch = record_batch_from_exported_arrays(columns, result.value, options)?;
    Ok(ArrowExportResult {
        value: batch,
        report: result.report,
    })
}

/// Export named COVE array views as Arrow arrays using a page-local row selection.
pub fn encoded_columns_to_arrow_arrays_with_options(
    columns: &[(&str, &EncodedArray<'_>)],
    selection: ArrowRowSelection<'_>,
    options: ArrowExportOptions,
) -> Result<ArrowExportResult<Vec<ArrayRef>>, CoveError> {
    let mut arrays = Vec::with_capacity(columns.len());
    let mut report = ArrowExportReport::default();
    for (name, array) in columns {
        let result = encoded_array_to_arrow_with_row_selection_options(array, selection, options)?;
        report.extend_with_field(name, result.report);
        arrays.push(result.value);
    }
    Ok(ArrowExportResult {
        value: arrays,
        report,
    })
}

fn selected_rows_are_all_rows(selected_rows: &[u32], row_count: u64) -> bool {
    u64::try_from(selected_rows.len()).ok() == Some(row_count)
        && selected_rows
            .iter()
            .enumerate()
            .all(|(index, row)| u32::try_from(index).ok() == Some(*row))
}

/// Export named COVE array views as an Arrow [`RecordBatch`].
pub fn encoded_columns_to_record_batch(
    columns: &[(&str, &EncodedArray<'_>)],
) -> Result<RecordBatch, CoveError> {
    let result =
        encoded_columns_to_record_batch_with_options(columns, ArrowExportOptions::default())?;
    if result.report.has_lossy_or_unsupported() {
        return Err(CoveError::UnsupportedEncoding(
            "Arrow export requires explicit fidelity reporting".into(),
        ));
    }
    Ok(result.value)
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
    let result =
        encoded_columns_to_arrow_arrays_with_options(columns, ArrowRowSelection::All, options)?;
    let batch = record_batch_from_exported_arrays(columns, result.value, options)?;
    Ok(ArrowExportResult {
        value: batch,
        report: result.report,
    })
}

fn record_batch_from_exported_arrays(
    columns: &[(&str, &EncodedArray<'_>)],
    arrays: Vec<ArrayRef>,
    options: ArrowExportOptions,
) -> Result<RecordBatch, CoveError> {
    let mut fields = Vec::with_capacity(columns.len());
    for ((name, array), arrow_array) in columns.iter().zip(arrays.iter()) {
        fields.push(arrow_field_for_cove(
            name,
            arrow_array.data_type().clone(),
            array.validity.is_some() || array.logical == CoveLogicalType::Null,
            array.logical,
            options,
        ));
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
        .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))
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

fn try_filecode_dictionary_array_for_selection(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<Option<ArrayRef>, CoveError> {
    if array.encoding != crate::constants::CoveEncodingKind::FileCode {
        return Ok(None);
    }
    let Some(dictionary) = array.dictionary else {
        return Ok(None);
    };
    let values = file_dictionary_values_to_arrow(array.logical, dictionary)?;
    let mut keys = Vec::with_capacity(selection.selected_len(array.row_count)?);
    selection.for_each_row(array.row_count, |row| {
        let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
        if array.is_null(row_u64)? {
            keys.push(None);
            return Ok(());
        }
        let offset = row.checked_mul(4).ok_or(CoveError::ArithOverflow)?;
        keys.push(Some(read_u32_le(array.data, offset)?));
        Ok(())
    })?;
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

fn read_u64_le(bytes: &[u8], offset: usize) -> Result<u64, CoveError> {
    let slice = wire::read_range_checked(bytes, offset, 8)?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

#[inline]
fn read_u32_len_prefixed_range(bytes: &[u8], offset: usize) -> Result<(usize, usize), CoveError> {
    let Some(data_start) = offset.checked_add(4) else {
        return Err(CoveError::ArithOverflow);
    };
    if data_start > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    let len = u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]) as usize;
    let Some(data_end) = data_start.checked_add(len) else {
        return Err(CoveError::ArithOverflow);
    };
    if data_end > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    Ok((data_start, data_end))
}

#[inline]
fn read_leb128_len_prefixed_range(
    bytes: &[u8],
    offset: usize,
) -> Result<(usize, usize), CoveError> {
    if offset > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    let (len, consumed) = wire::decode_u64_leb128(&bytes[offset..])?;
    let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
    let Some(data_start) = offset.checked_add(consumed) else {
        return Err(CoveError::ArithOverflow);
    };
    let Some(data_end) = data_start.checked_add(len) else {
        return Err(CoveError::ArithOverflow);
    };
    if data_end > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    Ok((data_start, data_end))
}

#[derive(Debug, Clone, Copy)]
enum BytePayloadLayout {
    U32LengthPrefixed,
    Leb128LengthPrefixed,
}

fn try_direct_byte_array(
    array: &EncodedArray<'_>,
    data_type: &DataType,
) -> Result<Option<ArrayRef>, CoveError> {
    if array.physical != CovePhysicalKind::VarBytes {
        return Ok(None);
    }
    let layout = match array.encoding {
        CoveEncodingKind::VarBytes => BytePayloadLayout::U32LengthPrefixed,
        CoveEncodingKind::Canonical
            if matches!(
                array.logical,
                CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json
            ) =>
        {
            BytePayloadLayout::Leb128LengthPrefixed
        }
        _ => return Ok(None),
    };
    byte_array_from_payload_plan(array, ArrowRowSelection::All, layout, data_type)
}

fn try_direct_byte_array_for_selection(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data_type: &DataType,
) -> Result<Option<ArrayRef>, CoveError> {
    if array.physical != CovePhysicalKind::VarBytes {
        return Ok(None);
    }
    let layout = match array.encoding {
        CoveEncodingKind::VarBytes => BytePayloadLayout::U32LengthPrefixed,
        CoveEncodingKind::Canonical
            if matches!(
                array.logical,
                CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json
            ) =>
        {
            BytePayloadLayout::Leb128LengthPrefixed
        }
        _ => return Ok(None),
    };
    byte_array_from_payload_plan(array, selection, layout, data_type)
}

fn byte_array_from_payload_plan(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    layout: BytePayloadLayout,
    data_type: &DataType,
) -> Result<Option<ArrayRef>, CoveError> {
    if !matches!(data_type, DataType::Utf8 | DataType::Binary) {
        return Ok(None);
    }
    let plan = BytePayloadPlan { layout };
    let (offsets, values, nulls) = plan.materialize(array, selection)?;
    let array_ref = match data_type {
        DataType::Utf8 => Arc::new(
            GenericByteArray::<GenericStringType<i32>>::try_new(offsets, values, nulls)
                .map_err(|err| CoveError::BadSection(format!("Arrow Utf8 export: {err}")))?,
        ) as ArrayRef,
        DataType::Binary => Arc::new(
            GenericByteArray::<GenericBinaryType<i32>>::try_new(offsets, values, nulls)
                .map_err(|err| CoveError::BadSection(format!("Arrow Binary export: {err}")))?,
        ) as ArrayRef,
        _ => unreachable!(),
    };
    Ok(Some(array_ref))
}

struct BytePayloadPlan {
    layout: BytePayloadLayout,
}

impl BytePayloadPlan {
    fn parse_ranges(&self, array: &EncodedArray<'_>) -> Result<Vec<(usize, usize)>, CoveError> {
        let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
        let mut ranges = Vec::with_capacity(row_count);
        let data = array.data;
        let mut pos = 0usize;
        for _ in 0..row_count {
            let (data_start, data_end) = match self.layout {
                BytePayloadLayout::U32LengthPrefixed => read_u32_len_prefixed_range(data, pos)?,
                BytePayloadLayout::Leb128LengthPrefixed => {
                    read_leb128_len_prefixed_range(data, pos)?
                }
            };
            ranges.push((data_start, data_end));
            pos = data_end;
        }
        if pos != data.len() {
            return Err(CoveError::PageCorrupt);
        }
        Ok(ranges)
    }

    fn materialize(
        &self,
        array: &EncodedArray<'_>,
        selection: ArrowRowSelection<'_>,
    ) -> Result<(OffsetBuffer<i32>, Buffer, Option<NullBuffer>), CoveError> {
        if matches!(selection, ArrowRowSelection::All) {
            return self.materialize_all_rows(array);
        }
        let ranges = self.parse_ranges(array)?;
        self.materialize_selected(array, selection, &ranges)
    }

    fn materialize_all_rows(
        &self,
        array: &EncodedArray<'_>,
    ) -> Result<(OffsetBuffer<i32>, Buffer, Option<NullBuffer>), CoveError> {
        let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
        let has_nulls = match array.validity {
            Some(validity) => validity.null_count()? > 0,
            None => false,
        };
        let Some(offset_capacity) = row_count.checked_add(1) else {
            return Err(CoveError::ArithOverflow);
        };
        let mut offsets = Vec::with_capacity(offset_capacity);
        let mut values = Vec::with_capacity(array.data.len());
        let mut valid_bits = has_nulls.then(|| Vec::with_capacity(row_count));
        offsets.push(0i32);

        let data = array.data;
        let mut pos = 0usize;
        for row in 0..array.row_count {
            let (data_start, data_end) = match self.layout {
                BytePayloadLayout::U32LengthPrefixed => read_u32_len_prefixed_range(data, pos)?,
                BytePayloadLayout::Leb128LengthPrefixed => {
                    read_leb128_len_prefixed_range(data, pos)?
                }
            };
            pos = data_end;

            let is_null = has_nulls && array.is_null(row)?;
            if let Some(bits) = &mut valid_bits {
                bits.push(!is_null);
            }
            if !is_null {
                values.extend_from_slice(&data[data_start..data_end]);
            }
            offsets.push(i32::try_from(values.len()).map_err(|_| CoveError::ArithOverflow)?);
        }
        if pos != data.len() {
            return Err(CoveError::PageCorrupt);
        }

        let offsets = OffsetBuffer::new(ScalarBuffer::from(offsets));
        let nulls = valid_bits.map(NullBuffer::from);
        Ok((offsets, Buffer::from_vec(values), nulls))
    }

    fn materialize_selected(
        &self,
        array: &EncodedArray<'_>,
        selection: ArrowRowSelection<'_>,
        ranges: &[(usize, usize)],
    ) -> Result<(OffsetBuffer<i32>, Buffer, Option<NullBuffer>), CoveError> {
        let selected_len = selection.selected_len(array.row_count)?;
        let has_nulls = match array.validity {
            Some(validity) => validity.null_count()? > 0,
            None => false,
        };
        let mut value_len = 0usize;
        let mut any_null = false;
        selection.for_each_row(array.row_count, |row| {
            let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
            let is_null = has_nulls && array.is_null(row_u64)?;
            any_null |= is_null;
            if !is_null {
                let (start, end) = ranges[row];
                value_len = value_len
                    .checked_add(end.checked_sub(start).ok_or(CoveError::PageCorrupt)?)
                    .ok_or(CoveError::ArithOverflow)?;
            }
            Ok(())
        })?;
        i32::try_from(value_len).map_err(|_| CoveError::ArithOverflow)?;

        let Some(offset_capacity) = selected_len.checked_add(1) else {
            return Err(CoveError::ArithOverflow);
        };
        let mut offsets = Vec::with_capacity(offset_capacity);
        let mut values = vec![0u8; value_len];
        let mut valid_bits = any_null.then(|| Vec::with_capacity(selected_len));
        let mut write = 0usize;
        offsets.push(0i32);
        selection.for_each_row(array.row_count, |row| {
            let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
            let is_null = has_nulls && array.is_null(row_u64)?;
            if let Some(bits) = &mut valid_bits {
                bits.push(!is_null);
            }
            if !is_null {
                let (start, end) = ranges[row];
                let len = end.checked_sub(start).ok_or(CoveError::PageCorrupt)?;
                let next = write.checked_add(len).ok_or(CoveError::ArithOverflow)?;
                values[write..next].copy_from_slice(&array.data[start..end]);
                write = next;
            }
            offsets.push(i32::try_from(write).map_err(|_| CoveError::ArithOverflow)?);
            Ok(())
        })?;
        debug_assert_eq!(write, value_len);

        let offsets = OffsetBuffer::new(ScalarBuffer::from(offsets));
        let nulls = valid_bits.map(NullBuffer::from);
        Ok((offsets, Buffer::from_vec(values), nulls))
    }
}

fn try_direct_primitive_array(
    array: &EncodedArray<'_>,
    data_type: &DataType,
) -> Result<Option<ArrayRef>, CoveError> {
    if array.validity.is_some() {
        return Ok(None);
    }

    match array.encoding {
        CoveEncodingKind::NumCode if array.physical == CovePhysicalKind::NumCode => match data_type
        {
            DataType::Int64 => Ok(Some(
                Arc::new(Int64Array::from(collect_numcode_i64(array)?)) as ArrayRef,
            )),
            DataType::UInt64 => Ok(Some(
                Arc::new(UInt64Array::from(collect_numcode_u64(array)?)) as ArrayRef,
            )),
            DataType::Timestamp(TimeUnit::Microsecond, None) => Ok(Some(Arc::new(
                TimestampMicrosecondArray::from(collect_numcode_i64(array)?),
            ) as ArrayRef)),
            DataType::Timestamp(TimeUnit::Nanosecond, None) => Ok(Some(Arc::new(
                TimestampNanosecondArray::from(collect_numcode_i64(array)?),
            ) as ArrayRef)),
            _ => Ok(None),
        },
        CoveEncodingKind::PlainFixed
            if array.logical == CoveLogicalType::Bool && *data_type == DataType::Boolean =>
        {
            Ok(Some(
                Arc::new(BooleanArray::from(collect_plain_bool(array)?)) as ArrayRef,
            ))
        }
        _ => Ok(None),
    }
}

fn try_direct_primitive_array_for_selection(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data_type: &DataType,
) -> Result<Option<ArrayRef>, CoveError> {
    match array.encoding {
        CoveEncodingKind::NumCode if array.physical == CovePhysicalKind::NumCode => match data_type
        {
            DataType::Int64 => Ok(Some(
                Arc::new(Int64Array::from(collect_numcode_i64_selected(
                    array, selection,
                )?)) as ArrayRef,
            )),
            DataType::UInt64 => Ok(Some(
                Arc::new(UInt64Array::from(collect_numcode_u64_selected(
                    array, selection,
                )?)) as ArrayRef,
            )),
            DataType::Timestamp(TimeUnit::Microsecond, None) => Ok(Some(Arc::new(
                TimestampMicrosecondArray::from(collect_numcode_i64_selected(array, selection)?),
            ) as ArrayRef)),
            DataType::Timestamp(TimeUnit::Nanosecond, None) => Ok(Some(Arc::new(
                TimestampNanosecondArray::from(collect_numcode_i64_selected(array, selection)?),
            ) as ArrayRef)),
            _ => Ok(None),
        },
        CoveEncodingKind::PlainFixed
            if array.logical == CoveLogicalType::Bool && *data_type == DataType::Boolean =>
        {
            Ok(Some(
                Arc::new(BooleanArray::from(collect_plain_bool_selected(
                    array, selection,
                )?)) as ArrayRef,
            ))
        }
        _ => Ok(None),
    }
}

fn fixed_width_payload_prefix<'a>(
    data: &'a [u8],
    row_count: usize,
    width: usize,
) -> Result<&'a [u8], CoveError> {
    let Some(required_len) = row_count.checked_mul(width) else {
        return Err(CoveError::ArithOverflow);
    };
    if data.len() < required_len {
        return Err(CoveError::OffsetRange);
    }
    Ok(&data[..required_len])
}

fn collect_numcode_u64(array: &EncodedArray<'_>) -> Result<Vec<u64>, CoveError> {
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let data = fixed_width_payload_prefix(array.data, row_count, 8)?;
    let mut out = Vec::with_capacity(row_count);
    for bytes in data.chunks_exact(8) {
        out.push(u64::from_le_bytes(bytes.try_into().unwrap()));
    }
    Ok(out)
}

fn collect_numcode_u64_selected(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<Vec<Option<u64>>, CoveError> {
    let mut out = Vec::with_capacity(selection.selected_len(array.row_count)?);
    selection.for_each_row(array.row_count, |row| {
        let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
        if array.is_null(row_u64)? {
            out.push(None);
            return Ok(());
        }
        let offset = row.checked_mul(8).ok_or(CoveError::ArithOverflow)?;
        out.push(Some(read_u64_le(array.data, offset)?));
        Ok(())
    })?;
    Ok(out)
}

fn collect_numcode_i64(array: &EncodedArray<'_>) -> Result<Vec<i64>, CoveError> {
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let data = fixed_width_payload_prefix(array.data, row_count, 8)?;
    let mut out = Vec::with_capacity(row_count);
    for bytes in data.chunks_exact(8) {
        let value = u64::from_le_bytes(bytes.try_into().unwrap());
        out.push(i64::try_from(value).map_err(|_| CoveError::PageCorrupt)?);
    }
    Ok(out)
}

fn collect_numcode_i64_selected(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<Vec<Option<i64>>, CoveError> {
    let mut out = Vec::with_capacity(selection.selected_len(array.row_count)?);
    selection.for_each_row(array.row_count, |row| {
        let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
        if array.is_null(row_u64)? {
            out.push(None);
            return Ok(());
        }
        let offset = row.checked_mul(8).ok_or(CoveError::ArithOverflow)?;
        let value = read_u64_le(array.data, offset)?;
        out.push(Some(
            i64::try_from(value).map_err(|_| CoveError::PageCorrupt)?,
        ));
        Ok(())
    })?;
    Ok(out)
}

fn collect_plain_bool(array: &EncodedArray<'_>) -> Result<Vec<bool>, CoveError> {
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let data = fixed_width_payload_prefix(array.data, row_count, 1)?;
    let mut out = Vec::with_capacity(row_count);
    for byte in data {
        out.push(match *byte {
            0 => false,
            1 => true,
            _ => return Err(CoveError::PageCorrupt),
        });
    }
    Ok(out)
}

fn collect_plain_bool_selected(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<Vec<Option<bool>>, CoveError> {
    let mut out = Vec::with_capacity(selection.selected_len(array.row_count)?);
    selection.for_each_row(array.row_count, |row| {
        let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
        if array.is_null(row_u64)? {
            out.push(None);
            return Ok(());
        }
        let byte = *array.data.get(row).ok_or(CoveError::OffsetRange)?;
        out.push(Some(match byte {
            0 => false,
            1 => true,
            _ => return Err(CoveError::PageCorrupt),
        }));
        Ok(())
    })?;
    Ok(out)
}

fn values_to_arrow_array_with_data_type(
    logical: CoveLogicalType,
    values: &[CoveArrayValue<'_>],
    data_type: DataType,
) -> Result<ArrayRef, CoveError> {
    Ok(match data_type {
        DataType::Boolean => Arc::new(BooleanArray::from(collect_bool(values)?)) as ArrayRef,
        DataType::Int8 => Arc::new(Int8Array::from(collect_i64(logical, values, |v| {
            i8::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?)) as ArrayRef,
        DataType::Int16 => Arc::new(Int16Array::from(collect_i64(logical, values, |v| {
            i16::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?)) as ArrayRef,
        DataType::Int32 => Arc::new(Int32Array::from(collect_i64(logical, values, |v| {
            i32::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?)) as ArrayRef,
        DataType::Int64 => {
            Arc::new(Int64Array::from(collect_i64(logical, values, Ok)?)) as ArrayRef
        }
        DataType::UInt8 => Arc::new(UInt8Array::from(collect_u64(logical, values, |v| {
            u8::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?)) as ArrayRef,
        DataType::UInt16 => Arc::new(UInt16Array::from(collect_u64(logical, values, |v| {
            u16::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?)) as ArrayRef,
        DataType::UInt32 => Arc::new(UInt32Array::from(collect_u64(logical, values, |v| {
            u32::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?)) as ArrayRef,
        DataType::UInt64 => {
            Arc::new(UInt64Array::from(collect_u64(logical, values, Ok)?)) as ArrayRef
        }
        DataType::Float32 => Arc::new(Float32Array::from(collect_f32(values)?)) as ArrayRef,
        DataType::Float64 => Arc::new(Float64Array::from(collect_f64(values)?)) as ArrayRef,
        DataType::Date32 => Arc::new(Date32Array::from(collect_i64(logical, values, |v| {
            i32::try_from(v).map_err(|_| CoveError::PageCorrupt)
        })?)) as ArrayRef,
        DataType::Timestamp(TimeUnit::Microsecond, None) => Arc::new(
            TimestampMicrosecondArray::from(collect_i64(logical, values, Ok)?),
        ) as ArrayRef,
        DataType::Timestamp(TimeUnit::Nanosecond, None) => Arc::new(TimestampNanosecondArray::from(
            collect_i64(logical, values, Ok)?,
        )) as ArrayRef,
        DataType::Utf8 => Arc::new(collect_utf8(logical, values)?) as ArrayRef,
        DataType::Binary => Arc::new(collect_binary(logical, values)?) as ArrayRef,
        DataType::FixedSizeBinary(size) => {
            Arc::new(collect_fixed_size_binary(logical, values, size)?) as ArrayRef
        }
        DataType::Decimal128(precision, scale) => Arc::new(
            Decimal128Array::from(collect_i128(logical, values)?)
                .with_precision_and_scale(precision, scale)
                .map_err(|err| CoveError::BadSection(format!("Arrow Decimal128: {err}")))?,
        ) as ArrayRef,
        other => {
            return Err(CoveError::UnsupportedEncoding(format!(
                "Arrow export for {other:?}"
            )));
        }
    })
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

/// Return the Arrow data type used by the default decoded COVE export path.
///
/// This mirrors [`encoded_array_to_arrow`] without requiring callers to build
/// a synthetic array just to construct an Arrow schema.
pub fn decoded_arrow_data_type(logical: CoveLogicalType) -> Result<DataType, CoveError> {
    let result = arrow_data_type_for_export_options(logical, ArrowExportOptions::default())?;
    if result.report.has_lossy_or_unsupported() {
        return Err(CoveError::UnsupportedEncoding(format!(
            "Arrow export for {logical:?} requires explicit fidelity reporting"
        )));
    }
    Ok(result.value)
}

/// Return the Arrow data type for a COVE logical type under explicit export
/// options, including the same fidelity diagnostics as value export.
pub fn arrow_data_type_for_export_options(
    logical: CoveLogicalType,
    options: ArrowExportOptions,
) -> Result<ArrowExportResult<DataType>, CoveError> {
    let mut report = ArrowExportReport::default();
    let value = arrow_data_type_with_report(logical, &options, &mut report)?;
    Ok(ArrowExportResult { value, report })
}

/// Return the Arrow data type for a concrete COVE column under explicit export
/// options. This includes physical representation choices such as FileCode
/// dictionary-key output when a file dictionary is available.
pub fn arrow_data_type_for_column_export_options(
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
    has_file_dictionary: bool,
    options: ArrowExportOptions,
) -> Result<ArrowExportResult<DataType>, CoveError> {
    let mut result = arrow_data_type_for_export_options(logical, options)?;
    if options.dictionary_policy == ArrowDictionaryPolicy::DictionaryKeys
        && physical == CovePhysicalKind::FileCode
        && has_file_dictionary
    {
        result.report.push(
            None,
            logical,
            ArrowFidelitySeverity::Informational,
            "FileCode values exported as Arrow dictionary keys",
        );
        result.value = DataType::Dictionary(Box::new(DataType::UInt32), Box::new(result.value));
    }
    Ok(result)
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
                    ArrowFidelitySeverity::Lossy,
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
        encoding::local_codebook::{LocalCodebookPayload, LocalCodebookValues, LocalIndexPayload},
        encoding::nested::{
            ListLayout, ListLayoutPayload, MapLayout, MapLayoutPayload, StructLayout,
            StructLayoutPayload,
        },
        encoding::rle::RlePayload,
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
        assert!(matches!(
            cove_null_to_arrow_validity(&[], 1),
            Err(CoveError::BufferTooShort)
        ));
    }

    #[test]
    fn rejects_short_arrow_validity_bitmap() {
        assert!(matches!(
            arrow_validity_to_cove_null(&[], 1),
            Err(CoveError::BufferTooShort)
        ));
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
    fn exports_numcode_int64_array() {
        let mut values = Vec::new();
        values.extend_from_slice(&10u64.to_le_bytes());
        values.extend_from_slice(&20u64.to_le_bytes());
        let cove = EncodedArray::new(
            CoveLogicalType::Int64,
            CovePhysicalKind::NumCode,
            2,
            CoveEncodingKind::NumCode,
            None,
            &values,
            None,
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let ints = arrow.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(ints.values(), &[10, 20]);
    }

    #[test]
    fn rejects_invalid_plain_bool_array() {
        let values = [1u8, 2u8];
        let cove = EncodedArray::new(
            CoveLogicalType::Bool,
            CovePhysicalKind::Boolean,
            2,
            CoveEncodingKind::PlainFixed,
            None,
            &values,
            None,
        );

        assert!(matches!(
            encoded_array_to_arrow(&cove),
            Err(CoveError::PageCorrupt)
        ));
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
    fn exports_utf8_varbytes_array_with_invalid_null_payload() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        values.extend_from_slice(&1u32.to_le_bytes());
        values.push(0xff);
        values.extend_from_slice(&5u32.to_le_bytes());
        values.extend_from_slice(b"there");
        let mut validity = ValidityBitmapBuilder::new(3).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 3);
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            3,
            CoveEncodingKind::VarBytes,
            Some(bitmap),
            &values,
            None,
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let strings = arrow
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(strings.value(0), "hi");
        assert!(strings.is_null(1));
        assert_eq!(strings.value(2), "there");
    }

    #[test]
    fn rejects_invalid_utf8_varbytes_non_null_payload() {
        let mut values = Vec::new();
        values.extend_from_slice(&1u32.to_le_bytes());
        values.push(0xff);
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            1,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        assert!(encoded_array_to_arrow(&cove).is_err());
    }

    #[test]
    fn rejects_truncated_utf8_varbytes_payload() {
        let mut values = Vec::new();
        values.extend_from_slice(&4u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            1,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        assert!(matches!(
            encoded_array_to_arrow(&cove),
            Err(CoveError::OffsetRange)
        ));
    }

    #[test]
    fn rejects_trailing_utf8_varbytes_payload() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        values.push(0);
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            1,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        assert!(matches!(
            encoded_array_to_arrow(&cove),
            Err(CoveError::PageCorrupt)
        ));
    }

    #[test]
    fn selected_export_filters_primitive_rows_and_nulls() {
        let mut values = Vec::new();
        values.extend_from_slice(&10i32.to_le_bytes());
        values.extend_from_slice(&20i32.to_le_bytes());
        values.extend_from_slice(&30i32.to_le_bytes());
        let mut validity = ValidityBitmapBuilder::new(3).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 3);
        let cove = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            3,
            CoveEncodingKind::PlainFixed,
            Some(bitmap),
            &values,
            None,
        );

        let result = encoded_array_to_arrow_selected_with_options(
            &cove,
            &[2, 1],
            ArrowExportOptions::default(),
        )
        .unwrap();
        let ints = result.value.as_any().downcast_ref::<Int32Array>().unwrap();
        assert_eq!(ints.len(), 2);
        assert_eq!(ints.value(0), 30);
        assert!(ints.is_null(1));
    }

    #[test]
    fn selected_record_batch_exports_varbytes_rows() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        values.extend_from_slice(&5u32.to_le_bytes());
        values.extend_from_slice(b"there");
        values.extend_from_slice(&3u32.to_le_bytes());
        values.extend_from_slice(b"bye");
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            3,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        let result = encoded_columns_to_record_batch_selected_with_options(
            &[("word", &cove)],
            &[1],
            ArrowExportOptions::default(),
        )
        .unwrap();
        let strings = result
            .value
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(strings.value(0), "there");
    }

    #[test]
    fn selected_export_reads_numcode_rows_directly_in_requested_order() {
        let mut values = Vec::new();
        values.extend_from_slice(&10u64.to_le_bytes());
        values.extend_from_slice(&20u64.to_le_bytes());
        values.extend_from_slice(&30u64.to_le_bytes());
        let cove = EncodedArray::new(
            CoveLogicalType::Int64,
            CovePhysicalKind::NumCode,
            3,
            CoveEncodingKind::NumCode,
            None,
            &values,
            None,
        );

        let result = encoded_array_to_arrow_selected_with_options(
            &cove,
            &[2, 0],
            ArrowExportOptions::default(),
        )
        .unwrap();
        let ints = result.value.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(ints.values(), &[30, 10]);
    }

    #[test]
    fn selected_export_reads_varbytes_rows_directly_with_nulls() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        values.extend_from_slice(&1u32.to_le_bytes());
        values.push(0xff);
        values.extend_from_slice(&5u32.to_le_bytes());
        values.extend_from_slice(b"there");
        let mut validity = ValidityBitmapBuilder::new(3).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 3);
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            3,
            CoveEncodingKind::VarBytes,
            Some(bitmap),
            &values,
            None,
        );

        let result = encoded_array_to_arrow_selected_with_options(
            &cove,
            &[2, 1, 0],
            ArrowExportOptions::default(),
        )
        .unwrap();
        let strings = result
            .value
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(strings.value(0), "there");
        assert!(strings.is_null(1));
        assert_eq!(strings.value(2), "hi");
    }

    #[test]
    fn bitset_export_reads_varbytes_rows_directly() {
        let mut values = Vec::new();
        values.extend_from_slice(&1u32.to_le_bytes());
        values.extend_from_slice(b"a");
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"bb");
        values.extend_from_slice(&3u32.to_le_bytes());
        values.extend_from_slice(b"ccc");
        values.extend_from_slice(&4u32.to_le_bytes());
        values.extend_from_slice(b"dddd");
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            4,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        let result = encoded_array_to_arrow_with_row_selection_options(
            &cove,
            ArrowRowSelection::Bitset {
                words: &[0b1010],
                len: 4,
            },
            ArrowExportOptions::default(),
        )
        .unwrap();
        let strings = result
            .value
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(strings.len(), 2);
        assert_eq!(strings.value(0), "bb");
        assert_eq!(strings.value(1), "dddd");
    }

    #[test]
    fn bitset_export_ignores_invalid_utf8_for_null_rows() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        values.extend_from_slice(&1u32.to_le_bytes());
        values.push(0xff);
        values.extend_from_slice(&5u32.to_le_bytes());
        values.extend_from_slice(b"there");
        let mut validity = ValidityBitmapBuilder::new(3).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 3);
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            3,
            CoveEncodingKind::VarBytes,
            Some(bitmap),
            &values,
            None,
        );

        let result = encoded_array_to_arrow_with_row_selection_options(
            &cove,
            ArrowRowSelection::Bitset {
                words: &[0b011],
                len: 3,
            },
            ArrowExportOptions::default(),
        )
        .unwrap();
        let strings = result
            .value
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(strings.value(0), "hi");
        assert!(strings.is_null(1));
    }

    #[test]
    fn bitset_export_rejects_wrong_page_length() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            1,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        assert!(matches!(
            encoded_array_to_arrow_with_row_selection_options(
                &cove,
                ArrowRowSelection::Bitset {
                    words: &[0b1],
                    len: 2,
                },
                ArrowExportOptions::default()
            ),
            Err(CoveError::OffsetRange)
        ));
    }

    #[test]
    fn selected_export_reads_binary_varbytes_rows_directly() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(&[0, 1]);
        values.extend_from_slice(&3u32.to_le_bytes());
        values.extend_from_slice(&[2, 3, 4]);
        let cove = EncodedArray::new(
            CoveLogicalType::Binary,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        let result = encoded_array_to_arrow_selected_with_options(
            &cove,
            &[1, 0],
            ArrowExportOptions::default(),
        )
        .unwrap();
        let binary = result
            .value
            .as_any()
            .downcast_ref::<arrow_array::BinaryArray>()
            .unwrap();
        assert_eq!(binary.value(0), &[2, 3, 4]);
        assert_eq!(binary.value(1), &[0, 1]);
    }

    #[test]
    fn selected_export_rejects_trailing_varbytes_payload() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        values.extend_from_slice(&5u32.to_le_bytes());
        values.extend_from_slice(b"there");
        values.push(0);
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        assert!(matches!(
            encoded_array_to_arrow_selected_with_options(
                &cove,
                &[1],
                ArrowExportOptions::default()
            ),
            Err(CoveError::PageCorrupt)
        ));
    }

    #[test]
    fn selected_export_reads_bool_rows_directly_with_nulls() {
        let values = [1u8, 0u8, 1u8];
        let mut validity = ValidityBitmapBuilder::new(3).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 3);
        let cove = EncodedArray::new(
            CoveLogicalType::Bool,
            CovePhysicalKind::Boolean,
            3,
            CoveEncodingKind::PlainFixed,
            Some(bitmap),
            &values,
            None,
        );

        let result = encoded_array_to_arrow_selected_with_options(
            &cove,
            &[2, 1, 0],
            ArrowExportOptions::default(),
        )
        .unwrap();
        let bools = result
            .value
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap();
        assert!(bools.value(0));
        assert!(bools.is_null(1));
        assert!(bools.value(2));
    }

    #[test]
    fn selected_export_full_row_selection_matches_bulk_transform_decode() {
        let payload = LocalCodebookPayload {
            values: LocalCodebookValues::VarBytes(vec![b"red".to_vec(), b"blue".to_vec()]),
            indexes: LocalIndexPayload::Rle(RlePayload {
                runs: vec![(0, 1), (1, 2)],
            }),
        };
        let data = payload.encode();
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            3,
            CoveEncodingKind::LocalCodebook,
            None,
            &data,
            None,
        );

        let full = encoded_array_to_arrow_with_options(&cove, ArrowExportOptions::default())
            .unwrap()
            .value;
        let selected = encoded_array_to_arrow_selected_with_options(
            &cove,
            &[0, 1, 2],
            ArrowExportOptions::default(),
        )
        .unwrap()
        .value;

        let full = full
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        let selected = selected
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(selected.len(), 3);
        assert_eq!(selected.value(0), full.value(0));
        assert_eq!(selected.value(1), full.value(1));
        assert_eq!(selected.value(2), full.value(2));
    }

    #[test]
    fn strict_export_rejects_json_without_extension_metadata() {
        let mut values = Vec::new();
        values.extend_from_slice(&7u32.to_le_bytes());
        values.extend_from_slice(br#"{"a":1}"#);
        let cove = EncodedArray::new(
            CoveLogicalType::Json,
            CovePhysicalKind::VarBytes,
            1,
            CoveEncodingKind::VarBytes,
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
        assert_eq!(result.value.data_type(), &DataType::Utf8);
    }

    #[test]
    fn record_batch_emits_json_extension_metadata_when_requested() {
        let mut values = Vec::new();
        values.extend_from_slice(&7u32.to_le_bytes());
        values.extend_from_slice(br#"{"a":1}"#);
        let cove = EncodedArray::new(
            CoveLogicalType::Json,
            CovePhysicalKind::VarBytes,
            1,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        let result = encoded_columns_to_record_batch_with_options(
            &[("payload", &cove)],
            ArrowExportOptions {
                emit_json_extension_metadata: true,
                ..ArrowExportOptions::default()
            },
        )
        .unwrap();
        let schema = result.value.schema();
        let field = schema.field(0);
        assert_eq!(
            field.metadata().get("ARROW:extension:name"),
            Some(&"cove.json".to_string())
        );
        assert!(result.report.issues.is_empty());
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
    fn selected_filecode_dictionary_output_preserves_keys() {
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
        codes.extend_from_slice(&1u32.to_le_bytes());
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::FileCode,
            3,
            CoveEncodingKind::FileCode,
            None,
            &codes,
            Some(&dictionary),
        );

        let result = encoded_array_to_arrow_selected_with_options(
            &cove,
            &[1, 2],
            ArrowExportOptions {
                dictionary_policy: ArrowDictionaryPolicy::DictionaryKeys,
                ..ArrowExportOptions::default()
            },
        )
        .unwrap();
        let dictionary = result
            .value
            .as_any()
            .downcast_ref::<DictionaryArray<UInt32Type>>()
            .unwrap();
        assert_eq!(dictionary.keys().value(0), 0);
        assert_eq!(dictionary.keys().value(1), 1);
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
