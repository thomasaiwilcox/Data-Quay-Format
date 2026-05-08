//! Spec §49 — Arrow interop helpers.
//!
//! COVE stores nulls as a *null* bitmap (bit set ⇒ null), Arrow stores them as
//! a *validity* bitmap (bit set ⇒ valid). This module owns the bit inversion
//! and byte-aligned conversion required to bridge the two.

mod selection_utils;
mod validity;

use std::{
    borrow::Cow,
    collections::HashMap,
    panic::RefUnwindSafe,
    ptr::{self, NonNull},
    sync::Arc,
};

use arrow_array::{
    builder::{BinaryBuilder, BinaryViewBuilder, StringBuilder, StringViewBuilder},
    types::{
        Float32Type, Float64Type, GenericBinaryType, GenericStringType, Int16Type, Int32Type,
        Int64Type, Int8Type, UInt16Type, UInt32Type, UInt64Type, UInt8Type,
    },
    Array, ArrayRef, BinaryArray, BinaryViewArray, BooleanArray, Date32Array, Decimal128Array,
    DictionaryArray, FixedSizeBinaryArray, Float32Array, Float64Array, GenericByteArray,
    Int16Array, Int32Array, Int64Array, Int8Array, ListArray, MapArray, RecordBatch,
    StringViewArray, StructArray, TimestampMicrosecondArray, TimestampNanosecondArray, UInt16Array,
    UInt32Array, UInt64Array, UInt8Array,
};
use arrow_buffer::{
    alloc::Allocation, ArrowNativeType, BooleanBuffer, Buffer, NullBuffer, OffsetBuffer,
    ScalarBuffer,
};
use arrow_data::{ByteView, MAX_INLINE_VIEW_LEN};
use arrow_schema::{DataType, Field, Fields, Schema, TimeUnit};

use crate::{
    array::{CoveArrayValue, EncodedArray},
    constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
    dictionary::DictionaryValue,
    encoding::{
        bit_packed::{BitPacked, BitPackedPayload},
        constant::ConstantPayload,
        delta::{Delta, DeltaPayload},
        frame_of_reference::{ForPayload, FrameOfReference},
        local_codebook::{LocalCodebookPayload, LocalCodebookValues},
        nested::{ListLayoutPayload, MapLayoutPayload, StructLayoutPayload},
        patched_base::{PatchedBase, PatchedBasePayload},
        rle::{Rle, RlePayload},
        run_end::{RunEnd, RunEndPayload},
        sparse::{Sparse, SparsePayload},
        Encoding,
    },
    validity::ValidityBitmap,
    wire, CoveError,
};

use selection_utils::{count_bitset_rows, mask_selection_tail, selected_rows_are_all_rows};
pub use validity::{arrow_validity_to_cove_null, cove_null_to_arrow_validity};

/// Policy for exporting FileCode-backed scalar columns to Arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// Policy for exporting COVE variable byte payloads to Arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArrowVarBytesExportPolicy {
    /// Materialise COVE length-prefixed bytes into standard Arrow Utf8/Binary
    /// offset/value buffers.
    Standard,
    /// Export COVE length-prefixed bytes as legal Arrow Utf8View/BinaryView
    /// arrays. The backing buffer must own or retain the COVE values bytes.
    View,
}

impl Default for ArrowVarBytesExportPolicy {
    fn default() -> Self {
        Self::Standard
    }
}

/// Policy for validating COVE byte payloads before constructing Arrow Utf8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArrowStringValidationPolicy {
    /// Validate all materialized non-null rows while exporting.
    Strict,
    /// Validate on the first export, but allow an outer caller to replace this
    /// with [`ArrowStringValidationPolicy::TrustedPageProof`] once it has
    /// recorded an exact page-level proof. Inside `cove-arrow` this behaves the
    /// same as [`ArrowStringValidationPolicy::Strict`].
    StrictOrCachedProof,
    /// Trust a caller-supplied page-level proof that every non-null row slice is
    /// valid UTF-8.
    TrustedPageProof,
}

impl Default for ArrowStringValidationPolicy {
    fn default() -> Self {
        Self::Strict
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArrowExportOptions {
    pub dictionary_policy: ArrowDictionaryPolicy,
    pub varbytes_policy: ArrowVarBytesExportPolicy,
    pub string_validation_policy: ArrowStringValidationPolicy,
    pub decimal: Option<ArrowDecimalContext>,
    pub emit_uuid_extension_metadata: bool,
    pub emit_json_extension_metadata: bool,
}

impl Default for ArrowExportOptions {
    fn default() -> Self {
        Self {
            dictionary_policy: ArrowDictionaryPolicy::DecodeValues,
            varbytes_policy: ArrowVarBytesExportPolicy::Standard,
            string_validation_policy: ArrowStringValidationPolicy::Strict,
            decimal: None,
            emit_uuid_extension_metadata: false,
            emit_json_extension_metadata: false,
        }
    }
}

/// Owner for an Arrow buffer that points into an externally retained COVE byte
/// allocation.
pub type ArrowBufferOwner = Arc<dyn Allocation>;

/// Convert an owned COVE allocation into an Arrow buffer owner.
pub fn arrow_buffer_owner<T>(owner: Arc<T>) -> ArrowBufferOwner
where
    T: RefUnwindSafe + Send + Sync + 'static,
{
    owner
}

#[derive(Clone)]
pub struct ArrowEncodedColumn<'name, 'array, 'data> {
    pub name: &'name str,
    pub array: &'array EncodedArray<'data>,
    pub data_owner: Option<ArrowBufferOwner>,
}

impl<'name, 'array, 'data> ArrowEncodedColumn<'name, 'array, 'data> {
    pub fn borrowed(name: &'name str, array: &'array EncodedArray<'data>) -> Self {
        Self {
            name,
            array,
            data_owner: None,
        }
    }

    pub fn with_data_owner(
        name: &'name str,
        array: &'array EncodedArray<'data>,
        data_owner: Option<ArrowBufferOwner>,
    ) -> Self {
        Self {
            name,
            array,
            data_owner,
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
    encoded_array_to_arrow_with_options_and_owner(array, options, None)
}

fn encoded_array_to_arrow_with_options_and_owner(
    array: &EncodedArray<'_>,
    options: ArrowExportOptions,
    data_owner: Option<&ArrowBufferOwner>,
) -> Result<ArrowExportResult<ArrayRef>, CoveError> {
    let mut report = ArrowExportReport::default();
    if options.dictionary_policy == ArrowDictionaryPolicy::DictionaryKeys {
        if let Some(dictionary_array) = try_filecode_dictionary_array(array, options)? {
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
    if let Some(array_ref) = try_direct_byte_array(
        array,
        &arrow_type,
        data_owner,
        options.string_validation_policy,
    )? {
        return Ok(ArrowExportResult {
            value: array_ref,
            report,
        });
    }
    if let Some(array_ref) = try_direct_primitive_array(array, &arrow_type, data_owner)? {
        return Ok(ArrowExportResult {
            value: array_ref,
            report,
        });
    }
    if let Some(array_ref) = try_direct_decoded_array(array, ArrowRowSelection::All, &arrow_type)? {
        return Ok(ArrowExportResult {
            value: array_ref,
            report,
        });
    }
    let values = array.decode_all_rows()?;
    let array_ref = values_to_arrow_array_with_data_type(array.logical, &values, arrow_type)?;
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
    encoded_array_to_arrow_with_row_selection_options_and_owner(array, selection, options, None)
}

pub fn encoded_array_to_arrow_with_row_selection_options_and_owner(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    options: ArrowExportOptions,
    data_owner: Option<&ArrowBufferOwner>,
) -> Result<ArrowExportResult<ArrayRef>, CoveError> {
    if selection.is_all_rows(array.row_count)? {
        return encoded_array_to_arrow_with_options_and_owner(array, options, data_owner);
    }
    let mut report = ArrowExportReport::default();
    if options.dictionary_policy == ArrowDictionaryPolicy::DictionaryKeys {
        if let Some(dictionary_array) =
            try_filecode_dictionary_array_for_selection(array, selection, options)?
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
    if let Some(array_ref) = try_direct_byte_array_for_selection(
        array,
        selection,
        &arrow_type,
        data_owner,
        options.string_validation_policy,
    )? {
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
    if let Some(array_ref) = try_direct_decoded_array(array, selection, &arrow_type)? {
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
    let owned_columns = columns
        .iter()
        .map(|(name, array)| ArrowEncodedColumn::borrowed(*name, *array))
        .collect::<Vec<_>>();
    encoded_columns_to_arrow_arrays_with_owners_options(&owned_columns, selection, options)
}

/// Export named COVE array views as Arrow arrays, retaining optional backing
/// owners for direct Arrow View buffers.
pub fn encoded_columns_to_arrow_arrays_with_owners_options(
    columns: &[ArrowEncodedColumn<'_, '_, '_>],
    selection: ArrowRowSelection<'_>,
    options: ArrowExportOptions,
) -> Result<ArrowExportResult<Vec<ArrayRef>>, CoveError> {
    let mut arrays = Vec::with_capacity(columns.len());
    let mut report = ArrowExportReport::default();
    for column in columns {
        let result = encoded_array_to_arrow_with_row_selection_options_and_owner(
            column.array,
            selection,
            options,
            column.data_owner.as_ref(),
        )?;
        report.extend_with_field(column.name, result.report);
        arrays.push(result.value);
    }
    Ok(ArrowExportResult {
        value: arrays,
        report,
    })
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
            let storage = if options.varbytes_policy == ArrowVarBytesExportPolicy::View {
                r#"{"storage":"utf8_view"}"#
            } else {
                r#"{"storage":"utf8"}"#
            };
            metadata.insert("ARROW:extension:metadata".into(), storage.into());
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

#[inline]
fn bitpacked_len(len: usize) -> Result<usize, CoveError> {
    len.checked_add(7)
        .ok_or(CoveError::ArithOverflow)
        .map(|len| len / 8)
}

#[inline]
fn set_packed_bit(bytes: &mut [u8], index: usize) {
    bytes[index / 8] |= 1u8 << (index % 8);
}

struct ArrowValidityBuilder {
    bytes: Vec<u8>,
    len: usize,
    pos: usize,
    null_count: usize,
}

impl ArrowValidityBuilder {
    fn new(len: usize) -> Result<Self, CoveError> {
        Ok(Self {
            bytes: vec![0u8; bitpacked_len(len)?],
            len,
            pos: 0,
            null_count: 0,
        })
    }

    fn append(&mut self, is_valid: bool) {
        debug_assert!(self.pos < self.len);
        if is_valid {
            set_packed_bit(&mut self.bytes, self.pos);
        } else {
            self.null_count += 1;
        }
        self.pos += 1;
    }

    fn finish(self) -> Option<NullBuffer> {
        debug_assert_eq!(self.pos, self.len);
        if self.null_count == 0 {
            return None;
        }
        let buffer = BooleanBuffer::new(Buffer::from_vec(self.bytes), 0, self.len);
        // INVARIANT: `append` writes exactly one validity bit per logical row,
        // setting true only for valid rows and incrementing `null_count` for
        // every false bit.
        // SAFETY: the packed BooleanBuffer therefore contains exactly
        // `null_count` zero bits over its declared logical length.
        Some(unsafe { NullBuffer::new_unchecked(buffer, self.null_count) })
    }
}

fn trusted_i32_offset_buffer(offsets: Vec<i32>) -> OffsetBuffer<i32> {
    debug_assert!(!offsets.is_empty());
    debug_assert_eq!(offsets[0], 0);
    debug_assert!(offsets.windows(2).all(|pair| pair[0] <= pair[1]));
    // INVARIANT: callers append offsets from checked cumulative byte lengths.
    // They start at zero, never decrease, and each value has already fit in
    // i32 before being pushed.
    // SAFETY: these are exactly Arrow's OffsetBuffer invariants for i32
    // offsets.
    unsafe { OffsetBuffer::new_unchecked(ScalarBuffer::from(offsets)) }
}

fn trusted_binary_array(
    offsets: OffsetBuffer<i32>,
    values: Buffer,
    nulls: Option<NullBuffer>,
) -> BinaryArray {
    debug_assert!(offsets.last().copied().unwrap_or_default() as usize <= values.len());
    // INVARIANT: `BytePayloadPlan::materialize*` builds monotonic offsets with
    // the final offset equal to the values buffer length, and any null buffer is
    // produced for the same logical row count.
    // SAFETY: Binary arrays do not require UTF-8 validation; with the proven
    // offset/value/null invariants, `try_new` would not fail.
    unsafe { GenericByteArray::<GenericBinaryType<i32>>::new_unchecked(offsets, values, nulls) }
}

fn trusted_string_array(
    offsets: OffsetBuffer<i32>,
    values: Buffer,
    nulls: Option<NullBuffer>,
) -> GenericByteArray<GenericStringType<i32>> {
    debug_assert!(offsets.last().copied().unwrap_or_default() as usize <= values.len());
    // INVARIANT: callers either validate every non-null string row against the
    // same offset/value/null buffers immediately before construction or carry
    // an explicit page-level proof that every non-null row slice is UTF-8.
    // SAFETY: with monotonic i32 offsets, final offset within `values`, null
    // buffer length matching the offset count, and proven UTF-8 row slices,
    // `try_new` would not fail.
    unsafe { GenericByteArray::<GenericStringType<i32>>::new_unchecked(offsets, values, nulls) }
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
    let mut validity_builder = ArrowValidityBuilder::new(row_count)?;
    for row in 0..row_count_u64 {
        let valid = validity.is_valid(row)?;
        validity_builder.append(valid);
    }
    Ok(validity_builder.finish())
}

fn try_filecode_dictionary_array(
    array: &EncodedArray<'_>,
    options: ArrowExportOptions,
) -> Result<Option<ArrayRef>, CoveError> {
    if array.encoding != crate::constants::CoveEncodingKind::FileCode {
        return Ok(None);
    }
    let Some(dictionary) = array.dictionary else {
        return Ok(None);
    };
    let values = file_dictionary_values_to_arrow(array.logical, dictionary, options)?;
    encoded_filecode_array_to_arrow_dictionary_with_values(array, ArrowRowSelection::All, values)
        .map(Some)
}

fn try_filecode_dictionary_array_for_selection(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    options: ArrowExportOptions,
) -> Result<Option<ArrayRef>, CoveError> {
    if array.encoding != crate::constants::CoveEncodingKind::FileCode {
        return Ok(None);
    }
    let Some(dictionary) = array.dictionary else {
        return Ok(None);
    };
    let values = file_dictionary_values_to_arrow(array.logical, dictionary, options)?;
    encoded_filecode_array_to_arrow_dictionary_with_values(array, selection, values).map(Some)
}

/// Build an Arrow dictionary array for a FileCode page using prebuilt
/// dictionary values.
///
/// This lets query engines cache the dictionary values for an immutable file
/// and materialise only the per-page key buffer on repeated scans.
pub fn encoded_filecode_array_to_arrow_dictionary_with_values(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    values: ArrayRef,
) -> Result<ArrayRef, CoveError> {
    if array.encoding != crate::constants::CoveEncodingKind::FileCode {
        return Err(CoveError::UnsupportedEncoding(
            "Arrow dictionary export requires FileCode encoding".into(),
        ));
    }
    let keys = filecode_key_array(array, selection)?;
    DictionaryArray::<UInt32Type>::try_new(keys, values)
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|err| CoveError::BadSection(format!("Arrow DictionaryArray: {err}")))
}

fn filecode_key_array(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<UInt32Array, CoveError> {
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let data = fixed_width_payload_prefix(array.data, row_count, 4)?;
    let has_nulls = array_has_nulls(array)?;
    let selected_len = selection.selected_len(array.row_count)?;
    let mut keys = Vec::<u32>::with_capacity(selected_len);
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(selected_len))
        .transpose()?;
    selection.for_each_row(array.row_count, |row| {
        let is_null = has_nulls && array.is_null(row as u64)?;
        if let Some(builder) = &mut validity_builder {
            builder.append(!is_null);
        }
        if is_null {
            keys.push(0);
        } else {
            keys.push(read_u32_le(
                data,
                row.checked_mul(4).ok_or(CoveError::ArithOverflow)?,
            )?);
        }
        Ok(())
    })?;
    Ok(UInt32Array::new(
        ScalarBuffer::from(keys),
        validity_builder.and_then(ArrowValidityBuilder::finish),
    ))
}

/// Decode COVE file dictionary entries into the Arrow dictionary values array.
pub fn file_dictionary_values_to_arrow(
    logical: CoveLogicalType,
    dictionary: &crate::dictionary::FileDictionary,
    options: ArrowExportOptions,
) -> Result<ArrayRef, CoveError> {
    let mut values = Vec::with_capacity(dictionary.entries.len());
    for code in 0..dictionary.len() {
        values.push(CoveArrayValue::DictValue(dictionary.decode_value(code)?));
    }
    let mut report = ArrowExportReport::default();
    match arrow_data_type_with_report(logical, &options, &mut report)? {
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
        DataType::Utf8View => Ok(Arc::new(collect_utf8_view(logical, &values)?)),
        DataType::Binary => Ok(Arc::new(collect_binary(logical, &values)?)),
        DataType::BinaryView => Ok(Arc::new(collect_binary_view(logical, &values)?)),
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
    data_owner: Option<&ArrowBufferOwner>,
    string_validation_policy: ArrowStringValidationPolicy,
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
    byte_array_from_payload_plan(
        array,
        ArrowRowSelection::All,
        layout,
        data_type,
        data_owner,
        string_validation_policy,
    )
}

fn try_direct_byte_array_for_selection(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data_type: &DataType,
    data_owner: Option<&ArrowBufferOwner>,
    string_validation_policy: ArrowStringValidationPolicy,
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
    byte_array_from_payload_plan(
        array,
        selection,
        layout,
        data_type,
        data_owner,
        string_validation_policy,
    )
}

fn byte_array_from_payload_plan(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    layout: BytePayloadLayout,
    data_type: &DataType,
    data_owner: Option<&ArrowBufferOwner>,
    string_validation_policy: ArrowStringValidationPolicy,
) -> Result<Option<ArrayRef>, CoveError> {
    if !matches!(
        data_type,
        DataType::Utf8 | DataType::Binary | DataType::Utf8View | DataType::BinaryView
    ) {
        return Ok(None);
    }
    let plan = BytePayloadPlan { layout };
    let array_ref = match data_type {
        DataType::Utf8 => {
            let (offsets, values, nulls) =
                plan.materialize_utf8(array, selection, string_validation_policy)?;
            // INVARIANT: Strict mode validates all materialized values before
            // construction. TrustedPageProof is an explicit caller contract
            // that every non-null source row slice is valid UTF-8 at the same
            // row boundaries used to build `offsets`.
            Arc::new(trusted_string_array(offsets, values, nulls)) as ArrayRef
        }
        DataType::Binary => {
            let (offsets, values, nulls) = plan.materialize(array, selection)?;
            Arc::new(trusted_binary_array(offsets, values, nulls)) as ArrayRef
        }
        DataType::Utf8View => {
            let (views, buffers, nulls) = plan.materialize_view(array, selection, data_owner)?;
            Arc::new(
                StringViewArray::try_new(views, buffers, nulls).map_err(|err| {
                    CoveError::BadSection(format!("Arrow Utf8View export: {err}"))
                })?,
            ) as ArrayRef
        }
        DataType::BinaryView => {
            let (views, buffers, nulls) = plan.materialize_view(array, selection, data_owner)?;
            Arc::new(
                BinaryViewArray::try_new(views, buffers, nulls).map_err(|err| {
                    CoveError::BadSection(format!("Arrow BinaryView export: {err}"))
                })?,
            ) as ArrayRef
        }
        _ => unreachable!(),
    };
    Ok(Some(array_ref))
}

fn byte_view_backing_buffer(
    array: &EncodedArray<'_>,
    data_owner: Option<&ArrowBufferOwner>,
) -> Result<Buffer, CoveError> {
    if array.data.is_empty() {
        return Ok(Buffer::from_vec(Vec::<u8>::new()));
    }
    let Some(owner) = data_owner else {
        return Ok(Buffer::from_vec(array.data.to_vec()));
    };
    let ptr = NonNull::new(array.data.as_ptr() as *mut u8).ok_or(CoveError::BufferTooShort)?;
    // SAFETY: `data_owner` is supplied by the caller for the allocation that
    // contains `array.data`, and the Arrow Buffer retains that owner for at
    // least as long as any view array referencing this byte range.
    Ok(unsafe { Buffer::from_custom_allocation(ptr, array.data.len(), Arc::clone(owner)) })
}

fn inline_byte_view(bytes: &[u8]) -> Result<u128, CoveError> {
    let len = u32::try_from(bytes.len()).map_err(|_| CoveError::ArithOverflow)?;
    if len > MAX_INLINE_VIEW_LEN {
        return Err(CoveError::ArithOverflow);
    }
    let mut raw = [0u8; 16];
    raw[..4].copy_from_slice(&len.to_le_bytes());
    raw[4..4 + bytes.len()].copy_from_slice(bytes);
    Ok(u128::from_le_bytes(raw))
}

fn buffered_byte_view(data: &[u8], start: usize, end: usize) -> Result<u128, CoveError> {
    let len = end.checked_sub(start).ok_or(CoveError::PageCorrupt)?;
    let len = u32::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
    let offset = u32::try_from(start).map_err(|_| CoveError::ArithOverflow)?;
    let prefix_end = start.checked_add(4).ok_or(CoveError::ArithOverflow)?;
    if prefix_end > end {
        return Err(CoveError::PageCorrupt);
    }
    Ok(ByteView::new(len, &data[start..prefix_end])
        .with_buffer_index(0)
        .with_offset(offset)
        .as_u128())
}

fn byte_view_for_range(data: &[u8], start: usize, end: usize) -> Result<u128, CoveError> {
    let len = end.checked_sub(start).ok_or(CoveError::PageCorrupt)?;
    if u32::try_from(len).map_err(|_| CoveError::ArithOverflow)? <= MAX_INLINE_VIEW_LEN {
        inline_byte_view(&data[start..end])
    } else {
        buffered_byte_view(data, start, end)
    }
}

fn validate_utf8_offsets_values(
    offsets: &OffsetBuffer<i32>,
    values: &Buffer,
) -> Result<(), CoveError> {
    validate_utf8_offsets_slice(offsets, values.as_slice())
}

fn validate_utf8_offsets_slice(offsets: &[i32], values: &[u8]) -> Result<(), CoveError> {
    for pair in offsets.windows(2) {
        let start = usize::try_from(pair[0]).map_err(|_| CoveError::OffsetRange)?;
        let end = usize::try_from(pair[1]).map_err(|_| CoveError::OffsetRange)?;
        if start > end || end > values.len() {
            return Err(CoveError::OffsetRange);
        }
        // If the concatenated values buffer is valid UTF-8, each row is valid
        // iff every non-empty row starts on a codepoint boundary. The only way
        // two adjacent rows can form a valid cross-row codepoint is when the
        // later row starts with a continuation byte.
        if start != 0 && start != end && (values[start] & 0b1100_0000) == 0b1000_0000 {
            return Err(CoveError::BadSection(
                "Arrow Utf8 export: row boundary splits a UTF-8 codepoint".into(),
            ));
        }
    }
    if values.is_ascii() {
        return Ok(());
    }
    validate_arrow_utf8(values)
}

const ASCII_HIGH_BIT_MASK_U64: u64 = 0x8080_8080_8080_8080;
const FIXED_U32_NO_NULLS_MAX_ROWS: usize = 16;
const FIXED_U32_NO_NULLS_MAX_DATA_BYTES: usize = 1024;

#[inline(always)]
fn validate_arrow_utf8(bytes: &[u8]) -> Result<(), CoveError> {
    simdutf8::basic::from_utf8(bytes)
        .map(|_| ())
        .map_err(|err| CoveError::BadSection(format!("Arrow Utf8 export: {err}")))
}

#[inline(always)]
unsafe fn read_u32_le_unaligned(src: *const u8) -> u32 {
    // SAFETY: callers prove that `src..src + 4` is in bounds for the backing
    // byte slice before invoking this helper. `read_unaligned` handles any byte
    // alignment accepted by COVE wire payloads.
    u32::from_le(unsafe { ptr::read_unaligned(src.cast::<u32>()) })
}

#[inline(always)]
unsafe fn copy_varbytes_value(src: *const u8, dst: *mut u8, len: usize) {
    if len <= 16 {
        // SAFETY: the caller proves both ranges are valid for `len` bytes and
        // non-overlapping. The small-copy helper only reads and writes within
        // those same ranges.
        unsafe {
            copy_small_varbytes_value(src, dst, len);
        }
    } else {
        // SAFETY: forwarded caller invariant.
        unsafe {
            ptr::copy_nonoverlapping(src, dst, len);
        }
    }
}

#[inline(always)]
unsafe fn copy_varbytes_value_ascii_mask(src: *const u8, dst: *mut u8, len: usize) -> u64 {
    if len <= 16 {
        // SAFETY: forwarded caller invariant.
        return unsafe { copy_small_varbytes_value_ascii_mask(src, dst, len) };
    }
    if len <= 64 {
        let mut mask = 0u64;
        let mut offset = 0usize;
        while offset + 8 <= len {
            // SAFETY: `offset + 8 <= len`, and callers prove both ranges are
            // valid for `len` bytes.
            let word = unsafe { ptr::read_unaligned(src.add(offset).cast::<u64>()) };
            // SAFETY: destination range is valid for the same initialized word.
            unsafe {
                ptr::write_unaligned(dst.add(offset).cast::<u64>(), word);
            }
            mask |= word & ASCII_HIGH_BIT_MASK_U64;
            offset += 8;
        }
        if offset < len {
            // SAFETY: tail lies inside the proven source/destination ranges.
            mask |= unsafe {
                copy_small_varbytes_value_ascii_mask(src.add(offset), dst.add(offset), len - offset)
            };
        }
        return mask;
    }

    // SAFETY: forwarded caller invariant. Large rows use the platform memcpy
    // for throughput, then scan the source with raw loads only on the strict
    // validation path.
    unsafe {
        ptr::copy_nonoverlapping(src, dst, len);
    }
    // SAFETY: source is valid for `len` bytes by caller invariant.
    unsafe { ascii_high_bit_mask(src, len) }
}

#[inline(always)]
unsafe fn ascii_high_bit_mask(src: *const u8, len: usize) -> u64 {
    let mut mask = 0u64;
    let mut offset = 0usize;
    while offset + 8 <= len {
        // SAFETY: `offset + 8 <= len`, and callers prove source validity.
        let word = unsafe { ptr::read_unaligned(src.add(offset).cast::<u64>()) };
        mask |= word & ASCII_HIGH_BIT_MASK_U64;
        offset += 8;
    }
    while offset < len {
        // SAFETY: tail byte lies inside the proven source range.
        mask |= (unsafe { src.add(offset).read() } as u64) & 0x80;
        offset += 1;
    }
    mask
}

#[inline(always)]
unsafe fn copy_small_varbytes_value(src: *const u8, dst: *mut u8, len: usize) {
    match len {
        0 => {}
        1 => {
            // SAFETY: caller proved one byte is readable and writable.
            unsafe {
                dst.write(src.read());
            }
        }
        2 => unsafe {
            ptr::write_unaligned(dst.cast::<u16>(), ptr::read_unaligned(src.cast::<u16>()));
        },
        3 => unsafe {
            ptr::write_unaligned(dst.cast::<u16>(), ptr::read_unaligned(src.cast::<u16>()));
            dst.add(2).write(src.add(2).read());
        },
        4 => unsafe {
            ptr::write_unaligned(dst.cast::<u32>(), ptr::read_unaligned(src.cast::<u32>()));
        },
        5..=7 => unsafe {
            ptr::write_unaligned(dst.cast::<u32>(), ptr::read_unaligned(src.cast::<u32>()));
            ptr::write_unaligned(
                dst.add(len - 4).cast::<u32>(),
                ptr::read_unaligned(src.add(len - 4).cast::<u32>()),
            );
        },
        8 => unsafe {
            ptr::write_unaligned(dst.cast::<u64>(), ptr::read_unaligned(src.cast::<u64>()));
        },
        9..=16 => unsafe {
            ptr::write_unaligned(dst.cast::<u64>(), ptr::read_unaligned(src.cast::<u64>()));
            ptr::write_unaligned(
                dst.add(len - 8).cast::<u64>(),
                ptr::read_unaligned(src.add(len - 8).cast::<u64>()),
            );
        },
        _ => unsafe {
            ptr::copy_nonoverlapping(src, dst, len);
        },
    }
}

#[inline(always)]
unsafe fn copy_small_varbytes_value_ascii_mask(src: *const u8, dst: *mut u8, len: usize) -> u64 {
    match len {
        0 => 0,
        1 => {
            // SAFETY: caller proved one byte is readable and writable.
            let byte = unsafe { src.read() };
            // SAFETY: destination byte is valid.
            unsafe {
                dst.write(byte);
            }
            (byte as u64) & 0x80
        }
        2 => unsafe {
            let word = ptr::read_unaligned(src.cast::<u16>());
            ptr::write_unaligned(dst.cast::<u16>(), word);
            (word as u64) & 0x8080
        },
        3 => unsafe {
            let first = ptr::read_unaligned(src.cast::<u16>());
            let last = src.add(2).read();
            ptr::write_unaligned(dst.cast::<u16>(), first);
            dst.add(2).write(last);
            ((first as u64) & 0x8080) | ((last as u64) & 0x80)
        },
        4 => unsafe {
            let word = ptr::read_unaligned(src.cast::<u32>());
            ptr::write_unaligned(dst.cast::<u32>(), word);
            (word as u64) & 0x8080_8080
        },
        5..=7 => unsafe {
            let first = ptr::read_unaligned(src.cast::<u32>());
            let last = ptr::read_unaligned(src.add(len - 4).cast::<u32>());
            ptr::write_unaligned(dst.cast::<u32>(), first);
            ptr::write_unaligned(dst.add(len - 4).cast::<u32>(), last);
            ((first as u64) | (last as u64)) & 0x8080_8080
        },
        8 => unsafe {
            let word = ptr::read_unaligned(src.cast::<u64>());
            ptr::write_unaligned(dst.cast::<u64>(), word);
            word & ASCII_HIGH_BIT_MASK_U64
        },
        9..=16 => unsafe {
            let first = ptr::read_unaligned(src.cast::<u64>());
            let last = ptr::read_unaligned(src.add(len - 8).cast::<u64>());
            ptr::write_unaligned(dst.cast::<u64>(), first);
            ptr::write_unaligned(dst.add(len - 8).cast::<u64>(), last);
            (first | last) & ASCII_HIGH_BIT_MASK_U64
        },
        _ => unsafe { copy_varbytes_value_ascii_mask(src, dst, len) },
    }
}

fn fixed_u32_no_nulls_len(data: &[u8], row_count: usize) -> Result<Option<usize>, CoveError> {
    if row_count == 0 {
        return Ok(None);
    }
    if data.len() < 4 {
        return Err(CoveError::OffsetRange);
    }
    // SAFETY: `data.len() >= 4` proves the first prefix is readable.
    let fixed_len = unsafe { read_u32_le_unaligned(data.as_ptr()) } as usize;
    let stride = fixed_len.checked_add(4).ok_or(CoveError::ArithOverflow)?;
    let expected_len = row_count
        .checked_mul(stride)
        .ok_or(CoveError::ArithOverflow)?;
    if expected_len != data.len() {
        return Ok(None);
    }
    for row in 1..row_count {
        let pos = row.checked_mul(stride).ok_or(CoveError::ArithOverflow)?;
        // SAFETY: `expected_len == data.len()` and `pos` is a stride boundary
        // for `row < row_count`, so `pos..pos + 4` is in-bounds.
        let len = unsafe { read_u32_le_unaligned(data.as_ptr().add(pos)) } as usize;
        if len != fixed_len {
            return Ok(None);
        }
    }
    Ok(Some(fixed_len))
}

#[inline(always)]
unsafe fn copy_fixed_varbytes_value(src: *const u8, dst: *mut u8, len: usize) {
    if len <= 16 {
        // SAFETY: forwarded caller invariant.
        unsafe {
            copy_small_varbytes_value(src, dst, len);
        }
    } else {
        // SAFETY: forwarded caller invariant.
        unsafe {
            ptr::copy_nonoverlapping(src, dst, len);
        }
    }
}

#[inline(always)]
unsafe fn copy_fixed_varbytes_value_ascii_mask(src: *const u8, dst: *mut u8, len: usize) -> u64 {
    if len <= 16 {
        // SAFETY: forwarded caller invariant.
        unsafe { copy_small_varbytes_value_ascii_mask(src, dst, len) }
    } else {
        // SAFETY: forwarded caller invariant.
        unsafe { copy_varbytes_value_ascii_mask(src, dst, len) }
    }
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
        if let Some(result) = self.materialize_selected_forward_u32(array, selection)? {
            return Ok(result);
        }
        let ranges = self.parse_ranges(array)?;
        self.materialize_selected(array, selection, &ranges)
    }

    fn materialize_utf8(
        &self,
        array: &EncodedArray<'_>,
        selection: ArrowRowSelection<'_>,
        string_validation_policy: ArrowStringValidationPolicy,
    ) -> Result<(OffsetBuffer<i32>, Buffer, Option<NullBuffer>), CoveError> {
        let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
        let has_nulls = match array.validity {
            Some(validity) => validity.null_count()? > 0,
            None => false,
        };
        if matches!(selection, ArrowRowSelection::All)
            && !has_nulls
            && matches!(self.layout, BytePayloadLayout::U32LengthPrefixed)
        {
            return match string_validation_policy {
                ArrowStringValidationPolicy::Strict
                | ArrowStringValidationPolicy::StrictOrCachedProof => {
                    self.materialize_all_u32_no_nulls_utf8_strict(array, row_count)
                }
                ArrowStringValidationPolicy::TrustedPageProof => {
                    self.materialize_all_u32_no_nulls(array, row_count)
                }
            };
        }
        let (offsets, values, nulls) = self.materialize(array, selection)?;
        if matches!(
            string_validation_policy,
            ArrowStringValidationPolicy::Strict | ArrowStringValidationPolicy::StrictOrCachedProof
        ) {
            validate_utf8_offsets_values(&offsets, &values)?;
        }
        Ok((offsets, values, nulls))
    }

    fn materialize_view(
        &self,
        array: &EncodedArray<'_>,
        selection: ArrowRowSelection<'_>,
        data_owner: Option<&ArrowBufferOwner>,
    ) -> Result<(ScalarBuffer<u128>, Vec<Buffer>, Option<NullBuffer>), CoveError> {
        if matches!(selection, ArrowRowSelection::All) {
            return self.materialize_view_all_rows(array, data_owner);
        }
        let ranges = self.parse_ranges(array)?;
        let selected_len = selection.selected_len(array.row_count)?;
        let has_nulls = match array.validity {
            Some(validity) => validity.null_count()? > 0,
            None => false,
        };
        let mut views = Vec::with_capacity(selected_len);
        let mut validity_builder = has_nulls
            .then(|| ArrowValidityBuilder::new(selected_len))
            .transpose()?;
        selection.for_each_row(array.row_count, |row| {
            let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
            let is_null = has_nulls && array.is_null(row_u64)?;
            if let Some(builder) = &mut validity_builder {
                builder.append(!is_null);
            }
            if is_null {
                views.push(0u128);
                return Ok(());
            }
            let (start, end) = ranges[row];
            views.push(byte_view_for_range(array.data, start, end)?);
            Ok(())
        })?;

        let buffers = vec![byte_view_backing_buffer(array, data_owner)?];
        Ok((
            ScalarBuffer::from(views),
            buffers,
            validity_builder.and_then(ArrowValidityBuilder::finish),
        ))
    }

    fn materialize_view_all_rows(
        &self,
        array: &EncodedArray<'_>,
        data_owner: Option<&ArrowBufferOwner>,
    ) -> Result<(ScalarBuffer<u128>, Vec<Buffer>, Option<NullBuffer>), CoveError> {
        let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
        let has_nulls = match array.validity {
            Some(validity) => validity.null_count()? > 0,
            None => false,
        };
        let mut views = Vec::with_capacity(row_count);
        let mut validity_builder = has_nulls
            .then(|| ArrowValidityBuilder::new(row_count))
            .transpose()?;

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
            if let Some(builder) = &mut validity_builder {
                builder.append(!is_null);
            }
            if is_null {
                views.push(0u128);
            } else {
                views.push(byte_view_for_range(data, data_start, data_end)?);
            }
        }
        if pos != data.len() {
            return Err(CoveError::PageCorrupt);
        }

        let buffers = vec![byte_view_backing_buffer(array, data_owner)?];
        Ok((
            ScalarBuffer::from(views),
            buffers,
            validity_builder.and_then(ArrowValidityBuilder::finish),
        ))
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
        if !has_nulls && matches!(self.layout, BytePayloadLayout::U32LengthPrefixed) {
            return self.materialize_all_u32_no_nulls(array, row_count);
        }
        let Some(offset_capacity) = row_count.checked_add(1) else {
            return Err(CoveError::ArithOverflow);
        };
        let mut offsets = Vec::<i32>::with_capacity(offset_capacity);
        let mut values = Vec::with_capacity(array.data.len());
        let mut validity_builder = has_nulls
            .then(|| ArrowValidityBuilder::new(row_count))
            .transpose()?;
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
            if let Some(builder) = &mut validity_builder {
                builder.append(!is_null);
            }
            if !is_null {
                values.extend_from_slice(&data[data_start..data_end]);
            }
            offsets.push(i32::try_from(values.len()).map_err(|_| CoveError::ArithOverflow)?);
        }
        if pos != data.len() {
            return Err(CoveError::PageCorrupt);
        }

        let offsets = trusted_i32_offset_buffer(offsets);
        let nulls = validity_builder.and_then(ArrowValidityBuilder::finish);
        Ok((offsets, Buffer::from_vec(values), nulls))
    }

    fn materialize_all_u32_no_nulls(
        &self,
        array: &EncodedArray<'_>,
        row_count: usize,
    ) -> Result<(OffsetBuffer<i32>, Buffer, Option<NullBuffer>), CoveError> {
        self.materialize_all_u32_no_nulls_impl::<false>(array, row_count)
    }

    fn materialize_all_u32_no_nulls_utf8_strict(
        &self,
        array: &EncodedArray<'_>,
        row_count: usize,
    ) -> Result<(OffsetBuffer<i32>, Buffer, Option<NullBuffer>), CoveError> {
        self.materialize_all_u32_no_nulls_impl::<true>(array, row_count)
    }

    fn materialize_all_u32_no_nulls_impl<const VALIDATE_UTF8: bool>(
        &self,
        array: &EncodedArray<'_>,
        row_count: usize,
    ) -> Result<(OffsetBuffer<i32>, Buffer, Option<NullBuffer>), CoveError> {
        let data = array.data;
        // The fixed-length detector needs its own prefix scan. Keep it narrow:
        // large pages are faster on the generic one-pass copy path.
        if row_count <= FIXED_U32_NO_NULLS_MAX_ROWS
            && data.len() <= FIXED_U32_NO_NULLS_MAX_DATA_BYTES
        {
            if let Some(fixed_len) = fixed_u32_no_nulls_len(data, row_count)? {
                return self.materialize_all_u32_no_nulls_fixed_impl::<VALIDATE_UTF8>(
                    data, row_count, fixed_len,
                );
            }
        }
        let Some(prefix_bytes) = row_count.checked_mul(4) else {
            return Err(CoveError::ArithOverflow);
        };
        if prefix_bytes > data.len() {
            return Err(CoveError::OffsetRange);
        }
        let value_len = data.len() - prefix_bytes;
        if value_len > i32::MAX as usize {
            return Err(CoveError::ArithOverflow);
        }

        let Some(offset_capacity) = row_count.checked_add(1) else {
            return Err(CoveError::ArithOverflow);
        };
        let mut offsets = Vec::<i32>::with_capacity(offset_capacity);
        let mut values = Vec::<u8>::with_capacity(value_len);
        let mut pos = 0usize;
        let mut write = 0usize;
        let mut saw_non_ascii = false;
        let offsets_ptr = offsets.as_mut_ptr();
        // INVARIANT: offset 0 is always initialized, and the vector length is
        // published only after all row offsets have been written.
        // SAFETY: `offsets` has capacity `row_count + 1`, so slot 0 is valid.
        unsafe {
            offsets_ptr.write(0i32);
        }
        for row in 0..row_count {
            if pos > data.len().saturating_sub(4) {
                return Err(CoveError::OffsetRange);
            }
            // INVARIANT: the branch above proves that `pos..pos + 4` is
            // in-bounds for `data`.
            // SAFETY: the length prefix pointer is valid for four bytes.
            let len = unsafe { read_u32_le_unaligned(data.as_ptr().add(pos)) } as usize;
            pos += 4;
            if len > data.len() - pos {
                return Err(CoveError::OffsetRange);
            }
            if len > value_len - write {
                return Err(CoveError::PageCorrupt);
            }
            let data_end = pos + len;
            let next_write = write + len;
            let src = data.as_ptr().wrapping_add(pos);
            let dst = values.as_mut_ptr().wrapping_add(write);
            // INVARIANT: bounds above prove source and destination ranges are
            // in-bounds and non-overlapping; destination length is published
            // only after every row has validated.
            // SAFETY: `values` has at least `value_len` capacity, source points
            // into immutable `data`, and both pointers are valid for `len`
            // bytes.
            if VALIDATE_UTF8 {
                saw_non_ascii |= unsafe { copy_varbytes_value_ascii_mask(src, dst, len) } != 0;
            } else {
                unsafe {
                    copy_varbytes_value(src, dst, len);
                }
            }
            pos = data_end;
            write = next_write;
            // INVARIANT: `value_len <= i32::MAX` was pre-proven and
            // `write <= value_len` is maintained by the checked length branch.
            // SAFETY: `row + 1 < offset_capacity`, so this raw write targets a
            // reserved offset slot.
            unsafe {
                offsets_ptr.add(row + 1).write(write as i32);
            }
        }
        if pos != data.len() || write != value_len {
            return Err(CoveError::PageCorrupt);
        }
        // INVARIANT: every byte in 0..value_len was initialized exactly once by
        // the checked copy loop above.
        // SAFETY: the vector has capacity `value_len`, and all elements in the
        // new initialized length have been written.
        unsafe {
            values.set_len(value_len);
        }
        // INVARIANT: slot 0 and one offset per row were initialized in order.
        // SAFETY: all elements in 0..offset_capacity have been written.
        unsafe {
            offsets.set_len(offset_capacity);
        }
        if VALIDATE_UTF8 && saw_non_ascii {
            validate_utf8_offsets_slice(&offsets, &values)?;
        }

        Ok((
            trusted_i32_offset_buffer(offsets),
            Buffer::from_vec(values),
            None,
        ))
    }

    fn materialize_all_u32_no_nulls_fixed_impl<const VALIDATE_UTF8: bool>(
        &self,
        data: &[u8],
        row_count: usize,
        fixed_len: usize,
    ) -> Result<(OffsetBuffer<i32>, Buffer, Option<NullBuffer>), CoveError> {
        let value_len = row_count
            .checked_mul(fixed_len)
            .ok_or(CoveError::ArithOverflow)?;
        if value_len > i32::MAX as usize {
            return Err(CoveError::ArithOverflow);
        }
        let Some(offset_capacity) = row_count.checked_add(1) else {
            return Err(CoveError::ArithOverflow);
        };
        let mut offsets = Vec::<i32>::with_capacity(offset_capacity);
        let mut values = Vec::<u8>::with_capacity(value_len);
        let mut saw_non_ascii = false;
        let offsets_ptr = offsets.as_mut_ptr();
        let values_ptr = values.as_mut_ptr();
        // INVARIANT: fixed-length U32 VarBytes pages have already been
        // pre-scanned for exact row count, equal prefixes, and total length.
        // Offsets are therefore an arithmetic progression by `fixed_len`.
        // SAFETY: `offsets` has `row_count + 1` capacity and every slot is
        // written exactly once before `set_len`.
        unsafe {
            offsets_ptr.write(0i32);
        }
        let stride = fixed_len.checked_add(4).ok_or(CoveError::ArithOverflow)?;
        for row in 0..row_count {
            let data_start = row
                .checked_mul(stride)
                .and_then(|offset| offset.checked_add(4))
                .ok_or(CoveError::ArithOverflow)?;
            let write = row.checked_mul(fixed_len).ok_or(CoveError::ArithOverflow)?;
            let src = data.as_ptr().wrapping_add(data_start);
            let dst = values_ptr.wrapping_add(write);
            // INVARIANT: `fixed_u32_no_nulls_len` proved every source range is
            // in-bounds. `value_len` was computed as `row_count * fixed_len`,
            // so each destination range lies inside the reserved capacity.
            // SAFETY: source and destination ranges are valid for `fixed_len`
            // bytes and do not overlap.
            if VALIDATE_UTF8 {
                saw_non_ascii |=
                    unsafe { copy_fixed_varbytes_value_ascii_mask(src, dst, fixed_len) } != 0;
            } else {
                unsafe {
                    copy_fixed_varbytes_value(src, dst, fixed_len);
                }
            }
            let next = write
                .checked_add(fixed_len)
                .ok_or(CoveError::ArithOverflow)?;
            // SAFETY: `row + 1 < offset_capacity`, and `next <= value_len <= i32::MAX`.
            unsafe {
                offsets_ptr.add(row + 1).write(next as i32);
            }
        }
        // SAFETY: all destination bytes and offset slots were initialized by
        // the fixed-length copy loop above.
        unsafe {
            values.set_len(value_len);
            offsets.set_len(offset_capacity);
        }
        if VALIDATE_UTF8 && saw_non_ascii {
            validate_utf8_offsets_slice(&offsets, &values)?;
        }
        Ok((
            trusted_i32_offset_buffer(offsets),
            Buffer::from_vec(values),
            None,
        ))
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
        let mut values = Vec::<u8>::with_capacity(value_len);
        let mut validity_builder = any_null
            .then(|| ArrowValidityBuilder::new(selected_len))
            .transpose()?;
        let mut write = 0usize;
        offsets.push(0i32);
        selection.for_each_row(array.row_count, |row| {
            let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
            let is_null = has_nulls && array.is_null(row_u64)?;
            if let Some(builder) = &mut validity_builder {
                builder.append(!is_null);
            }
            if !is_null {
                let (start, end) = ranges[row];
                let len = end.checked_sub(start).ok_or(CoveError::PageCorrupt)?;
                let next = write.checked_add(len).ok_or(CoveError::ArithOverflow)?;
                if next > value_len {
                    return Err(CoveError::PageCorrupt);
                }
                // INVARIANT: the prepass computed `value_len` from the same
                // selected non-null ranges, and every source range was parsed
                // and bounds-checked before this copy pass.
                // SAFETY: `values` has capacity `value_len`, source and
                // destination ranges are in-bounds and non-overlapping, and the
                // initialized length is set only after all copies finish.
                unsafe {
                    ptr::copy_nonoverlapping(
                        array.data.as_ptr().add(start),
                        values.as_mut_ptr().add(write),
                        len,
                    );
                }
                write = next;
            }
            offsets.push(i32::try_from(write).map_err(|_| CoveError::ArithOverflow)?);
            Ok(())
        })?;
        if write != value_len {
            return Err(CoveError::PageCorrupt);
        }
        // INVARIANT: selected copy loop initialized exactly the `write` prefix,
        // and `write == value_len` was proven by the prepass/debug assertion.
        // SAFETY: the vector capacity is `value_len` and all bytes in that
        // range have been written.
        unsafe {
            values.set_len(value_len);
        }

        let offsets = trusted_i32_offset_buffer(offsets);
        let nulls = validity_builder.and_then(ArrowValidityBuilder::finish);
        Ok((offsets, Buffer::from_vec(values), nulls))
    }

    fn materialize_selected_forward_u32(
        &self,
        array: &EncodedArray<'_>,
        selection: ArrowRowSelection<'_>,
    ) -> Result<Option<(OffsetBuffer<i32>, Buffer, Option<NullBuffer>)>, CoveError> {
        if !matches!(self.layout, BytePayloadLayout::U32LengthPrefixed) {
            return Ok(None);
        }
        let selected_len = selection.selected_len(array.row_count)?;
        if selected_len == 0 {
            return Ok(Some((
                trusted_i32_offset_buffer(vec![0]),
                Buffer::from_vec(Vec::<u8>::new()),
                None,
            )));
        }
        let last_selected = match selection {
            ArrowRowSelection::Rows(rows) => {
                if !rows.windows(2).all(|pair| pair[0] < pair[1]) {
                    return Ok(None);
                }
                let last = *rows.last().ok_or(CoveError::OffsetRange)? as usize;
                if u64::try_from(last).map_err(|_| CoveError::ArithOverflow)? >= array.row_count {
                    return Err(CoveError::OffsetRange);
                }
                last
            }
            ArrowRowSelection::Bitset { words, len } => {
                selection.validate_for_row_count(array.row_count)?;
                last_selected_bitset_row(words, len).ok_or(CoveError::OffsetRange)?
            }
            ArrowRowSelection::All => return Ok(None),
        };

        let has_nulls = array_has_nulls(array)?;
        let Some(offset_capacity) = selected_len.checked_add(1) else {
            return Err(CoveError::ArithOverflow);
        };
        let mut offsets = Vec::with_capacity(offset_capacity);
        let mut values = Vec::<u8>::with_capacity(array.data.len().min(selected_len * 16));
        let mut validity_builder = has_nulls
            .then(|| ArrowValidityBuilder::new(selected_len))
            .transpose()?;
        offsets.push(0);
        let mut pos = 0usize;
        let mut next_row_index = 0usize;
        for row in 0..=last_selected {
            let (data_start, data_end) = read_u32_len_prefixed_range(array.data, pos)?;
            pos = data_end;
            let selected = match selection {
                ArrowRowSelection::Rows(rows) => {
                    if rows
                        .get(next_row_index)
                        .map(|candidate| *candidate as usize == row)
                        .unwrap_or(false)
                    {
                        next_row_index += 1;
                        true
                    } else {
                        false
                    }
                }
                ArrowRowSelection::Bitset { words, .. } => bitset_row_selected(words, row),
                ArrowRowSelection::All => false,
            };
            if !selected {
                continue;
            }
            let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
            let is_null = has_nulls && array.is_null(row_u64)?;
            if let Some(builder) = &mut validity_builder {
                builder.append(!is_null);
            }
            if !is_null {
                values.extend_from_slice(&array.data[data_start..data_end]);
            }
            offsets.push(i32::try_from(values.len()).map_err(|_| CoveError::ArithOverflow)?);
        }
        if last_selected + 1
            == usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?
            && pos != array.data.len()
        {
            return Err(CoveError::PageCorrupt);
        }
        if offsets.len() != offset_capacity {
            return Err(CoveError::PageCorrupt);
        }
        let offsets = trusted_i32_offset_buffer(offsets);
        let nulls = validity_builder.and_then(ArrowValidityBuilder::finish);
        Ok(Some((offsets, Buffer::from_vec(values), nulls)))
    }
}

fn bitset_row_selected(words: &[u64], row: usize) -> bool {
    words
        .get(row / 64)
        .map(|word| (*word & (1u64 << (row % 64))) != 0)
        .unwrap_or(false)
}

fn last_selected_bitset_row(words: &[u64], len: usize) -> Option<usize> {
    let word_len = len.div_ceil(64);
    for word_index in (0..word_len).rev() {
        let mut word = words.get(word_index).copied().unwrap_or(0);
        if word_index + 1 == word_len {
            word = mask_selection_tail(word, len);
        }
        if word != 0 {
            return Some(word_index * 64 + (63 - word.leading_zeros() as usize));
        }
    }
    None
}

fn try_direct_primitive_array(
    array: &EncodedArray<'_>,
    data_type: &DataType,
    data_owner: Option<&ArrowBufferOwner>,
) -> Result<Option<ArrayRef>, CoveError> {
    match array.encoding {
        CoveEncodingKind::NumCode if array.physical == CovePhysicalKind::NumCode => match data_type
        {
            DataType::Int64 => {
                if let Some(values) = retained_numcode_i64_values(array, data_owner)? {
                    return Ok(Some(Arc::new(Int64Array::new(values, None)) as ArrayRef));
                }
                Ok(Some(Arc::new(numcode_i64_array(array)?) as ArrayRef))
            }
            DataType::UInt64 => {
                if let Some(values) = retained_numcode_u64_values(array, data_owner)? {
                    return Ok(Some(Arc::new(UInt64Array::new(values, None)) as ArrayRef));
                }
                Ok(Some(Arc::new(numcode_u64_array(array)?) as ArrayRef))
            }
            DataType::Timestamp(TimeUnit::Microsecond, None) => {
                if let Some(values) = retained_numcode_i64_values(array, data_owner)? {
                    return Ok(Some(
                        Arc::new(TimestampMicrosecondArray::new(values, None)) as ArrayRef
                    ));
                }
                Ok(Some(
                    Arc::new(timestamp_micros_array(array, ArrowRowSelection::All)?) as ArrayRef,
                ))
            }
            DataType::Timestamp(TimeUnit::Nanosecond, None) => {
                if let Some(values) = retained_numcode_i64_values(array, data_owner)? {
                    return Ok(Some(
                        Arc::new(TimestampNanosecondArray::new(values, None)) as ArrayRef
                    ));
                }
                Ok(Some(
                    Arc::new(timestamp_nanos_array(array, ArrowRowSelection::All)?) as ArrayRef,
                ))
            }
            _ => Ok(None),
        },
        CoveEncodingKind::PlainFixed
            if array.logical == CoveLogicalType::Bool && *data_type == DataType::Boolean =>
        {
            Ok(Some(Arc::new(plain_bool_array(array)?) as ArrayRef))
        }
        CoveEncodingKind::PlainFixed => {
            try_direct_plain_fixed_array(array, ArrowRowSelection::All, data_type, data_owner)
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
                Arc::new(numcode_i64_array_for_selection(array, selection)?) as ArrayRef,
            )),
            DataType::UInt64 => Ok(Some(
                Arc::new(numcode_u64_array_for_selection(array, selection)?) as ArrayRef,
            )),
            DataType::Timestamp(TimeUnit::Microsecond, None) => Ok(Some(Arc::new(
                timestamp_micros_array(array, selection)?,
            ) as ArrayRef)),
            DataType::Timestamp(TimeUnit::Nanosecond, None) => Ok(Some(Arc::new(
                timestamp_nanos_array(array, selection)?,
            ) as ArrayRef)),
            _ => Ok(None),
        },
        CoveEncodingKind::PlainFixed
            if array.logical == CoveLogicalType::Bool && *data_type == DataType::Boolean =>
        {
            Ok(Some(
                Arc::new(plain_bool_array_for_selection(array, selection)?) as ArrayRef,
            ))
        }
        CoveEncodingKind::PlainFixed => {
            try_direct_plain_fixed_array(array, selection, data_type, None)
        }
        _ => Ok(None),
    }
}

fn try_direct_plain_fixed_array(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data_type: &DataType,
    _data_owner: Option<&ArrowBufferOwner>,
) -> Result<Option<ArrayRef>, CoveError> {
    if array.encoding != CoveEncodingKind::PlainFixed {
        return Ok(None);
    }
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let width = crate::array::logical_type_fixed_width(array.logical).ok_or_else(|| {
        CoveError::UnsupportedEncoding(format!(
            "PlainFixed Arrow export requires fixed-width logical type, got {:?}",
            array.logical
        ))
    })?;
    let data = fixed_width_payload_prefix(array.data, row_count, width)?;
    match data_type {
        DataType::Int8 => Ok(Some(Arc::new(plain_fixed_native_array::<Int8Type, _>(
            array,
            selection,
            data,
            width,
            |bytes| exact_bytes::<1>(bytes).map(i8::from_le_bytes),
        )?) as ArrayRef)),
        DataType::Int16 => Ok(Some(Arc::new(plain_fixed_native_array::<Int16Type, _>(
            array,
            selection,
            data,
            width,
            |bytes| exact_bytes::<2>(bytes).map(i16::from_le_bytes),
        )?) as ArrayRef)),
        DataType::Int32 => Ok(Some(Arc::new(plain_fixed_native_array::<Int32Type, _>(
            array,
            selection,
            data,
            width,
            |bytes| exact_bytes::<4>(bytes).map(i32::from_le_bytes),
        )?) as ArrayRef)),
        DataType::Date32 => {
            let (values, nulls) =
                collect_plain_fixed_native::<i32, _>(array, selection, data, width, |bytes| {
                    exact_bytes::<4>(bytes).map(i32::from_le_bytes)
                })?;
            Ok(Some(
                Arc::new(Date32Array::new(ScalarBuffer::from(values), nulls)) as ArrayRef,
            ))
        }
        DataType::Int64 => Ok(Some(Arc::new(plain_fixed_native_array::<Int64Type, _>(
            array,
            selection,
            data,
            width,
            |bytes| exact_bytes::<8>(bytes).map(i64::from_le_bytes),
        )?) as ArrayRef)),
        DataType::UInt8 => Ok(Some(Arc::new(plain_fixed_native_array::<UInt8Type, _>(
            array,
            selection,
            data,
            width,
            |bytes| exact_bytes::<1>(bytes).map(u8::from_le_bytes),
        )?) as ArrayRef)),
        DataType::UInt16 => Ok(Some(Arc::new(plain_fixed_native_array::<UInt16Type, _>(
            array,
            selection,
            data,
            width,
            |bytes| exact_bytes::<2>(bytes).map(u16::from_le_bytes),
        )?) as ArrayRef)),
        DataType::UInt32 => Ok(Some(Arc::new(plain_fixed_native_array::<UInt32Type, _>(
            array,
            selection,
            data,
            width,
            |bytes| exact_bytes::<4>(bytes).map(u32::from_le_bytes),
        )?) as ArrayRef)),
        DataType::UInt64 => Ok(Some(Arc::new(plain_fixed_native_array::<UInt64Type, _>(
            array,
            selection,
            data,
            width,
            |bytes| exact_bytes::<8>(bytes).map(u64::from_le_bytes),
        )?) as ArrayRef)),
        DataType::Float32 => Ok(Some(Arc::new(plain_fixed_native_array::<Float32Type, _>(
            array,
            selection,
            data,
            width,
            |bytes| exact_bytes::<4>(bytes).map(|raw| f32::from_bits(u32::from_le_bytes(raw))),
        )?) as ArrayRef)),
        DataType::Float64 => Ok(Some(Arc::new(plain_fixed_native_array::<Float64Type, _>(
            array,
            selection,
            data,
            width,
            |bytes| exact_bytes::<8>(bytes).map(|raw| f64::from_bits(u64::from_le_bytes(raw))),
        )?) as ArrayRef)),
        DataType::Timestamp(TimeUnit::Microsecond, None) => {
            let (values, nulls) =
                collect_plain_fixed_native::<i64, _>(array, selection, data, width, |bytes| {
                    exact_bytes::<8>(bytes).map(i64::from_le_bytes)
                })?;
            Ok(Some(Arc::new(TimestampMicrosecondArray::new(
                ScalarBuffer::from(values),
                nulls,
            )) as ArrayRef))
        }
        DataType::Timestamp(TimeUnit::Nanosecond, None) => {
            let (values, nulls) =
                collect_plain_fixed_native::<i64, _>(array, selection, data, width, |bytes| {
                    exact_bytes::<8>(bytes).map(i64::from_le_bytes)
                })?;
            Ok(Some(Arc::new(TimestampNanosecondArray::new(
                ScalarBuffer::from(values),
                nulls,
            )) as ArrayRef))
        }
        DataType::Decimal128(precision, scale) => {
            let (values, nulls) = collect_plain_fixed_decimal128(array, selection, data, width)?;
            let array = Decimal128Array::new(ScalarBuffer::from(values), nulls)
                .with_precision_and_scale(*precision, *scale)
                .map_err(|err| CoveError::BadSection(format!("Arrow Decimal128: {err}")))?;
            Ok(Some(Arc::new(array) as ArrayRef))
        }
        DataType::FixedSizeBinary(size) => {
            let (values, nulls) = collect_plain_fixed_bytes(array, selection, data, width)?;
            let array = FixedSizeBinaryArray::try_new(*size, Buffer::from_vec(values), nulls)
                .map_err(|err| CoveError::BadSection(format!("Arrow FixedSizeBinary: {err}")))?;
            Ok(Some(Arc::new(array) as ArrayRef))
        }
        _ => Ok(None),
    }
}

fn plain_fixed_native_array<T, F>(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data: &[u8],
    width: usize,
    decode: F,
) -> Result<arrow_array::PrimitiveArray<T>, CoveError>
where
    T: arrow_array::types::ArrowPrimitiveType,
    T::Native: Default,
    F: Fn(&[u8]) -> Result<T::Native, CoveError>,
{
    let (values, nulls) =
        collect_plain_fixed_native::<T::Native, F>(array, selection, data, width, decode)?;
    Ok(arrow_array::PrimitiveArray::<T>::new(
        ScalarBuffer::from(values),
        nulls,
    ))
}

fn collect_plain_fixed_native<T, F>(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data: &[u8],
    width: usize,
    decode: F,
) -> Result<(Vec<T>, Option<NullBuffer>), CoveError>
where
    T: ArrowNativeType + Default,
    F: Fn(&[u8]) -> Result<T, CoveError>,
{
    let has_nulls = array_has_nulls(array)?;
    let selected_len = selection.selected_len(array.row_count)?;
    let mut values = Vec::with_capacity(selected_len);
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(selected_len))
        .transpose()?;
    selection.for_each_row(array.row_count, |row| {
        let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
        let is_null = has_nulls && array.is_null(row_u64)?;
        if let Some(builder) = &mut validity_builder {
            builder.append(!is_null);
        }
        if is_null {
            values.push(T::default());
            return Ok(());
        }
        let offset = row.checked_mul(width).ok_or(CoveError::ArithOverflow)?;
        let end = offset.checked_add(width).ok_or(CoveError::ArithOverflow)?;
        values.push(decode(
            data.get(offset..end).ok_or(CoveError::OffsetRange)?,
        )?);
        Ok(())
    })?;
    Ok((
        values,
        validity_builder.and_then(ArrowValidityBuilder::finish),
    ))
}

fn collect_plain_fixed_decimal128(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data: &[u8],
    width: usize,
) -> Result<(Vec<i128>, Option<NullBuffer>), CoveError> {
    match array.logical {
        CoveLogicalType::Decimal64 => {
            collect_plain_fixed_native(array, selection, data, width, |bytes| {
                exact_bytes::<8>(bytes)
                    .map(i64::from_le_bytes)
                    .map(i128::from)
            })
        }
        CoveLogicalType::Decimal128 => {
            collect_plain_fixed_native(array, selection, data, width, |bytes| {
                exact_bytes::<16>(bytes).map(i128::from_le_bytes)
            })
        }
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "Decimal128 Arrow export from {:?}",
            array.logical
        ))),
    }
}

fn collect_plain_fixed_bytes(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data: &[u8],
    width: usize,
) -> Result<(Vec<u8>, Option<NullBuffer>), CoveError> {
    let has_nulls = array_has_nulls(array)?;
    let selected_len = selection.selected_len(array.row_count)?;
    let value_len = selected_len
        .checked_mul(width)
        .ok_or(CoveError::ArithOverflow)?;
    let mut values = Vec::<u8>::with_capacity(value_len);
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(selected_len))
        .transpose()?;
    selection.for_each_row(array.row_count, |row| {
        let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
        let is_null = has_nulls && array.is_null(row_u64)?;
        if let Some(builder) = &mut validity_builder {
            builder.append(!is_null);
        }
        let offset = row.checked_mul(width).ok_or(CoveError::ArithOverflow)?;
        let end = offset.checked_add(width).ok_or(CoveError::ArithOverflow)?;
        if is_null {
            values.resize(values.len() + width, 0);
        } else {
            values.extend_from_slice(data.get(offset..end).ok_or(CoveError::OffsetRange)?);
        }
        Ok(())
    })?;
    Ok((
        values,
        validity_builder.and_then(ArrowValidityBuilder::finish),
    ))
}

fn try_direct_decoded_array(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data_type: &DataType,
) -> Result<Option<ArrayRef>, CoveError> {
    match array.encoding {
        CoveEncodingKind::Constant => {
            let payload = ConstantPayload::parse(array.data)?;
            if payload.row_count != array.row_count {
                return Err(CoveError::PageCorrupt);
            }
            direct_i64_values_to_arrow(array, selection, data_type, |row| {
                let _ = row;
                Ok(payload.value)
            })
        }
        CoveEncodingKind::PlainVarint => {
            let values = decode_plain_varint_u64_values(array)?;
            direct_u64_slice_to_arrow(array, &values, selection, data_type)
        }
        CoveEncodingKind::Rle => {
            let payload = RlePayload::parse(array.data)?;
            let values = Rle::fast_decode(&payload)?;
            direct_i64_slice_to_arrow(array, &values, selection, data_type)
        }
        CoveEncodingKind::RunEnd => {
            let payload = RunEndPayload::parse(array.data)?;
            let values = RunEnd::fast_decode(&payload)?;
            direct_i64_slice_to_arrow(array, &values, selection, data_type)
        }
        CoveEncodingKind::BitPacked => {
            let payload = BitPackedPayload::parse(array.data)?;
            let values = BitPacked::fast_decode(&payload)?;
            direct_i64_slice_to_arrow(array, &values, selection, data_type)
        }
        CoveEncodingKind::Delta => {
            let payload = DeltaPayload::parse(array.data)?;
            let values = Delta::fast_decode(&payload)?;
            direct_i64_slice_to_arrow(array, &values, selection, data_type)
        }
        CoveEncodingKind::FrameOfReference => {
            let payload = ForPayload::parse(array.data)?;
            let values = FrameOfReference::fast_decode(&payload)?;
            direct_i64_slice_to_arrow(array, &values, selection, data_type)
        }
        CoveEncodingKind::PatchedBase => {
            let payload = PatchedBasePayload::parse(array.data)?;
            let values = PatchedBase::fast_decode(&payload)?;
            direct_i64_slice_to_arrow(array, &values, selection, data_type)
        }
        CoveEncodingKind::Sparse => {
            let payload = SparsePayload::parse(array.data)?;
            let values = Sparse::fast_decode(&payload)?;
            direct_i64_slice_to_arrow(array, &values, selection, data_type)
        }
        CoveEncodingKind::LocalCodebook => {
            try_direct_local_codebook_array(array, selection, data_type)
        }
        _ => Ok(None),
    }
}

fn decode_plain_varint_u64_values(array: &EncodedArray<'_>) -> Result<Vec<u64>, CoveError> {
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let mut values = Vec::with_capacity(row_count);
    let mut pos = 0usize;
    for _ in 0..row_count {
        if pos >= array.data.len() {
            return Err(CoveError::OffsetRange);
        }
        let (value, consumed) = wire::decode_u64_leb128(&array.data[pos..])?;
        pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
        values.push(value);
    }
    if pos != array.data.len() {
        return Err(CoveError::PageCorrupt);
    }
    Ok(values)
}

fn direct_i64_slice_to_arrow(
    array: &EncodedArray<'_>,
    values: &[i64],
    selection: ArrowRowSelection<'_>,
    data_type: &DataType,
) -> Result<Option<ArrayRef>, CoveError> {
    if values.len() != usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)? {
        return Err(CoveError::PageCorrupt);
    }
    direct_i64_values_to_arrow(array, selection, data_type, |row| {
        values.get(row).copied().ok_or(CoveError::PageCorrupt)
    })
}

fn direct_i64_values_to_arrow<F>(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data_type: &DataType,
    value_at: F,
) -> Result<Option<ArrayRef>, CoveError>
where
    F: Fn(usize) -> Result<i64, CoveError>,
{
    match data_type {
        DataType::Boolean => {
            let (values, selected_len, nulls) =
                collect_i64_bool_values(array, selection, value_at)?;
            let values = BooleanBuffer::new(Buffer::from_vec(values), 0, selected_len);
            Ok(Some(Arc::new(BooleanArray::new(values, nulls)) as ArrayRef))
        }
        DataType::Int8 => Ok(Some(Arc::new(i64_values_primitive_array::<Int8Type, _, _>(
            array,
            selection,
            value_at,
            |value| i8::try_from(value).map_err(|_| CoveError::PageCorrupt),
        )?) as ArrayRef)),
        DataType::Int16 => Ok(Some(
            Arc::new(i64_values_primitive_array::<Int16Type, _, _>(
                array,
                selection,
                value_at,
                |value| i16::try_from(value).map_err(|_| CoveError::PageCorrupt),
            )?) as ArrayRef,
        )),
        DataType::Int32 => Ok(Some(
            Arc::new(i64_values_primitive_array::<Int32Type, _, _>(
                array,
                selection,
                value_at,
                |value| i32::try_from(value).map_err(|_| CoveError::PageCorrupt),
            )?) as ArrayRef,
        )),
        DataType::Date32 => {
            let (values, nulls) =
                collect_i64_values::<i32, _, _>(array, selection, value_at, |value| {
                    i32::try_from(value).map_err(|_| CoveError::PageCorrupt)
                })?;
            Ok(Some(
                Arc::new(Date32Array::new(ScalarBuffer::from(values), nulls)) as ArrayRef,
            ))
        }
        DataType::Int64 => Ok(Some(
            Arc::new(i64_values_primitive_array::<Int64Type, _, _>(
                array, selection, value_at, Ok,
            )?) as ArrayRef,
        )),
        DataType::UInt8 => Ok(Some(
            Arc::new(i64_values_primitive_array::<UInt8Type, _, _>(
                array,
                selection,
                value_at,
                |value| u8::try_from(value).map_err(|_| CoveError::PageCorrupt),
            )?) as ArrayRef,
        )),
        DataType::UInt16 => Ok(Some(
            Arc::new(i64_values_primitive_array::<UInt16Type, _, _>(
                array,
                selection,
                value_at,
                |value| u16::try_from(value).map_err(|_| CoveError::PageCorrupt),
            )?) as ArrayRef,
        )),
        DataType::UInt32 => Ok(Some(
            Arc::new(i64_values_primitive_array::<UInt32Type, _, _>(
                array,
                selection,
                value_at,
                |value| u32::try_from(value).map_err(|_| CoveError::PageCorrupt),
            )?) as ArrayRef,
        )),
        DataType::UInt64 => Ok(Some(
            Arc::new(i64_values_primitive_array::<UInt64Type, _, _>(
                array,
                selection,
                value_at,
                |value| u64::try_from(value).map_err(|_| CoveError::PageCorrupt),
            )?) as ArrayRef,
        )),
        DataType::Timestamp(TimeUnit::Microsecond, None) => {
            let (values, nulls) = collect_i64_values::<i64, _, _>(array, selection, value_at, Ok)?;
            Ok(Some(Arc::new(TimestampMicrosecondArray::new(
                ScalarBuffer::from(values),
                nulls,
            )) as ArrayRef))
        }
        DataType::Timestamp(TimeUnit::Nanosecond, None) => {
            let (values, nulls) = collect_i64_values::<i64, _, _>(array, selection, value_at, Ok)?;
            Ok(Some(Arc::new(TimestampNanosecondArray::new(
                ScalarBuffer::from(values),
                nulls,
            )) as ArrayRef))
        }
        DataType::Decimal128(precision, scale) if array.logical == CoveLogicalType::Decimal64 => {
            let (values, nulls) =
                collect_i64_values::<i128, _, _>(array, selection, value_at, |value| {
                    Ok(i128::from(value))
                })?;
            let array = Decimal128Array::new(ScalarBuffer::from(values), nulls)
                .with_precision_and_scale(*precision, *scale)
                .map_err(|err| CoveError::BadSection(format!("Arrow Decimal128: {err}")))?;
            Ok(Some(Arc::new(array) as ArrayRef))
        }
        _ => Ok(None),
    }
}

fn i64_values_primitive_array<T, F, C>(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    value_at: F,
    cast: C,
) -> Result<arrow_array::PrimitiveArray<T>, CoveError>
where
    T: arrow_array::types::ArrowPrimitiveType,
    T::Native: Default,
    F: Fn(usize) -> Result<i64, CoveError>,
    C: Fn(i64) -> Result<T::Native, CoveError>,
{
    let (values, nulls) = collect_i64_values::<T::Native, F, C>(array, selection, value_at, cast)?;
    Ok(arrow_array::PrimitiveArray::<T>::new(
        ScalarBuffer::from(values),
        nulls,
    ))
}

fn collect_i64_values<T, F, C>(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    value_at: F,
    cast: C,
) -> Result<(Vec<T>, Option<NullBuffer>), CoveError>
where
    T: ArrowNativeType + Default,
    F: Fn(usize) -> Result<i64, CoveError>,
    C: Fn(i64) -> Result<T, CoveError>,
{
    let has_nulls = array_has_nulls(array)?;
    let selected_len = selection.selected_len(array.row_count)?;
    let mut values = Vec::with_capacity(selected_len);
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(selected_len))
        .transpose()?;
    selection.for_each_row(array.row_count, |row| {
        let is_null = has_nulls && array.is_null(row as u64)?;
        if let Some(builder) = &mut validity_builder {
            builder.append(!is_null);
        }
        if is_null {
            values.push(T::default());
        } else {
            values.push(cast(value_at(row)?)?);
        }
        Ok(())
    })?;
    Ok((
        values,
        validity_builder.and_then(ArrowValidityBuilder::finish),
    ))
}

fn collect_i64_bool_values<F>(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    value_at: F,
) -> Result<(Vec<u8>, usize, Option<NullBuffer>), CoveError>
where
    F: Fn(usize) -> Result<i64, CoveError>,
{
    let has_nulls = array_has_nulls(array)?;
    let selected_len = selection.selected_len(array.row_count)?;
    let mut values = vec![0u8; bitpacked_len(selected_len)?];
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(selected_len))
        .transpose()?;
    let mut out_row = 0usize;
    selection.for_each_row(array.row_count, |row| {
        let is_null = has_nulls && array.is_null(row as u64)?;
        if let Some(builder) = &mut validity_builder {
            builder.append(!is_null);
        }
        if !is_null {
            match value_at(row)? {
                0 => {}
                1 => set_packed_bit(&mut values, out_row),
                _ => return Err(CoveError::PageCorrupt),
            }
        }
        out_row += 1;
        Ok(())
    })?;
    Ok((
        values,
        selected_len,
        validity_builder.and_then(ArrowValidityBuilder::finish),
    ))
}

fn try_direct_local_codebook_array(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    data_type: &DataType,
) -> Result<Option<ArrayRef>, CoveError> {
    let payload = LocalCodebookPayload::parse(array.data)?;
    match (&payload.values, data_type) {
        (LocalCodebookValues::FileCode(_), _) | (LocalCodebookValues::NumCode(_), _) => {
            let values = payload.decode_num_codes().or_else(|_| {
                payload
                    .decode_file_codes()
                    .map(|codes| codes.into_iter().map(u64::from).collect::<Vec<_>>())
            })?;
            if values.len()
                != usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?
            {
                return Err(CoveError::PageCorrupt);
            }
            direct_u64_slice_to_arrow(array, &values, selection, data_type)
        }
        (LocalCodebookValues::Boolean(_), DataType::Boolean) => {
            let values = payload.decode_booleans()?;
            if values.len()
                != usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?
            {
                return Err(CoveError::PageCorrupt);
            }
            let (packed, selected_len, nulls) =
                collect_bool_slice_values(array, &values, selection)?;
            let packed = BooleanBuffer::new(Buffer::from_vec(packed), 0, selected_len);
            Ok(Some(Arc::new(BooleanArray::new(packed, nulls)) as ArrayRef))
        }
        (LocalCodebookValues::VarBytes(_), DataType::Utf8 | DataType::Binary) => {
            let values = payload.decode_var_bytes()?;
            if values.len()
                != usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?
            {
                return Err(CoveError::PageCorrupt);
            }
            direct_bytes_vec_to_arrow(array, &values, selection, data_type)
        }
        _ => Ok(None),
    }
}

fn direct_u64_slice_to_arrow(
    array: &EncodedArray<'_>,
    values: &[u64],
    selection: ArrowRowSelection<'_>,
    data_type: &DataType,
) -> Result<Option<ArrayRef>, CoveError> {
    if values.len() != usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)? {
        return Err(CoveError::PageCorrupt);
    }
    match data_type {
        DataType::UInt8 => Ok(Some(
            Arc::new(u64_values_primitive_array::<UInt8Type, _, _>(
                array,
                selection,
                |row| values.get(row).copied().ok_or(CoveError::PageCorrupt),
                |value| u8::try_from(value).map_err(|_| CoveError::PageCorrupt),
            )?) as ArrayRef,
        )),
        DataType::UInt16 => Ok(Some(
            Arc::new(u64_values_primitive_array::<UInt16Type, _, _>(
                array,
                selection,
                |row| values.get(row).copied().ok_or(CoveError::PageCorrupt),
                |value| u16::try_from(value).map_err(|_| CoveError::PageCorrupt),
            )?) as ArrayRef,
        )),
        DataType::UInt32 => Ok(Some(
            Arc::new(u64_values_primitive_array::<UInt32Type, _, _>(
                array,
                selection,
                |row| values.get(row).copied().ok_or(CoveError::PageCorrupt),
                |value| u32::try_from(value).map_err(|_| CoveError::PageCorrupt),
            )?) as ArrayRef,
        )),
        DataType::UInt64 => Ok(Some(
            Arc::new(u64_values_primitive_array::<UInt64Type, _, _>(
                array,
                selection,
                |row| values.get(row).copied().ok_or(CoveError::PageCorrupt),
                Ok,
            )?) as ArrayRef,
        )),
        _ => direct_i64_values_to_arrow(array, selection, data_type, |row| {
            let value = values.get(row).copied().ok_or(CoveError::PageCorrupt)?;
            i64::try_from(value).map_err(|_| CoveError::PageCorrupt)
        }),
    }
}

fn u64_values_primitive_array<T, F, C>(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
    value_at: F,
    cast: C,
) -> Result<arrow_array::PrimitiveArray<T>, CoveError>
where
    T: arrow_array::types::ArrowPrimitiveType,
    T::Native: Default,
    F: Fn(usize) -> Result<u64, CoveError>,
    C: Fn(u64) -> Result<T::Native, CoveError>,
{
    let has_nulls = array_has_nulls(array)?;
    let selected_len = selection.selected_len(array.row_count)?;
    let mut values = Vec::with_capacity(selected_len);
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(selected_len))
        .transpose()?;
    selection.for_each_row(array.row_count, |row| {
        let is_null = has_nulls && array.is_null(row as u64)?;
        if let Some(builder) = &mut validity_builder {
            builder.append(!is_null);
        }
        if is_null {
            values.push(T::Native::default());
        } else {
            values.push(cast(value_at(row)?)?);
        }
        Ok(())
    })?;
    Ok(arrow_array::PrimitiveArray::<T>::new(
        ScalarBuffer::from(values),
        validity_builder.and_then(ArrowValidityBuilder::finish),
    ))
}

fn collect_bool_slice_values(
    array: &EncodedArray<'_>,
    values: &[bool],
    selection: ArrowRowSelection<'_>,
) -> Result<(Vec<u8>, usize, Option<NullBuffer>), CoveError> {
    let has_nulls = array_has_nulls(array)?;
    let selected_len = selection.selected_len(array.row_count)?;
    let mut packed = vec![0u8; bitpacked_len(selected_len)?];
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(selected_len))
        .transpose()?;
    let mut out_row = 0usize;
    selection.for_each_row(array.row_count, |row| {
        let is_null = has_nulls && array.is_null(row as u64)?;
        if let Some(builder) = &mut validity_builder {
            builder.append(!is_null);
        }
        if !is_null && *values.get(row).ok_or(CoveError::PageCorrupt)? {
            set_packed_bit(&mut packed, out_row);
        }
        out_row += 1;
        Ok(())
    })?;
    Ok((
        packed,
        selected_len,
        validity_builder.and_then(ArrowValidityBuilder::finish),
    ))
}

fn direct_bytes_vec_to_arrow(
    array: &EncodedArray<'_>,
    values: &[Vec<u8>],
    selection: ArrowRowSelection<'_>,
    data_type: &DataType,
) -> Result<Option<ArrayRef>, CoveError> {
    let has_nulls = array_has_nulls(array)?;
    match data_type {
        DataType::Utf8 => {
            let mut builder = StringBuilder::new();
            selection.for_each_row(array.row_count, |row| {
                let is_null = has_nulls && array.is_null(row as u64)?;
                if is_null {
                    builder.append_null();
                    return Ok(());
                }
                let bytes = values.get(row).ok_or(CoveError::PageCorrupt)?;
                let text = std::str::from_utf8(bytes)
                    .map_err(|err| CoveError::BadSection(format!("Arrow Utf8 export: {err}")))?;
                builder.append_value(text);
                Ok(())
            })?;
            Ok(Some(Arc::new(builder.finish()) as ArrayRef))
        }
        DataType::Binary => {
            let mut builder = BinaryBuilder::new();
            selection.for_each_row(array.row_count, |row| {
                let is_null = has_nulls && array.is_null(row as u64)?;
                if is_null {
                    builder.append_null();
                } else {
                    builder.append_value(values.get(row).ok_or(CoveError::PageCorrupt)?);
                }
                Ok(())
            })?;
            Ok(Some(Arc::new(builder.finish()) as ArrayRef))
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

fn array_has_nulls(array: &EncodedArray<'_>) -> Result<bool, CoveError> {
    Ok(match array.validity {
        Some(validity) => validity.null_count()? > 0,
        None => false,
    })
}

#[inline]
fn read_numcode_u64(data: &[u8], row: usize) -> u64 {
    let offset = row * 8;
    // INVARIANT: callers validate `data` as an 8-byte fixed-width prefix for
    // the full row count and validate every selected row before reading.
    // SAFETY: `offset..offset + 8` is therefore in-bounds; unaligned loads are
    // explicitly allowed by `read_unaligned`.
    unsafe { u64::from_le(ptr::read_unaligned(data.as_ptr().add(offset) as *const u64)) }
}

fn retained_numcode_u64_values(
    array: &EncodedArray<'_>,
    data_owner: Option<&ArrowBufferOwner>,
) -> Result<Option<ScalarBuffer<u64>>, CoveError> {
    let Some(owner) = data_owner else {
        return Ok(None);
    };
    if !cfg!(target_endian = "little") || array_has_nulls(array)? {
        return Ok(None);
    }
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let data = fixed_width_payload_prefix(array.data, row_count, 8)?;
    retained_numcode_scalar_buffer::<u64>(data, row_count, owner)
}

fn retained_numcode_i64_values(
    array: &EncodedArray<'_>,
    data_owner: Option<&ArrowBufferOwner>,
) -> Result<Option<ScalarBuffer<i64>>, CoveError> {
    let Some(owner) = data_owner else {
        return Ok(None);
    };
    if !cfg!(target_endian = "little") || array_has_nulls(array)? {
        return Ok(None);
    }
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let data = fixed_width_payload_prefix(array.data, row_count, 8)?;
    for row in 0..row_count {
        checked_numcode_i64(read_numcode_u64(data, row))?;
    }
    retained_numcode_scalar_buffer::<i64>(data, row_count, owner)
}

fn retained_numcode_scalar_buffer<T: ArrowNativeType>(
    data: &[u8],
    row_count: usize,
    owner: &ArrowBufferOwner,
) -> Result<Option<ScalarBuffer<T>>, CoveError> {
    let Some(byte_len) = row_count.checked_mul(std::mem::size_of::<T>()) else {
        return Err(CoveError::ArithOverflow);
    };
    if byte_len == 0 {
        return Ok(Some(ScalarBuffer::from(Vec::<T>::new())));
    }
    if data.len() < byte_len {
        return Err(CoveError::OffsetRange);
    }
    let align = std::mem::align_of::<T>();
    if (data.as_ptr() as usize) % align != 0 {
        return Ok(None);
    }
    let Some(ptr) = NonNull::new(data.as_ptr() as *mut u8) else {
        return Err(CoveError::BufferTooShort);
    };
    // INVARIANT: the returned Arrow buffer points into immutable retained COVE
    // page data. The `owner` is cloned into Arrow's custom allocation so the
    // backing bytes outlive every array using this buffer.
    // SAFETY: `data` was proven valid for `byte_len` bytes, the pointer is
    // non-null and aligned for `T`, and only little-endian no-null NumCode
    // payloads reach this helper.
    let buffer = unsafe { Buffer::from_custom_allocation(ptr, byte_len, Arc::clone(owner)) };
    Ok(Some(ScalarBuffer::new(buffer, 0, row_count)))
}

fn numcode_u64_array(array: &EncodedArray<'_>) -> Result<UInt64Array, CoveError> {
    let (values, nulls) = collect_numcode_u64_buffers(array, ArrowRowSelection::All)?;
    Ok(UInt64Array::new(ScalarBuffer::from(values), nulls))
}

fn numcode_u64_array_for_selection(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<UInt64Array, CoveError> {
    let (values, nulls) = collect_numcode_u64_buffers(array, selection)?;
    Ok(UInt64Array::new(ScalarBuffer::from(values), nulls))
}

fn numcode_i64_array(array: &EncodedArray<'_>) -> Result<Int64Array, CoveError> {
    let (values, nulls) = collect_numcode_i64_buffers(array, ArrowRowSelection::All)?;
    Ok(Int64Array::new(ScalarBuffer::from(values), nulls))
}

fn numcode_i64_array_for_selection(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<Int64Array, CoveError> {
    let (values, nulls) = collect_numcode_i64_buffers(array, selection)?;
    Ok(Int64Array::new(ScalarBuffer::from(values), nulls))
}

fn copy_numcode_bytes_to_vec<T: ArrowNativeType>(
    data: &[u8],
    row_count: usize,
    out: &mut Vec<T>,
) -> Result<(), CoveError> {
    let byte_len = row_count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or(CoveError::ArithOverflow)?;
    if data.len() < byte_len {
        return Err(CoveError::OffsetRange);
    }
    // INVARIANT: NumCode uses little-endian fixed-width 8-byte payloads. This
    // helper is only called for no-null native Arrow buffers whose bytes are
    // identical to the checked COVE payload representation.
    // SAFETY: `out` has capacity for `row_count` native values. Copying through
    // `u8` pointers avoids source alignment requirements, and every destination
    // byte for the final vector length is initialized before `set_len`.
    unsafe {
        ptr::copy_nonoverlapping(data.as_ptr(), out.as_mut_ptr().cast::<u8>(), byte_len);
        out.set_len(row_count);
    }
    Ok(())
}

fn timestamp_micros_array(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<TimestampMicrosecondArray, CoveError> {
    let (values, nulls) = collect_numcode_i64_buffers(array, selection)?;
    Ok(TimestampMicrosecondArray::new(
        ScalarBuffer::from(values),
        nulls,
    ))
}

fn timestamp_nanos_array(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<TimestampNanosecondArray, CoveError> {
    let (values, nulls) = collect_numcode_i64_buffers(array, selection)?;
    Ok(TimestampNanosecondArray::new(
        ScalarBuffer::from(values),
        nulls,
    ))
}

fn collect_numcode_u64_buffers(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<(Vec<u64>, Option<NullBuffer>), CoveError> {
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let data = fixed_width_payload_prefix(array.data, row_count, 8)?;
    let has_nulls = array_has_nulls(array)?;
    match selection {
        ArrowRowSelection::All => collect_numcode_u64_all(array, data, row_count, has_nulls),
        ArrowRowSelection::Rows(rows) => {
            selection.validate_for_row_count(array.row_count)?;
            collect_numcode_u64_rows(array, data, rows, has_nulls)
        }
        ArrowRowSelection::Bitset { words, len } => {
            selection.validate_for_row_count(array.row_count)?;
            let selected_len = count_bitset_rows(words, len)?;
            collect_numcode_u64_bitset(array, data, words, len, selected_len, has_nulls)
        }
    }
}

fn collect_numcode_i64_buffers(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<(Vec<i64>, Option<NullBuffer>), CoveError> {
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let data = fixed_width_payload_prefix(array.data, row_count, 8)?;
    let has_nulls = array_has_nulls(array)?;
    match selection {
        ArrowRowSelection::All => collect_numcode_i64_all(array, data, row_count, has_nulls),
        ArrowRowSelection::Rows(rows) => {
            selection.validate_for_row_count(array.row_count)?;
            collect_numcode_i64_rows(array, data, rows, has_nulls)
        }
        ArrowRowSelection::Bitset { words, len } => {
            selection.validate_for_row_count(array.row_count)?;
            let selected_len = count_bitset_rows(words, len)?;
            collect_numcode_i64_bitset(array, data, words, len, selected_len, has_nulls)
        }
    }
}

fn collect_numcode_u64_all(
    array: &EncodedArray<'_>,
    data: &[u8],
    row_count: usize,
    has_nulls: bool,
) -> Result<(Vec<u64>, Option<NullBuffer>), CoveError> {
    let mut out = Vec::<u64>::with_capacity(row_count);
    if !has_nulls {
        copy_numcode_bytes_to_vec(data, row_count, &mut out)?;
        return Ok((out, None));
    }

    let mut validity_builder = ArrowValidityBuilder::new(row_count)?;
    for row in 0..row_count {
        let is_null = array.is_null(row as u64)?;
        validity_builder.append(!is_null);
        out.push(if is_null {
            0
        } else {
            read_numcode_u64(data, row)
        });
    }
    Ok((out, validity_builder.finish()))
}

fn collect_numcode_u64_rows(
    array: &EncodedArray<'_>,
    data: &[u8],
    rows: &[u32],
    has_nulls: bool,
) -> Result<(Vec<u64>, Option<NullBuffer>), CoveError> {
    let mut out = Vec::with_capacity(rows.len());
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(rows.len()))
        .transpose()?;
    for row in rows {
        let row = *row as usize;
        let is_null = has_nulls && array.is_null(row as u64)?;
        if let Some(builder) = &mut validity_builder {
            builder.append(!is_null);
        }
        out.push(if is_null {
            0
        } else {
            read_numcode_u64(data, row)
        });
    }
    Ok((out, validity_builder.and_then(ArrowValidityBuilder::finish)))
}

fn collect_numcode_u64_bitset(
    array: &EncodedArray<'_>,
    data: &[u8],
    words: &[u64],
    len: usize,
    selected_len: usize,
    has_nulls: bool,
) -> Result<(Vec<u64>, Option<NullBuffer>), CoveError> {
    let mut out = Vec::with_capacity(selected_len);
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(selected_len))
        .transpose()?;
    let word_len = len.div_ceil(64);
    for (word_index, raw_word) in words.iter().take(word_len).copied().enumerate() {
        let mut word = if word_index + 1 == word_len {
            mask_selection_tail(raw_word, len)
        } else {
            raw_word
        };
        while word != 0 {
            let row = word_index * 64 + word.trailing_zeros() as usize;
            let is_null = has_nulls && array.is_null(row as u64)?;
            if let Some(builder) = &mut validity_builder {
                builder.append(!is_null);
            }
            out.push(if is_null {
                0
            } else {
                read_numcode_u64(data, row)
            });
            word &= word - 1;
        }
    }
    Ok((out, validity_builder.and_then(ArrowValidityBuilder::finish)))
}

#[inline]
fn checked_numcode_i64(value: u64) -> Result<i64, CoveError> {
    if value > i64::MAX as u64 {
        return Err(CoveError::PageCorrupt);
    }
    Ok(value as i64)
}

fn collect_numcode_i64_all(
    array: &EncodedArray<'_>,
    data: &[u8],
    row_count: usize,
    has_nulls: bool,
) -> Result<(Vec<i64>, Option<NullBuffer>), CoveError> {
    let mut out = Vec::<i64>::with_capacity(row_count);
    if !has_nulls {
        for row in 0..row_count {
            checked_numcode_i64(read_numcode_u64(data, row))?;
        }
        copy_numcode_bytes_to_vec(data, row_count, &mut out)?;
        return Ok((out, None));
    }

    let mut validity_builder = ArrowValidityBuilder::new(row_count)?;
    for row in 0..row_count {
        let is_null = array.is_null(row as u64)?;
        validity_builder.append(!is_null);
        out.push(if is_null {
            0
        } else {
            checked_numcode_i64(read_numcode_u64(data, row))?
        });
    }
    Ok((out, validity_builder.finish()))
}

fn collect_numcode_i64_rows(
    array: &EncodedArray<'_>,
    data: &[u8],
    rows: &[u32],
    has_nulls: bool,
) -> Result<(Vec<i64>, Option<NullBuffer>), CoveError> {
    let mut out = Vec::with_capacity(rows.len());
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(rows.len()))
        .transpose()?;
    for row in rows {
        let row = *row as usize;
        let is_null = has_nulls && array.is_null(row as u64)?;
        if let Some(builder) = &mut validity_builder {
            builder.append(!is_null);
        }
        out.push(if is_null {
            0
        } else {
            checked_numcode_i64(read_numcode_u64(data, row))?
        });
    }
    Ok((out, validity_builder.and_then(ArrowValidityBuilder::finish)))
}

fn collect_numcode_i64_bitset(
    array: &EncodedArray<'_>,
    data: &[u8],
    words: &[u64],
    len: usize,
    selected_len: usize,
    has_nulls: bool,
) -> Result<(Vec<i64>, Option<NullBuffer>), CoveError> {
    let mut out = Vec::with_capacity(selected_len);
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(selected_len))
        .transpose()?;
    let word_len = len.div_ceil(64);
    for (word_index, raw_word) in words.iter().take(word_len).copied().enumerate() {
        let mut word = if word_index + 1 == word_len {
            mask_selection_tail(raw_word, len)
        } else {
            raw_word
        };
        while word != 0 {
            let row = word_index * 64 + word.trailing_zeros() as usize;
            let is_null = has_nulls && array.is_null(row as u64)?;
            if let Some(builder) = &mut validity_builder {
                builder.append(!is_null);
            }
            out.push(if is_null {
                0
            } else {
                checked_numcode_i64(read_numcode_u64(data, row))?
            });
            word &= word - 1;
        }
    }
    Ok((out, validity_builder.and_then(ArrowValidityBuilder::finish)))
}

fn plain_bool_array(array: &EncodedArray<'_>) -> Result<BooleanArray, CoveError> {
    plain_bool_array_for_selection(array, ArrowRowSelection::All)
}

fn plain_bool_array_for_selection(
    array: &EncodedArray<'_>,
    selection: ArrowRowSelection<'_>,
) -> Result<BooleanArray, CoveError> {
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    let data = fixed_width_payload_prefix(array.data, row_count, 1)?;
    let has_nulls = array_has_nulls(array)?;
    let (values, selected_len, nulls) = match selection {
        ArrowRowSelection::All => collect_bool_all(array, data, row_count, has_nulls)?,
        ArrowRowSelection::Rows(rows) => {
            selection.validate_for_row_count(array.row_count)?;
            collect_bool_rows(array, data, rows, has_nulls)?
        }
        ArrowRowSelection::Bitset { words, len } => {
            selection.validate_for_row_count(array.row_count)?;
            let selected_len = count_bitset_rows(words, len)?;
            collect_bool_bitset(array, data, words, len, selected_len, has_nulls)?
        }
    };
    let values = BooleanBuffer::new(Buffer::from_vec(values), 0, selected_len);
    Ok(BooleanArray::new(values, nulls))
}

#[inline(always)]
fn checked_bool_byte(byte: u8) -> Result<bool, CoveError> {
    match byte {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(CoveError::PageCorrupt),
    }
}

#[inline(always)]
fn pack_bool_chunk_8(chunk: &[u8]) -> Result<u8, CoveError> {
    debug_assert_eq!(chunk.len(), 8);
    let b0 = chunk[0];
    let b1 = chunk[1];
    let b2 = chunk[2];
    let b3 = chunk[3];
    let b4 = chunk[4];
    let b5 = chunk[5];
    let b6 = chunk[6];
    let b7 = chunk[7];
    if (b0 | b1 | b2 | b3 | b4 | b5 | b6 | b7) > 1 {
        return Err(CoveError::PageCorrupt);
    }
    Ok(b0 | (b1 << 1) | (b2 << 2) | (b3 << 3) | (b4 << 4) | (b5 << 5) | (b6 << 6) | (b7 << 7))
}

#[inline(always)]
fn pack_bool_chunk_16(chunk: &[u8]) -> Result<u16, CoveError> {
    debug_assert_eq!(chunk.len(), 16);
    let mut low = [0u8; 8];
    let mut high = [0u8; 8];
    low.copy_from_slice(&chunk[..8]);
    high.copy_from_slice(&chunk[8..16]);
    let packed_low = pack_bool_chunk_8(&low)?;
    let packed_high = pack_bool_chunk_8(&high)?;
    Ok(u16::from(packed_low) | (u16::from(packed_high) << 8))
}

fn collect_bool_all(
    array: &EncodedArray<'_>,
    data: &[u8],
    row_count: usize,
    has_nulls: bool,
) -> Result<(Vec<u8>, usize, Option<NullBuffer>), CoveError> {
    let mut values = vec![0u8; bitpacked_len(row_count)?];
    if !has_nulls {
        let mut chunks = data.chunks_exact(16);
        for (word_index, chunk) in chunks.by_ref().enumerate() {
            let packed = pack_bool_chunk_16(chunk)?.to_le_bytes();
            let offset = word_index * 2;
            values[offset] = packed[0];
            if offset + 1 < values.len() {
                values[offset + 1] = packed[1];
            }
        }
        let tail_start = row_count - chunks.remainder().len();
        for (bit, byte) in chunks.remainder().iter().copied().enumerate() {
            if checked_bool_byte(byte)? {
                set_packed_bit(&mut values, tail_start + bit);
            }
        }
        return Ok((values, row_count, None));
    }

    let mut validity_builder = ArrowValidityBuilder::new(row_count)?;
    for (row, byte) in data.iter().copied().enumerate() {
        let bit = checked_bool_byte(byte)?;
        let is_null = array.is_null(row as u64)?;
        validity_builder.append(!is_null);
        if !is_null && bit {
            set_packed_bit(&mut values, row);
        }
    }
    Ok((values, row_count, validity_builder.finish()))
}

fn collect_bool_rows(
    array: &EncodedArray<'_>,
    data: &[u8],
    rows: &[u32],
    has_nulls: bool,
) -> Result<(Vec<u8>, usize, Option<NullBuffer>), CoveError> {
    let mut values = vec![0u8; bitpacked_len(rows.len())?];
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(rows.len()))
        .transpose()?;
    for (out_row, row) in rows.iter().copied().enumerate() {
        let row = row as usize;
        let bit = checked_bool_byte(data[row])?;
        let is_null = has_nulls && array.is_null(row as u64)?;
        if let Some(builder) = &mut validity_builder {
            builder.append(!is_null);
        }
        if !is_null && bit {
            set_packed_bit(&mut values, out_row);
        }
    }
    Ok((
        values,
        rows.len(),
        validity_builder.and_then(ArrowValidityBuilder::finish),
    ))
}

fn collect_bool_bitset(
    array: &EncodedArray<'_>,
    data: &[u8],
    words: &[u64],
    len: usize,
    selected_len: usize,
    has_nulls: bool,
) -> Result<(Vec<u8>, usize, Option<NullBuffer>), CoveError> {
    let mut values = vec![0u8; bitpacked_len(selected_len)?];
    let mut validity_builder = has_nulls
        .then(|| ArrowValidityBuilder::new(selected_len))
        .transpose()?;
    let mut out_row = 0usize;
    let word_len = len.div_ceil(64);
    for (word_index, raw_word) in words.iter().take(word_len).copied().enumerate() {
        let mut word = if word_index + 1 == word_len {
            mask_selection_tail(raw_word, len)
        } else {
            raw_word
        };
        while word != 0 {
            let row = word_index * 64 + word.trailing_zeros() as usize;
            let bit = checked_bool_byte(data[row])?;
            let is_null = has_nulls && array.is_null(row as u64)?;
            if let Some(builder) = &mut validity_builder {
                builder.append(!is_null);
            }
            if !is_null && bit {
                set_packed_bit(&mut values, out_row);
            }
            out_row += 1;
            word &= word - 1;
        }
    }
    Ok((
        values,
        selected_len,
        validity_builder.and_then(ArrowValidityBuilder::finish),
    ))
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
        DataType::Utf8View => Arc::new(collect_utf8_view(logical, values)?) as ArrayRef,
        DataType::Binary => Arc::new(collect_binary(logical, values)?) as ArrayRef,
        DataType::BinaryView => Arc::new(collect_binary_view(logical, values)?) as ArrayRef,
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
                let storage = if options.varbytes_policy == ArrowVarBytesExportPolicy::View {
                    "Utf8View"
                } else {
                    "Utf8"
                };
                report.push(
                    None,
                    logical,
                    ArrowFidelitySeverity::Lossy,
                    format!("Json exported as {storage} without Arrow extension metadata"),
                );
            }
            if options.varbytes_policy == ArrowVarBytesExportPolicy::View {
                Ok(DataType::Utf8View)
            } else {
                Ok(DataType::Utf8)
            }
        }
        CoveLogicalType::Utf8 if options.varbytes_policy == ArrowVarBytesExportPolicy::View => {
            Ok(DataType::Utf8View)
        }
        CoveLogicalType::Binary if options.varbytes_policy == ArrowVarBytesExportPolicy::View => {
            Ok(DataType::BinaryView)
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

fn collect_utf8_view(
    logical: CoveLogicalType,
    values: &[CoveArrayValue<'_>],
) -> Result<StringViewArray, CoveError> {
    let mut builder = StringViewBuilder::new();
    for value in values {
        match value {
            CoveArrayValue::Null => builder.append_null(),
            _ => {
                let bytes = value_to_bytes(logical, value)?;
                let text = std::str::from_utf8(bytes.as_ref()).map_err(|_| {
                    CoveError::BadSection("Arrow Utf8View export requires valid UTF-8".into())
                })?;
                builder.append_value(text);
            }
        }
    }
    Ok(builder.finish())
}

fn collect_binary_view(
    logical: CoveLogicalType,
    values: &[CoveArrayValue<'_>],
) -> Result<BinaryViewArray, CoveError> {
    let mut builder = BinaryViewBuilder::new();
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

    fn view_options() -> ArrowExportOptions {
        ArrowExportOptions {
            varbytes_policy: ArrowVarBytesExportPolicy::View,
            ..ArrowExportOptions::default()
        }
    }

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
    fn exports_plain_float64_array_directly() {
        let mut values = Vec::new();
        values.extend_from_slice(&1.5f64.to_bits().to_le_bytes());
        values.extend_from_slice(&(-2.25f64).to_bits().to_le_bytes());
        let cove = EncodedArray::new(
            CoveLogicalType::Float64,
            CovePhysicalKind::FixedBytes,
            2,
            CoveEncodingKind::PlainFixed,
            None,
            &values,
            None,
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let floats = arrow.as_any().downcast_ref::<Float64Array>().unwrap();
        assert_eq!(floats.value(0), 1.5);
        assert_eq!(floats.value(1), -2.25);
    }

    #[test]
    fn exports_constant_int64_without_row_value_fallback() {
        let payload = ConstantPayload {
            value: 42,
            row_count: 3,
        }
        .encode();
        let cove = EncodedArray::new(
            CoveLogicalType::Int64,
            CovePhysicalKind::FixedBytes,
            3,
            CoveEncodingKind::Constant,
            None,
            &payload,
            None,
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let ints = arrow.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(ints.values(), &[42, 42, 42]);
    }

    #[test]
    fn exports_rle_int64_without_row_value_fallback() {
        let payload = RlePayload {
            runs: vec![(7, 2), (9, 1)],
        }
        .encode();
        let cove = EncodedArray::new(
            CoveLogicalType::Int64,
            CovePhysicalKind::FixedBytes,
            3,
            CoveEncodingKind::Rle,
            None,
            &payload,
            None,
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let ints = arrow.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(ints.values(), &[7, 7, 9]);
    }

    #[test]
    fn exports_fixed_length_varbytes_fast_path_values() {
        let mut values = Vec::new();
        for value in [b"aa".as_slice(), b"bb".as_slice(), b"cc".as_slice()] {
            values.extend_from_slice(&(value.len() as u32).to_le_bytes());
            values.extend_from_slice(value);
        }
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            3,
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
        assert_eq!(strings.value(0), "aa");
        assert_eq!(strings.value(1), "bb");
        assert_eq!(strings.value(2), "cc");
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
    fn retained_numcode_uint64_array_can_use_backing_owner() {
        let mut values = Vec::new();
        values.extend_from_slice(&10u64.to_le_bytes());
        values.extend_from_slice(&20u64.to_le_bytes());
        let owner = Arc::new(values);
        let cove = EncodedArray::new(
            CoveLogicalType::UInt64,
            CovePhysicalKind::NumCode,
            2,
            CoveEncodingKind::NumCode,
            None,
            owner.as_slice(),
            None,
        );
        let buffer_owner = arrow_buffer_owner(Arc::clone(&owner));

        let result = encoded_array_to_arrow_with_options_and_owner(
            &cove,
            ArrowExportOptions::default(),
            Some(&buffer_owner),
        )
        .unwrap();
        let uints = result.value.as_any().downcast_ref::<UInt64Array>().unwrap();
        assert_eq!(uints.values(), &[10, 20]);
        if (owner.as_ptr() as usize) % std::mem::align_of::<u64>() == 0 {
            assert_eq!(uints.to_data().buffers()[0].as_ptr(), owner.as_ptr());
        }
    }

    #[test]
    fn exports_numcode_int64_array_with_nulls() {
        let mut values = Vec::new();
        values.extend_from_slice(&10u64.to_le_bytes());
        values.extend_from_slice(&99u64.to_le_bytes());
        values.extend_from_slice(&30u64.to_le_bytes());
        let mut validity = ValidityBitmapBuilder::new(3).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 3);
        let cove = EncodedArray::new(
            CoveLogicalType::Int64,
            CovePhysicalKind::NumCode,
            3,
            CoveEncodingKind::NumCode,
            Some(bitmap),
            &values,
            None,
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let ints = arrow.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(ints.value(0), 10);
        assert!(ints.is_null(1));
        assert_eq!(ints.value(2), 30);
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
    fn exports_plain_bool_array_with_nulls() {
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

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let bools = arrow.as_any().downcast_ref::<BooleanArray>().unwrap();
        assert!(bools.value(0));
        assert!(bools.is_null(1));
        assert!(bools.value(2));
    }

    #[test]
    fn rejects_invalid_plain_bool_array_even_for_null_row() {
        let values = [1u8, 2u8];
        let mut validity = ValidityBitmapBuilder::new(2).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 2);
        let cove = EncodedArray::new(
            CoveLogicalType::Bool,
            CovePhysicalKind::Boolean,
            2,
            CoveEncodingKind::PlainFixed,
            Some(bitmap),
            &values,
            None,
        );

        assert!(matches!(
            encoded_array_to_arrow(&cove),
            Err(CoveError::PageCorrupt)
        ));
    }

    #[test]
    fn bool_chunk_packer_matches_scalar_bits_and_rejects_invalid_bytes() {
        for pattern in 0u16..=255 {
            let mut chunk = [0u8; 8];
            let mut expected = 0u8;
            for bit in 0..8 {
                let value = ((pattern >> bit) & 1) as u8;
                chunk[bit] = value;
                expected |= value << bit;
            }
            assert_eq!(pack_bool_chunk_8(&chunk).unwrap(), expected);
        }

        for pos in 0..8 {
            let mut chunk = [0u8; 8];
            chunk[pos] = 2;
            assert!(matches!(
                pack_bool_chunk_8(&chunk),
                Err(CoveError::PageCorrupt)
            ));
        }
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
    fn trusted_utf8_validation_policy_preserves_valid_standard_output() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        values.extend_from_slice(&5u32.to_le_bytes());
        values.extend_from_slice(b"there");
        let cove = EncodedArray::new(
            CoveLogicalType::Json,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        let strict = encoded_array_to_arrow_with_options(&cove, ArrowExportOptions::default())
            .unwrap()
            .value;
        let trusted = encoded_array_to_arrow_with_options(
            &cove,
            ArrowExportOptions {
                string_validation_policy: ArrowStringValidationPolicy::TrustedPageProof,
                ..ArrowExportOptions::default()
            },
        )
        .unwrap()
        .value;
        assert_eq!(trusted.data_type(), &DataType::Utf8);
        let strict = strict
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        let trusted = trusted
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(trusted.value(0), strict.value(0));
        assert_eq!(trusted.value(1), strict.value(1));
    }

    #[test]
    fn exports_binary_varbytes_array_with_nulls() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(&[0, 1]);
        values.extend_from_slice(&3u32.to_le_bytes());
        values.extend_from_slice(&[2, 3, 4]);
        values.extend_from_slice(&1u32.to_le_bytes());
        values.extend_from_slice(&[5]);
        let mut validity = ValidityBitmapBuilder::new(3).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 3);
        let cove = EncodedArray::new(
            CoveLogicalType::Binary,
            CovePhysicalKind::VarBytes,
            3,
            CoveEncodingKind::VarBytes,
            Some(bitmap),
            &values,
            None,
        );

        let arrow = encoded_array_to_arrow(&cove).unwrap();
        let binary = arrow.as_any().downcast_ref::<BinaryArray>().unwrap();
        assert_eq!(binary.value(0), &[0, 1]);
        assert!(binary.is_null(1));
        assert_eq!(binary.value(2), &[5]);
    }

    #[test]
    fn view_export_maps_varbytes_to_arrow_view_arrays() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(b"hi");
        values.extend_from_slice(&19u32.to_le_bytes());
        values.extend_from_slice(b"long-string-payload");
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        let standard = encoded_array_to_arrow_with_options(&cove, ArrowExportOptions::default())
            .unwrap()
            .value;
        let view = encoded_array_to_arrow_with_options(&cove, view_options())
            .unwrap()
            .value;
        assert_eq!(view.data_type(), &DataType::Utf8View);
        let standard = standard
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        let view = view.as_any().downcast_ref::<StringViewArray>().unwrap();
        assert_eq!(view.value(0), standard.value(0));
        assert_eq!(view.value(1), standard.value(1));
    }

    #[test]
    fn view_export_handles_binary_and_selected_rows() {
        let mut values = Vec::new();
        values.extend_from_slice(&1u32.to_le_bytes());
        values.extend_from_slice(&[0xaa]);
        values.extend_from_slice(&14u32.to_le_bytes());
        values.extend_from_slice(b"binary-payload");
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(&[0xbb, 0xcc]);
        let cove = EncodedArray::new(
            CoveLogicalType::Binary,
            CovePhysicalKind::VarBytes,
            3,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        let result = encoded_array_to_arrow_with_row_selection_options(
            &cove,
            ArrowRowSelection::Rows(&[2, 1, 2]),
            view_options(),
        )
        .unwrap();
        assert_eq!(result.value.data_type(), &DataType::BinaryView);
        let binary = result
            .value
            .as_any()
            .downcast_ref::<BinaryViewArray>()
            .unwrap();
        assert_eq!(binary.value(0), &[0xbb, 0xcc]);
        assert_eq!(binary.value(1), b"binary-payload");
        assert_eq!(binary.value(2), &[0xbb, 0xcc]);
    }

    #[test]
    fn view_export_schema_policy_is_opt_in() {
        assert_eq!(
            arrow_data_type_for_export_options(
                CoveLogicalType::Utf8,
                ArrowExportOptions::default()
            )
            .unwrap()
            .value,
            DataType::Utf8
        );
        assert_eq!(
            arrow_data_type_for_export_options(CoveLogicalType::Utf8, view_options())
                .unwrap()
                .value,
            DataType::Utf8View
        );
        assert_eq!(
            arrow_data_type_for_export_options(CoveLogicalType::Json, view_options())
                .unwrap()
                .value,
            DataType::Utf8View
        );
        assert_eq!(
            arrow_data_type_for_export_options(CoveLogicalType::Binary, view_options())
                .unwrap()
                .value,
            DataType::BinaryView
        );
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

        let view = encoded_array_to_arrow_with_options(&cove, view_options())
            .unwrap()
            .value;
        let strings = view.as_any().downcast_ref::<StringViewArray>().unwrap();
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
        assert!(encoded_array_to_arrow_with_options(&cove, view_options()).is_err());
    }

    #[test]
    fn strict_utf8_rejects_invalid_row_boundaries_even_if_concat_valid() {
        let mut values = Vec::new();
        values.extend_from_slice(&1u32.to_le_bytes());
        values.push(0xc2);
        values.extend_from_slice(&1u32.to_le_bytes());
        values.push(0xa2);
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        assert!(encoded_array_to_arrow(&cove).is_err());
    }

    #[test]
    fn strict_utf8_accepts_valid_multibyte_rows() {
        let mut values = Vec::new();
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(&[0xc2, 0xa2]);
        values.extend_from_slice(&2u32.to_le_bytes());
        values.extend_from_slice(&[0xc3, 0xa9]);
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
        assert_eq!(strings.value(0), "¢");
        assert_eq!(strings.value(1), "é");
    }

    #[test]
    fn selected_utf8_rejects_invalid_row_boundaries_even_if_concat_valid() {
        let mut values = Vec::new();
        values.extend_from_slice(&1u32.to_le_bytes());
        values.push(0xc2);
        values.extend_from_slice(&1u32.to_le_bytes());
        values.push(0xa2);
        let cove = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            2,
            CoveEncodingKind::VarBytes,
            None,
            &values,
            None,
        );

        assert!(encoded_array_to_arrow_with_row_selection_options(
            &cove,
            ArrowRowSelection::Rows(&[0, 1]),
            ArrowExportOptions::default(),
        )
        .is_err());
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
    fn small_varbytes_copy_matches_slice_copy_for_short_and_large_values() {
        for len in 0usize..=128 {
            let src = (0..len).map(|i| i as u8).collect::<Vec<_>>();
            let mut dst = vec![0xaa; len];
            // SAFETY: source and destination are separate allocations and both
            // are valid for `len` bytes.
            unsafe {
                copy_varbytes_value(src.as_ptr(), dst.as_mut_ptr(), len);
            }
            assert_eq!(dst, src);
        }
    }

    #[test]
    fn fused_varbytes_copy_reports_ascii_high_bits() {
        for len in 0usize..=128 {
            let src = vec![b'a'; len];
            let mut dst = vec![0xaa; len];
            // SAFETY: source and destination are separate allocations and both
            // are valid for `len` bytes.
            let mask =
                unsafe { copy_varbytes_value_ascii_mask(src.as_ptr(), dst.as_mut_ptr(), len) };
            assert_eq!(dst, src);
            assert_eq!(mask, 0, "len={len}");
        }

        for len in 1usize..=128 {
            let mut src = vec![b'a'; len];
            src[len - 1] = 0xc2;
            let mut dst = vec![0xaa; len];
            // SAFETY: source and destination are separate allocations and both
            // are valid for `len` bytes.
            let mask =
                unsafe { copy_varbytes_value_ascii_mask(src.as_ptr(), dst.as_mut_ptr(), len) };
            assert_eq!(dst, src);
            assert_ne!(mask, 0, "len={len}");
        }
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
    fn selected_export_reads_numcode_rows_directly_with_nulls() {
        let mut values = Vec::new();
        values.extend_from_slice(&10u64.to_le_bytes());
        values.extend_from_slice(&20u64.to_le_bytes());
        values.extend_from_slice(&30u64.to_le_bytes());
        let mut validity = ValidityBitmapBuilder::new(3).unwrap();
        validity.set_null(1).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 3);
        let cove = EncodedArray::new(
            CoveLogicalType::Int64,
            CovePhysicalKind::NumCode,
            3,
            CoveEncodingKind::NumCode,
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
        let ints = result.value.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(ints.value(0), 30);
        assert!(ints.is_null(1));
        assert_eq!(ints.value(2), 10);
    }

    #[test]
    fn bitset_export_reads_numcode_rows_directly_with_nulls() {
        let mut values = Vec::new();
        values.extend_from_slice(&10u64.to_le_bytes());
        values.extend_from_slice(&20u64.to_le_bytes());
        values.extend_from_slice(&30u64.to_le_bytes());
        values.extend_from_slice(&40u64.to_le_bytes());
        let mut validity = ValidityBitmapBuilder::new(4).unwrap();
        validity.set_null(2).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 4);
        let cove = EncodedArray::new(
            CoveLogicalType::Int64,
            CovePhysicalKind::NumCode,
            4,
            CoveEncodingKind::NumCode,
            Some(bitmap),
            &values,
            None,
        );

        let result = encoded_array_to_arrow_with_row_selection_options(
            &cove,
            ArrowRowSelection::Bitset {
                words: &[0b1101],
                len: 4,
            },
            ArrowExportOptions::default(),
        )
        .unwrap();
        let ints = result.value.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(ints.value(0), 10);
        assert!(ints.is_null(1));
        assert_eq!(ints.value(2), 40);
    }

    #[test]
    fn rejects_numcode_int64_overflow_all_rows() {
        let mut values = Vec::new();
        values.extend_from_slice(&10u64.to_le_bytes());
        values.extend_from_slice(&(i64::MAX as u64 + 1).to_le_bytes());
        let cove = EncodedArray::new(
            CoveLogicalType::Int64,
            CovePhysicalKind::NumCode,
            2,
            CoveEncodingKind::NumCode,
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
    fn bitset_export_reads_bool_rows_directly_with_nulls() {
        let values = [1u8, 0u8, 1u8, 1u8];
        let mut validity = ValidityBitmapBuilder::new(4).unwrap();
        validity.set_null(2).unwrap();
        let validity_bytes = validity.into_bytes();
        let bitmap = crate::validity::ValidityBitmap::new(&validity_bytes, 4);
        let cove = EncodedArray::new(
            CoveLogicalType::Bool,
            CovePhysicalKind::Boolean,
            4,
            CoveEncodingKind::PlainFixed,
            Some(bitmap),
            &values,
            None,
        );

        let result = encoded_array_to_arrow_with_row_selection_options(
            &cove,
            ArrowRowSelection::Bitset {
                words: &[0b1101],
                len: 4,
            },
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
