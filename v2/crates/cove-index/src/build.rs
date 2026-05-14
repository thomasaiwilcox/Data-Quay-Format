use std::collections::BTreeMap;

use crate::{
    execution::CoviAggregateKindV2, CoviAggregateAnswerBlockHeaderV2, CoviAggregateAnswerBlockV2,
    CoviAggregateAnswerV2, CoviArtifactV2, CoviComparatorKindV2, CoviEntryBlockHeaderV2,
    CoviEntryBlockV2, CoviIndexEntryV2, CoviIndexKindV2, CoviIndexRootV2, CoviIndexedTargetKindV2,
    CoviKeyBlockHeaderV2, CoviKeyBlockV2, CoviKeyEncodingKindV2, CoviPostingRepresentationV2,
    CoviPostingsBlockHeaderV2, CoviPostingsBlockV2, CoviPostingsHeaderV2, CoviReferencedFileV2,
    CoviRowRangePostingV2, CoviSectionKindV2, CoviSectionPayloadV2, CoviSnapshotValidityV2,
    IndexCapabilityExactnessV2, IndexCapabilityV2,
};
use cove_core::{
    array::{CoveArrayValue, EncodedArray},
    canonical::CanonicalValue,
    checksum,
    compression::{column_page_payload, section_payload},
    constants::{CoveLogicalType, CovePhysicalKind, DigestAlgorithm, SectionKind},
    dictionary::DictionaryValue,
    digest::compute_digest,
    mount::{mount_cove_file, MountOptions, MountedCoveFile, OutputRepresentation},
    page::{page_uses_payload_elision, ColumnPageIndex},
    page_payload::{ColumnPagePayloadV1, PageBufferKind},
    postscript::CovePostscriptV1,
    segment::TableSegmentPayloadV1,
    table::{ColumnEntry, TableEntry},
    types,
    validity::ValidityBitmap,
    CoveError,
};
use cove_coverage::{CoverageExactnessV2, CoverageGranularityV2, CoverageProofStrengthV2};

#[derive(Debug, Clone, Default)]
pub struct CoviBuildOptions {
    pub target: CoviBuildTarget,
    pub table_id: Option<u32>,
    pub column_ids: Vec<u32>,
    pub all_columns: bool,
    pub include_index_only_counts: bool,
    pub include_index_only_min_max: bool,
    pub include_index_only_distinct_count: bool,
    pub include_index_only_exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoviBuildTarget {
    #[default]
    TableColumn,
    ObjectProperty {
        object_type_id: u32,
        property_id: u32,
    },
    ObjectPath {
        object_type_id: u32,
        path_ref: u32,
    },
    SemanticDimension {
        semantic_dimension_ref: u32,
    },
    DimensionalTuple {
        semantic_dimension_ref: u32,
    },
}

#[derive(Debug, Clone, Copy)]
struct IndexedTargetMetadata {
    indexed_target_kind: CoviIndexedTargetKindV2,
    table_id: u32,
    column_id: u32,
    object_type_id: u32,
    property_id: u32,
    path_ref: u32,
    semantic_dimension_ref: u32,
}

pub fn build_covi_from_cove_bytes(
    input: &[u8],
    options: &CoviBuildOptions,
) -> Result<Vec<u8>, CoveError> {
    if options.all_columns && !options.column_ids.is_empty() {
        return Err(CoveError::BadCovi);
    }
    let postscript = CovePostscriptV1::parse_from_tail(input)?;
    let mounted = mount_cove_file(
        input,
        MountOptions {
            representation: OutputRepresentation::DecodeToValue,
            ..MountOptions::default()
        },
        None,
    )?;
    let table_catalog = mounted
        .table_catalog
        .as_ref()
        .ok_or_else(|| CoveError::BadSchema("no table catalog found".into()))?;
    let table = match options.table_id {
        Some(table_id) => table_catalog
            .tables
            .iter()
            .find(|table| table.table_id == table_id)
            .ok_or_else(|| CoveError::BadSchema(format!("table_id {table_id} not found")))?,
        None => table_catalog
            .tables
            .first()
            .ok_or_else(|| CoveError::BadSchema("table catalog is empty".into()))?,
    };
    let columns = selected_columns(table, options)?;
    if columns.is_empty() {
        return Err(CoveError::BadSchema("no columns selected".into()));
    }

    let dataset_id = mounted.header.file_id;
    let snapshot_id = derived_snapshot_id(input, postscript.footer.crc32c);
    let file_digest = compute_digest(DigestAlgorithm::Sha256, input)?;
    let referenced_file = CoviReferencedFileV2 {
        file_ref: 0,
        flags: 0,
        file_id: mounted.header.file_id,
        file_len: input.len() as u64,
        footer_crc32c: postscript.footer.crc32c,
        digest_algorithm: DigestAlgorithm::Sha256 as u16,
        digest_len: file_digest
            .len()
            .try_into()
            .map_err(|_| CoveError::ArithOverflow)?,
        digest_offset: 0,
        uri_ref: u32::MAX,
        schema_fingerprint_ref: u32::MAX,
        checksum: 0,
    };
    let snapshot_validity = CoviSnapshotValidityV2 {
        snapshot_validity_ref: 0,
        dataset_id,
        snapshot_id,
        schema_fingerprint_ref: u32::MAX,
        semantic_map_fingerprint_ref: u32::MAX,
        external_visibility_ref: u32::MAX,
        data_checksum_root_ref: u32::MAX,
        valid_from_us: mounted.header.created_at_us,
        valid_until_us: i64::MAX,
        flags: 0,
        checksum: 0,
    };
    let mut roots = Vec::new();
    let mut capabilities = Vec::new();
    let mut section_payloads = vec![CoviSectionPayloadV2 {
        section_id: 1,
        section_kind: CoviSectionKindV2::StringTable,
        payload: file_digest,
        item_count: 1,
        required_features: 0,
        optional_features: 0,
    }];
    for (root_index, column) in columns.iter().enumerate() {
        let root_id = u32::try_from(root_index).map_err(|_| CoveError::ArithOverflow)?;
        let built_index = build_column_index(
            root_id,
            input,
            &mounted,
            table,
            column,
            options.include_index_only_counts.then_some(root_id),
        )?;
        let key_section_id = 2 + root_id * 4;
        let entry_section_id = key_section_id + 1;
        let postings_section_id = key_section_id + 2;
        let aggregate_section_id = key_section_id + 3;
        let include_aggregate_block = options.include_index_only_counts
            || options.include_index_only_min_max
            || options.include_index_only_distinct_count
            || options.include_index_only_exists;
        let aggregate_block_section_id = if include_aggregate_block {
            aggregate_section_id
        } else {
            u32::MAX
        };
        let target = indexed_target_metadata(options.target, table.table_id, column.column_id);
        roots.push(CoviIndexRootV2 {
            index_root_id: root_id,
            indexed_target_kind: target.indexed_target_kind,
            index_kind: CoviIndexKindV2::Sorted,
            coverage_granularity: CoverageGranularityV2::Morsel as u8,
            proof_strength: CoverageProofStrengthV2::ExactConservative as u8,
            exactness: CoverageExactnessV2::Exact as u8,
            flags: 0,
            table_id: target.table_id,
            column_id: target.column_id,
            object_type_id: target.object_type_id,
            property_id: target.property_id,
            path_ref: target.path_ref,
            semantic_dimension_ref: target.semantic_dimension_ref,
            logical_type: column.logical as u16,
            physical_kind: column.physical as u8,
            key_encoding_kind: built_index.key_encoding_kind as u8,
            comparator_kind: built_index.comparator_kind as u16,
            collation_id: column.collation_id,
            null_semantics: 0,
            sort_order: column.sort_order.min(u16::from(u8::MAX)) as u8,
            value_count: table.row_count,
            distinct_count: built_index.distinct_count,
            null_count: built_index.null_count,
            min_key_ref: built_index.min_key_ref,
            max_key_ref: built_index.max_key_ref,
            key_block_section_id: key_section_id,
            entry_block_section_id: entry_section_id,
            postings_block_section_id: postings_section_id,
            aggregate_block_section_id,
            coverage_set_ref: u32::MAX,
            capability_ref: root_id,
            snapshot_validity_ref: 0,
            checksum: 0,
        });
        capabilities.push(IndexCapabilityV2 {
            capability_id: root_id,
            index_root_id: root_id,
            flags: 0,
            supports_eq: 1,
            supports_range: u8::from(built_index.supports_range),
            supports_membership: 1,
            supports_prefix: 0,
            supports_contains: 0,
            supports_count: u8::from(
                options.include_index_only_counts || options.include_index_only_exists,
            ),
            supports_min: u8::from(options.include_index_only_min_max),
            supports_max: u8::from(options.include_index_only_min_max),
            supports_sum: 0,
            supports_distinct_count: u8::from(options.include_index_only_distinct_count),
            supports_join_coverage: 0,
            supports_index_only: u8::from(include_aggregate_block),
            exactness: IndexCapabilityExactnessV2::Exact,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            null_semantics: 0,
            reserved: 0,
            snapshot_validity_ref: 0,
            coverage_provider_ref: u32::MAX,
            checksum: 0,
        });
        section_payloads.extend([
            CoviSectionPayloadV2 {
                section_id: key_section_id,
                section_kind: CoviSectionKindV2::KeyBlock,
                payload: built_index.key_block,
                item_count: built_index.distinct_count,
                required_features: 0,
                optional_features: 0,
            },
            CoviSectionPayloadV2 {
                section_id: entry_section_id,
                section_kind: CoviSectionKindV2::EntryBlock,
                payload: built_index.entry_block,
                item_count: built_index.distinct_count,
                required_features: 0,
                optional_features: 0,
            },
            CoviSectionPayloadV2 {
                section_id: postings_section_id,
                section_kind: CoviSectionKindV2::PostingsBlock,
                payload: built_index.postings_block,
                item_count: built_index.distinct_count,
                required_features: 0,
                optional_features: 0,
            },
        ]);
        if include_aggregate_block {
            let aggregate_block = aggregate_answer_block(
                root_id,
                table.row_count,
                built_index.null_count,
                built_index.distinct_count,
                options,
                built_index.min_key.as_deref(),
                built_index.max_key.as_deref(),
            )?;
            section_payloads.push(CoviSectionPayloadV2 {
                section_id: aggregate_section_id,
                section_kind: CoviSectionKindV2::AggregateAnswerBlock,
                payload: aggregate_block,
                item_count: 1,
                required_features: 0,
                optional_features: 0,
            });
        }
    }

    let bytes = CoviArtifactV2::serialize_with_sections(
        dataset_id,
        snapshot_id,
        &[referenced_file],
        &[snapshot_validity],
        &roots,
        &capabilities,
        &section_payloads,
    )?;
    CoviArtifactV2::parse(&bytes)?;
    Ok(bytes)
}

fn indexed_target_metadata(
    target: CoviBuildTarget,
    table_id: u32,
    column_id: u32,
) -> IndexedTargetMetadata {
    match target {
        CoviBuildTarget::TableColumn => IndexedTargetMetadata {
            indexed_target_kind: CoviIndexedTargetKindV2::TableColumn,
            table_id,
            column_id,
            object_type_id: u32::MAX,
            property_id: u32::MAX,
            path_ref: u32::MAX,
            semantic_dimension_ref: u32::MAX,
        },
        CoviBuildTarget::ObjectProperty {
            object_type_id,
            property_id,
        } => IndexedTargetMetadata {
            indexed_target_kind: CoviIndexedTargetKindV2::ObjectProperty,
            table_id: u32::MAX,
            column_id: u32::MAX,
            object_type_id,
            property_id,
            path_ref: u32::MAX,
            semantic_dimension_ref: u32::MAX,
        },
        CoviBuildTarget::ObjectPath {
            object_type_id,
            path_ref,
        } => IndexedTargetMetadata {
            indexed_target_kind: CoviIndexedTargetKindV2::ObjectPath,
            table_id: u32::MAX,
            column_id: u32::MAX,
            object_type_id,
            property_id: u32::MAX,
            path_ref,
            semantic_dimension_ref: u32::MAX,
        },
        CoviBuildTarget::SemanticDimension {
            semantic_dimension_ref,
        } => IndexedTargetMetadata {
            indexed_target_kind: CoviIndexedTargetKindV2::SemanticDimension,
            table_id: u32::MAX,
            column_id: u32::MAX,
            object_type_id: u32::MAX,
            property_id: u32::MAX,
            path_ref: u32::MAX,
            semantic_dimension_ref,
        },
        CoviBuildTarget::DimensionalTuple {
            semantic_dimension_ref,
        } => IndexedTargetMetadata {
            indexed_target_kind: CoviIndexedTargetKindV2::DimensionalTuple,
            table_id: u32::MAX,
            column_id: u32::MAX,
            object_type_id: u32::MAX,
            property_id: u32::MAX,
            path_ref: u32::MAX,
            semantic_dimension_ref,
        },
    }
}

fn selected_columns<'a>(
    table: &'a TableEntry,
    options: &CoviBuildOptions,
) -> Result<Vec<&'a ColumnEntry>, CoveError> {
    if options.all_columns {
        return Ok(table.columns.iter().collect());
    }
    if options.column_ids.is_empty() {
        return table
            .columns
            .first()
            .map(|column| vec![column])
            .ok_or_else(|| CoveError::BadSchema("selected table has no columns".into()));
    }
    let mut columns = Vec::new();
    for column_id in &options.column_ids {
        let column = table
            .columns
            .iter()
            .find(|column| column.column_id == *column_id)
            .ok_or_else(|| {
                CoveError::BadSchema(format!(
                    "column_id {column_id} not found in table {}",
                    table.table_id
                ))
            })?;
        columns.push(column);
    }
    Ok(columns)
}

struct BuiltColumnIndex {
    key_block: Vec<u8>,
    entry_block: Vec<u8>,
    postings_block: Vec<u8>,
    key_encoding_kind: CoviKeyEncodingKindV2,
    comparator_kind: CoviComparatorKindV2,
    supports_range: bool,
    distinct_count: u64,
    null_count: u64,
    min_key_ref: u32,
    max_key_ref: u32,
    min_key: Option<Vec<u8>>,
    max_key: Option<Vec<u8>>,
}

fn build_column_index(
    root_id: u32,
    input: &[u8],
    mounted: &MountedCoveFile,
    table: &TableEntry,
    column: &ColumnEntry,
    aggregate_block_id: Option<u32>,
) -> Result<BuiltColumnIndex, CoveError> {
    let raw_filecode_mode =
        column.physical == CovePhysicalKind::FileCode && mounted.dictionary.is_none();
    let key_encoding_kind = if raw_filecode_mode {
        CoviKeyEncodingKindV2::FileCode
    } else {
        CoviKeyEncodingKindV2::CanonicalValueBytes
    };
    let comparator_kind = if raw_filecode_mode {
        CoviComparatorKindV2::DomainRankOrdering
    } else {
        CoviComparatorKindV2::CanonicalOrdering
    };
    let supports_range = !raw_filecode_mode;
    let mut keys: BTreeMap<Vec<u8>, Vec<CoviRowRangePostingV2>> = BTreeMap::new();
    let mut null_count = 0u64;
    let mut rows_seen = 0u64;

    for section in mounted
        .footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::TableSegmentData as u16)
    {
        let segment_bytes = section_payload(input, section)?;
        let segment = TableSegmentPayloadV1::parse_with_required_features(
            &segment_bytes,
            mounted.header.required_features,
        )?;
        if segment.header.table_id != table.table_id {
            continue;
        }
        let Some(column_dir) = segment
            .columns
            .iter()
            .find(|entry| entry.column_id == column.column_id)
        else {
            continue;
        };
        let page_index_start =
            usize::try_from(column_dir.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
        let page_index_len =
            usize::try_from(column_dir.page_index_length).map_err(|_| CoveError::OffsetRange)?;
        let page_index_end = page_index_start
            .checked_add(page_index_len)
            .ok_or(CoveError::ArithOverflow)?;
        let page_index = ColumnPageIndex::parse(&segment_bytes[page_index_start..page_index_end])?;
        for page in page_index.entries {
            let morsel = segment
                .morsels
                .entries
                .get(page.morsel_id as usize)
                .ok_or(CoveError::SegmentCorrupt)?;
            if page_uses_payload_elision(page.flags) && page.page_length == 0 {
                if page.null_count == page.row_count {
                    null_count = null_count
                        .checked_add(u64::from(page.row_count))
                        .ok_or(CoveError::ArithOverflow)?;
                    rows_seen = rows_seen
                        .checked_add(u64::from(page.row_count))
                        .ok_or(CoveError::ArithOverflow)?;
                    continue;
                }
                return Err(CoveError::BadCovi);
            }
            let page_start =
                usize::try_from(page.page_offset).map_err(|_| CoveError::OffsetRange)?;
            let page_len = usize::try_from(page.page_length).map_err(|_| CoveError::OffsetRange)?;
            let page_end = page_start
                .checked_add(page_len)
                .ok_or(CoveError::ArithOverflow)?;
            let page_wire = &segment_bytes[page_start..page_end];
            let decoded_page = column_page_payload(page_wire, &page)?;
            let payload = ColumnPagePayloadV1::parse(&decoded_page)?;
            let root = payload.root_node()?;
            let values = payload.buffer_bytes(PageBufferKind::Values)?.unwrap_or(&[]);
            let validity = payload
                .buffer_bytes(PageBufferKind::NullBitmap)?
                .map(|bytes| ValidityBitmap::new(bytes, u64::from(page.row_count)));
            let array = EncodedArray::new(
                root.logical_type,
                root.physical_kind,
                u64::from(page.row_count),
                root.encoding_kind,
                validity,
                values,
                None,
            );
            let decoded_rows = array.decode_all_rows()?;
            for (row_index, value) in decoded_rows.into_iter().enumerate() {
                rows_seen = rows_seen.checked_add(1).ok_or(CoveError::ArithOverflow)?;
                let Some(key) = key_for_value(
                    value,
                    column.logical,
                    mounted.dictionary.as_ref(),
                    raw_filecode_mode,
                )?
                else {
                    null_count = null_count.checked_add(1).ok_or(CoveError::ArithOverflow)?;
                    continue;
                };
                let row_start = segment
                    .header
                    .row_start
                    .checked_add(u64::from(morsel.first_row_in_segment))
                    .and_then(|start| start.checked_add(row_index as u64))
                    .ok_or(CoveError::ArithOverflow)?;
                append_row_range(
                    keys.entry(key).or_default(),
                    CoviRowRangePostingV2 {
                        file_ref: 0,
                        table_id: table.table_id,
                        segment_id: segment.header.segment_id,
                        morsel_id: page.morsel_id,
                        row_start,
                        row_count: 1,
                        flags: 0,
                        checksum: 0,
                    },
                )?;
            }
        }
    }

    if rows_seen != table.row_count {
        return Err(CoveError::BadCovi);
    }
    build_blocks_from_keys(
        root_id,
        keys,
        key_encoding_kind,
        comparator_kind,
        supports_range,
        null_count,
        aggregate_block_id,
    )
}

fn build_blocks_from_keys(
    root_id: u32,
    keys: BTreeMap<Vec<u8>, Vec<CoviRowRangePostingV2>>,
    key_encoding_kind: CoviKeyEncodingKindV2,
    comparator_kind: CoviComparatorKindV2,
    supports_range: bool,
    null_count: u64,
    aggregate_block_id: Option<u32>,
) -> Result<BuiltColumnIndex, CoveError> {
    let distinct_count = u64::try_from(keys.len()).map_err(|_| CoveError::ArithOverflow)?;
    let min_key = keys.keys().next().cloned();
    let max_key = keys.keys().next_back().cloned();
    let mut key_data = Vec::new();
    let mut entries = Vec::new();
    let mut postings = Vec::new();
    let mut postings_payload = Vec::new();

    for (entry_index, (key, ranges)) in keys.iter().enumerate() {
        let ranges = normalize_row_ranges(ranges)?;
        let entry_ref = u32::try_from(entry_index).map_err(|_| CoveError::ArithOverflow)?;
        let key_offset = u64::try_from(key_data.len()).map_err(|_| CoveError::ArithOverflow)?;
        let key_length = u32::try_from(key.len()).map_err(|_| CoveError::ArithOverflow)?;
        key_data.extend_from_slice(key);

        let payload_offset = postings_payload.len();
        for range in &ranges {
            postings_payload.extend_from_slice(&range.serialize()?);
        }
        let payload_length = ranges
            .len()
            .checked_mul(CoviRowRangePostingV2::LEN)
            .ok_or(CoveError::ArithOverflow)?;
        postings.push(CoviPostingsHeaderV2 {
            postings_ref: entry_ref,
            index_root_id: root_id,
            representation: CoviPostingRepresentationV2::RowRangeList,
            target_granularity: CoverageGranularityV2::RowRange as u8,
            flags: 0,
            item_count: u64::try_from(ranges.len()).map_err(|_| CoveError::ArithOverflow)?,
            payload_offset: u64::try_from(payload_offset).map_err(|_| CoveError::ArithOverflow)?,
            payload_length: u64::try_from(payload_length).map_err(|_| CoveError::ArithOverflow)?,
            coverage_set_ref: u32::MAX,
            checksum: 0,
        });
        entries.push(CoviIndexEntryV2 {
            entry_ref,
            index_root_id: root_id,
            entry_id: u64::from(entry_ref),
            key_kind: key_encoding_kind,
            comparator_kind,
            flags: 0,
            key_offset,
            key_length,
            key_hash64: stable_hash64(key),
            postings_ref: entry_ref,
            coverage_set_ref: u32::MAX,
            aggregate_answer_ref: u32::MAX,
            next_duplicate_ref: u32::MAX,
            checksum: 0,
        });
    }

    let key_block = CoviKeyBlockV2 {
        header: CoviKeyBlockHeaderV2 {
            magic: CoviKeyBlockHeaderV2::MAGIC,
            version_major: 2,
            version_minor: 0,
            header_len: CoviKeyBlockHeaderV2::LEN as u16,
            reserved0: 0,
            key_block_id: root_id,
            index_root_id: root_id,
            key_count: distinct_count,
            encoding_kind: key_encoding_kind,
            comparator_kind,
            flags: 0,
            key_data_offset: CoviKeyBlockHeaderV2::LEN as u64,
            key_data_length: key_data.len() as u64,
            checksum: 0,
        },
        key_data,
    }
    .serialize()?;
    let entry_block = CoviEntryBlockV2 {
        header: CoviEntryBlockHeaderV2 {
            magic: CoviEntryBlockHeaderV2::MAGIC,
            version_major: 2,
            version_minor: 0,
            header_len: CoviEntryBlockHeaderV2::LEN as u16,
            entry_len: CoviIndexEntryV2::LEN as u16,
            entry_block_id: root_id,
            index_root_id: root_id,
            entry_count: entries.len() as u32,
            key_block_id: root_id,
            postings_block_id: root_id,
            aggregate_block_id: aggregate_block_id.unwrap_or(u32::MAX),
            entries_offset: CoviEntryBlockHeaderV2::LEN as u64,
            entries_length: (entries.len() * CoviIndexEntryV2::LEN) as u64,
            flags: 0,
            checksum: 0,
        },
        entries,
    }
    .serialize()?;
    let postings_block = CoviPostingsBlockV2 {
        header: CoviPostingsBlockHeaderV2 {
            magic: CoviPostingsBlockHeaderV2::MAGIC,
            version_major: 2,
            version_minor: 0,
            header_len: CoviPostingsBlockHeaderV2::LEN as u16,
            postings_header_len: CoviPostingsHeaderV2::LEN as u16,
            postings_block_id: root_id,
            index_root_id: root_id,
            postings_count: postings.len() as u32,
            row_ordinal_set_count: 0,
            postings_headers_offset: CoviPostingsBlockHeaderV2::LEN as u64,
            row_ordinal_headers_offset: 0,
            postings_payload_offset: 0,
            postings_payload_length: postings_payload.len() as u64,
            flags: 0,
            checksum: 0,
        },
        postings,
        row_ordinal_sets: Vec::new(),
        payload: postings_payload,
    }
    .serialize()?;
    let max_key_ref = if distinct_count == 0 {
        u32::MAX
    } else {
        u32::try_from(distinct_count - 1).map_err(|_| CoveError::ArithOverflow)?
    };
    Ok(BuiltColumnIndex {
        key_block,
        entry_block,
        postings_block,
        key_encoding_kind,
        comparator_kind,
        supports_range,
        distinct_count,
        null_count,
        min_key_ref: if distinct_count == 0 { u32::MAX } else { 0 },
        max_key_ref,
        min_key,
        max_key,
    })
}

fn aggregate_answer_block(
    root_id: u32,
    row_count: u64,
    null_count: u64,
    distinct_count: u64,
    options: &CoviBuildOptions,
    min_key: Option<&[u8]>,
    max_key: Option<&[u8]>,
) -> Result<Vec<u8>, CoveError> {
    let non_null_count = row_count
        .checked_sub(null_count)
        .ok_or(CoveError::ArithOverflow)?;
    let mut answers = Vec::new();
    let mut payload = Vec::new();
    if options.include_index_only_counts {
        push_aggregate_answer(
            &mut answers,
            &mut payload,
            root_id,
            CoviAggregateKindV2::Count,
            row_count,
            null_count,
            non_null_count,
            None,
        )?;
    }
    if options.include_index_only_exists {
        push_aggregate_answer(
            &mut answers,
            &mut payload,
            root_id,
            CoviAggregateKindV2::Exists,
            row_count,
            null_count,
            non_null_count,
            None,
        )?;
    }
    if options.include_index_only_min_max {
        push_aggregate_answer(
            &mut answers,
            &mut payload,
            root_id,
            CoviAggregateKindV2::Min,
            row_count,
            null_count,
            non_null_count,
            min_key,
        )?;
        push_aggregate_answer(
            &mut answers,
            &mut payload,
            root_id,
            CoviAggregateKindV2::Max,
            row_count,
            null_count,
            non_null_count,
            max_key,
        )?;
    }
    if options.include_index_only_distinct_count {
        push_aggregate_answer(
            &mut answers,
            &mut payload,
            root_id,
            CoviAggregateKindV2::DistinctCount,
            distinct_count,
            0,
            distinct_count,
            None,
        )?;
    }
    CoviAggregateAnswerBlockV2 {
        header: CoviAggregateAnswerBlockHeaderV2 {
            magic: CoviAggregateAnswerBlockHeaderV2::MAGIC,
            version_major: 2,
            version_minor: 0,
            header_len: CoviAggregateAnswerBlockHeaderV2::LEN as u16,
            aggregate_answer_len: CoviAggregateAnswerV2::LEN as u16,
            aggregate_block_id: root_id,
            index_root_id: root_id,
            aggregate_answer_count: answers.len() as u32,
            aggregate_answers_offset: CoviAggregateAnswerBlockHeaderV2::LEN as u64,
            aggregate_payload_offset: 0,
            aggregate_payload_length: payload.len() as u64,
            flags: 0,
            checksum: 0,
        },
        answers,
        payload,
    }
    .serialize()
}

#[allow(clippy::too_many_arguments)]
fn push_aggregate_answer(
    answers: &mut Vec<CoviAggregateAnswerV2>,
    payload: &mut Vec<u8>,
    root_id: u32,
    aggregate_kind: CoviAggregateKindV2,
    row_count: u64,
    null_count: u64,
    non_null_count: u64,
    value: Option<&[u8]>,
) -> Result<(), CoveError> {
    let value_ref = if let Some(value) = value {
        let offset = u32::try_from(payload.len()).map_err(|_| CoveError::ArithOverflow)?;
        payload.extend_from_slice(value);
        offset
    } else {
        u32::MAX
    };
    answers.push(CoviAggregateAnswerV2 {
        aggregate_answer_ref: u32::try_from(answers.len()).map_err(|_| CoveError::ArithOverflow)?,
        index_root_id: root_id,
        aggregate_kind: aggregate_kind as u16,
        exactness: IndexCapabilityExactnessV2::Exact as u8,
        null_semantics: 0,
        flags: 0,
        row_count,
        null_count,
        non_null_count,
        value_ref,
        predicate_form_ref: u32::MAX,
        snapshot_validity_ref: 0,
        checksum: 0,
    });
    Ok(())
}

fn append_row_range(
    ranges: &mut Vec<CoviRowRangePostingV2>,
    next: CoviRowRangePostingV2,
) -> Result<(), CoveError> {
    if let Some(last) = ranges.last_mut() {
        let last_end = last
            .row_start
            .checked_add(last.row_count)
            .ok_or(CoveError::ArithOverflow)?;
        let same_scope = last.file_ref == next.file_ref
            && last.table_id == next.table_id
            && last.segment_id == next.segment_id
            && last.morsel_id == next.morsel_id;
        if same_scope {
            if last_end == next.row_start {
                last.row_count = last
                    .row_count
                    .checked_add(next.row_count)
                    .ok_or(CoveError::ArithOverflow)?;
                return Ok(());
            }
            if last_end > next.row_start {
                return Err(CoveError::BadCovi);
            }
        }
    }
    ranges.push(next);
    Ok(())
}

fn normalize_row_ranges(
    ranges: &[CoviRowRangePostingV2],
) -> Result<Vec<CoviRowRangePostingV2>, CoveError> {
    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|range| {
        (
            range.file_ref,
            range.table_id,
            range.segment_id,
            range.morsel_id,
            range.row_start,
        )
    });
    let mut normalized = Vec::with_capacity(sorted.len());
    for range in sorted {
        append_row_range(&mut normalized, range)?;
    }
    Ok(normalized)
}

fn key_for_value(
    value: CoveArrayValue<'_>,
    logical: CoveLogicalType,
    dictionary: Option<&cove_core::dictionary::FileDictionary>,
    raw_filecode_mode: bool,
) -> Result<Option<Vec<u8>>, CoveError> {
    match value {
        CoveArrayValue::Null => Ok(None),
        CoveArrayValue::FileCode(code) if raw_filecode_mode => {
            Ok(Some(code.to_le_bytes().to_vec()))
        }
        CoveArrayValue::FileCode(code) => {
            let dictionary = dictionary.ok_or(CoveError::BadCovi)?;
            let entry = dictionary.get_entry(code)?;
            let bytes = match dictionary.decode_value(code)? {
                DictionaryValue::RawBytes(bytes) => bytes,
                DictionaryValue::RedactedPresent => return Err(CoveError::BadCovi),
                _ => return Err(CoveError::BadCovi),
            };
            Ok(Some(tagged_key(entry.value_tag, bytes)))
        }
        CoveArrayValue::NumCode(code) | CoveArrayValue::Varint(code) => {
            key_from_numcode(logical, code).map(Some)
        }
        CoveArrayValue::Int64(value) => key_from_i64(logical, value).map(Some),
        CoveArrayValue::Boolean(value) | CoveArrayValue::ValidityBit(value) => {
            tagged_canonical(CanonicalValue::Bool(value)).map(Some)
        }
        CoveArrayValue::Bytes(bytes) => key_from_bytes(logical, bytes).map(Some),
        CoveArrayValue::OwnedBytes(bytes) => key_from_bytes(logical, &bytes).map(Some),
        CoveArrayValue::DictValue(_) => Err(CoveError::BadCovi),
        _ => Err(CoveError::BadCovi),
    }
}

fn key_from_numcode(logical: CoveLogicalType, code: u64) -> Result<Vec<u8>, CoveError> {
    let value = match logical {
        CoveLogicalType::Bool => CanonicalValue::Bool(code != 0),
        CoveLogicalType::Int8 => CanonicalValue::Int {
            width: 1,
            value: i128::from(types::numcode_as_i8(code)),
        },
        CoveLogicalType::Int16 => CanonicalValue::Int {
            width: 2,
            value: i128::from(types::numcode_as_i16(code)),
        },
        CoveLogicalType::Int32 => CanonicalValue::Int {
            width: 4,
            value: i128::from(types::numcode_as_i32(code)),
        },
        CoveLogicalType::Int64 => CanonicalValue::Int {
            width: 8,
            value: i128::from(types::numcode_as_i64(code)),
        },
        CoveLogicalType::UInt8 => CanonicalValue::Uint {
            width: 1,
            value: u128::from(types::numcode_as_u8(code)),
        },
        CoveLogicalType::UInt16 => CanonicalValue::Uint {
            width: 2,
            value: u128::from(types::numcode_as_u16(code)),
        },
        CoveLogicalType::UInt32 => CanonicalValue::Uint {
            width: 4,
            value: u128::from(types::numcode_as_u32(code)),
        },
        CoveLogicalType::UInt64 => CanonicalValue::Uint {
            width: 8,
            value: u128::from(types::numcode_as_u64(code)),
        },
        CoveLogicalType::Float32 => CanonicalValue::Float32(types::numcode_as_f32(code)),
        CoveLogicalType::Float64 => CanonicalValue::Float64(types::numcode_as_f64(code)),
        CoveLogicalType::Decimal64 => CanonicalValue::Decimal64(types::numcode_as_decimal64(code)),
        CoveLogicalType::DateDays => CanonicalValue::DateDays(types::numcode_as_date_days(code)),
        CoveLogicalType::TimestampMicros => {
            CanonicalValue::TimestampMicros(types::numcode_as_timestamp_micros(code))
        }
        CoveLogicalType::TimestampNanos => {
            CanonicalValue::TimestampNanos(types::numcode_as_timestamp_nanos(code))
        }
        _ => return Err(CoveError::BadCovi),
    };
    tagged_canonical(value)
}

fn key_from_i64(logical: CoveLogicalType, value: i64) -> Result<Vec<u8>, CoveError> {
    let canonical = match logical {
        CoveLogicalType::Int8 => CanonicalValue::Int {
            width: 1,
            value: i128::from(value),
        },
        CoveLogicalType::Int16 => CanonicalValue::Int {
            width: 2,
            value: i128::from(value),
        },
        CoveLogicalType::Int32 => CanonicalValue::Int {
            width: 4,
            value: i128::from(value),
        },
        CoveLogicalType::Int64 => CanonicalValue::Int {
            width: 8,
            value: i128::from(value),
        },
        CoveLogicalType::Decimal64 => CanonicalValue::Decimal64(value),
        CoveLogicalType::DateDays => {
            CanonicalValue::DateDays(i32::try_from(value).map_err(|_| CoveError::BadCovi)?)
        }
        CoveLogicalType::TimestampMicros => CanonicalValue::TimestampMicros(value),
        CoveLogicalType::TimestampNanos => CanonicalValue::TimestampNanos(value),
        _ => return Err(CoveError::BadCovi),
    };
    tagged_canonical(canonical)
}

fn key_from_bytes(logical: CoveLogicalType, bytes: &[u8]) -> Result<Vec<u8>, CoveError> {
    let canonical = match logical {
        CoveLogicalType::Bool => match bytes {
            [0] => CanonicalValue::Bool(false),
            [1] => CanonicalValue::Bool(true),
            _ => return Err(CoveError::BadCovi),
        },
        CoveLogicalType::Int8 => CanonicalValue::Int {
            width: 1,
            value: i128::from(i8::from_le_bytes(fixed_array(bytes)?)),
        },
        CoveLogicalType::Int16 => CanonicalValue::Int {
            width: 2,
            value: i128::from(i16::from_le_bytes(fixed_array(bytes)?)),
        },
        CoveLogicalType::Int32 => CanonicalValue::Int {
            width: 4,
            value: i128::from(i32::from_le_bytes(fixed_array(bytes)?)),
        },
        CoveLogicalType::Int64 => CanonicalValue::Int {
            width: 8,
            value: i128::from(i64::from_le_bytes(fixed_array(bytes)?)),
        },
        CoveLogicalType::UInt8 => CanonicalValue::Uint {
            width: 1,
            value: u128::from(u8::from_le_bytes(fixed_array(bytes)?)),
        },
        CoveLogicalType::UInt16 => CanonicalValue::Uint {
            width: 2,
            value: u128::from(u16::from_le_bytes(fixed_array(bytes)?)),
        },
        CoveLogicalType::UInt32 => CanonicalValue::Uint {
            width: 4,
            value: u128::from(u32::from_le_bytes(fixed_array(bytes)?)),
        },
        CoveLogicalType::UInt64 => CanonicalValue::Uint {
            width: 8,
            value: u128::from(u64::from_le_bytes(fixed_array(bytes)?)),
        },
        CoveLogicalType::Float32 => {
            CanonicalValue::Float32(f32::from_bits(u32::from_le_bytes(fixed_array(bytes)?)))
        }
        CoveLogicalType::Float64 => {
            CanonicalValue::Float64(f64::from_bits(u64::from_le_bytes(fixed_array(bytes)?)))
        }
        CoveLogicalType::Decimal64 => {
            CanonicalValue::Decimal64(i64::from_le_bytes(fixed_array(bytes)?))
        }
        CoveLogicalType::DateDays => {
            CanonicalValue::DateDays(i32::from_le_bytes(fixed_array(bytes)?))
        }
        CoveLogicalType::TimestampMicros => {
            CanonicalValue::TimestampMicros(i64::from_le_bytes(fixed_array(bytes)?))
        }
        CoveLogicalType::TimestampNanos => {
            CanonicalValue::TimestampNanos(i64::from_le_bytes(fixed_array(bytes)?))
        }
        CoveLogicalType::Utf8 => {
            CanonicalValue::Utf8(std::str::from_utf8(bytes).map_err(|_| CoveError::BadCovi)?)
        }
        CoveLogicalType::Json => {
            CanonicalValue::Json(std::str::from_utf8(bytes).map_err(|_| CoveError::BadCovi)?)
        }
        CoveLogicalType::Binary => CanonicalValue::Bytes(bytes),
        CoveLogicalType::Uuid => {
            let uuid: [u8; 16] = bytes.try_into().map_err(|_| CoveError::BadCovi)?;
            CanonicalValue::Uuid(uuid)
        }
        CoveLogicalType::Decimal128 => {
            let decimal: [u8; 16] = bytes.try_into().map_err(|_| CoveError::BadCovi)?;
            CanonicalValue::Decimal128(i128::from_le_bytes(decimal))
        }
        _ => return Err(CoveError::BadCovi),
    };
    tagged_canonical(canonical)
}

fn fixed_array<const N: usize>(bytes: &[u8]) -> Result<[u8; N], CoveError> {
    bytes.try_into().map_err(|_| CoveError::BadCovi)
}

fn tagged_canonical(value: CanonicalValue<'_>) -> Result<Vec<u8>, CoveError> {
    Ok(tagged_key(value.value_tag() as u16, value.encode()?))
}

fn tagged_key(value_tag: u16, payload: Vec<u8>) -> Vec<u8> {
    let mut key = Vec::with_capacity(2 + payload.len());
    key.extend_from_slice(&value_tag.to_le_bytes());
    key.extend_from_slice(&payload);
    key
}

fn stable_hash64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn derived_snapshot_id(bytes: &[u8], footer_crc32c: u32) -> [u8; 16] {
    let mut snapshot_id = [0u8; 16];
    snapshot_id[0..4].copy_from_slice(&checksum::crc32c(bytes).to_le_bytes());
    snapshot_id[4..8].copy_from_slice(&footer_crc32c.to_le_bytes());
    snapshot_id[8..16].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
    snapshot_id
}
