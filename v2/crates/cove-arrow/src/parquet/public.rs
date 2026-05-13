use serde_json::{json, Value};

use super::*;
use cove_core::index::aggregate::{DEFAULT_HLL_PRECISION, DEFAULT_KLL_K, DEFAULT_TOPK_K};

/// One step in the Parquet → COVE conversion pipeline (Spec §51.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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
#[non_exhaustive]
pub enum UnsupportedNestedFallback {
    Json,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParquetDictionaryPolicy {
    Auto,
    Never,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParquetStatsPolicy {
    None,
    Recompute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParquetAccelerationPolicy {
    None,
    DeclaredOnly,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParquetAggregatePolicy {
    None,
    Auto,
    DeclaredOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParquetClusteringPolicy {
    PreserveSourceOrder,
    StableClusterDeclaredColumns,
}

/// Controls Parquet → COVE conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParquetConversionOptions {
    pub table_name: String,
    pub namespace: String,
    pub morsel_row_count: u32,
    pub segment_row_count: u32,
    pub page_compression: CompressionCodec,
    pub dictionary_policy: ParquetDictionaryPolicy,
    pub stats_policy: ParquetStatsPolicy,
    pub acceleration_policy: ParquetAccelerationPolicy,
    pub point_lookup_columns: Vec<String>,
    pub cluster_columns: Vec<String>,
    pub topn_columns: Vec<String>,
    pub aggregate_policy: ParquetAggregatePolicy,
    pub aggregate_columns: Vec<String>,
    pub aggregate_topk_columns: Vec<String>,
    pub distinct_sketch_columns: Vec<String>,
    pub quantile_sketch_columns: Vec<String>,
    pub aggregate_topk_k: u32,
    pub hll_precision: u8,
    pub kll_k: u32,
    pub composite_zone_groups: Vec<Vec<String>>,
    pub emit_covx: bool,
    pub emit_covm: bool,
    pub clustering_policy: ParquetClusteringPolicy,
}

impl Default for ParquetConversionOptions {
    fn default() -> Self {
        Self {
            table_name: "parquet_import".into(),
            namespace: "interop".into(),
            morsel_row_count: 4096,
            segment_row_count: u32::MAX,
            page_compression: CompressionCodec::None,
            dictionary_policy: ParquetDictionaryPolicy::Auto,
            stats_policy: ParquetStatsPolicy::None,
            acceleration_policy: ParquetAccelerationPolicy::None,
            point_lookup_columns: Vec::new(),
            cluster_columns: Vec::new(),
            topn_columns: Vec::new(),
            aggregate_policy: ParquetAggregatePolicy::None,
            aggregate_columns: Vec::new(),
            aggregate_topk_columns: Vec::new(),
            distinct_sketch_columns: Vec::new(),
            quantile_sketch_columns: Vec::new(),
            aggregate_topk_k: DEFAULT_TOPK_K,
            hll_precision: DEFAULT_HLL_PRECISION,
            kll_k: DEFAULT_KLL_K,
            composite_zone_groups: Vec::new(),
            emit_covx: false,
            emit_covm: false,
            clustering_policy: ParquetClusteringPolicy::PreserveSourceOrder,
        }
    }
}

/// A scalar value decoded from a materialized Parquet conversion page.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ParquetScalarValue {
    Null,
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
    Json(Value),
    Binary(Vec<u8>),
}

impl ParquetScalarValue {
    pub fn to_json_value(&self) -> Value {
        match self {
            ParquetScalarValue::Null => Value::Null,
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
            ParquetScalarValue::Json(value) => value.clone(),
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
    pub pushdown_limited: bool,
    pub fallback: Option<UnsupportedNestedFallback>,
    pub notes: Vec<String>,
}

/// Machine-readable conversion report for Spec §51.5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParquetConversionReport {
    pub source_format: String,
    pub table_name: String,
    pub namespace: String,
    pub row_count: u64,
    pub segment_count: u32,
    pub column_count: u32,
    pub required_features: u64,
    pub optional_features: u64,
    pub plan: Vec<ConversionStep>,
    pub source_schema_fingerprint: String,
    pub target_schema_fingerprint: String,
    pub validation_result: bool,
    pub generated_section_kinds: Vec<String>,
    pub aggregate_synopsis_kinds: Vec<String>,
    pub unsupported_features: Vec<String>,
    pub lossy_features: Vec<String>,
    pub nested_shape_fallbacks: Vec<String>,
    pub notes: Vec<String>,
    pub columns: Vec<ParquetColumnReport>,
}

impl ParquetConversionReport {
    pub fn to_json_value(&self) -> Value {
        json!({
            "source_format": self.source_format,
            "table_name": self.table_name,
            "namespace": self.namespace,
            "row_count": self.row_count,
            "segment_count": self.segment_count,
            "column_count": self.column_count,
            "required_features": self.required_features,
            "optional_features": self.optional_features,
            "plan": self.plan.iter().map(|step| step.as_str()).collect::<Vec<_>>(),
            "source_schema_fingerprint": self.source_schema_fingerprint,
            "target_schema_fingerprint": self.target_schema_fingerprint,
            "validation_result": self.validation_result,
            "generated_section_kinds": self.generated_section_kinds,
            "aggregate_synopsis_kinds": self.aggregate_synopsis_kinds,
            "unsupported_features": self.unsupported_features,
            "lossy_features": self.lossy_features,
            "nested_shape_fallbacks": self.nested_shape_fallbacks,
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
                        "pushdown_limited": column.pushdown_limited,
                        "fallback": column.fallback.map(|fallback| format!("{fallback:?}")),
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
    pub covx_bytes: Option<Vec<u8>>,
    pub covm_bytes: Option<Vec<u8>>,
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
        CovePhysicalKind::FileCode => Ok(CoveEncodingKind::FileCode),
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
    decode_materialized_page_values_with_nulls(column, row_count, 0, payload)
}

/// Decode materialized page payload bytes, including the nullable-page layout
/// emitted by [`convert_parquet_bytes`].
///
/// INVARIANT: when `null_count > 0`, payload starts with the COVE null bitmap
/// for exactly `row_count` rows, followed by one physical payload slot per row.
pub fn decode_materialized_page_values_with_nulls(
    column: &ColumnEntry,
    row_count: u32,
    null_count: u32,
    payload: &[u8],
) -> Result<Vec<ParquetScalarValue>, CoveError> {
    if null_count > row_count {
        return Err(CoveError::PageCorrupt);
    }
    let encoding = materialized_page_encoding(column.physical)?;
    let (validity, data) = if null_count == 0 {
        (None, payload)
    } else {
        let bitmap_len = validity_bitmap_len(row_count)?;
        if payload.len() < bitmap_len {
            return Err(CoveError::BufferTooShort);
        }
        let (validity_bytes, data) = payload.split_at(bitmap_len);
        let bitmap = ValidityBitmap::new(validity_bytes, row_count as u64);
        bitmap.validate_len(row_count as u64)?;
        if bitmap.null_count()? != null_count as u64 {
            return Err(CoveError::PageCorrupt);
        }
        (Some(bitmap), data)
    };
    let array = EncodedArray::new(
        column.logical,
        column.physical,
        row_count as u64,
        encoding,
        validity,
        data,
        None,
    );
    array
        .decode_all_rows()?
        .into_iter()
        .map(|value| decoded_value_to_scalar(column, value))
        .collect()
}
