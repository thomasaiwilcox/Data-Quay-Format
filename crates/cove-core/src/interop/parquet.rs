//! Spec §51 — Parquet conversion profile.
//!
//! The current implementation is an MVP converter that materializes a single
//! COVE-T scan-profile file from Parquet bytes. It supports non-null primitive,
//! temporal, UTF-8, binary, and decimal128 columns and emits explicit scan page
//! payloads through [`crate::writer::ScanProfileCoveWriter`].

use arrow_array::{
    Array, BinaryArray, BooleanArray, Date32Array, Decimal128Array, Float32Array, Float64Array,
    Int16Array, Int32Array, Int64Array, Int8Array, LargeBinaryArray, LargeStringArray, StringArray,
    TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_schema::{DataType, TimeUnit};
use bytes::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde_json::{json, Value};

use crate::{
    array::{CoveArrayValue, EncodedArray},
    constants::{CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
    reader::{validate_bytes_with_options, ValidationOptions},
    table::{ColumnEntry, TableCatalog, TableEntry},
    types,
    writer::{ScanPageSpec, ScanProfileCoveWriter, ScanSegment},
    CoveError,
};

/// One step in the Parquet → COVE conversion pipeline (Spec §51.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionStep {
    DecodeSource,
    PartitionSegments,
    BuildDictionaries,
    ChooseFileOrNumCode,
    RecomputeStats,
    BuildDomainsAndIndexes,
    EncodePages,
    WriteSections,
    EmitOptionalCovmCovx,
}

impl ConversionStep {
    pub fn as_str(self) -> &'static str {
        match self {
            ConversionStep::DecodeSource => "DecodeSource",
            ConversionStep::PartitionSegments => "PartitionSegments",
            ConversionStep::BuildDictionaries => "BuildDictionaries",
            ConversionStep::ChooseFileOrNumCode => "ChooseFileOrNumCode",
            ConversionStep::RecomputeStats => "RecomputeStats",
            ConversionStep::BuildDomainsAndIndexes => "BuildDomainsAndIndexes",
            ConversionStep::EncodePages => "EncodePages",
            ConversionStep::WriteSections => "WriteSections",
            ConversionStep::EmitOptionalCovmCovx => "EmitOptionalCovmCovx",
        }
    }
}

/// Canonical conversion plan in Spec §51.2 order.
pub fn canonical_plan() -> Vec<ConversionStep> {
    vec![
        ConversionStep::DecodeSource,
        ConversionStep::PartitionSegments,
        ConversionStep::BuildDictionaries,
        ConversionStep::ChooseFileOrNumCode,
        ConversionStep::RecomputeStats,
        ConversionStep::BuildDomainsAndIndexes,
        ConversionStep::EncodePages,
        ConversionStep::WriteSections,
        ConversionStep::EmitOptionalCovmCovx,
    ]
}

/// Spec §51.3: unsupported nested source shapes MUST be downgraded to JSON
/// or Binary and marked pushdown-limited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedNestedFallback {
    Json,
    Binary,
}

/// Controls Parquet → COVE conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParquetConversionOptions {
    pub table_name: String,
    pub namespace: String,
    pub morsel_row_count: u32,
    pub page_compression: CompressionCodec,
}

impl Default for ParquetConversionOptions {
    fn default() -> Self {
        Self {
            table_name: "parquet_import".into(),
            namespace: "interop".into(),
            morsel_row_count: 4096,
            page_compression: CompressionCodec::None,
        }
    }
}

/// A scalar value decoded from a materialized Parquet conversion page.
#[derive(Debug, Clone, PartialEq)]
pub enum ParquetScalarValue {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float32(f32),
    Float64(f64),
    Decimal64(i64),
    Decimal128(i128),
    DateDays(i32),
    TimestampMicros(i64),
    TimestampNanos(i64),
    Utf8(String),
    Binary(Vec<u8>),
}

impl ParquetScalarValue {
    pub fn to_json_value(&self) -> Value {
        match self {
            ParquetScalarValue::Bool(value) => json!(value),
            ParquetScalarValue::Int(value) => json!(value),
            ParquetScalarValue::UInt(value) => json!(value),
            ParquetScalarValue::Float32(value) => serde_json::Number::from_f64(*value as f64)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(value.to_string())),
            ParquetScalarValue::Float64(value) => serde_json::Number::from_f64(*value)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(value.to_string())),
            ParquetScalarValue::Decimal64(value) => json!(value),
            ParquetScalarValue::Decimal128(value) => Value::String(value.to_string()),
            ParquetScalarValue::DateDays(value) => json!(value),
            ParquetScalarValue::TimestampMicros(value) => json!(value),
            ParquetScalarValue::TimestampNanos(value) => json!(value),
            ParquetScalarValue::Utf8(value) => json!(value),
            ParquetScalarValue::Binary(value) => Value::String(hex_encode(value)),
        }
    }
}

/// Per-column conversion metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParquetColumnReport {
    pub column_id: u32,
    pub name: String,
    pub source_type: String,
    pub logical: CoveLogicalType,
    pub physical: CovePhysicalKind,
    pub nullable: bool,
    pub notes: Vec<String>,
}

/// Machine-readable conversion report for Spec §51.5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParquetConversionReport {
    pub table_name: String,
    pub namespace: String,
    pub row_count: u64,
    pub column_count: u32,
    pub required_features: u64,
    pub optional_features: u64,
    pub plan: Vec<ConversionStep>,
    pub notes: Vec<String>,
    pub columns: Vec<ParquetColumnReport>,
}

impl ParquetConversionReport {
    pub fn to_json_value(&self) -> Value {
        json!({
            "source_format": "parquet",
            "table_name": self.table_name,
            "namespace": self.namespace,
            "row_count": self.row_count,
            "column_count": self.column_count,
            "required_features": self.required_features,
            "optional_features": self.optional_features,
            "plan": self.plan.iter().map(|step| step.as_str()).collect::<Vec<_>>(),
            "notes": self.notes,
            "columns": self
                .columns
                .iter()
                .map(|column| {
                    json!({
                        "column_id": column.column_id,
                        "name": column.name,
                        "source_type": column.source_type,
                        "logical": format!("{:?}", column.logical),
                        "physical": format!("{:?}", column.physical),
                        "nullable": column.nullable,
                        "notes": column.notes,
                    })
                })
                .collect::<Vec<_>>(),
        })
    }
}

/// Output of a Parquet conversion run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParquetConversionResult {
    pub cove_bytes: Vec<u8>,
    pub report: ParquetConversionReport,
}

/// Validate that a conversion plan starts with `DecodeSource` and ends with
/// either `WriteSections` or `EmitOptionalCovmCovx` (Spec §51.2).
pub fn validate_plan(plan: &[ConversionStep]) -> Result<(), CoveError> {
    if plan.first() != Some(&ConversionStep::DecodeSource) {
        return Err(CoveError::BadSection(
            "conversion plan must start with DecodeSource (Spec §51.2)".into(),
        ));
    }
    let last = plan.last();
    if !matches!(
        last,
        Some(ConversionStep::WriteSections) | Some(ConversionStep::EmitOptionalCovmCovx)
    ) {
        return Err(CoveError::BadSection(
            "conversion plan must end with WriteSections or EmitOptionalCovmCovx".into(),
        ));
    }
    Ok(())
}

/// Returns the materialized page encoding used by the MVP converter for a
/// physical kind.
pub fn materialized_page_encoding(
    physical: CovePhysicalKind,
) -> Result<CoveEncodingKind, CoveError> {
    match physical {
        CovePhysicalKind::NumCode => Ok(CoveEncodingKind::NumCode),
        CovePhysicalKind::Boolean | CovePhysicalKind::FixedBytes => {
            Ok(CoveEncodingKind::PlainFixed)
        }
        CovePhysicalKind::VarBytes => Ok(CoveEncodingKind::VarBytes),
        other => Err(CoveError::BadSchema(format!(
            "Parquet MVP converter cannot materialize physical kind {other:?}"
        ))),
    }
}

/// Decode materialized page payload bytes emitted by [`convert_parquet_bytes`]
/// back into logical scalar values.
pub fn decode_materialized_page_values(
    column: &ColumnEntry,
    row_count: u32,
    payload: &[u8],
) -> Result<Vec<ParquetScalarValue>, CoveError> {
    let encoding = materialized_page_encoding(column.physical)?;
    let array = EncodedArray::new(
        column.logical,
        column.physical,
        row_count as u64,
        encoding,
        None,
        payload,
        None,
    );
    array
        .decode_all_rows()?
        .into_iter()
        .map(|value| decoded_value_to_scalar(column, value))
        .collect()
}

/// Convert Parquet bytes into a semantically valid COVE-T scan-profile file.
pub fn convert_parquet_bytes(
    bytes: &[u8],
    options: &ParquetConversionOptions,
) -> Result<ParquetConversionResult, CoveError> {
    validate_plan(&canonical_plan())?;
    if options.morsel_row_count == 0 {
        return Err(CoveError::BadSchema(
            "morsel_row_count must be greater than zero".into(),
        ));
    }

    let builder = ParquetRecordBatchReaderBuilder::try_new(Bytes::copy_from_slice(bytes))
        .map_err(|error| CoveError::BadSection(format!("cannot open parquet source: {error}")))?;
    let schema = builder.schema().clone();
    let mut columns = schema
        .fields()
        .iter()
        .enumerate()
        .map(|(index, field)| ConvertedColumn::from_field(index as u32 + 1, field))
        .collect::<Result<Vec<_>, _>>()?;

    let reader = builder
        .with_batch_size(options.morsel_row_count as usize)
        .build()
        .map_err(|error| CoveError::BadSection(format!("cannot build parquet reader: {error}")))?;

    let mut total_rows = 0usize;
    for batch in reader {
        let batch = batch.map_err(|error| {
            CoveError::BadSection(format!("cannot read parquet batch: {error}"))
        })?;
        total_rows = total_rows
            .checked_add(batch.num_rows())
            .ok_or(CoveError::ArithOverflow)?;
        for (column, array) in columns.iter_mut().zip(batch.columns()) {
            column.append_array(array.as_ref())?;
        }
    }

    let row_count = u32::try_from(total_rows)
        .map_err(|_| CoveError::BadSchema("Parquet row count exceeds u32::MAX".into()))?;
    let column_entries = columns
        .iter()
        .map(|column| column.entry.clone())
        .collect::<Vec<_>>();
    let table_catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 1,
            namespace: options.namespace.clone(),
            name: options.table_name.clone(),
            row_count: row_count as u64,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: column_entries,
        }],
    };

    let mut segment = ScanSegment::new(1, 0, 0, row_count, columns.len() as u32);
    segment.morsel_row_count = options.morsel_row_count;
    for column in &columns {
        segment.set_column_pages(
            column.entry.column_id,
            column.page_specs(options.morsel_row_count, options.page_compression)?,
        );
    }

    let mut writer = ScanProfileCoveWriter::new(table_catalog);
    writer.push_segment(segment);
    let cove_bytes = writer.write()?;
    let validated = validate_bytes_with_options(
        &cove_bytes,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
        },
    )?;

    let mut notes = Vec::new();
    if columns.iter().any(|column| {
        matches!(
            column.entry.logical,
            CoveLogicalType::Utf8 | CoveLogicalType::Binary
        )
    }) {
        notes.push(
            "MVP converter materializes Utf8/Binary columns as VarBytes pages without file-dictionary synthesis"
                .into(),
        );
    }
    if columns.iter().any(|column| !column.notes.is_empty()) {
        notes.push(
            "One or more columns required source-unit normalization during conversion".into(),
        );
    }

    Ok(ParquetConversionResult {
        cove_bytes,
        report: ParquetConversionReport {
            table_name: options.table_name.clone(),
            namespace: options.namespace.clone(),
            row_count: row_count as u64,
            column_count: columns.len() as u32,
            required_features: validated.validated.header.required_features,
            optional_features: validated.validated.header.optional_features,
            plan: canonical_plan(),
            notes,
            columns: columns.into_iter().map(|column| column.report()).collect(),
        },
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceColumnKind {
    Boolean,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
    Date32,
    TimestampSecond,
    TimestampMillisecond,
    TimestampMicrosecond,
    TimestampNanosecond,
    Utf8,
    LargeUtf8,
    Binary,
    LargeBinary,
    Decimal128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MaterializedValues {
    Boolean(Vec<u8>),
    NumCode(Vec<u64>),
    VarBytes(Vec<Vec<u8>>),
    FixedBytes { width: usize, values: Vec<Vec<u8>> },
}

impl MaterializedValues {
    fn row_count(&self) -> usize {
        match self {
            MaterializedValues::Boolean(values) => values.len(),
            MaterializedValues::NumCode(values) => values.len(),
            MaterializedValues::VarBytes(values) => values.len(),
            MaterializedValues::FixedBytes { values, .. } => values.len(),
        }
    }

    fn encode_rows(&self, start: usize, len: usize) -> Result<Vec<u8>, CoveError> {
        match self {
            MaterializedValues::Boolean(values) => Ok(values[start..start + len].to_vec()),
            MaterializedValues::NumCode(values) => {
                let mut out = Vec::with_capacity(len * 8);
                for value in &values[start..start + len] {
                    out.extend_from_slice(&value.to_le_bytes());
                }
                Ok(out)
            }
            MaterializedValues::VarBytes(values) => {
                let slice = &values[start..start + len];
                let capacity = slice
                    .iter()
                    .try_fold(0usize, |cap, value| {
                        cap.checked_add(4)
                            .and_then(|next| next.checked_add(value.len()))
                    })
                    .ok_or(CoveError::ArithOverflow)?;
                let mut out = Vec::with_capacity(capacity);
                for value in slice {
                    let len = u32::try_from(value.len()).map_err(|_| CoveError::ArithOverflow)?;
                    out.extend_from_slice(&len.to_le_bytes());
                    out.extend_from_slice(value);
                }
                Ok(out)
            }
            MaterializedValues::FixedBytes { width, values } => {
                let mut out = Vec::with_capacity(len * width);
                for value in &values[start..start + len] {
                    if value.len() != *width {
                        return Err(CoveError::BadSchema(format!(
                            "fixed-width materialized value length {} does not match width {}",
                            value.len(),
                            width
                        )));
                    }
                    out.extend_from_slice(value);
                }
                Ok(out)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConvertedColumn {
    entry: ColumnEntry,
    source_kind: SourceColumnKind,
    source_type: String,
    encoding: CoveEncodingKind,
    notes: Vec<String>,
    values: MaterializedValues,
}

impl ConvertedColumn {
    fn from_field(column_id: u32, field: &arrow_schema::Field) -> Result<Self, CoveError> {
        let nullable = field.is_nullable();
        let (logical, physical, source_kind, values, precision, scale, notes) =
            match field.data_type() {
                DataType::Boolean => (
                    CoveLogicalType::Bool,
                    CovePhysicalKind::Boolean,
                    SourceColumnKind::Boolean,
                    MaterializedValues::Boolean(Vec::new()),
                    0,
                    0,
                    Vec::new(),
                ),
                DataType::Int8 => numcode_column(CoveLogicalType::Int8, SourceColumnKind::Int8),
                DataType::Int16 => {
                    numcode_column(CoveLogicalType::Int16, SourceColumnKind::Int16)
                }
                DataType::Int32 => {
                    numcode_column(CoveLogicalType::Int32, SourceColumnKind::Int32)
                }
                DataType::Int64 => {
                    numcode_column(CoveLogicalType::Int64, SourceColumnKind::Int64)
                }
                DataType::UInt8 => {
                    numcode_column(CoveLogicalType::UInt8, SourceColumnKind::UInt8)
                }
                DataType::UInt16 => {
                    numcode_column(CoveLogicalType::UInt16, SourceColumnKind::UInt16)
                }
                DataType::UInt32 => {
                    numcode_column(CoveLogicalType::UInt32, SourceColumnKind::UInt32)
                }
                DataType::UInt64 => {
                    numcode_column(CoveLogicalType::UInt64, SourceColumnKind::UInt64)
                }
                DataType::Float32 => {
                    numcode_column(CoveLogicalType::Float32, SourceColumnKind::Float32)
                }
                DataType::Float64 => {
                    numcode_column(CoveLogicalType::Float64, SourceColumnKind::Float64)
                }
                DataType::Date32 => {
                    numcode_column(CoveLogicalType::DateDays, SourceColumnKind::Date32)
                }
                DataType::Timestamp(TimeUnit::Second, _) => (
                    CoveLogicalType::TimestampMicros,
                    CovePhysicalKind::NumCode,
                    SourceColumnKind::TimestampSecond,
                    MaterializedValues::NumCode(Vec::new()),
                    0,
                    0,
                    vec!["normalized seconds timestamps to TimestampMicros".into()],
                ),
                DataType::Timestamp(TimeUnit::Millisecond, _) => (
                    CoveLogicalType::TimestampMicros,
                    CovePhysicalKind::NumCode,
                    SourceColumnKind::TimestampMillisecond,
                    MaterializedValues::NumCode(Vec::new()),
                    0,
                    0,
                    vec!["normalized millisecond timestamps to TimestampMicros".into()],
                ),
                DataType::Timestamp(TimeUnit::Microsecond, _) => (
                    CoveLogicalType::TimestampMicros,
                    CovePhysicalKind::NumCode,
                    SourceColumnKind::TimestampMicrosecond,
                    MaterializedValues::NumCode(Vec::new()),
                    0,
                    0,
                    Vec::new(),
                ),
                DataType::Timestamp(TimeUnit::Nanosecond, _) => (
                    CoveLogicalType::TimestampNanos,
                    CovePhysicalKind::NumCode,
                    SourceColumnKind::TimestampNanosecond,
                    MaterializedValues::NumCode(Vec::new()),
                    0,
                    0,
                    Vec::new(),
                ),
                DataType::Utf8 => (
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    SourceColumnKind::Utf8,
                    MaterializedValues::VarBytes(Vec::new()),
                    0,
                    0,
                    Vec::new(),
                ),
                DataType::LargeUtf8 => (
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    SourceColumnKind::LargeUtf8,
                    MaterializedValues::VarBytes(Vec::new()),
                    0,
                    0,
                    Vec::new(),
                ),
                DataType::Binary => (
                    CoveLogicalType::Binary,
                    CovePhysicalKind::VarBytes,
                    SourceColumnKind::Binary,
                    MaterializedValues::VarBytes(Vec::new()),
                    0,
                    0,
                    Vec::new(),
                ),
                DataType::LargeBinary => (
                    CoveLogicalType::Binary,
                    CovePhysicalKind::VarBytes,
                    SourceColumnKind::LargeBinary,
                    MaterializedValues::VarBytes(Vec::new()),
                    0,
                    0,
                    Vec::new(),
                ),
                DataType::Decimal128(precision, scale) => (
                    CoveLogicalType::Decimal128,
                    CovePhysicalKind::FixedBytes,
                    SourceColumnKind::Decimal128,
                    MaterializedValues::FixedBytes {
                        width: 16,
                        values: Vec::new(),
                    },
                    *precision as u16,
                    *scale as i16,
                    Vec::new(),
                ),
                other if is_nested_arrow_type(other) => {
                    return Err(CoveError::BadSchema(format!(
                        "Parquet MVP converter does not support nested source column '{}' with type {other:?}; use JSON/Binary fallback in a future converter",
                        field.name()
                    )))
                }
                other => {
                    return Err(CoveError::BadSchema(format!(
                        "Parquet MVP converter does not support source column '{}' with type {other:?}",
                        field.name()
                    )))
                }
            };

        let entry = ColumnEntry {
            column_id,
            name: field.name().to_string(),
            logical,
            physical,
            nullable,
            sort_order: 0,
            collation_id: 0,
            precision,
            scale,
            flags: 0,
        };
        let encoding = materialized_page_encoding(physical)?;
        Ok(Self {
            entry,
            source_kind,
            source_type: format!("{:?}", field.data_type()),
            encoding,
            notes,
            values,
        })
    }

    fn append_array(&mut self, array: &dyn Array) -> Result<(), CoveError> {
        if array.null_count() != 0 {
            return Err(CoveError::BadSchema(format!(
                "Parquet MVP converter does not support materializing null values for column '{}'",
                self.entry.name
            )));
        }

        match self.source_kind {
            SourceColumnKind::Boolean => {
                let array = downcast_array::<BooleanArray>(array, &self.entry.name)?;
                let values = expect_boolean_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| u8::from(array.value(row))));
            }
            SourceColumnKind::Int8 => {
                let array = downcast_array::<Int8Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row) as i64 as u64));
            }
            SourceColumnKind::Int16 => {
                let array = downcast_array::<Int16Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row) as i64 as u64));
            }
            SourceColumnKind::Int32 => {
                let array = downcast_array::<Int32Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row) as i64 as u64));
            }
            SourceColumnKind::Int64 => {
                let array = downcast_array::<Int64Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row) as u64));
            }
            SourceColumnKind::UInt8 => {
                let array = downcast_array::<UInt8Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row) as u64));
            }
            SourceColumnKind::UInt16 => {
                let array = downcast_array::<UInt16Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row) as u64));
            }
            SourceColumnKind::UInt32 => {
                let array = downcast_array::<UInt32Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row) as u64));
            }
            SourceColumnKind::UInt64 => {
                let array = downcast_array::<UInt64Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row)));
            }
            SourceColumnKind::Float32 => {
                let array = downcast_array::<Float32Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row).to_bits() as u64));
            }
            SourceColumnKind::Float64 => {
                let array = downcast_array::<Float64Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row).to_bits()));
            }
            SourceColumnKind::Date32 => {
                let array = downcast_array::<Date32Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row) as i64 as u64));
            }
            SourceColumnKind::TimestampSecond => {
                let array = downcast_array::<TimestampSecondArray>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                for row in 0..array.len() {
                    let micros = array
                        .value(row)
                        .checked_mul(1_000_000)
                        .ok_or(CoveError::ArithOverflow)?;
                    values.push(micros as u64);
                }
            }
            SourceColumnKind::TimestampMillisecond => {
                let array = downcast_array::<TimestampMillisecondArray>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                for row in 0..array.len() {
                    let micros = array
                        .value(row)
                        .checked_mul(1_000)
                        .ok_or(CoveError::ArithOverflow)?;
                    values.push(micros as u64);
                }
            }
            SourceColumnKind::TimestampMicrosecond => {
                let array = downcast_array::<TimestampMicrosecondArray>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row) as u64));
            }
            SourceColumnKind::TimestampNanosecond => {
                let array = downcast_array::<TimestampNanosecondArray>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row) as u64));
            }
            SourceColumnKind::Utf8 => {
                let array = downcast_array::<StringArray>(array, &self.entry.name)?;
                let values = expect_varbytes_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row).as_bytes().to_vec()));
            }
            SourceColumnKind::LargeUtf8 => {
                let array = downcast_array::<LargeStringArray>(array, &self.entry.name)?;
                let values = expect_varbytes_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row).as_bytes().to_vec()));
            }
            SourceColumnKind::Binary => {
                let array = downcast_array::<BinaryArray>(array, &self.entry.name)?;
                let values = expect_varbytes_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row).to_vec()));
            }
            SourceColumnKind::LargeBinary => {
                let array = downcast_array::<LargeBinaryArray>(array, &self.entry.name)?;
                let values = expect_varbytes_values(&mut self.values)?;
                values.extend((0..array.len()).map(|row| array.value(row).to_vec()));
            }
            SourceColumnKind::Decimal128 => {
                let array = downcast_array::<Decimal128Array>(array, &self.entry.name)?;
                let values = expect_fixed_values(&mut self.values, 16)?;
                values.extend((0..array.len()).map(|row| array.value(row).to_le_bytes().to_vec()));
            }
        }
        Ok(())
    }

    fn page_specs(
        &self,
        morsel_row_count: u32,
        compression: CompressionCodec,
    ) -> Result<Vec<ScanPageSpec>, CoveError> {
        if morsel_row_count == 0 {
            return Err(CoveError::BadSchema(
                "morsel_row_count must be greater than zero".into(),
            ));
        }
        let total_rows = self.values.row_count();
        if total_rows == 0 {
            return Ok(Vec::new());
        }
        let mut pages = Vec::new();
        let mut start = 0usize;
        let step = morsel_row_count as usize;
        while start < total_rows {
            let len = (total_rows - start).min(step);
            let payload = self.values.encode_rows(start, len)?;
            pages.push(
                ScanPageSpec::new(len as u32, payload)
                    .with_compression(compression)
                    .with_encoding_root(self.encoding as u32),
            );
            start += len;
        }
        Ok(pages)
    }

    fn report(self) -> ParquetColumnReport {
        ParquetColumnReport {
            column_id: self.entry.column_id,
            name: self.entry.name,
            source_type: self.source_type,
            logical: self.entry.logical,
            physical: self.entry.physical,
            nullable: self.entry.nullable,
            notes: self.notes,
        }
    }
}

fn numcode_column(
    logical: CoveLogicalType,
    source_kind: SourceColumnKind,
) -> (
    CoveLogicalType,
    CovePhysicalKind,
    SourceColumnKind,
    MaterializedValues,
    u16,
    i16,
    Vec<String>,
) {
    (
        logical,
        CovePhysicalKind::NumCode,
        source_kind,
        MaterializedValues::NumCode(Vec::new()),
        0,
        0,
        Vec::new(),
    )
}

fn is_nested_arrow_type(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::List(_)
            | DataType::LargeList(_)
            | DataType::FixedSizeList(_, _)
            | DataType::Struct(_)
            | DataType::Map(_, _)
    )
}

fn downcast_array<'a, T: 'static>(
    array: &'a dyn Array,
    column_name: &str,
) -> Result<&'a T, CoveError> {
    array.as_any().downcast_ref::<T>().ok_or_else(|| {
        CoveError::BadSchema(format!(
            "Parquet reader produced an unexpected Arrow array type for column '{column_name}'"
        ))
    })
}

fn expect_boolean_values(values: &mut MaterializedValues) -> Result<&mut Vec<u8>, CoveError> {
    match values {
        MaterializedValues::Boolean(values) => Ok(values),
        _ => Err(CoveError::BadSchema(
            "expected boolean materialized values".into(),
        )),
    }
}

fn expect_numcode_values(values: &mut MaterializedValues) -> Result<&mut Vec<u64>, CoveError> {
    match values {
        MaterializedValues::NumCode(values) => Ok(values),
        _ => Err(CoveError::BadSchema(
            "expected NumCode materialized values".into(),
        )),
    }
}

fn expect_varbytes_values(values: &mut MaterializedValues) -> Result<&mut Vec<Vec<u8>>, CoveError> {
    match values {
        MaterializedValues::VarBytes(values) => Ok(values),
        _ => Err(CoveError::BadSchema(
            "expected VarBytes materialized values".into(),
        )),
    }
}

fn expect_fixed_values(
    values: &mut MaterializedValues,
    expected_width: usize,
) -> Result<&mut Vec<Vec<u8>>, CoveError> {
    match values {
        MaterializedValues::FixedBytes { width, values } if *width == expected_width => Ok(values),
        MaterializedValues::FixedBytes { width, .. } => Err(CoveError::BadSchema(format!(
            "expected fixed-width materialized values of width {expected_width}, got {width}"
        ))),
        _ => Err(CoveError::BadSchema(
            "expected fixed-width materialized values".into(),
        )),
    }
}

fn decoded_value_to_scalar(
    column: &ColumnEntry,
    value: CoveArrayValue<'_>,
) -> Result<ParquetScalarValue, CoveError> {
    match value {
        CoveArrayValue::Null => Err(CoveError::BadSection(
            "Parquet MVP pages do not materialize null rows".into(),
        )),
        CoveArrayValue::Bytes(bytes) => match column.logical {
            CoveLogicalType::Bool => Ok(ParquetScalarValue::Bool(
                bytes.first().copied().unwrap_or(0) != 0,
            )),
            CoveLogicalType::Utf8 => Ok(ParquetScalarValue::Utf8(
                String::from_utf8(bytes.to_vec()).map_err(|error| {
                    CoveError::BadSection(format!("invalid UTF-8 page payload: {error}"))
                })?,
            )),
            CoveLogicalType::Binary => Ok(ParquetScalarValue::Binary(bytes.to_vec())),
            CoveLogicalType::Decimal128 => {
                let raw: [u8; 16] = bytes.try_into().map_err(|_| {
                    CoveError::BadSection("decimal128 page payload must be 16 bytes".into())
                })?;
                Ok(ParquetScalarValue::Decimal128(i128::from_le_bytes(raw)))
            }
            other => Err(CoveError::BadSection(format!(
                "unexpected byte-backed logical type {other:?} in Parquet materialized page"
            ))),
        },
        CoveArrayValue::NumCode(code) => match column.logical {
            CoveLogicalType::Int8 => Ok(ParquetScalarValue::Int(types::numcode_as_i8(code) as i64)),
            CoveLogicalType::Int16 => {
                Ok(ParquetScalarValue::Int(types::numcode_as_i16(code) as i64))
            }
            CoveLogicalType::Int32 => {
                Ok(ParquetScalarValue::Int(types::numcode_as_i32(code) as i64))
            }
            CoveLogicalType::Int64 => Ok(ParquetScalarValue::Int(types::numcode_as_i64(code))),
            CoveLogicalType::UInt8 => {
                Ok(ParquetScalarValue::UInt(types::numcode_as_u8(code) as u64))
            }
            CoveLogicalType::UInt16 => {
                Ok(ParquetScalarValue::UInt(types::numcode_as_u16(code) as u64))
            }
            CoveLogicalType::UInt32 => {
                Ok(ParquetScalarValue::UInt(types::numcode_as_u32(code) as u64))
            }
            CoveLogicalType::UInt64 => Ok(ParquetScalarValue::UInt(types::numcode_as_u64(code))),
            CoveLogicalType::Float32 => {
                Ok(ParquetScalarValue::Float32(types::numcode_as_f32(code)))
            }
            CoveLogicalType::Float64 => {
                Ok(ParquetScalarValue::Float64(types::numcode_as_f64(code)))
            }
            CoveLogicalType::Decimal64 => Ok(ParquetScalarValue::Decimal64(
                types::numcode_as_decimal64(code),
            )),
            CoveLogicalType::DateDays => Ok(ParquetScalarValue::DateDays(
                types::numcode_as_date_days(code),
            )),
            CoveLogicalType::TimestampMicros => Ok(ParquetScalarValue::TimestampMicros(
                types::numcode_as_timestamp_micros(code),
            )),
            CoveLogicalType::TimestampNanos => Ok(ParquetScalarValue::TimestampNanos(
                types::numcode_as_timestamp_nanos(code),
            )),
            other => Err(CoveError::BadSection(format!(
                "unexpected NumCode logical type {other:?} in Parquet materialized page"
            ))),
        },
        other => Err(CoveError::BadSection(format!(
            "unexpected decoded Parquet materialized value {other:?}"
        ))),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::Arc};

    use arrow_array::{
        ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
        TimestampMicrosecondArray,
    };
    use parquet::arrow::ArrowWriter;

    use crate::{
        compression::column_page_payload,
        constants::SectionKind,
        page::ColumnPageIndex,
        reader::{validate_bytes_with_options, ValidationOptions},
        segment::TableSegmentPayloadV1,
        table::TableCatalog,
    };

    #[test]
    fn canonical_plan_is_valid() {
        validate_plan(&canonical_plan()).unwrap();
    }

    #[test]
    fn rejects_plan_missing_decode_step() {
        let bad = vec![ConversionStep::WriteSections];
        assert!(matches!(validate_plan(&bad), Err(CoveError::BadSection(_))));
    }

    #[test]
    fn converts_supported_parquet_columns_into_scan_profile_cove() {
        let batch = RecordBatch::try_from_iter(vec![
            (
                "active",
                Arc::new(BooleanArray::from(vec![true, false, true])) as ArrayRef,
            ),
            (
                "id",
                Arc::new(Int64Array::from(vec![10, 20, 30])) as ArrayRef,
            ),
            (
                "score",
                Arc::new(Float64Array::from(vec![1.5, 2.0, 3.25])) as ArrayRef,
            ),
            (
                "city",
                Arc::new(StringArray::from(vec!["sea", "lon", "par"])) as ArrayRef,
            ),
            (
                "blob",
                Arc::new(BinaryArray::from(vec![
                    b"aa".as_ref(),
                    b"bb".as_ref(),
                    b"cc".as_ref(),
                ])) as ArrayRef,
            ),
            (
                "ts_us",
                Arc::new(TimestampMicrosecondArray::from(vec![1000, 2000, 3000])) as ArrayRef,
            ),
        ])
        .unwrap();

        let parquet_bytes = parquet_bytes(&batch);
        let result =
            convert_parquet_bytes(&parquet_bytes, &ParquetConversionOptions::default()).unwrap();
        assert_eq!(result.report.row_count, 3);
        assert_eq!(result.report.column_count, 6);

        let catalog = first_table_catalog(&result.cove_bytes);
        assert_eq!(catalog.tables[0].name, "parquet_import");
        let decoded_columns = decoded_table_values(&result.cove_bytes, &catalog);
        assert_eq!(
            decoded_columns[0],
            vec![json!(true), json!(false), json!(true)]
        );
        assert_eq!(decoded_columns[1], vec![json!(10), json!(20), json!(30)]);
        assert_eq!(
            decoded_columns[2],
            vec![json!(1.5), json!(2.0), json!(3.25)]
        );
        assert_eq!(
            decoded_columns[3],
            vec![json!("sea"), json!("lon"), json!("par")]
        );
        assert_eq!(
            decoded_columns[4],
            vec![json!("6161"), json!("6262"), json!("6363")]
        );
        assert_eq!(
            decoded_columns[5],
            vec![json!(1000), json!(2000), json!(3000)]
        );
    }

    #[test]
    fn rejects_parquet_columns_with_actual_nulls() {
        let batch = RecordBatch::try_from_iter(vec![(
            "id",
            Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])) as ArrayRef,
        )])
        .unwrap();

        let parquet_bytes = parquet_bytes(&batch);
        assert!(matches!(
            convert_parquet_bytes(&parquet_bytes, &ParquetConversionOptions::default()),
            Err(CoveError::BadSchema(message)) if message.contains("null values")
        ));
    }

    fn parquet_bytes(batch: &RecordBatch) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = ArrowWriter::try_new(&mut cursor, batch.schema(), None).unwrap();
            writer.write(batch).unwrap();
            writer.close().unwrap();
        }
        cursor.into_inner()
    }

    fn first_table_catalog(bytes: &[u8]) -> TableCatalog {
        let report = validate_bytes_with_options(
            bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
            },
        )
        .unwrap();
        let entry = report
            .validated
            .footer
            .sections
            .iter()
            .find(|entry| entry.section_kind == SectionKind::TableCatalog as u16)
            .unwrap();
        TableCatalog::parse(&bytes[entry.offset as usize..entry.end_offset().unwrap() as usize])
            .unwrap()
    }

    fn decoded_table_values(bytes: &[u8], catalog: &TableCatalog) -> Vec<Vec<Value>> {
        let report = validate_bytes_with_options(
            bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
            },
        )
        .unwrap();
        let entry = report
            .validated
            .footer
            .sections
            .iter()
            .find(|entry| entry.section_kind == SectionKind::TableSegmentData as u16)
            .unwrap();
        let segment_bytes = &bytes[entry.offset as usize..entry.end_offset().unwrap() as usize];
        let segment = TableSegmentPayloadV1::parse(segment_bytes).unwrap();
        catalog.tables[0]
            .columns
            .iter()
            .map(|column| {
                let column_dir = segment
                    .columns
                    .iter()
                    .find(|dir| dir.column_id == column.column_id)
                    .unwrap();
                let page_index = ColumnPageIndex::parse(
                    &segment_bytes[column_dir.page_index_offset as usize
                        ..(column_dir.page_index_offset + column_dir.page_index_length) as usize],
                )
                .unwrap();
                let mut rows = Vec::new();
                for page in &page_index.entries {
                    let page_wire = &segment_bytes
                        [page.page_offset as usize..(page.page_offset + page.page_length) as usize];
                    let payload = column_page_payload(page_wire, page).unwrap();
                    rows.extend(
                        decode_materialized_page_values(column, page.row_count, payload.as_ref())
                            .unwrap()
                            .into_iter()
                            .map(|value| value.to_json_value()),
                    );
                }
                rows
            })
            .collect()
    }
}
