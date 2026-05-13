use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use bytes::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use super::*;

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
    let reader = builder
        .with_batch_size(options.morsel_row_count as usize)
        .build()
        .map_err(|error| CoveError::BadSection(format!("cannot build parquet reader: {error}")))?;
    let mut batches = Vec::new();
    for batch in reader {
        batches.push(batch.map_err(|error| {
            CoveError::BadSection(format!("cannot read parquet batch: {error}"))
        })?);
    }
    convert_arrow_record_batches(
        "parquet",
        source_schema_fingerprint,
        schema,
        batches,
        options,
    )
}

/// Convert Arrow record batches into a semantically valid COVE-T scan-profile
/// file using the same writer, statistics, dictionary, and acceleration path as
/// the Parquet converter.
pub fn convert_arrow_record_batches<I>(
    source_format: impl Into<String>,
    source_schema_fingerprint: String,
    schema: SchemaRef,
    batches: I,
    options: &ParquetConversionOptions,
) -> Result<ParquetConversionResult, CoveError>
where
    I: IntoIterator<Item = RecordBatch>,
{
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
    let mut columns = schema
        .fields()
        .iter()
        .enumerate()
        .map(|(index, field)| ConvertedColumn::from_field(index as u32 + 1, field))
        .collect::<Result<Vec<_>, _>>()?;

    let mut total_rows = 0usize;
    for batch in batches {
        if batch.num_columns() != columns.len() {
            return Err(CoveError::BadSchema(format!(
                "source batch has {} columns but schema declares {}",
                batch.num_columns(),
                columns.len()
            )));
        }
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
    let nested_entries = columns
        .iter()
        .filter_map(|column| {
            column.nested.as_ref().map(|nested| NestedSchemaEntryV1 {
                table_id: 1,
                column_id: column.entry.column_id,
                root: nested.schema.clone(),
            })
        })
        .collect::<Vec<_>>();
    if !nested_entries.is_empty() {
        writer.push_nested_schema(&NestedSchemaSectionV1::new(nested_entries))?;
        notes.push("Emitted native COVE-T NestedSchema metadata".into());
    }
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
    let aggregate_synopsis_kinds = acceleration
        .aggregates
        .iter()
        .flat_map(|synopsis| {
            synopsis.entries.iter().enumerate().map(|(index, entry)| {
                let payload = synopsis
                    .payload_for_entry(index)
                    .map(aggregate_payload_summary)
                    .unwrap_or_else(|| "missing-payload".into());
                format!(
                    "column_id={}, kind={:?}, accuracy={:?}, rows={}, nulls={}, {}",
                    entry.column_id,
                    entry.synopsis_kind,
                    entry.accuracy,
                    entry.row_count,
                    entry.null_count,
                    payload
                )
            })
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
            source_format: source_format.into(),
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
            aggregate_synopsis_kinds,
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
