//! Spec §51 — Parquet conversion profile.
//!
//! The current implementation materializes COVE-T scan-profile files from
//! Parquet bytes. It supports primitive, temporal, UTF-8, binary, decimal128,
//! and nested JSON-fallback columns and emits explicit scan page payloads
//! through [`crate::writer::ScanProfileCoveWriter`].

mod public;

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use arrow_array::{
    Array, BinaryArray, BooleanArray, Date32Array, Decimal128Array, FixedSizeListArray,
    Float32Array, Float64Array, Int16Array, Int32Array, Int64Array, Int8Array, LargeBinaryArray,
    LargeListArray, LargeStringArray, ListArray, MapArray, StringArray, StructArray,
    TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_schema::{DataType, TimeUnit};
use bytes::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde_json::{json, Value};

use crate::{
    array::{CoveArrayValue, EncodedArray},
    artifact::{
        covm::{CovmFile, CovmFileEntryV1, CovmHeaderV1, CovmPostscriptV1},
        covx::{CovxFile, CovxHeaderV1, CovxPostscriptV1, CovxReferencedFileV1},
    },
    checksum,
    constants::{
        CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind, DigestAlgorithm,
        SectionKind,
    },
    dictionary::{
        file_dictionary_candidate_len, FileDictionary, FileDictionaryEncoding, FileDictionaryKey,
    },
    digest::compute_digest,
    domain::ColumnDomain,
    index::{
        aggregate::{AggregateEntry, AggregateSynopsis, SynopsisAccuracy, SynopsisKind},
        bloom::{
            BloomAlgorithm, BloomFilterIndex, BloomGranularity, BloomHashDomain,
            BloomIndexHeaderV1, BLOOM_INDEX_HEADER_LEN,
        },
        composite::{CompositeIndex, CompositeTransformKind, CompositeZoneIndexHeaderV1},
        exact_set::{
            ExactSetGranularity, ExactSetIndex, ExactSetIndexHeaderV1, ExactSetKeyKind,
            ExactSetRepresentation, EXACT_SET_HEADER_LEN,
        },
        lookup::{
            LookupEntry, LookupIndex, LookupIndexHeaderV1, LookupIndexKind, LookupKeyKind,
            LookupUniqueness,
        },
        topn::{TopNDirection, TopNSummary},
    },
    page::{PAGE_FLAG_ALL_NON_NULL, PAGE_FLAG_ALL_NULL},
    reader::{validate_bytes_with_options, ValidationOptions},
    row_ref::RowRef,
    table::{ColumnEntry, TableCatalog, TableEntry},
    types,
    validity::{ValidityBitmap, ValidityBitmapBuilder},
    writer::{ScanPageSpec, ScanProfileCoveWriter, ScanSegment},
    zone_stats::{
        StatKind, StatScalar, ZoneScope, ZoneStatFlags, ZoneStats, ZoneStatsEntry, ZoneStatsSection,
    },
    CoveError,
};

pub use public::*;

/// Convert Parquet bytes into a semantically valid COVE-T scan-profile file.
pub fn convert_parquet_bytes(
    bytes: &[u8],
    options: &ParquetConversionOptions,
) -> Result<ParquetConversionResult, CoveError> {
    if options.morsel_row_count == 0 {
        return Err(CoveError::BadSchema(
            "morsel_row_count must be greater than zero".into(),
        ));
    }
    if options.segment_row_count == 0 {
        return Err(CoveError::BadSchema(
            "segment_row_count must be greater than zero".into(),
        ));
    }

    let builder = ParquetRecordBatchReaderBuilder::try_new(Bytes::copy_from_slice(bytes))
        .map_err(|error| CoveError::BadSection(format!("cannot open parquet source: {error}")))?;
    let schema = builder.schema().clone();
    let source_schema_fingerprint = format!(
        "crc32c:{:08x}",
        checksum::crc32c(format!("{schema:?}").as_bytes())
    );
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

    let row_count = u64::try_from(total_rows).map_err(|_| CoveError::ArithOverflow)?;
    let segment_layouts = build_segment_layouts(total_rows, options.segment_row_count)?;
    let mut notes = Vec::new();
    let mut unsupported_features = Vec::new();
    let lossy_features = Vec::new();

    if let Some(note) = apply_stable_clustering(&mut columns, options)? {
        notes.push(note);
    }

    let dictionary = apply_dictionary_synthesis(&mut columns, options.dictionary_policy)?;
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
            row_count,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: column_entries,
        }],
    };
    let target_schema_fingerprint = format!(
        "crc32c:{:08x}",
        checksum::crc32c(&table_catalog.serialize()?)
    );

    let mut writer = ScanProfileCoveWriter::new(table_catalog);
    if let Some(dictionary) = &dictionary {
        writer.push_file_dictionary(dictionary);
        notes.push(format!(
            "Synthesized a deterministic file dictionary with {} entries",
            dictionary.len()
        ));
    }

    let mut domain_count = 0usize;
    if options.stats_policy == ParquetStatsPolicy::Recompute {
        for domain in build_column_domains(
            &columns,
            dictionary.as_ref().map(|dictionary| dictionary.len()),
        )? {
            writer.push_column_domain(&domain)?;
            domain_count += 1;
        }
        if let Some(zone_stats) =
            build_zone_stats(&columns, &segment_layouts, options.morsel_row_count)?
        {
            writer.push_zone_stats(&zone_stats)?;
            notes.push(format!(
                "Recomputed {} morsel-level zone-stat entries from decoded Arrow values",
                zone_stats.entries.len()
            ));
        }
    }

    let acceleration = build_acceleration_artifacts(&columns, options, &segment_layouts)?;
    for index in &acceleration.exact_sets {
        writer.push_exact_set_index(index);
    }
    for index in &acceleration.blooms {
        writer.push_bloom_index(index);
    }
    for index in &acceleration.lookups {
        writer.push_lookup_index(index)?;
    }
    for synopsis in &acceleration.aggregates {
        writer.push_aggregate_synopsis(synopsis);
    }
    for index in &acceleration.composites {
        writer.push_composite_zone_index(index);
    }
    for summary in &acceleration.topn {
        writer.push_topn_summary(summary);
    }
    notes.extend(acceleration.notes);
    unsupported_features.extend(acceleration.unsupported);

    if domain_count != 0 {
        notes.push(format!("Generated {domain_count} ColumnDomain section(s)"));
    }

    for layout in &segment_layouts {
        let mut segment = ScanSegment::new(
            1,
            layout.segment_id,
            u64::try_from(layout.row_start).map_err(|_| CoveError::ArithOverflow)?,
            u32::try_from(layout.row_count).map_err(|_| CoveError::ArithOverflow)?,
            columns.len() as u32,
        );
        segment.morsel_row_count = options.morsel_row_count;
        for column in &columns {
            segment.set_column_pages(
                column.entry.column_id,
                column.page_specs_range(
                    layout.row_start,
                    layout.row_count,
                    options.morsel_row_count,
                    options.page_compression,
                )?,
            );
        }
        writer.push_segment(segment);
    }
    let cove_bytes = writer.write()?;
    let validated = validate_bytes_with_options(
        &cove_bytes,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        },
    )?;
    let generated_section_kinds = validated
        .validated
        .footer
        .sections
        .iter()
        .map(|entry| {
            SectionKind::from_u16(entry.section_kind)
                .map(|kind| format!("{kind:?}"))
                .unwrap_or_else(|| format!("Unknown({})", entry.section_kind))
        })
        .collect::<Vec<_>>();

    if columns.iter().any(|column| {
        matches!(
            column.entry.logical,
            CoveLogicalType::Utf8 | CoveLogicalType::Binary
        ) && column.entry.physical == CovePhysicalKind::VarBytes
    }) {
        notes.push(
            "Some Utf8/Binary columns stayed VarBytes because dictionary synthesis was not smaller or was disabled"
                .into(),
        );
    }
    if columns.iter().any(|column| !column.notes.is_empty()) {
        notes.push(
            "One or more columns required source-unit normalization during conversion".into(),
        );
    }
    let sidecars = build_optional_sidecars(&cove_bytes, &validated, options, row_count)?;
    if sidecars.covx_bytes.is_some() {
        notes.push("Emitted COVX accelerator sidecar metadata".into());
    }
    if sidecars.covm_bytes.is_some() {
        notes.push("Emitted COVM dataset manifest metadata".into());
    }

    let mut plan = vec![
        ConversionStep::DecodeSource,
        ConversionStep::PartitionSegments,
    ];
    if dictionary.is_some() {
        plan.push(ConversionStep::BuildDictionaries);
        plan.push(ConversionStep::ChooseFileOrNumCode);
    }
    if options.stats_policy == ParquetStatsPolicy::Recompute {
        plan.push(ConversionStep::RecomputeStats);
    }
    if domain_count != 0
        || !acceleration.exact_sets.is_empty()
        || !acceleration.blooms.is_empty()
        || !acceleration.lookups.is_empty()
        || !acceleration.aggregates.is_empty()
        || !acceleration.composites.is_empty()
        || !acceleration.topn.is_empty()
    {
        plan.push(ConversionStep::BuildDomainsAndIndexes);
    }
    plan.push(ConversionStep::EncodePages);
    plan.push(ConversionStep::WriteSections);
    if sidecars.covx_bytes.is_some() || sidecars.covm_bytes.is_some() {
        plan.push(ConversionStep::EmitOptionalCovmCovx);
    }
    validate_plan(&plan)?;

    Ok(ParquetConversionResult {
        cove_bytes,
        covx_bytes: sidecars.covx_bytes,
        covm_bytes: sidecars.covm_bytes,
        report: ParquetConversionReport {
            table_name: options.table_name.clone(),
            namespace: options.namespace.clone(),
            row_count,
            segment_count: u32::try_from(segment_layouts.len())
                .map_err(|_| CoveError::ArithOverflow)?,
            column_count: columns.len() as u32,
            required_features: validated.validated.header.required_features,
            optional_features: validated.validated.header.optional_features,
            plan,
            source_schema_fingerprint,
            target_schema_fingerprint,
            validation_result: true,
            generated_section_kinds,
            unsupported_features,
            lossy_features,
            nested_shape_fallbacks: columns
                .iter()
                .filter(|column| column.fallback.is_some())
                .map(|column| {
                    format!(
                        "{}: {:?} fallback is pushdown-limited",
                        column.entry.name,
                        column.fallback.unwrap()
                    )
                })
                .collect(),
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
    NestedJson,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MaterializedValues {
    Boolean(Vec<u8>),
    FileCode(Vec<u32>),
    NumCode(Vec<u64>),
    VarBytes(Vec<Vec<u8>>),
    FixedBytes { width: usize, values: Vec<Vec<u8>> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SegmentLayout {
    segment_id: u32,
    row_start: usize,
    row_count: usize,
}

fn build_segment_layouts(
    total_rows: usize,
    segment_row_count: u32,
) -> Result<Vec<SegmentLayout>, CoveError> {
    if segment_row_count == 0 {
        return Err(CoveError::BadSchema(
            "segment_row_count must be greater than zero".into(),
        ));
    }
    if total_rows == 0 {
        return Ok(vec![SegmentLayout {
            segment_id: 0,
            row_start: 0,
            row_count: 0,
        }]);
    }
    let step = segment_row_count as usize;
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < total_rows {
        let len = (total_rows - start).min(step);
        out.push(SegmentLayout {
            segment_id: u32::try_from(out.len()).map_err(|_| CoveError::ArithOverflow)?,
            row_start: start,
            row_count: len,
        });
        start = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    }
    Ok(out)
}

impl MaterializedValues {
    fn row_count(&self) -> usize {
        match self {
            MaterializedValues::Boolean(values) => values.len(),
            MaterializedValues::FileCode(values) => values.len(),
            MaterializedValues::NumCode(values) => values.len(),
            MaterializedValues::VarBytes(values) => values.len(),
            MaterializedValues::FixedBytes { values, .. } => values.len(),
        }
    }

    fn encode_rows(&self, start: usize, len: usize) -> Result<Vec<u8>, CoveError> {
        match self {
            MaterializedValues::Boolean(values) => Ok(values[start..start + len].to_vec()),
            MaterializedValues::FileCode(values) => {
                let mut out = Vec::with_capacity(len * 4);
                for value in &values[start..start + len] {
                    out.extend_from_slice(&value.to_le_bytes());
                }
                Ok(out)
            }
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

    fn reorder(&mut self, order: &[usize]) {
        match self {
            MaterializedValues::Boolean(values) => reorder_copy(values, order),
            MaterializedValues::FileCode(values) => reorder_copy(values, order),
            MaterializedValues::NumCode(values) => reorder_copy(values, order),
            MaterializedValues::VarBytes(values) => reorder_clone(values, order),
            MaterializedValues::FixedBytes { values, .. } => reorder_clone(values, order),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConvertedColumn {
    entry: ColumnEntry,
    source_kind: SourceColumnKind,
    source_type: String,
    encoding: CoveEncodingKind,
    fallback: Option<UnsupportedNestedFallback>,
    pushdown_limited: bool,
    notes: Vec<String>,
    values: MaterializedValues,
    nulls: Vec<bool>,
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
                other if is_nested_arrow_type(other) => (
                    CoveLogicalType::Json,
                    CovePhysicalKind::VarBytes,
                    SourceColumnKind::NestedJson,
                    MaterializedValues::VarBytes(Vec::new()),
                    0,
                    0,
                    vec![format!(
                        "nested Parquet source type {other:?} encoded as opaque JSON fallback; pushdown-limited"
                    )],
                ),
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
            fallback: is_nested_arrow_type(field.data_type())
                .then_some(UnsupportedNestedFallback::Json),
            pushdown_limited: is_nested_arrow_type(field.data_type()),
            notes,
            values,
            nulls: Vec::new(),
        })
    }

    fn append_array(&mut self, array: &dyn Array) -> Result<(), CoveError> {
        if array.null_count() != 0 && !self.entry.nullable {
            return Err(CoveError::BadSchema(format!(
                "Parquet source produced null values for non-nullable column '{}'",
                self.entry.name
            )));
        }

        match self.source_kind {
            SourceColumnKind::Boolean => {
                let array = downcast_array::<BooleanArray>(array, &self.entry.name)?;
                let values = expect_boolean_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(u8::from(array.value(row))),
                )?;
            }
            SourceColumnKind::Int8 => {
                let array = downcast_array::<Int8Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row) as i64 as u64),
                )?;
            }
            SourceColumnKind::Int16 => {
                let array = downcast_array::<Int16Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row) as i64 as u64),
                )?;
            }
            SourceColumnKind::Int32 => {
                let array = downcast_array::<Int32Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row) as i64 as u64),
                )?;
            }
            SourceColumnKind::Int64 => {
                let array = downcast_array::<Int64Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row) as u64),
                )?;
            }
            SourceColumnKind::UInt8 => {
                let array = downcast_array::<UInt8Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row) as u64),
                )?;
            }
            SourceColumnKind::UInt16 => {
                let array = downcast_array::<UInt16Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row) as u64),
                )?;
            }
            SourceColumnKind::UInt32 => {
                let array = downcast_array::<UInt32Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row) as u64),
                )?;
            }
            SourceColumnKind::UInt64 => {
                let array = downcast_array::<UInt64Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row)),
                )?;
            }
            SourceColumnKind::Float32 => {
                let array = downcast_array::<Float32Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row).to_bits() as u64),
                )?;
            }
            SourceColumnKind::Float64 => {
                let array = downcast_array::<Float64Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row).to_bits()),
                )?;
            }
            SourceColumnKind::Date32 => {
                let array = downcast_array::<Date32Array>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row) as i64 as u64),
                )?;
            }
            SourceColumnKind::TimestampSecond => {
                let array = downcast_array::<TimestampSecondArray>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| {
                        Ok(array
                            .value(row)
                            .checked_mul(1_000_000)
                            .ok_or(CoveError::ArithOverflow)? as u64)
                    },
                )?;
            }
            SourceColumnKind::TimestampMillisecond => {
                let array = downcast_array::<TimestampMillisecondArray>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| {
                        Ok(array
                            .value(row)
                            .checked_mul(1_000)
                            .ok_or(CoveError::ArithOverflow)? as u64)
                    },
                )?;
            }
            SourceColumnKind::TimestampMicrosecond => {
                let array = downcast_array::<TimestampMicrosecondArray>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row) as u64),
                )?;
            }
            SourceColumnKind::TimestampNanosecond => {
                let array = downcast_array::<TimestampNanosecondArray>(array, &self.entry.name)?;
                let values = expect_numcode_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    0,
                    |row| array.is_null(row),
                    |row| Ok(array.value(row) as u64),
                )?;
            }
            SourceColumnKind::Utf8 => {
                let array = downcast_array::<StringArray>(array, &self.entry.name)?;
                let values = expect_varbytes_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    Vec::new(),
                    |row| array.is_null(row),
                    |row| Ok(array.value(row).as_bytes().to_vec()),
                )?;
            }
            SourceColumnKind::LargeUtf8 => {
                let array = downcast_array::<LargeStringArray>(array, &self.entry.name)?;
                let values = expect_varbytes_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    Vec::new(),
                    |row| array.is_null(row),
                    |row| Ok(array.value(row).as_bytes().to_vec()),
                )?;
            }
            SourceColumnKind::Binary => {
                let array = downcast_array::<BinaryArray>(array, &self.entry.name)?;
                let values = expect_varbytes_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    Vec::new(),
                    |row| array.is_null(row),
                    |row| Ok(array.value(row).to_vec()),
                )?;
            }
            SourceColumnKind::LargeBinary => {
                let array = downcast_array::<LargeBinaryArray>(array, &self.entry.name)?;
                let values = expect_varbytes_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    Vec::new(),
                    |row| array.is_null(row),
                    |row| Ok(array.value(row).to_vec()),
                )?;
            }
            SourceColumnKind::Decimal128 => {
                let array = downcast_array::<Decimal128Array>(array, &self.entry.name)?;
                let values = expect_fixed_values(&mut self.values, 16)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    vec![0u8; 16],
                    |row| array.is_null(row),
                    |row| Ok(array.value(row).to_le_bytes().to_vec()),
                )?;
            }
            SourceColumnKind::NestedJson => {
                let values = expect_varbytes_values(&mut self.values)?;
                append_materialized_values(
                    array.len(),
                    values,
                    &mut self.nulls,
                    b"null".to_vec(),
                    |row| array.is_null(row),
                    |row| {
                        serde_json::to_vec(&arrow_value_to_json(array, row)?).map_err(|error| {
                            CoveError::BadSection(format!("JSON fallback encode failed: {error}"))
                        })
                    },
                )?;
            }
        }
        if self.values.row_count() != self.nulls.len() {
            return Err(CoveError::BadSchema(format!(
                "column '{}' materialized row/null counts diverged",
                self.entry.name
            )));
        }
        Ok(())
    }

    fn page_specs_range(
        &self,
        row_start: usize,
        row_count: usize,
        morsel_row_count: u32,
        compression: CompressionCodec,
    ) -> Result<Vec<ScanPageSpec>, CoveError> {
        if morsel_row_count == 0 {
            return Err(CoveError::BadSchema(
                "morsel_row_count must be greater than zero".into(),
            ));
        }
        let total_rows = self.values.row_count();
        let row_end = row_start
            .checked_add(row_count)
            .ok_or(CoveError::ArithOverflow)?;
        if row_end > total_rows {
            return Err(CoveError::BadSchema(format!(
                "column '{}' page range exceeds materialized rows",
                self.entry.name
            )));
        }
        if row_count == 0 {
            return Ok(Vec::new());
        }
        if self.nulls.len() != total_rows {
            return Err(CoveError::BadSchema(format!(
                "column '{}' materialized row/null counts diverged",
                self.entry.name
            )));
        }
        let mut pages = Vec::new();
        let mut start = row_start;
        let step = morsel_row_count as usize;
        while start < row_end {
            let len = (row_end - start).min(step);
            let physical_payload = self.values.encode_rows(start, len)?;
            let null_count = self.null_count_range(start, len)?;
            let (payload, flags) = if null_count == 0 {
                (physical_payload, PAGE_FLAG_ALL_NON_NULL)
            } else {
                let mut payload = self.validity_bytes(start, len)?;
                let capacity = payload
                    .len()
                    .checked_add(physical_payload.len())
                    .ok_or(CoveError::ArithOverflow)?;
                payload.reserve(capacity.saturating_sub(payload.len()));
                payload.extend_from_slice(&physical_payload);
                let flags = if null_count == len {
                    PAGE_FLAG_ALL_NULL
                } else {
                    0
                };
                (payload, flags)
            };
            let non_null_count = len
                .checked_sub(null_count)
                .ok_or(CoveError::ArithOverflow)?;
            pages.push(
                ScanPageSpec::new(len as u32, payload)
                    .with_compression(compression)
                    .with_encoding_root(self.encoding as u32)
                    .with_counts(non_null_count as u32, null_count as u32)
                    .with_flags(flags),
            );
            start += len;
        }
        Ok(pages)
    }

    fn key_u64(&self, row: usize) -> Option<(u64, IndexKeyKind)> {
        if self.is_null(row) {
            return None;
        }
        match &self.values {
            MaterializedValues::Boolean(values) => values
                .get(row)
                .map(|value| (u64::from(*value != 0), IndexKeyKind::NumCode)),
            MaterializedValues::FileCode(values) => values
                .get(row)
                .map(|value| (u64::from(*value), IndexKeyKind::FileCode)),
            MaterializedValues::NumCode(values) => {
                values.get(row).map(|value| (*value, IndexKeyKind::NumCode))
            }
            MaterializedValues::VarBytes(_) | MaterializedValues::FixedBytes { .. } => None,
        }
    }

    fn key_kind(&self) -> Option<IndexKeyKind> {
        match &self.values {
            MaterializedValues::Boolean(_) | MaterializedValues::NumCode(_) => {
                Some(IndexKeyKind::NumCode)
            }
            MaterializedValues::FileCode(_) => Some(IndexKeyKind::FileCode),
            MaterializedValues::VarBytes(_) | MaterializedValues::FixedBytes { .. } => None,
        }
    }

    fn key_bytes_for_bloom(&self, row: usize) -> Option<(Vec<u8>, BloomHashDomain)> {
        let (key, kind) = self.key_u64(row)?;
        match kind {
            IndexKeyKind::FileCode => Some((
                (key as u32).to_le_bytes().to_vec(),
                BloomHashDomain::FileCode,
            )),
            IndexKeyKind::NumCode => Some((key.to_le_bytes().to_vec(), BloomHashDomain::NumCode)),
        }
    }

    fn dictionary_key_for_row(&self, row: usize) -> Result<Option<FileDictionaryKey>, CoveError> {
        if self.is_null(row) {
            return Ok(None);
        }
        let MaterializedValues::VarBytes(values) = &self.values else {
            return Ok(None);
        };
        let Some(value) = values.get(row) else {
            return Ok(None);
        };
        Ok(Some(FileDictionaryKey::from_logical_bytes(
            self.entry.logical,
            value,
        )?))
    }

    fn compare_rows_for_cluster(&self, left: usize, right: usize) -> Ordering {
        match (self.is_null(left), self.is_null(right)) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => {}
        }
        match &self.values {
            MaterializedValues::Boolean(values) => values[left].cmp(&values[right]),
            MaterializedValues::FileCode(values) => values[left].cmp(&values[right]),
            MaterializedValues::NumCode(values) => compare_numcode_rows(
                self.source_kind,
                self.entry.logical,
                values[left],
                values[right],
            ),
            MaterializedValues::VarBytes(values) => values[left].cmp(&values[right]),
            MaterializedValues::FixedBytes { values, .. } => values[left].cmp(&values[right]),
        }
    }

    fn is_null(&self, row: usize) -> bool {
        self.nulls.get(row).copied().unwrap_or(false)
    }

    fn null_count_range(&self, start: usize, len: usize) -> Result<usize, CoveError> {
        let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        let slice = self
            .nulls
            .get(start..end)
            .ok_or_else(|| CoveError::BadSchema("null bitmap range exceeds column rows".into()))?;
        Ok(slice.iter().filter(|is_null| **is_null).count())
    }

    fn non_null_indices(&self, start: usize, len: usize) -> Result<Vec<usize>, CoveError> {
        let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        self.nulls
            .get(start..end)
            .ok_or_else(|| CoveError::BadSchema("null bitmap range exceeds column rows".into()))?;
        Ok((start..end).filter(|row| !self.is_null(*row)).collect())
    }

    fn validity_bytes(&self, start: usize, len: usize) -> Result<Vec<u8>, CoveError> {
        let row_count = u64::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
        let mut builder = ValidityBitmapBuilder::new(row_count)?;
        for relative_row in 0..len {
            if self.is_null(start + relative_row) {
                builder.set_null(relative_row as u64)?;
            }
        }
        Ok(builder.into_bytes())
    }

    fn report(self) -> ParquetColumnReport {
        ParquetColumnReport {
            column_id: self.entry.column_id,
            name: self.entry.name,
            source_type: self.source_type,
            logical: self.entry.logical,
            physical: self.entry.physical,
            nullable: self.entry.nullable,
            pushdown_limited: self.pushdown_limited,
            fallback: self.fallback,
            notes: self.notes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexKeyKind {
    FileCode,
    NumCode,
}

impl IndexKeyKind {
    fn exact_set_kind(self) -> ExactSetKeyKind {
        match self {
            Self::FileCode => ExactSetKeyKind::FileCode,
            Self::NumCode => ExactSetKeyKind::NumCode,
        }
    }

    fn lookup_kind(self) -> LookupKeyKind {
        match self {
            Self::FileCode => LookupKeyKind::FileCode,
            Self::NumCode => LookupKeyKind::NumCode,
        }
    }
}

fn apply_stable_clustering(
    columns: &mut [ConvertedColumn],
    options: &ParquetConversionOptions,
) -> Result<Option<String>, CoveError> {
    if options.clustering_policy == ParquetClusteringPolicy::PreserveSourceOrder {
        if options.cluster_columns.is_empty() {
            return Ok(None);
        }
        return Ok(Some(
            "Cluster columns were declared, but stable clustering was not enabled; source row order was preserved"
                .into(),
        ));
    }
    if options.cluster_columns.is_empty() {
        return Ok(Some(
            "Stable clustering was requested without declared cluster columns; source row order was preserved"
                .into(),
        ));
    }

    let mut cluster_indices = Vec::with_capacity(options.cluster_columns.len());
    for name in &options.cluster_columns {
        let Some(index) = columns.iter().position(|column| column.entry.name == *name) else {
            return Err(CoveError::BadSchema(format!(
                "stable clustering references unknown column '{name}'"
            )));
        };
        cluster_indices.push(index);
    }

    let row_count = columns
        .first()
        .map(|column| column.values.row_count())
        .unwrap_or(0);
    let mut order = (0..row_count).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        for index in &cluster_indices {
            let ordering = columns[*index].compare_rows_for_cluster(*left, *right);
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        left.cmp(right)
    });
    for column in columns {
        column.values.reorder(&order);
        reorder_copy(&mut column.nulls, &order);
    }
    Ok(Some(format!(
        "Applied stable clustering by declared columns: {}",
        options.cluster_columns.join(",")
    )))
}

fn apply_dictionary_synthesis(
    columns: &mut [ConvertedColumn],
    policy: ParquetDictionaryPolicy,
) -> Result<Option<FileDictionary>, CoveError> {
    if policy == ParquetDictionaryPolicy::Never {
        return Ok(None);
    }

    let mut selected = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        if !matches!(
            column.entry.logical,
            CoveLogicalType::Utf8 | CoveLogicalType::Binary
        ) || !matches!(column.values, MaterializedValues::VarBytes(_))
        {
            continue;
        }
        let unique = dictionary_unique_keys(column)?;
        let raw_len = varbytes_payload_len(column)?;
        let dict_len = file_dictionary_candidate_len(&unique, column.values.row_count())?;
        if policy == ParquetDictionaryPolicy::Always || dict_len < raw_len {
            selected.push(index);
        }
    }
    if selected.is_empty() {
        return Ok(None);
    }

    let mut all_keys = BTreeSet::new();
    for index in &selected {
        all_keys.extend(dictionary_unique_keys(&columns[*index])?);
    }
    let encoding = FileDictionaryEncoding::from_keys(all_keys)?;

    for index in selected {
        let row_count = columns[index].values.row_count();
        let mut codes = Vec::with_capacity(row_count);
        for row in 0..row_count {
            if columns[index].is_null(row) {
                codes.push(0);
                continue;
            }
            let key = columns[index]
                .dictionary_key_for_row(row)?
                .ok_or_else(|| CoveError::BadSchema("missing dictionary key".into()))?;
            codes.push(encoding.file_code_for_key(&key)?);
        }
        columns[index].values = MaterializedValues::FileCode(codes);
        columns[index].entry.physical = CovePhysicalKind::FileCode;
        columns[index].encoding = CoveEncodingKind::FileCode;
        columns[index]
            .notes
            .push("encoded as deterministic FileCode dictionary codes".into());
    }

    Ok(Some(encoding.dictionary))
}

fn dictionary_unique_keys(
    column: &ConvertedColumn,
) -> Result<BTreeSet<FileDictionaryKey>, CoveError> {
    let mut keys = BTreeSet::new();
    for row in 0..column.values.row_count() {
        if let Some(key) = column.dictionary_key_for_row(row)? {
            keys.insert(key);
        }
    }
    Ok(keys)
}

fn varbytes_payload_len(column: &ConvertedColumn) -> Result<usize, CoveError> {
    let MaterializedValues::VarBytes(values) = &column.values else {
        return Err(CoveError::BadSchema(
            "expected VarBytes column for dictionary sizing".into(),
        ));
    };
    values.iter().try_fold(0usize, |total, value| {
        total
            .checked_add(4)
            .and_then(|total| total.checked_add(value.len()))
            .ok_or(CoveError::ArithOverflow)
    })
}

fn build_column_domains(
    columns: &[ConvertedColumn],
    dictionary_entry_count: Option<u32>,
) -> Result<Vec<ColumnDomain>, CoveError> {
    let Some(dictionary_entry_count) = dictionary_entry_count else {
        return Ok(Vec::new());
    };
    let mut domains = Vec::new();
    for column in columns {
        let MaterializedValues::FileCode(codes) = &column.values else {
            continue;
        };
        let sorted_codes = codes
            .iter()
            .enumerate()
            .filter_map(|(row, code)| (!column.is_null(row)).then_some(*code))
            .collect::<BTreeSet<_>>();
        if sorted_codes.is_empty() {
            continue;
        }
        let domain = ColumnDomain::from_sorted_present_codes(
            &sorted_codes.into_iter().collect::<Vec<_>>(),
            dictionary_entry_count,
            1,
            column.entry.column_id,
            column.entry.logical as u16,
            column.entry.collation_id,
            0,
        )?;
        domains.push(domain);
    }
    Ok(domains)
}

fn build_zone_stats(
    columns: &[ConvertedColumn],
    segments: &[SegmentLayout],
    morsel_row_count: u32,
) -> Result<Option<ZoneStatsSection>, CoveError> {
    let mut entries = Vec::new();
    for column in columns {
        for segment in segments {
            let row_end = segment
                .row_start
                .checked_add(segment.row_count)
                .ok_or(CoveError::ArithOverflow)?;
            let mut start = segment.row_start;
            let mut morsel_id = 0u32;
            while start < row_end {
                let len = (row_end - start).min(morsel_row_count as usize);
                if let Some(entry) =
                    build_zone_stats_entry(column, start, len, segment.segment_id, morsel_id)?
                {
                    entries.push(entry);
                }
                start = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
                morsel_id = morsel_id.checked_add(1).ok_or(CoveError::ArithOverflow)?;
            }
        }
    }
    if entries.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ZoneStatsSection { entries }))
    }
}

fn build_zone_stats_entry(
    column: &ConvertedColumn,
    start: usize,
    len: usize,
    segment_id: u32,
    morsel_id: u32,
) -> Result<Option<ZoneStatsEntry>, CoveError> {
    if len == 0 {
        return Ok(None);
    }
    let null_count = column.null_count_range(start, len)?;
    if null_count == len {
        return Ok(Some(zone_entry(
            column,
            len,
            null_count,
            segment_id,
            morsel_id,
            0,
            0,
            ZoneStats {
                scope: ZoneScope::Morsel,
                row_count: len as u64,
                null_count: null_count as u64,
                min: None,
                max: None,
                flags: ZoneStatFlags::empty(),
            },
            u32::MAX,
            u32::MAX,
        )));
    }
    if let Some((min_rank, max_rank, distinct_count, run_count, constant)) =
        filecode_domain_stats(column, start, len)?
    {
        let flags = ZoneStatFlags::HAS_DOMAIN_RANGE
            | ZoneStatFlags::DISTINCT_EXACT
            | if constant {
                ZoneStatFlags::CONSTANT
            } else {
                ZoneStatFlags::empty()
            };
        return Ok(Some(zone_entry(
            column,
            len,
            null_count,
            segment_id,
            morsel_id,
            distinct_count,
            run_count,
            ZoneStats {
                scope: ZoneScope::Morsel,
                row_count: len as u64,
                null_count: null_count as u64,
                min: None,
                max: None,
                flags,
            },
            min_rank,
            max_rank,
        )));
    }

    let Some((kind, min, max, distinct_count, run_count, mut flags)) =
        scalar_min_max_stats(column, start, len)?
    else {
        return Ok(None);
    };
    flags = flags | ZoneStatFlags::HAS_MIN_MAX | ZoneStatFlags::DISTINCT_EXACT;
    if distinct_count == 1 {
        flags = flags | ZoneStatFlags::CONSTANT;
    }
    Ok(Some(zone_entry(
        column,
        len,
        null_count,
        segment_id,
        morsel_id,
        distinct_count,
        run_count,
        ZoneStats {
            scope: ZoneScope::Morsel,
            row_count: len as u64,
            null_count: null_count as u64,
            min: Some(StatScalar {
                kind,
                bytes: min,
                truncated: false,
            }),
            max: Some(StatScalar {
                kind,
                bytes: max,
                truncated: false,
            }),
            flags,
        },
        u32::MAX,
        u32::MAX,
    )))
}

fn zone_entry(
    column: &ConvertedColumn,
    row_count: usize,
    null_count: usize,
    segment_id: u32,
    morsel_id: u32,
    distinct_count: u32,
    run_count: u32,
    stats: ZoneStats,
    min_domain_rank: u32,
    max_domain_rank: u32,
) -> ZoneStatsEntry {
    ZoneStatsEntry {
        table_id: 1,
        segment_id,
        morsel_id,
        column_id: column.entry.column_id,
        non_null_count: row_count.saturating_sub(null_count) as u32,
        distinct_count,
        run_count,
        stats,
        min_domain_rank,
        max_domain_rank,
        exact_set_ref: 0,
        bloom_ref: 0,
    }
}

fn filecode_domain_stats(
    column: &ConvertedColumn,
    start: usize,
    len: usize,
) -> Result<Option<(u32, u32, u32, u32, bool)>, CoveError> {
    let MaterializedValues::FileCode(values) = &column.values else {
        return Ok(None);
    };
    let all_codes = values
        .iter()
        .enumerate()
        .filter_map(|(row, code)| (!column.is_null(row)).then_some(*code))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let rows = column.non_null_indices(start, len)?;
    if rows.is_empty() {
        return Ok(None);
    }
    let slice = rows.iter().map(|row| values[*row]).collect::<Vec<_>>();
    let min_code = *slice.iter().min().ok_or(CoveError::BadStats)?;
    let max_code = *slice.iter().max().ok_or(CoveError::BadStats)?;
    let min_rank = all_codes
        .binary_search(&min_code)
        .map_err(|_| CoveError::BadDomain)? as u32;
    let max_rank = all_codes
        .binary_search(&max_code)
        .map_err(|_| CoveError::BadDomain)? as u32;
    let distinct_count = u32::try_from(slice.iter().copied().collect::<BTreeSet<_>>().len())
        .map_err(|_| CoveError::ArithOverflow)?;
    let run_count = run_count_u32(slice.iter().copied())?;
    Ok(Some((
        min_rank,
        max_rank,
        distinct_count,
        run_count,
        distinct_count == 1,
    )))
}

fn scalar_min_max_stats(
    column: &ConvertedColumn,
    start: usize,
    len: usize,
) -> Result<Option<(StatKind, Vec<u8>, Vec<u8>, u32, u32, ZoneStatFlags)>, CoveError> {
    let rows = column.non_null_indices(start, len)?;
    if rows.is_empty() {
        return Ok(None);
    }
    match (&column.values, column.entry.logical) {
        (MaterializedValues::Boolean(values), CoveLogicalType::Bool) => {
            let slice = rows.iter().map(|row| values[*row]).collect::<Vec<_>>();
            let min = u64::from(*slice.iter().min().ok_or(CoveError::BadStats)? != 0);
            let max = u64::from(*slice.iter().max().ok_or(CoveError::BadStats)? != 0);
            Ok(Some((
                StatKind::UInt64,
                min.to_le_bytes().to_vec(),
                max.to_le_bytes().to_vec(),
                u32::try_from(slice.iter().copied().collect::<BTreeSet<_>>().len())
                    .map_err(|_| CoveError::ArithOverflow)?,
                run_count_u32(slice.iter().copied())?,
                ZoneStatFlags::empty(),
            )))
        }
        (MaterializedValues::NumCode(values), logical) => {
            numcode_min_max_stats(values, logical, column.source_kind, &rows)
        }
        (MaterializedValues::FixedBytes { values, width: 16 }, CoveLogicalType::Decimal128) => {
            let mut decoded = Vec::with_capacity(rows.len());
            for row in &rows {
                let value = &values[*row];
                let raw: [u8; 16] = value.as_slice().try_into().map_err(|_| {
                    CoveError::BadSchema("decimal128 fixed value must be 16 bytes".into())
                })?;
                decoded.push(i128::from_le_bytes(raw));
            }
            let min = *decoded.iter().min().ok_or(CoveError::BadStats)?;
            let max = *decoded.iter().max().ok_or(CoveError::BadStats)?;
            Ok(Some((
                StatKind::Decimal128,
                min.to_le_bytes().to_vec(),
                max.to_le_bytes().to_vec(),
                u32::try_from(decoded.iter().copied().collect::<BTreeSet<_>>().len())
                    .map_err(|_| CoveError::ArithOverflow)?,
                run_count_u32(decoded.iter().copied())?,
                ZoneStatFlags::empty(),
            )))
        }
        _ => Ok(None),
    }
}

fn numcode_min_max_stats(
    values: &[u64],
    logical: CoveLogicalType,
    source_kind: SourceColumnKind,
    rows: &[usize],
) -> Result<Option<(StatKind, Vec<u8>, Vec<u8>, u32, u32, ZoneStatFlags)>, CoveError> {
    let slice = rows.iter().map(|row| values[*row]).collect::<Vec<_>>();
    match logical {
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64 => {
            let decoded = slice
                .iter()
                .map(|value| signed_numcode(logical, *value))
                .collect::<Vec<_>>();
            let min = *decoded.iter().min().ok_or(CoveError::BadStats)?;
            let max = *decoded.iter().max().ok_or(CoveError::BadStats)?;
            Ok(Some((
                StatKind::Int64,
                min.to_le_bytes().to_vec(),
                max.to_le_bytes().to_vec(),
                distinct_len(&decoded)?,
                run_count_u32(decoded.iter().copied())?,
                ZoneStatFlags::empty(),
            )))
        }
        CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64 => {
            let min = *slice.iter().min().ok_or(CoveError::BadStats)?;
            let max = *slice.iter().max().ok_or(CoveError::BadStats)?;
            Ok(Some((
                StatKind::UInt64,
                min.to_le_bytes().to_vec(),
                max.to_le_bytes().to_vec(),
                distinct_len(&slice)?,
                run_count_u32(slice.iter().copied())?,
                ZoneStatFlags::empty(),
            )))
        }
        CoveLogicalType::Float32 | CoveLogicalType::Float64 => {
            let mut decoded = Vec::new();
            let mut has_nan = false;
            for value in &slice {
                let value = if source_kind == SourceColumnKind::Float32 {
                    f32::from_bits(*value as u32) as f64
                } else {
                    f64::from_bits(*value)
                };
                if value.is_nan() {
                    has_nan = true;
                } else {
                    decoded.push(value);
                }
            }
            if decoded.is_empty() {
                return Ok(None);
            }
            decoded.sort_by(f64::total_cmp);
            let min = decoded[0];
            let max = decoded[decoded.len() - 1];
            let flags = if has_nan {
                ZoneStatFlags::HAS_NAN
            } else {
                ZoneStatFlags::empty()
            };
            Ok(Some((
                StatKind::Float64Bits,
                min.to_bits().to_le_bytes().to_vec(),
                max.to_bits().to_le_bytes().to_vec(),
                u32::try_from(decoded.len()).map_err(|_| CoveError::ArithOverflow)?,
                u32::try_from(slice.len()).map_err(|_| CoveError::ArithOverflow)?,
                flags,
            )))
        }
        CoveLogicalType::DateDays => {
            let decoded = slice
                .iter()
                .map(|value| types::numcode_as_date_days(*value))
                .collect::<Vec<_>>();
            let min = *decoded.iter().min().ok_or(CoveError::BadStats)?;
            let max = *decoded.iter().max().ok_or(CoveError::BadStats)?;
            Ok(Some((
                StatKind::DateDays,
                min.to_le_bytes().to_vec(),
                max.to_le_bytes().to_vec(),
                distinct_len(&decoded)?,
                run_count_u32(decoded.iter().copied())?,
                ZoneStatFlags::empty(),
            )))
        }
        CoveLogicalType::TimestampMicros | CoveLogicalType::TimestampNanos => {
            let decoded = slice.iter().map(|value| *value as i64).collect::<Vec<_>>();
            let min = *decoded.iter().min().ok_or(CoveError::BadStats)?;
            let max = *decoded.iter().max().ok_or(CoveError::BadStats)?;
            Ok(Some((
                if logical == CoveLogicalType::TimestampMicros {
                    StatKind::TimestampMicros
                } else {
                    StatKind::TimestampNanos
                },
                min.to_le_bytes().to_vec(),
                max.to_le_bytes().to_vec(),
                distinct_len(&decoded)?,
                run_count_u32(decoded.iter().copied())?,
                ZoneStatFlags::empty(),
            )))
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Default)]
struct AccelerationArtifacts {
    exact_sets: Vec<ExactSetIndex>,
    blooms: Vec<BloomFilterIndex>,
    lookups: Vec<LookupIndex>,
    aggregates: Vec<AggregateSynopsis>,
    composites: Vec<CompositeIndex>,
    topn: Vec<TopNSummary>,
    notes: Vec<String>,
    unsupported: Vec<String>,
}

fn build_acceleration_artifacts(
    columns: &[ConvertedColumn],
    options: &ParquetConversionOptions,
    segments: &[SegmentLayout],
) -> Result<AccelerationArtifacts, CoveError> {
    let mut artifacts = AccelerationArtifacts::default();
    let row_count = columns
        .first()
        .map(|column| column.values.row_count())
        .unwrap_or(0);
    let point_lookup = options
        .point_lookup_columns
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let topn = options
        .topn_columns
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let aggregate_columns = options
        .aggregate_columns
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    validate_declared_columns(columns, &point_lookup, "point lookup")?;
    validate_declared_columns(columns, &topn, "Top-N")?;
    validate_declared_columns(columns, &aggregate_columns, "aggregate synopsis")?;
    for group in &options.composite_zone_groups {
        let declared = group.iter().cloned().collect::<BTreeSet<_>>();
        validate_declared_columns(columns, &declared, "composite zone")?;
        if group.len() < 2 {
            return Err(CoveError::BadSchema(
                "composite-zone groups require at least two columns".into(),
            ));
        }
    }

    for column in columns {
        let key_kind = column.key_kind();
        let unique_keys = column_unique_keys(column)?;
        let low_or_medium = !unique_keys.is_empty()
            && (unique_keys.len() <= 4096 || unique_keys.len().saturating_mul(2) <= row_count);
        let declared_lookup = point_lookup.contains(&column.entry.name);

        if should_emit_exact_set(options.acceleration_policy, declared_lookup, low_or_medium)
            && key_kind.is_some()
        {
            artifacts
                .exact_sets
                .push(build_exact_set(column, &unique_keys, key_kind.unwrap())?);
        }
        if declared_lookup {
            if let Some(kind) = key_kind {
                artifacts.lookups.push(build_lookup_index(
                    column,
                    kind,
                    segments,
                    options.morsel_row_count,
                )?);
                if !low_or_medium && !unique_keys.is_empty() {
                    artifacts.blooms.push(build_bloom_index(column)?);
                }
            } else {
                artifacts.unsupported.push(format!(
                    "Point-lookup index for column '{}' requires FileCode, NumCode, or Boolean materialization",
                    column.entry.name
                ));
            }
        }
        if topn.contains(&column.entry.name) {
            if key_kind.is_some() {
                artifacts
                    .topn
                    .extend(build_topn_summaries(column, segments)?);
            } else {
                artifacts.unsupported.push(format!(
                    "Top-N summary for column '{}' requires FileCode, NumCode, or Boolean materialization",
                    column.entry.name
                ));
            }
        }
        if should_emit_aggregate(
            options.aggregate_policy,
            aggregate_columns.contains(&column.entry.name),
            key_kind.is_some(),
        ) {
            if let Some(synopsis) = build_aggregate_synopsis(column, segments)? {
                artifacts.aggregates.push(synopsis);
            } else {
                artifacts.unsupported.push(format!(
                    "Aggregate synopsis for column '{}' requires Boolean, FileCode, or NumCode materialization",
                    column.entry.name
                ));
            }
        }
    }

    for group in &options.composite_zone_groups {
        match build_composite_zone_index(columns, group, options.morsel_row_count)? {
            Some(index) => artifacts.composites.push(index),
            None => artifacts.unsupported.push(format!(
                "Composite zone group '{}' requires FileCode, NumCode, or Boolean materialization",
                group.join(",")
            )),
        }
    }

    if !artifacts.exact_sets.is_empty() {
        artifacts.notes.push(format!(
            "Generated {} exact-set index section(s)",
            artifacts.exact_sets.len()
        ));
    }
    if !artifacts.blooms.is_empty() {
        artifacts.notes.push(format!(
            "Generated {} bloom index section(s)",
            artifacts.blooms.len()
        ));
    }
    if !artifacts.lookups.is_empty() {
        artifacts.notes.push(format!(
            "Generated {} lookup index section(s)",
            artifacts.lookups.len()
        ));
    }
    if !artifacts.aggregates.is_empty() {
        artifacts.notes.push(format!(
            "Generated {} aggregate synopsis section(s)",
            artifacts.aggregates.len()
        ));
    }
    if !artifacts.composites.is_empty() {
        artifacts.notes.push(format!(
            "Generated {} composite zone index section(s)",
            artifacts.composites.len()
        ));
    }
    if !artifacts.topn.is_empty() {
        artifacts.notes.push(format!(
            "Generated {} Top-N summary section(s)",
            artifacts.topn.len()
        ));
    }
    Ok(artifacts)
}

fn validate_declared_columns(
    columns: &[ConvertedColumn],
    declared: &BTreeSet<String>,
    label: &str,
) -> Result<(), CoveError> {
    for name in declared {
        if !columns.iter().any(|column| column.entry.name == *name) {
            return Err(CoveError::BadSchema(format!(
                "{label} option references unknown column '{name}'"
            )));
        }
    }
    Ok(())
}

fn should_emit_exact_set(
    policy: ParquetAccelerationPolicy,
    declared_lookup: bool,
    low_or_medium: bool,
) -> bool {
    match policy {
        ParquetAccelerationPolicy::None => false,
        ParquetAccelerationPolicy::DeclaredOnly => declared_lookup && low_or_medium,
        ParquetAccelerationPolicy::Auto => low_or_medium,
    }
}

fn should_emit_aggregate(policy: ParquetAggregatePolicy, declared: bool, supported: bool) -> bool {
    supported
        && match policy {
            ParquetAggregatePolicy::None => false,
            ParquetAggregatePolicy::DeclaredOnly => declared,
            ParquetAggregatePolicy::Auto => true,
        }
}

fn column_unique_keys(column: &ConvertedColumn) -> Result<Vec<u64>, CoveError> {
    let mut keys = BTreeSet::new();
    for row in 0..column.values.row_count() {
        if column.is_null(row) {
            continue;
        }
        let Some((key, _)) = column.key_u64(row) else {
            return Ok(Vec::new());
        };
        keys.insert(key);
    }
    Ok(keys.into_iter().collect())
}

fn build_aggregate_synopsis(
    column: &ConvertedColumn,
    segments: &[SegmentLayout],
) -> Result<Option<AggregateSynopsis>, CoveError> {
    if column.key_kind().is_none() {
        return Ok(None);
    }
    let mut entries = Vec::with_capacity(segments.len());
    for segment in segments {
        let row_end = segment
            .row_start
            .checked_add(segment.row_count)
            .ok_or(CoveError::ArithOverflow)?;
        let row_count = u32::try_from(segment.row_count).map_err(|_| CoveError::ArithOverflow)?;
        let null_count = u32::try_from(
            (segment.row_start..row_end)
                .filter(|row| column.is_null(*row))
                .count(),
        )
        .map_err(|_| CoveError::ArithOverflow)?;
        entries.push(AggregateEntry {
            table_id: 1,
            segment_id: segment.segment_id,
            morsel_id: u32::MAX,
            column_id: column.entry.column_id,
            synopsis_kind: SynopsisKind::Count,
            key_kind: column
                .key_kind()
                .map(|kind| kind.exact_set_kind() as u8)
                .unwrap_or(0),
            accuracy: SynopsisAccuracy::Exact,
            flags: 0,
            row_count,
            null_count,
            payload_offset: 0,
            payload_length: 0,
            checksum: 0,
        });
    }
    Ok(Some(AggregateSynopsis { entries }))
}

fn build_exact_set(
    column: &ConvertedColumn,
    unique_keys: &[u64],
    key_kind: IndexKeyKind,
) -> Result<ExactSetIndex, CoveError> {
    let mut data = Vec::with_capacity(unique_keys.len() * 8);
    for key in unique_keys {
        data.extend_from_slice(&key.to_le_bytes());
    }
    Ok(ExactSetIndex {
        header: ExactSetIndexHeaderV1 {
            table_id: 1,
            column_id: column.entry.column_id,
            granularity: ExactSetGranularity::Morsel,
            key_kind: key_kind.exact_set_kind(),
            representation: ExactSetRepresentation::SortedList,
            flags: 0,
            entry_count: u32::try_from(unique_keys.len()).map_err(|_| CoveError::ArithOverflow)?,
            data_offset: EXACT_SET_HEADER_LEN as u64,
            data_length: data.len() as u64,
            checksum: 0,
        },
        keys: unique_keys.to_vec(),
        data,
    })
}

fn build_composite_zone_index(
    columns: &[ConvertedColumn],
    group: &[String],
    morsel_row_count: u32,
) -> Result<Option<CompositeIndex>, CoveError> {
    let mut selected = Vec::with_capacity(group.len());
    for name in group {
        let Some(column) = columns.iter().find(|column| column.entry.name == *name) else {
            return Err(CoveError::BadSchema(format!(
                "composite-zone option references unknown column '{name}'"
            )));
        };
        if column.key_kind().is_none() {
            return Ok(None);
        }
        selected.push(column);
    }
    let row_count = columns
        .first()
        .map(|column| column.values.row_count())
        .unwrap_or(0);
    if row_count == 0 {
        return Ok(None);
    }
    let mut entries = Vec::new();
    let mut zone_count = 0u32;
    let step = morsel_row_count as usize;
    if step == 0 {
        return Err(CoveError::BadSchema(
            "morsel_row_count must be greater than zero".into(),
        ));
    }
    let mut start = 0usize;
    while start < row_count {
        let len = (row_count - start).min(step);
        entries.extend_from_slice(&zone_count.to_le_bytes());
        entries.extend_from_slice(&(start as u32).to_le_bytes());
        entries.extend_from_slice(&(len as u32).to_le_bytes());
        for column in &selected {
            let mut min = u64::MAX;
            let mut max = 0u64;
            let mut any = false;
            for row in start..start + len {
                if let Some((key, _)) = column.key_u64(row) {
                    min = min.min(key);
                    max = max.max(key);
                    any = true;
                }
            }
            if !any {
                min = 0;
                max = 0;
            }
            entries.extend_from_slice(&min.to_le_bytes());
            entries.extend_from_slice(&max.to_le_bytes());
        }
        zone_count = zone_count.checked_add(1).ok_or(CoveError::ArithOverflow)?;
        start += len;
    }
    Ok(Some(CompositeIndex {
        header: CompositeZoneIndexHeaderV1 {
            table_id: 1,
            key_column_count: selected.len() as u16,
            transform_kind: CompositeTransformKind::Tuple,
            flags: 0,
            zone_count,
            key_columns_offset: 0,
            entries_offset: 0,
            entries_length: 0,
            checksum: 0,
        },
        key_columns: selected
            .iter()
            .map(|column| column.entry.column_id)
            .collect(),
        entries,
    }))
}

fn build_bloom_index(column: &ConvertedColumn) -> Result<BloomFilterIndex, CoveError> {
    let row_count = column.values.row_count().max(1);
    let bits_len =
        (row_count.checked_mul(12).ok_or(CoveError::ArithOverflow)? / 8).clamp(64, 16 * 1024);
    let (_, domain) = (0..column.values.row_count())
        .find_map(|row| column.key_bytes_for_bloom(row))
        .ok_or_else(|| CoveError::BadSchema("column is not bloom-index keyable".into()))?;
    let mut index = BloomFilterIndex {
        header: BloomIndexHeaderV1 {
            table_id: 1,
            column_id: column.entry.column_id,
            granularity: BloomGranularity::Morsel,
            hash_domain: domain,
            algorithm: BloomAlgorithm::SplitBlock,
            flags: 0,
            target_fpr_ppm: 10_000,
            filter_count: 1,
            data_offset: BLOOM_INDEX_HEADER_LEN as u64,
            data_length: bits_len as u64,
            checksum: 0,
        },
        hash_count: 7,
        bits: vec![0u8; bits_len],
    };
    for row in 0..column.values.row_count() {
        if let Some((key, _)) = column.key_bytes_for_bloom(row) {
            index.insert(&key);
        }
    }
    Ok(index)
}

fn build_lookup_index(
    column: &ConvertedColumn,
    key_kind: IndexKeyKind,
    segments: &[SegmentLayout],
    morsel_row_count: u32,
) -> Result<LookupIndex, CoveError> {
    let mut rows_by_key: BTreeMap<u64, Vec<RowRef>> = BTreeMap::new();
    for row in 0..column.values.row_count() {
        if column.is_null(row) {
            continue;
        }
        let (key, _) = column
            .key_u64(row)
            .ok_or_else(|| CoveError::BadSchema("column is not lookup keyable".into()))?;
        let position = segment_position_for_row(row, segments)?;
        let row_in_segment = row
            .checked_sub(position.row_start)
            .ok_or(CoveError::ArithOverflow)?;
        let morsel_id = u32::try_from(row_in_segment / morsel_row_count as usize)
            .map_err(|_| CoveError::ArithOverflow)?;
        let row_in_morsel =
            u16::try_from(row_in_segment % morsel_row_count as usize).map_err(|_| {
                CoveError::BadSchema(
                    "lookup row_in_morsel exceeds u16::MAX; choose a smaller morsel_row_count"
                        .into(),
                )
            })?;
        rows_by_key.entry(key).or_default().push(RowRef {
            table_id: 1,
            segment_id: position.segment_id,
            morsel_id,
            row_in_morsel,
        });
    }
    let uniqueness = if rows_by_key.values().all(|rows| rows.len() == 1) {
        LookupUniqueness::Unique
    } else {
        LookupUniqueness::NonUnique
    };
    Ok(LookupIndex {
        header: LookupIndexHeaderV1 {
            table_id: 1,
            column_id: column.entry.column_id,
            key_kind: key_kind.lookup_kind(),
            index_kind: LookupIndexKind::SparseSorted,
            uniqueness,
            flags: 0,
            entry_count: 0,
            entries_offset: 0,
            entries_length: 0,
            rowref_offset: 0,
            rowref_length: 0,
            checksum: 0,
        },
        entries: rows_by_key
            .into_iter()
            .map(|(key, rows)| LookupEntry { key, rows })
            .collect(),
    })
}

fn segment_position_for_row(
    row: usize,
    segments: &[SegmentLayout],
) -> Result<SegmentLayout, CoveError> {
    for segment in segments {
        let end = segment
            .row_start
            .checked_add(segment.row_count)
            .ok_or(CoveError::ArithOverflow)?;
        if row >= segment.row_start && row < end {
            return Ok(*segment);
        }
    }
    Err(CoveError::BadSchema(format!(
        "row {row} is not covered by any Parquet conversion segment"
    )))
}

fn build_topn_summaries(
    column: &ConvertedColumn,
    segments: &[SegmentLayout],
) -> Result<Vec<TopNSummary>, CoveError> {
    let mut summaries = Vec::new();
    for segment in segments {
        let row_end = segment
            .row_start
            .checked_add(segment.row_count)
            .ok_or(CoveError::ArithOverflow)?;
        let mut unique_keys = BTreeSet::new();
        for row in segment.row_start..row_end {
            if let Some((key, _)) = column.key_u64(row) {
                unique_keys.insert(key);
            }
        }
        if unique_keys.is_empty() {
            continue;
        }
        let unique_keys = unique_keys.into_iter().collect::<Vec<_>>();
        let value_count = unique_keys.len().min(16);
        let mut payload = Vec::with_capacity(value_count * 8);
        for key in unique_keys.iter().rev().take(value_count) {
            payload.extend_from_slice(&key.to_le_bytes());
        }
        summaries.push(TopNSummary {
            table_id: 1,
            column_id: column.entry.column_id,
            segment_id: segment.segment_id,
            morsel_id: u32::MAX,
            direction: TopNDirection::Largest,
            value_count: u16::try_from(value_count).map_err(|_| CoveError::ArithOverflow)?,
            flags: 0,
            payload_offset: 0,
            payload_length: 0,
            checksum: 0,
            payload,
        });
    }
    Ok(summaries)
}

#[derive(Debug, Default)]
struct OptionalSidecars {
    covx_bytes: Option<Vec<u8>>,
    covm_bytes: Option<Vec<u8>>,
}

fn build_optional_sidecars(
    cove_bytes: &[u8],
    validated: &crate::reader::ValidationReport,
    options: &ParquetConversionOptions,
    row_count: u64,
) -> Result<OptionalSidecars, CoveError> {
    if !options.emit_covx && !options.emit_covm {
        return Ok(OptionalSidecars::default());
    }
    let digest = compute_digest(DigestAlgorithm::Sha256, cove_bytes)?;
    let mut sidecar_id = [0u8; 16];
    sidecar_id.copy_from_slice(&digest[..16]);
    let file_id = validated.validated.header.file_id;
    let file_len = cove_bytes.len() as u64;
    let footer_crc32c = validated.validated.postscript.footer.crc32c;
    let segment_count = validated
        .validated
        .footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::TableSegmentData as u16)
        .count();

    let covx_bytes = if options.emit_covx {
        Some(
            CovxFile {
                header: CovxHeaderV1::new(sidecar_id, 1, 0),
                referenced_files: vec![CovxReferencedFileV1 {
                    file_id,
                    file_len,
                    footer_crc32c,
                    digest_algorithm: DigestAlgorithm::Sha256 as u16,
                    digest: digest.clone(),
                }],
                postscript: CovxPostscriptV1 {
                    header_offset: 0,
                    header_len: 0,
                    entries_offset: 0,
                    entries_len: 0,
                    file_len: 0,
                    flags: 0,
                    checksum: 0,
                },
            }
            .serialize()?,
        )
    } else {
        None
    };
    let covm_bytes = if options.emit_covm {
        Some(
            CovmFile {
                header: CovmHeaderV1::new(sidecar_id, 1, 1, 0),
                files: vec![CovmFileEntryV1 {
                    file_id,
                    uri: format!("cove://{}/{}", options.namespace, options.table_name),
                    file_len,
                    footer_crc32c,
                    digest_algorithm: DigestAlgorithm::Sha256 as u16,
                    digest,
                    row_count,
                    segment_count: u32::try_from(segment_count)
                        .map_err(|_| CoveError::ArithOverflow)?,
                    file_stats_ref: 0,
                    file_exact_set_ref: 0,
                    flags: 0,
                }],
                postscript: CovmPostscriptV1 {
                    header_offset: 0,
                    header_len: 0,
                    entries_offset: 0,
                    entries_len: 0,
                    file_len: 0,
                    flags: 0,
                    checksum: 0,
                },
            }
            .serialize()?,
        )
    } else {
        None
    };
    Ok(OptionalSidecars {
        covx_bytes,
        covm_bytes,
    })
}

fn compare_numcode_rows(
    source_kind: SourceColumnKind,
    logical: CoveLogicalType,
    left: u64,
    right: u64,
) -> Ordering {
    match logical {
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64 => {
            signed_numcode(logical, left).cmp(&signed_numcode(logical, right))
        }
        CoveLogicalType::Float32 if source_kind == SourceColumnKind::Float32 => {
            f32::from_bits(left as u32).total_cmp(&f32::from_bits(right as u32))
        }
        CoveLogicalType::Float64 => f64::from_bits(left).total_cmp(&f64::from_bits(right)),
        CoveLogicalType::DateDays => {
            types::numcode_as_date_days(left).cmp(&types::numcode_as_date_days(right))
        }
        CoveLogicalType::TimestampMicros | CoveLogicalType::TimestampNanos => {
            (left as i64).cmp(&(right as i64))
        }
        _ => left.cmp(&right),
    }
}

fn signed_numcode(logical: CoveLogicalType, value: u64) -> i64 {
    match logical {
        CoveLogicalType::Int8 => types::numcode_as_i8(value) as i64,
        CoveLogicalType::Int16 => types::numcode_as_i16(value) as i64,
        CoveLogicalType::Int32 => types::numcode_as_i32(value) as i64,
        CoveLogicalType::Int64 => types::numcode_as_i64(value),
        _ => value as i64,
    }
}

fn distinct_len<T>(values: &[T]) -> Result<u32, CoveError>
where
    T: Ord + Copy,
{
    u32::try_from(values.iter().copied().collect::<BTreeSet<_>>().len())
        .map_err(|_| CoveError::ArithOverflow)
}

fn run_count_u32<T>(values: impl Iterator<Item = T>) -> Result<u32, CoveError>
where
    T: PartialEq,
{
    let mut previous = None;
    let mut runs = 0u32;
    for value in values {
        if previous.as_ref() != Some(&value) {
            runs = runs.checked_add(1).ok_or(CoveError::ArithOverflow)?;
        }
        previous = Some(value);
    }
    Ok(runs)
}

fn reorder_copy<T: Copy>(values: &mut Vec<T>, order: &[usize]) {
    let original = values.clone();
    values.clear();
    values.extend(order.iter().map(|index| original[*index]));
}

fn reorder_clone<T: Clone>(values: &mut Vec<T>, order: &[usize]) {
    let original = values.clone();
    values.clear();
    values.extend(order.iter().map(|index| original[*index].clone()));
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

fn arrow_value_to_json(array: &dyn Array, row: usize) -> Result<Value, CoveError> {
    if row >= array.len() {
        return Err(CoveError::BadSchema(
            "nested JSON fallback row exceeds array length".into(),
        ));
    }
    if array.is_null(row) {
        return Ok(Value::Null);
    }
    match array.data_type() {
        DataType::Boolean => Ok(json!(
            downcast_array::<BooleanArray>(array, "json")?.value(row)
        )),
        DataType::Int8 => Ok(json!(downcast_array::<Int8Array>(array, "json")?.value(row))),
        DataType::Int16 => Ok(json!(
            downcast_array::<Int16Array>(array, "json")?.value(row)
        )),
        DataType::Int32 => Ok(json!(
            downcast_array::<Int32Array>(array, "json")?.value(row)
        )),
        DataType::Int64 => Ok(json!(
            downcast_array::<Int64Array>(array, "json")?.value(row)
        )),
        DataType::UInt8 => Ok(json!(
            downcast_array::<UInt8Array>(array, "json")?.value(row)
        )),
        DataType::UInt16 => Ok(json!(
            downcast_array::<UInt16Array>(array, "json")?.value(row)
        )),
        DataType::UInt32 => Ok(json!(
            downcast_array::<UInt32Array>(array, "json")?.value(row)
        )),
        DataType::UInt64 => Ok(json!(
            downcast_array::<UInt64Array>(array, "json")?.value(row)
        )),
        DataType::Float32 => {
            let value = downcast_array::<Float32Array>(array, "json")?.value(row) as f64;
            Ok(serde_json::Number::from_f64(value)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(value.to_string())))
        }
        DataType::Float64 => {
            let value = downcast_array::<Float64Array>(array, "json")?.value(row);
            Ok(serde_json::Number::from_f64(value)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(value.to_string())))
        }
        DataType::Date32 => Ok(json!(
            downcast_array::<Date32Array>(array, "json")?.value(row)
        )),
        DataType::Timestamp(TimeUnit::Second, _)
        | DataType::Timestamp(TimeUnit::Millisecond, _)
        | DataType::Timestamp(TimeUnit::Microsecond, _)
        | DataType::Timestamp(TimeUnit::Nanosecond, _) => {
            Ok(Value::String(timestamp_value_to_string(array, row)?))
        }
        DataType::Utf8 => Ok(json!(
            downcast_array::<StringArray>(array, "json")?.value(row)
        )),
        DataType::LargeUtf8 => Ok(json!(
            downcast_array::<LargeStringArray>(array, "json")?.value(row)
        )),
        DataType::Binary => Ok(Value::String(hex_encode(
            downcast_array::<BinaryArray>(array, "json")?.value(row),
        ))),
        DataType::LargeBinary => Ok(Value::String(hex_encode(
            downcast_array::<LargeBinaryArray>(array, "json")?.value(row),
        ))),
        DataType::Decimal128(_, _) => Ok(Value::String(
            downcast_array::<Decimal128Array>(array, "json")?
                .value(row)
                .to_string(),
        )),
        DataType::List(_) => {
            let list = downcast_array::<ListArray>(array, "json")?;
            arrow_list_value_to_json(list.value(row).as_ref())
        }
        DataType::LargeList(_) => {
            let list = downcast_array::<LargeListArray>(array, "json")?;
            arrow_list_value_to_json(list.value(row).as_ref())
        }
        DataType::FixedSizeList(_, _) => {
            let list = downcast_array::<FixedSizeListArray>(array, "json")?;
            arrow_list_value_to_json(list.value(row).as_ref())
        }
        DataType::Struct(_) => {
            let struct_array = downcast_array::<StructArray>(array, "json")?;
            arrow_struct_row_to_json(struct_array, row)
        }
        DataType::Map(_, _) => {
            let map_array = downcast_array::<MapArray>(array, "json")?;
            let entries = map_array.value(row);
            let keys = entries.column(0);
            let values = entries.column(1);
            let mut rows = Vec::with_capacity(entries.len());
            for index in 0..entries.len() {
                rows.push(json!({
                    "key": arrow_value_to_json(keys.as_ref(), index)?,
                    "value": arrow_value_to_json(values.as_ref(), index)?,
                }));
            }
            Ok(Value::Array(rows))
        }
        other => Ok(Value::String(format!(
            "unsupported Arrow JSON fallback value for {other:?}: {:?}",
            array.slice(row, 1)
        ))),
    }
}

fn arrow_list_value_to_json(values: &dyn Array) -> Result<Value, CoveError> {
    let mut out = Vec::with_capacity(values.len());
    for row in 0..values.len() {
        out.push(arrow_value_to_json(values, row)?);
    }
    Ok(Value::Array(out))
}

fn arrow_struct_row_to_json(struct_array: &StructArray, row: usize) -> Result<Value, CoveError> {
    let mut out = serde_json::Map::new();
    for (index, field) in struct_array.fields().iter().enumerate() {
        out.insert(
            field.name().clone(),
            arrow_value_to_json(struct_array.column(index).as_ref(), row)?,
        );
    }
    Ok(Value::Object(out))
}

fn timestamp_value_to_string(array: &dyn Array, row: usize) -> Result<String, CoveError> {
    match array.data_type() {
        DataType::Timestamp(TimeUnit::Second, _) => {
            Ok(downcast_array::<TimestampSecondArray>(array, "json")?
                .value(row)
                .to_string())
        }
        DataType::Timestamp(TimeUnit::Millisecond, _) => {
            Ok(downcast_array::<TimestampMillisecondArray>(array, "json")?
                .value(row)
                .to_string())
        }
        DataType::Timestamp(TimeUnit::Microsecond, _) => {
            Ok(downcast_array::<TimestampMicrosecondArray>(array, "json")?
                .value(row)
                .to_string())
        }
        DataType::Timestamp(TimeUnit::Nanosecond, _) => {
            Ok(downcast_array::<TimestampNanosecondArray>(array, "json")?
                .value(row)
                .to_string())
        }
        _ => Err(CoveError::BadSchema(
            "expected timestamp array for JSON fallback".into(),
        )),
    }
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

fn append_materialized_values<T: Clone>(
    row_count: usize,
    values: &mut Vec<T>,
    nulls: &mut Vec<bool>,
    null_placeholder: T,
    mut is_null_at: impl FnMut(usize) -> bool,
    mut value_at: impl FnMut(usize) -> Result<T, CoveError>,
) -> Result<(), CoveError> {
    values.reserve(row_count);
    nulls.reserve(row_count);
    for row in 0..row_count {
        let is_null = is_null_at(row);
        nulls.push(is_null);
        if is_null {
            values.push(null_placeholder.clone());
        } else {
            values.push(value_at(row)?);
        }
    }
    Ok(())
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

fn validity_bitmap_len(row_count: u32) -> Result<usize, CoveError> {
    let bytes = (u64::from(row_count))
        .checked_add(7)
        .ok_or(CoveError::ArithOverflow)?
        / 8;
    usize::try_from(bytes).map_err(|_| CoveError::ArithOverflow)
}

fn decoded_value_to_scalar(
    column: &ColumnEntry,
    value: CoveArrayValue<'_>,
) -> Result<ParquetScalarValue, CoveError> {
    match value {
        CoveArrayValue::Null => Ok(ParquetScalarValue::Null),
        CoveArrayValue::Bytes(bytes) => match column.logical {
            CoveLogicalType::Bool => Ok(ParquetScalarValue::Bool(
                bytes.first().copied().unwrap_or(0) != 0,
            )),
            CoveLogicalType::Utf8 => Ok(ParquetScalarValue::Utf8(
                String::from_utf8(bytes.to_vec()).map_err(|error| {
                    CoveError::BadSection(format!("invalid UTF-8 page payload: {error}"))
                })?,
            )),
            CoveLogicalType::Json => serde_json::from_slice(bytes)
                .map(ParquetScalarValue::Json)
                .map_err(|error| {
                    CoveError::BadSection(format!("invalid JSON page payload: {error}"))
                }),
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
        builder::{Int32Builder, ListBuilder},
        ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
        TimestampMicrosecondArray,
    };
    use parquet::arrow::ArrowWriter;

    use crate::{
        compression::column_page_payload,
        constants::SectionKind,
        page::ColumnPageIndex,
        page_payload::{ColumnPagePayloadV1, PageBufferKind},
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
        assert_eq!(
            result.report.plan,
            vec![
                ConversionStep::DecodeSource,
                ConversionStep::PartitionSegments,
                ConversionStep::EncodePages,
                ConversionStep::WriteSections,
            ]
        );
        assert!(result.report.validation_result);
        assert_eq!(
            result.report.generated_section_kinds,
            vec!["TableCatalog", "TableSegmentIndex", "TableSegmentData"]
        );
        assert!(result.report.unsupported_features.is_empty());
        assert!(result
            .report
            .source_schema_fingerprint
            .starts_with("crc32c:"));
        assert!(result
            .report
            .target_schema_fingerprint
            .starts_with("crc32c:"));

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

    #[cfg(feature = "digest-sha2")]
    #[test]
    fn emits_dictionary_stats_indexes_and_sidecars_when_requested() {
        let batch = RecordBatch::try_from_iter(vec![
            (
                "id",
                Arc::new(Int64Array::from(vec![10, 20, 10, 30, 20, 10])) as ArrayRef,
            ),
            (
                "city",
                Arc::new(StringArray::from(vec![
                    "lon", "lon", "par", "lon", "par", "lon",
                ])) as ArrayRef,
            ),
            (
                "active",
                Arc::new(BooleanArray::from(vec![
                    true, true, false, true, false, true,
                ])) as ArrayRef,
            ),
        ])
        .unwrap();

        let options = ParquetConversionOptions {
            dictionary_policy: ParquetDictionaryPolicy::Always,
            stats_policy: ParquetStatsPolicy::Recompute,
            acceleration_policy: ParquetAccelerationPolicy::Auto,
            point_lookup_columns: vec!["id".into()],
            topn_columns: vec!["id".into()],
            aggregate_policy: ParquetAggregatePolicy::Auto,
            composite_zone_groups: vec![vec!["id".into(), "active".into()]],
            emit_covx: true,
            emit_covm: true,
            morsel_row_count: 3,
            ..ParquetConversionOptions::default()
        };
        let result = convert_parquet_bytes(&parquet_bytes(&batch), &options).unwrap();

        for expected in [
            "FileDictionaryIndex",
            "ColumnDomain",
            "ZoneStats",
            "ExactSetIndex",
            "LookupIndex",
            "AggregateSynopsis",
            "CompositeZoneIndex",
            "TopNZoneSummary",
            "TableSegmentIndex",
            "TableSegmentData",
        ] {
            assert!(
                result
                    .report
                    .generated_section_kinds
                    .iter()
                    .any(|kind| kind == expected),
                "missing generated section {expected}: {:?}",
                result.report.generated_section_kinds
            );
        }
        assert!(result
            .report
            .plan
            .contains(&ConversionStep::BuildDictionaries));
        assert!(result.report.plan.contains(&ConversionStep::RecomputeStats));
        assert!(result
            .report
            .plan
            .contains(&ConversionStep::BuildDomainsAndIndexes));
        assert!(result
            .report
            .plan
            .contains(&ConversionStep::EmitOptionalCovmCovx));
        assert!(result.report.unsupported_features.is_empty());

        let catalog = first_table_catalog(&result.cove_bytes);
        let city = catalog.tables[0]
            .columns
            .iter()
            .find(|column| column.name == "city")
            .unwrap();
        assert_eq!(city.physical, CovePhysicalKind::FileCode);

        CovxFile::parse(result.covx_bytes.as_ref().unwrap()).unwrap();
        CovmFile::parse(result.covm_bytes.as_ref().unwrap()).unwrap();
    }

    #[test]
    fn stable_clustering_reorders_rows_deterministically() {
        let batch = RecordBatch::try_from_iter(vec![
            ("id", Arc::new(Int64Array::from(vec![3, 1, 2])) as ArrayRef),
            (
                "city",
                Arc::new(StringArray::from(vec!["c", "a", "b"])) as ArrayRef,
            ),
        ])
        .unwrap();
        let options = ParquetConversionOptions {
            dictionary_policy: ParquetDictionaryPolicy::Never,
            clustering_policy: ParquetClusteringPolicy::StableClusterDeclaredColumns,
            cluster_columns: vec!["id".into()],
            ..ParquetConversionOptions::default()
        };
        let result = convert_parquet_bytes(&parquet_bytes(&batch), &options).unwrap();
        let catalog = first_table_catalog(&result.cove_bytes);
        let decoded = decoded_table_values(&result.cove_bytes, &catalog);
        assert_eq!(decoded[0], vec![json!(1), json!(2), json!(3)]);
        assert_eq!(decoded[1], vec![json!("a"), json!("b"), json!("c")]);
        assert!(result
            .report
            .notes
            .iter()
            .any(|note| note.contains("Applied stable clustering")));
    }

    #[test]
    fn converts_nullable_parquet_columns_with_cove_validity_payloads() {
        let batch = RecordBatch::try_from_iter(vec![
            (
                "id",
                Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])) as ArrayRef,
            ),
            (
                "city",
                Arc::new(StringArray::from(vec![Some("sea"), None, Some("lon")])) as ArrayRef,
            ),
        ])
        .unwrap();

        let parquet_bytes = parquet_bytes(&batch);
        let result =
            convert_parquet_bytes(&parquet_bytes, &ParquetConversionOptions::default()).unwrap();
        let catalog = first_table_catalog(&result.cove_bytes);
        assert!(catalog.tables[0]
            .columns
            .iter()
            .all(|column| column.nullable));
        let decoded = decoded_table_values(&result.cove_bytes, &catalog);
        assert_eq!(decoded[0], vec![json!(1), Value::Null, json!(3)]);
        assert_eq!(decoded[1], vec![json!("sea"), Value::Null, json!("lon")]);

        let report = validate_bytes_with_options(
            &result.cove_bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                ..ValidationOptions::default()
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
        let segment_bytes =
            &result.cove_bytes[entry.offset as usize..entry.end_offset().unwrap() as usize];
        let segment = TableSegmentPayloadV1::parse(segment_bytes).unwrap();
        let first_dir = &segment.columns[0];
        let page_index = ColumnPageIndex::parse(
            &segment_bytes[first_dir.page_index_offset as usize
                ..(first_dir.page_index_offset + first_dir.page_index_length) as usize],
        )
        .unwrap();
        assert_eq!(page_index.entries[0].row_count, 3);
        assert_eq!(page_index.entries[0].non_null_count, 2);
        assert_eq!(page_index.entries[0].null_count, 1);
    }

    #[test]
    fn splits_parquet_rows_across_multiple_cove_segments() {
        let batch = RecordBatch::try_from_iter(vec![(
            "id",
            Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5, 6, 7, 8])) as ArrayRef,
        )])
        .unwrap();
        let options = ParquetConversionOptions {
            segment_row_count: 3,
            morsel_row_count: 2,
            stats_policy: ParquetStatsPolicy::Recompute,
            acceleration_policy: ParquetAccelerationPolicy::DeclaredOnly,
            point_lookup_columns: vec!["id".into()],
            ..ParquetConversionOptions::default()
        };

        let result = convert_parquet_bytes(&parquet_bytes(&batch), &options).unwrap();
        assert_eq!(result.report.row_count, 8);
        assert_eq!(result.report.segment_count, 3);

        let report = validate_bytes_with_options(
            &result.cove_bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                ..ValidationOptions::default()
            },
        )
        .unwrap();
        let segment_entries = report
            .validated
            .footer
            .sections
            .iter()
            .filter(|entry| entry.section_kind == SectionKind::TableSegmentData as u16)
            .collect::<Vec<_>>();
        assert_eq!(segment_entries.len(), 3);
        let mut starts = Vec::new();
        let mut counts = Vec::new();
        for entry in segment_entries {
            let segment_bytes =
                &result.cove_bytes[entry.offset as usize..entry.end_offset().unwrap() as usize];
            let segment = TableSegmentPayloadV1::parse(segment_bytes).unwrap();
            starts.push(segment.header.row_start);
            counts.push(segment.header.row_count);
        }
        assert_eq!(starts, vec![0, 3, 6]);
        assert_eq!(counts, vec![3, 3, 2]);

        let catalog = first_table_catalog(&result.cove_bytes);
        assert_eq!(
            decoded_table_values(&result.cove_bytes, &catalog)[0],
            vec![
                json!(1),
                json!(2),
                json!(3),
                json!(4),
                json!(5),
                json!(6),
                json!(7),
                json!(8)
            ]
        );
    }

    #[test]
    fn converts_nested_parquet_columns_to_json_fallback() {
        let mut builder = ListBuilder::new(Int32Builder::new());
        builder.values().append_value(1);
        builder.values().append_value(2);
        builder.append(true);
        builder.append(false);
        builder.values().append_value(3);
        builder.append(true);
        let batch =
            RecordBatch::try_from_iter(vec![("tags", Arc::new(builder.finish()) as ArrayRef)])
                .unwrap();

        let result =
            convert_parquet_bytes(&parquet_bytes(&batch), &ParquetConversionOptions::default())
                .unwrap();
        assert_eq!(result.report.segment_count, 1);
        assert_eq!(result.report.nested_shape_fallbacks.len(), 1);
        assert!(result.report.columns[0].pushdown_limited);
        assert_eq!(
            result.report.columns[0].fallback,
            Some(UnsupportedNestedFallback::Json)
        );

        let catalog = first_table_catalog(&result.cove_bytes);
        let column = &catalog.tables[0].columns[0];
        assert_eq!(column.logical, CoveLogicalType::Json);
        assert_eq!(column.physical, CovePhysicalKind::VarBytes);
        assert_eq!(
            decoded_table_values(&result.cove_bytes, &catalog)[0],
            vec![json!([1, 2]), Value::Null, json!([3])]
        );
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
                ..ValidationOptions::default()
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
                ..ValidationOptions::default()
            },
        )
        .unwrap();
        let segment_sections = report
            .validated
            .footer
            .sections
            .iter()
            .filter(|entry| entry.section_kind == SectionKind::TableSegmentData as u16)
            .collect::<Vec<_>>();
        let mut out = catalog.tables[0]
            .columns
            .iter()
            .map(|_| Vec::new())
            .collect::<Vec<Vec<Value>>>();
        for entry in segment_sections {
            let segment_bytes = &bytes[entry.offset as usize..entry.end_offset().unwrap() as usize];
            let segment = TableSegmentPayloadV1::parse(segment_bytes).unwrap();
            for (column_index, column) in catalog.tables[0].columns.iter().enumerate() {
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
                    let page_payload = ColumnPagePayloadV1::parse(payload.as_ref()).unwrap();
                    let values = page_payload
                        .buffer_bytes(PageBufferKind::Values)
                        .unwrap()
                        .unwrap_or(&[]);
                    let payload = if page.null_count == 0 {
                        values.to_vec()
                    } else {
                        let nulls = page_payload
                            .buffer_bytes(PageBufferKind::NullBitmap)
                            .unwrap()
                            .unwrap();
                        let mut combined = nulls.to_vec();
                        combined.extend_from_slice(values);
                        combined
                    };
                    rows.extend(
                        decode_materialized_page_values_with_nulls(
                            column,
                            page.row_count,
                            page.null_count,
                            &payload,
                        )
                        .unwrap()
                        .into_iter()
                        .map(|value| value.to_json_value()),
                    );
                }
                out[column_index].extend(rows);
            }
        }
        out
    }
}
