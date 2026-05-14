use super::*;

pub(super) struct DecodedArrowColumn<'name, 'array, 'data> {
    pub(super) name: &'name str,
    pub(super) array: &'array EncodedArray<'data>,
    pub(super) payload: Option<&'array RetainedColumnPagePayloadV1>,
    pub(super) nested_schema: Option<NestedSchemaNodeV1>,
    pub(super) data_owner: Option<ArrowBufferOwner>,
    pub(super) utf8_proof_key: Option<Utf8ProofKey>,
    pub(super) zero_copy: Option<ZeroCopyCompatibilityV2>,
}

#[inline]
pub(super) fn arrow_encoded_columns_for_payloads<'name, 'array, 'data>(
    state: &DatasetState,
    segment_id: u32,
    columns: &[&ColumnEntry],
    encoded_columns: &'array [(&'name str, EncodedArray<'data>)],
    page_indexes: &'array [ColumnPageIndexEntryV1],
    page_payloads: &'array [RetainedColumnPagePayloadV1],
    _options: ArrowExportOptions,
) -> Vec<DecodedArrowColumn<'name, 'array, 'data>> {
    debug_assert_eq!(columns.len(), encoded_columns.len());
    debug_assert_eq!(page_indexes.len(), encoded_columns.len());
    debug_assert_eq!(encoded_columns.len(), page_payloads.len());
    columns
        .iter()
        .zip(encoded_columns.iter())
        .zip(page_indexes.iter())
        .zip(page_payloads.iter())
        .map(|(((column, (name, array)), page), payload)| {
            let nested_schema = state.nested_schema_for_column(column.column_id).cloned();
            let data_owner = if array.physical == CovePhysicalKind::VarBytes {
                Some(arrow_buffer_owner(payload.data.owner()))
            } else {
                None
            };
            DecodedArrowColumn {
                name,
                array,
                payload: Some(payload),
                nested_schema,
                data_owner,
                utf8_proof_key: Utf8ProofKey::new(state.identity(), column, page),
                zero_copy: state.zero_copy_compatibility_for_page(segment_id, column, page),
            }
        })
        .collect()
}

#[inline]
pub(super) fn record_batch_for_selection(
    state: &DatasetState,
    columns: &[DecodedArrowColumn<'_, '_, '_>],
    selection: &Selection,
    schema: SchemaRef,
    options: ArrowExportOptions,
    cache: Option<&ScanExecutionCache>,
    stats: &mut DecodeStats,
) -> Result<cove_arrow::arrow::ArrowExportResult<RecordBatch>, CoveError> {
    let arrow_selection = match selection {
        Selection::None => ArrowRowSelection::Rows(&[]),
        Selection::AllRows { .. } => ArrowRowSelection::All,
        Selection::RowIndices(rows) => ArrowRowSelection::Rows(rows),
        Selection::Bitset(mask) => ArrowRowSelection::Bitset {
            words: &mask.words,
            len: mask.len,
        },
    };
    let mut arrays = Vec::with_capacity(columns.len());
    let mut report = cove_arrow::arrow::ArrowExportReport::default();
    for column in columns {
        if let Some(nested_schema) = column.nested_schema.as_ref() {
            let payload = column.payload.ok_or(CoveError::PageCorrupt)?;
            let result = nested_page_payload_to_arrow_array(
                payload,
                nested_schema,
                arrow_selection,
                state.mounted().dictionary.as_ref(),
                options,
            )?;
            merge_export_report(column.name, &mut report, result.report);
            arrays.push(result.value);
            continue;
        }
        let mut column_options = options;
        let mut record_utf8_proof = None;
        if let Some(key) = column.utf8_proof_key {
            if options.string_validation_policy == ArrowStringValidationPolicy::StrictOrCachedProof
            {
                if state.utf8_proof_cache().contains(&key)? {
                    column_options.string_validation_policy =
                        ArrowStringValidationPolicy::TrustedPageProof;
                    stats.utf8_proof_hits += 1;
                } else {
                    column_options.string_validation_policy = ArrowStringValidationPolicy::Strict;
                    stats.utf8_proof_misses += 1;
                    record_utf8_proof = Some(key);
                }
            }
        }
        let export_path = classify_arrow_export(column.array, column_options);
        let zero_copy_data_owner =
            zero_copy_data_owner_for_export(column, selection, export_path, column_options);
        record_zero_copy_decision(
            stats,
            column.zero_copy,
            selection,
            export_path,
            column_options,
            zero_copy_data_owner.is_some(),
        );
        let result = export_arrow_column(
            state,
            column,
            arrow_selection,
            column_options,
            zero_copy_data_owner,
            cache,
            stats,
        )?;
        record_arrow_export_stats(stats, export_path, result.value.as_ref());
        if let Some(key) = record_utf8_proof {
            if state.utf8_proof_cache().insert(key)? {
                stats.utf8_proofs_earned += 1;
            }
        }
        merge_export_report(column.name, &mut report, result.report);
        arrays.push(result.value);
    }
    let batch = RecordBatch::try_new(schema, arrays)
        .map_err(|err| CoveError::BadSection(format!("Arrow RecordBatch: {err}")))?;
    Ok(cove_arrow::arrow::ArrowExportResult {
        value: batch,
        report,
    })
}

#[inline]
fn export_arrow_column(
    state: &DatasetState,
    column: &DecodedArrowColumn<'_, '_, '_>,
    arrow_selection: ArrowRowSelection<'_>,
    options: ArrowExportOptions,
    data_owner: Option<&ArrowBufferOwner>,
    cache: Option<&ScanExecutionCache>,
    stats: &mut DecodeStats,
) -> Result<cove_arrow::arrow::ArrowExportResult<ArrayRef>, CoveError> {
    if options.dictionary_policy == ArrowDictionaryPolicy::DictionaryKeys
        && column.array.encoding == CoveEncodingKind::FileCode
    {
        if let Some(dictionary) = column.array.dictionary {
            let value = if file_dictionary_entries_compatible_with_logical(
                column.array.logical,
                dictionary,
            )? {
                let value_options = filecode_dictionary_value_export_options(options);
                let values = if let Some(cache) = cache {
                    let key = ArrowDictionaryValuesCacheKey::new(
                        state.identity(),
                        column.array.logical,
                        value_options,
                    );
                    let (values, was_hit) =
                        cache.get_or_build_arrow_dictionary_values(key, || {
                            file_dictionary_values_to_arrow(
                                column.array.logical,
                                dictionary,
                                value_options,
                            )
                        })?;
                    if was_hit {
                        stats.filecode_dictionary_value_cache_hits += 1;
                    } else {
                        stats.filecode_dictionary_value_cache_misses += 1;
                        stats.filecode_dictionary_values_bytes += values.get_buffer_memory_size();
                    }
                    values
                } else {
                    let values = file_dictionary_values_to_arrow(
                        column.array.logical,
                        dictionary,
                        value_options,
                    )?;
                    stats.filecode_dictionary_values_bytes += values.get_buffer_memory_size();
                    values
                };
                encoded_filecode_array_to_arrow_dictionary_with_values(
                    column.array,
                    arrow_selection,
                    values,
                )?
            } else {
                let value = encoded_filecode_array_to_arrow_dictionary_remapped(
                    column.array,
                    arrow_selection,
                    options,
                )?;
                stats.filecode_dictionary_remapped_rows += value.len();
                if let Some(dictionary) =
                    value.as_any().downcast_ref::<DictionaryArray<UInt32Type>>()
                {
                    stats.filecode_dictionary_values_bytes +=
                        dictionary.values().get_buffer_memory_size();
                }
                value
            };
            return Ok(cove_arrow::arrow::ArrowExportResult {
                value,
                report: cove_arrow::arrow::ArrowExportReport::default(),
            });
        }
    }

    encoded_array_to_arrow_with_row_selection_options_and_owner(
        column.array,
        arrow_selection,
        options,
        data_owner,
    )
}

fn merge_export_report(
    field: &str,
    report: &mut cove_arrow::arrow::ArrowExportReport,
    mut other: cove_arrow::arrow::ArrowExportReport,
) {
    for issue in &mut other.issues {
        if issue.field.is_none() {
            issue.field = Some(field.to_string());
        }
    }
    report.issues.extend(other.issues);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrowExportPath {
    DirectVarBytes,
    DirectNumCode,
    DirectPlainFixed,
    DirectFileCodeDictionary,
    DirectTransform,
    DirectConstantPlainVarint,
    FileCodeDecodedFallback,
    Fallback,
}

#[inline]
fn classify_arrow_export(array: &EncodedArray<'_>, options: ArrowExportOptions) -> ArrowExportPath {
    if options.dictionary_policy == ArrowDictionaryPolicy::DictionaryKeys
        && array.encoding == CoveEncodingKind::FileCode
        && array.dictionary.is_some()
    {
        return ArrowExportPath::DirectFileCodeDictionary;
    }
    if array.encoding == CoveEncodingKind::FileCode && array.dictionary.is_some() {
        return ArrowExportPath::FileCodeDecodedFallback;
    }
    if array.physical == CovePhysicalKind::VarBytes
        && matches!(
            array.encoding,
            CoveEncodingKind::VarBytes | CoveEncodingKind::Canonical
        )
        && matches!(
            array.logical,
            CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json
        )
    {
        return ArrowExportPath::DirectVarBytes;
    }
    if array.encoding == CoveEncodingKind::NumCode && array.physical == CovePhysicalKind::NumCode {
        return ArrowExportPath::DirectNumCode;
    }
    if array.encoding == CoveEncodingKind::PlainFixed {
        return ArrowExportPath::DirectPlainFixed;
    }
    if matches!(
        array.encoding,
        CoveEncodingKind::Constant | CoveEncodingKind::PlainVarint
    ) {
        return ArrowExportPath::DirectConstantPlainVarint;
    }
    if matches!(
        array.encoding,
        CoveEncodingKind::Rle
            | CoveEncodingKind::RunEnd
            | CoveEncodingKind::BitPacked
            | CoveEncodingKind::Delta
            | CoveEncodingKind::FrameOfReference
            | CoveEncodingKind::PatchedBase
            | CoveEncodingKind::Sparse
            | CoveEncodingKind::LocalCodebook
    ) {
        return ArrowExportPath::DirectTransform;
    }
    ArrowExportPath::Fallback
}

fn record_arrow_export_stats(stats: &mut DecodeStats, path: ArrowExportPath, array: &dyn Array) {
    let rows = array.len();
    match path {
        ArrowExportPath::DirectVarBytes => {
            stats.arrow_export_direct_varbytes_rows += rows;
            stats.arrow_export_direct_varbytes_bytes += array.get_buffer_memory_size();
        }
        ArrowExportPath::DirectNumCode => stats.arrow_export_direct_numcode_rows += rows,
        ArrowExportPath::DirectPlainFixed => stats.arrow_export_direct_plainfixed_rows += rows,
        ArrowExportPath::DirectFileCodeDictionary => {
            stats.arrow_export_direct_filecode_dictionary_rows += rows;
            stats.filecode_dictionary_keys_rows += rows;
        }
        ArrowExportPath::DirectTransform => stats.arrow_export_direct_transform_rows += rows,
        ArrowExportPath::DirectConstantPlainVarint => {
            stats.arrow_export_direct_constant_plainvarint_rows += rows;
        }
        ArrowExportPath::Fallback => stats.arrow_export_fallback_rows += rows,
        ArrowExportPath::FileCodeDecodedFallback => {
            stats.filecode_dictionary_decoded_fallback_rows += rows;
            stats.arrow_export_fallback_rows += rows;
        }
    }
}

fn record_zero_copy_decision(
    stats: &mut DecodeStats,
    decision: Option<ZeroCopyCompatibilityV2>,
    selection: &Selection,
    export_path: ArrowExportPath,
    options: ArrowExportOptions,
    direct_owner_used: bool,
) {
    if options.varbytes_policy != ArrowVarBytesExportPolicy::View
        || !zero_copy_direct_export_path(export_path)
    {
        return;
    }
    let Some(decision) = decision else {
        return;
    };
    match decision {
        ZeroCopyCompatibilityV2::Compatible => {
            if direct_owner_used {
                stats.zero_copy_compatible_buffers += 1;
            } else if !matches!(selection, Selection::AllRows { .. }) {
                stats.zero_copy_materialized_buffers += 1;
                stats.zero_copy_materialized_selection_mismatch += 1;
            } else {
                stats.zero_copy_materialized_buffers += 1;
                stats.zero_copy_materialized_insufficient_lifetime += 1;
            }
        }
        ZeroCopyCompatibilityV2::MaterializeRequired(reason) => {
            stats.zero_copy_materialized_buffers += 1;
            match reason {
                ZeroCopyMaterializationReasonV2::UnknownRole => {
                    stats.zero_copy_materialized_unknown_role += 1;
                }
                ZeroCopyMaterializationReasonV2::NullPolarityMismatch => {
                    stats.zero_copy_materialized_null_polarity_mismatch += 1;
                }
                ZeroCopyMaterializationReasonV2::CompressedBuffer => {
                    stats.zero_copy_materialized_compressed_buffer += 1;
                }
                ZeroCopyMaterializationReasonV2::DictionaryMismatch => {
                    stats.zero_copy_materialized_dictionary_mismatch += 1;
                }
                ZeroCopyMaterializationReasonV2::NestedLayoutMismatch => {
                    stats.zero_copy_materialized_nested_layout_mismatch += 1;
                }
                ZeroCopyMaterializationReasonV2::InsufficientLifetime => {
                    stats.zero_copy_materialized_insufficient_lifetime += 1;
                }
                ZeroCopyMaterializationReasonV2::ActiveVisibilityOverlay => {
                    stats.zero_copy_materialized_active_visibility_overlay += 1;
                }
            }
        }
    }
}

fn zero_copy_data_owner_for_export<'a>(
    column: &'a DecodedArrowColumn<'_, '_, '_>,
    selection: &Selection,
    export_path: ArrowExportPath,
    options: ArrowExportOptions,
) -> Option<&'a ArrowBufferOwner> {
    if options.varbytes_policy != ArrowVarBytesExportPolicy::View
        || !matches!(selection, Selection::AllRows { .. })
        || !zero_copy_direct_export_path(export_path)
        || !matches!(column.zero_copy, Some(ZeroCopyCompatibilityV2::Compatible))
    {
        return None;
    }
    column.data_owner.as_ref()
}

fn zero_copy_direct_export_path(path: ArrowExportPath) -> bool {
    matches!(path, ArrowExportPath::DirectVarBytes)
}

pub(super) fn materialize_page_payload(
    segment_bytes: &[u8],
    column: &ColumnEntry,
    page: &ColumnPageIndexEntryV1,
    codec_descriptors: &[cove_core::codec::CodecExtensionDescriptorV2],
    dictionary_len: Option<u32>,
    validation_policy: PagePayloadValidationPolicy,
) -> Result<RetainedColumnPagePayloadV1, CoveError> {
    if page.flags & PAGE_FLAG_STATS_ONLY_CONSTANT != 0 {
        return materialize_stats_only_page(column, page);
    }

    let start = usize::try_from(page.page_offset).map_err(|_| CoveError::OffsetRange)?;
    let len = usize::try_from(page.page_length).map_err(|_| CoveError::OffsetRange)?;
    let page_wire = wire::read_range_checked(segment_bytes, start, len)?;
    let decoded = compression::column_page_payload_with_checksum_validation(
        page_wire,
        page,
        page_checksum_validation(validation_policy),
    )?;
    let decoded = match decoded {
        Cow::Borrowed(bytes) => bytes.to_vec(),
        Cow::Owned(bytes) => bytes,
    };
    let payload = RetainedColumnPagePayloadV1::parse_with_buffer_checksum_validation(
        RetainedBytes::from_vec(decoded),
        buffer_checksum_validation(validation_policy),
    )?;
    materialize_registered_page_payload_if_needed(
        payload,
        column,
        page,
        codec_descriptors,
        dictionary_len,
    )
}

pub(super) fn materialize_page_payload_from_wire(
    column: &ColumnEntry,
    page: &ColumnPageIndexEntryV1,
    page_wire: Option<RetainedBytes>,
    codec_descriptors: &[cove_core::codec::CodecExtensionDescriptorV2],
    dictionary_len: Option<u32>,
    validation_policy: PagePayloadValidationPolicy,
) -> Result<RetainedColumnPagePayloadV1, CoveError> {
    if page.flags & PAGE_FLAG_STATS_ONLY_CONSTANT != 0 {
        return materialize_stats_only_page(column, page);
    }
    let Some(page_wire) = page_wire else {
        return Err(CoveError::PageCorrupt);
    };
    let decoded = compression::column_page_payload_retained_with_checksum_validation(
        page_wire,
        page,
        page_checksum_validation(validation_policy),
    )?;
    let payload = RetainedColumnPagePayloadV1::parse_with_buffer_checksum_validation(
        decoded,
        buffer_checksum_validation(validation_policy),
    )?;
    materialize_registered_page_payload_if_needed(
        payload,
        column,
        page,
        codec_descriptors,
        dictionary_len,
    )
}

fn page_checksum_validation(
    validation_policy: PagePayloadValidationPolicy,
) -> compression::PageChecksumValidation {
    match validation_policy {
        PagePayloadValidationPolicy::Trusted => compression::PageChecksumValidation::Trusted,
        PagePayloadValidationPolicy::Strict => compression::PageChecksumValidation::Verify,
    }
}

fn buffer_checksum_validation(
    validation_policy: PagePayloadValidationPolicy,
) -> cove_core::page_payload::BufferChecksumValidation {
    match validation_policy {
        PagePayloadValidationPolicy::Trusted => {
            cove_core::page_payload::BufferChecksumValidation::Trusted
        }
        PagePayloadValidationPolicy::Strict => {
            cove_core::page_payload::BufferChecksumValidation::Verify
        }
    }
}

fn materialize_stats_only_page(
    column: &ColumnEntry,
    page: &ColumnPageIndexEntryV1,
) -> Result<RetainedColumnPagePayloadV1, CoveError> {
    if page.flags & PAGE_FLAG_ALL_NULL != 0 {
        let bitmap_len = (page.row_count as usize)
            .checked_add(7)
            .ok_or(CoveError::ArithOverflow)?
            / 8;
        let mut bitmap = vec![0xff; bitmap_len];
        if !page.row_count.is_multiple_of(8) && !bitmap.is_empty() {
            let valid_bits = page.row_count % 8;
            bitmap[bitmap_len - 1] = (1u8 << valid_bits) - 1;
        }
        let payload = ColumnPagePayloadV1::build_single_node(
            page.row_count,
            default_encoding_kind(column.physical),
            column.logical,
            column.physical,
            Some(bitmap),
            Vec::new(),
        )?;
        return RetainedColumnPagePayloadV1::parse(RetainedBytes::from_vec(payload));
    }
    if page.flags & PAGE_FLAG_ALL_NON_NULL != 0 {
        return Err(CoveError::UnsupportedEncoding(
            "native decoder cannot decode stats-only non-null constant pages without materialized values"
                .into(),
        ));
    }
    Err(CoveError::PageCorrupt)
}

fn materialize_registered_page_payload_if_needed(
    payload: RetainedColumnPagePayloadV1,
    column: &ColumnEntry,
    page: &ColumnPageIndexEntryV1,
    codec_descriptors: &[cove_core::codec::CodecExtensionDescriptorV2],
    dictionary_len: Option<u32>,
) -> Result<RetainedColumnPagePayloadV1, CoveError> {
    if payload.root_node()?.encoding_kind != CoveEncodingKind::RegisteredEncoding {
        return Ok(payload);
    }
    let owned = ColumnPagePayloadV1 {
        header: payload.header.clone(),
        nodes: payload.nodes.clone(),
        buffers: payload.buffers.clone(),
        data: payload.data.as_slice().to_vec(),
    };
    let materialized = cove_core::codec::materialize_registered_page_payload(
        &owned,
        page,
        column.logical,
        column.physical,
        codec_descriptors,
        &cove_core::codec::StableRegisteredCodecResolver,
        dictionary_len,
    )?
    .ok_or(CoveError::BadCodecExtension)?;
    RetainedColumnPagePayloadV1::parse(RetainedBytes::from_vec(materialized.payload.serialize()?))
}

pub(super) fn encoded_array_for_page<'a>(
    payload: &'a RetainedColumnPagePayloadV1,
    page: &ColumnPageIndexEntryV1,
    dictionary: Option<&'a cove_core::dictionary::FileDictionary>,
) -> Result<EncodedArray<'a>, CoveError> {
    let root = payload
        .nodes
        .iter()
        .find(|node| node.node_id == payload.header.root_node_id)
        .ok_or(CoveError::PageCorrupt)?;
    let validity = buffer_slice(payload, PageBufferKind::NullBitmap)?
        .map(|bytes| ValidityBitmap::new(bytes, page.row_count as u64));
    let values = buffer_slice(payload, PageBufferKind::Values)?.unwrap_or(&[]);
    Ok(EncodedArray::new(
        root.logical_type,
        root.physical_kind,
        page.row_count as u64,
        root.encoding_kind,
        validity,
        values,
        dictionary,
    ))
}

fn buffer_slice(
    payload: &RetainedColumnPagePayloadV1,
    kind: PageBufferKind,
) -> Result<Option<&[u8]>, CoveError> {
    let mut matches = payload.buffers.iter().filter(|buffer| buffer.kind == kind);
    let Some(buffer) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(CoveError::PageCorrupt);
    }
    let start = usize::try_from(buffer.offset).map_err(|_| CoveError::OffsetRange)?;
    let len = usize::try_from(buffer.length).map_err(|_| CoveError::OffsetRange)?;
    wire::read_range_checked(payload.data.as_slice(), start, len).map(Some)
}

fn default_encoding_kind(physical: CovePhysicalKind) -> CoveEncodingKind {
    match physical {
        CovePhysicalKind::FileCode => CoveEncodingKind::FileCode,
        CovePhysicalKind::NumCode => CoveEncodingKind::NumCode,
        CovePhysicalKind::Boolean | CovePhysicalKind::FixedBytes => CoveEncodingKind::PlainFixed,
        CovePhysicalKind::VarBytes => CoveEncodingKind::VarBytes,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            CoveEncodingKind::Canonical
        }
        _ => CoveEncodingKind::Canonical,
    }
}
