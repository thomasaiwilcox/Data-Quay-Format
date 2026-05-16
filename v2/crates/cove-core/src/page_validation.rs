use crate::{
    checksum,
    codec::{
        materialize_registered_page_payload, CodecExtensionDescriptorV2, RegisteredCodecResolver,
        StableRegisteredCodecResolver,
    },
    compression,
    constants::{CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
    dictionary::FileDictionaryView,
    encoding::{
        bit_packed::{BitPacked, BitPackedPayload},
        constant::ConstantPayload,
        delta::{Delta, DeltaPayload},
        frame_of_reference::{ForPayload, FrameOfReference},
        local_codebook::{LocalCodebookPayload, LocalCodebookValue},
        patched_base::{PatchedBase, PatchedBasePayload},
        rle::{Rle, RlePayload},
        run_end::{RunEnd, RunEndPayload},
        sparse::{Sparse, SparsePayload},
        Encoding,
    },
    nested_schema::NestedSchemaNodeV1,
    page::{
        page_flag_codec, ColumnPageIndexEntryV1, PAGE_FLAG_ALL_NON_NULL,
        PAGE_FLAG_STATS_ONLY_CONSTANT, PAGE_FLAG_VALUE_STREAM_ELIDED,
    },
    page_payload::{ColumnPagePayloadV1, PageBufferKind, PagePayloadTreeNode},
    wire,
    zone_stats::{StatKind, ZoneStatFlags, ZoneStatsEntry},
    CoveError,
};

pub(crate) struct PageValidationContext<'a> {
    pub table_id: Option<u32>,
    pub segment_id: Option<u32>,
    pub column_id: u32,
    pub logical_type: CoveLogicalType,
    pub physical_kind: CovePhysicalKind,
    pub dictionary: Option<&'a FileDictionaryView<'a>>,
    pub zone_stats: Option<&'a [ZoneStatsEntry]>,
    pub codec_descriptors: &'a [CodecExtensionDescriptorV2],
    pub nested_schema: Option<&'a NestedSchemaNodeV1>,
}

pub(crate) fn validate_column_page_wire(
    context: &PageValidationContext<'_>,
    page: &ColumnPageIndexEntryV1,
    page_wire: &[u8],
) -> Result<(), CoveError> {
    let payload = compression::column_page_payload(page_wire, page)?;
    let payload = ColumnPagePayloadV1::parse(payload.as_ref())?;
    validate_column_page_payload(context, page, &payload)
}

pub(crate) fn validate_column_page_payload(
    context: &PageValidationContext<'_>,
    page: &ColumnPageIndexEntryV1,
    payload: &ColumnPagePayloadV1,
) -> Result<(), CoveError> {
    validate_column_page_payload_with_registered_codecs(
        context,
        page,
        payload,
        &StableRegisteredCodecResolver,
    )
}

pub(crate) fn validate_column_page_payload_with_registered_codecs<
    R: RegisteredCodecResolver + ?Sized,
>(
    context: &PageValidationContext<'_>,
    page: &ColumnPageIndexEntryV1,
    payload: &ColumnPagePayloadV1,
    resolver: &R,
) -> Result<(), CoveError> {
    if page.flags & PAGE_FLAG_STATS_ONLY_CONSTANT != 0 {
        return validate_stats_only_constant_page(context, page);
    }

    let root = payload.root_node()?;
    if root.encoding_kind == CoveEncodingKind::RegisteredEncoding {
        let materialized = materialize_registered_page_payload(
            payload,
            page,
            context.logical_type,
            context.physical_kind,
            context.codec_descriptors,
            resolver,
            context.dictionary.map(|dictionary| dictionary.len()),
        )?
        .ok_or(CoveError::BadCodecExtension)?;
        let mut materialized_page = page.clone();
        materialized_page.encoding_root = materialized.payload.root_node()?.encoding_kind as u32;
        return validate_column_page_payload_with_registered_codecs(
            context,
            &materialized_page,
            &materialized.payload,
            resolver,
        );
    }
    if root.logical_type != context.logical_type
        || root.physical_kind != context.physical_kind
        || root.logical_len != page.row_count
        || page.encoding_root != root.encoding_kind as u32
    {
        return Err(CoveError::PageCorrupt);
    }

    let tree = payload.tree()?;
    if let Some(schema) = context.nested_schema {
        validate_tree_matches_nested_schema(&tree, schema)?;
    }
    validate_tree_null_bitmap(payload, &tree, page.row_count, Some(page.null_count))?;
    if page.flags & PAGE_FLAG_VALUE_STREAM_ELIDED != 0 {
        validate_value_stream_elided_page(context, page, root.encoding_kind)?;
    }

    match context.physical_kind {
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            validate_nested_tree(context, payload, &tree)?;
        }
        _ => {
            let values = tree_buffer_bytes(payload, &tree, PageBufferKind::Values)?
                .ok_or(CoveError::PageCorrupt)?;
            validate_values_buffer(
                context.logical_type,
                context.physical_kind,
                root.encoding_kind,
                page.row_count,
                values,
                context.dictionary,
            )?;
        }
    }

    Ok(())
}

fn validate_nested_tree(
    context: &PageValidationContext<'_>,
    payload: &ColumnPagePayloadV1,
    tree: &PagePayloadTreeNode<'_>,
) -> Result<(), CoveError> {
    validate_nested_node(context, payload, tree, context.nested_schema)
}

fn validate_nested_node(
    context: &PageValidationContext<'_>,
    payload: &ColumnPagePayloadV1,
    tree: &PagePayloadTreeNode<'_>,
    schema: Option<&NestedSchemaNodeV1>,
) -> Result<(), CoveError> {
    match tree.node.physical_kind {
        CovePhysicalKind::List => {
            if tree.children.len() != 1 {
                return Err(CoveError::PageCorrupt);
            }
            let layout_bytes = tree_buffer_bytes(payload, tree, PageBufferKind::ChildLayout)?
                .ok_or(CoveError::PageCorrupt)?;
            let layout = crate::encoding::nested::ListLayoutPayload::parse(layout_bytes)?;
            layout.validate()?;
            if layout.layout.row_count() != tree.node.logical_len as usize {
                return Err(CoveError::PageCorrupt);
            }
            if let Some(schema) = schema {
                if schema.fixed_size_list_len != 0 {
                    let width = schema.fixed_size_list_len;
                    for pair in layout.layout.offsets.windows(2) {
                        if pair[1]
                            .checked_sub(pair[0])
                            .ok_or(CoveError::ArithOverflow)?
                            != width
                        {
                            return Err(CoveError::PageCorrupt);
                        }
                    }
                    if layout.child_row_count
                        != tree
                            .node
                            .logical_len
                            .checked_mul(width)
                            .ok_or(CoveError::ArithOverflow)?
                    {
                        return Err(CoveError::PageCorrupt);
                    }
                }
            }
            let child = &tree.children[0];
            if child.node.logical_len != layout.child_row_count {
                return Err(CoveError::PageCorrupt);
            }
            validate_tree_null_bitmap(payload, child, child.node.logical_len, None)?;
            validate_nested_node(
                context,
                payload,
                child,
                schema.and_then(|schema| schema.children.first()),
            )
        }
        CovePhysicalKind::Struct => {
            let layout_bytes = tree_buffer_bytes(payload, tree, PageBufferKind::ChildLayout)?
                .ok_or(CoveError::PageCorrupt)?;
            let layout = crate::encoding::nested::StructLayoutPayload::parse(layout_bytes)?;
            layout.validate(u64::from(tree.node.logical_len))?;
            if tree.children.len() != layout.layout.field_row_counts.len() {
                return Err(CoveError::PageCorrupt);
            }
            for (child_index, (child, expected)) in tree
                .children
                .iter()
                .zip(&layout.layout.field_row_counts)
                .enumerate()
            {
                if child.node.logical_len as u64 != *expected
                    || child.node.logical_len != tree.node.logical_len
                {
                    return Err(CoveError::PageCorrupt);
                }
                validate_tree_null_bitmap(payload, child, child.node.logical_len, None)?;
                validate_nested_node(
                    context,
                    payload,
                    child,
                    schema.and_then(|schema| schema.children.get(child_index)),
                )?;
            }
            Ok(())
        }
        CovePhysicalKind::Map => {
            if tree.children.len() != 2 {
                return Err(CoveError::PageCorrupt);
            }
            let layout_bytes = tree_buffer_bytes(payload, tree, PageBufferKind::ChildLayout)?
                .ok_or(CoveError::PageCorrupt)?;
            let layout = crate::encoding::nested::MapLayoutPayload::parse(layout_bytes)?;
            layout.validate()?;
            if layout.layout.row_count() != tree.node.logical_len as usize {
                return Err(CoveError::PageCorrupt);
            }
            let key = &tree.children[0];
            let value = &tree.children[1];
            if matches!(
                key.node.physical_kind,
                CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map
            ) || key.buffer_of_kind(PageBufferKind::NullBitmap)?.is_some()
                || key.node.logical_len != layout.layout.key_row_count
                || value.node.logical_len != layout.layout.value_row_count
            {
                return Err(CoveError::PageCorrupt);
            }
            validate_nested_node(
                context,
                payload,
                key,
                schema.and_then(|schema| schema.children.first()),
            )?;
            validate_tree_null_bitmap(payload, value, value.node.logical_len, None)?;
            validate_nested_node(
                context,
                payload,
                value,
                schema.and_then(|schema| schema.children.get(1)),
            )
        }
        _ => {
            if !tree.children.is_empty() {
                return Err(CoveError::PageCorrupt);
            }
            let values = tree_buffer_bytes(payload, tree, PageBufferKind::Values)?
                .ok_or(CoveError::PageCorrupt)?;
            validate_values_buffer(
                tree.node.logical_type,
                tree.node.physical_kind,
                tree.node.encoding_kind,
                tree.node.logical_len,
                values,
                context.dictionary,
            )
        }
    }
}

fn validate_tree_matches_nested_schema(
    tree: &PagePayloadTreeNode<'_>,
    schema: &NestedSchemaNodeV1,
) -> Result<(), CoveError> {
    if tree.node.logical_type != schema.logical
        || tree.node.physical_kind != schema.physical
        || tree.children.len() != schema.children.len()
    {
        return Err(CoveError::PageCorrupt);
    }
    for (child_tree, child_schema) in tree.children.iter().zip(&schema.children) {
        validate_tree_matches_nested_schema(child_tree, child_schema)?;
    }
    Ok(())
}

fn validate_value_stream_elided_page(
    context: &PageValidationContext<'_>,
    page: &ColumnPageIndexEntryV1,
    encoding_kind: CoveEncodingKind,
) -> Result<(), CoveError> {
    if page.non_null_count == 0 || encoding_kind != CoveEncodingKind::Constant {
        return Err(CoveError::PageCorrupt);
    }
    match context.physical_kind {
        CovePhysicalKind::Boolean | CovePhysicalKind::FileCode | CovePhysicalKind::NumCode => {
            Ok(())
        }
        _ => Err(CoveError::PageCorrupt),
    }
}

pub(crate) fn validate_stats_only_constant_page(
    context: &PageValidationContext<'_>,
    page: &ColumnPageIndexEntryV1,
) -> Result<(), CoveError> {
    if page_flag_codec(page.flags)? != CompressionCodec::None
        || page.page_offset != 0
        || page.page_length != 0
        || page.uncompressed_length != 0
        || page.encoding_root != u32::MAX
        || page.checksum != checksum::crc32c(&[])
    {
        return Err(CoveError::PageCorrupt);
    }

    if page.flags & PAGE_FLAG_ALL_NON_NULL == 0 {
        return Ok(());
    }

    let Some(zone_stats) = context.zone_stats else {
        return Ok(());
    };
    let entry = zone_stats
        .get(usize::try_from(page.stats_ref).map_err(|_| CoveError::ArithOverflow)?)
        .ok_or(CoveError::PageCorrupt)?;
    if let Some(table_id) = context.table_id {
        if entry.table_id != table_id {
            return Err(CoveError::PageCorrupt);
        }
    }
    if let Some(segment_id) = context.segment_id {
        if entry.segment_id != segment_id {
            return Err(CoveError::PageCorrupt);
        }
    }
    if entry.morsel_id != page.morsel_id
        || entry.column_id != context.column_id
        || entry.stats.row_count != u64::from(page.row_count)
        || entry.stats.null_count != 0
        || entry.non_null_count != page.row_count
        || page.null_count != 0
        || page.non_null_count != page.row_count
    {
        return Err(CoveError::PageCorrupt);
    }
    if !entry.stats.flags.contains(ZoneStatFlags::CONSTANT)
        || !entry.stats.flags.contains(ZoneStatFlags::HAS_MIN_MAX)
        || entry.stats.flags.contains(ZoneStatFlags::MINMAX_TRUNCATED)
    {
        return Err(CoveError::PageCorrupt);
    }
    let (Some(min), Some(max)) = (&entry.stats.min, &entry.stats.max) else {
        return Err(CoveError::PageCorrupt);
    };
    if min.truncated || max.truncated || min != max {
        return Err(CoveError::PageCorrupt);
    }
    if !stat_kind_matches_logical(context.logical_type, min.kind) {
        return Err(CoveError::PageCorrupt);
    }
    Ok(())
}

fn validate_tree_null_bitmap(
    payload: &ColumnPagePayloadV1,
    tree: &PagePayloadTreeNode<'_>,
    row_count: u32,
    expected_null_count: Option<u32>,
) -> Result<(), CoveError> {
    let null_bitmap = tree_buffer_bytes(payload, tree, PageBufferKind::NullBitmap)?;
    if expected_null_count == Some(0) && null_bitmap.is_some() {
        return Err(CoveError::PageCorrupt);
    }
    if expected_null_count.is_some_and(|count| count != 0) && null_bitmap.is_none() {
        return Err(CoveError::PageCorrupt);
    }
    let Some(bytes) = null_bitmap else {
        return Ok(());
    };
    let expected_len = bitmap_len(row_count)?;
    if bytes.len() != expected_len {
        return Err(CoveError::PageCorrupt);
    }
    if !row_count.is_multiple_of(8) && expected_len != 0 {
        let valid_bits = row_count % 8;
        let mask = (1u8 << valid_bits) - 1;
        if bytes[expected_len - 1] & !mask != 0 {
            return Err(CoveError::PageCorrupt);
        }
    }
    let mut counted = 0u32;
    for byte in bytes {
        counted = counted
            .checked_add(byte.count_ones())
            .ok_or(CoveError::ArithOverflow)?;
    }
    if expected_null_count.is_some_and(|expected| counted != expected) {
        return Err(CoveError::PageCorrupt);
    }
    Ok(())
}

fn tree_buffer_bytes<'a>(
    payload: &'a ColumnPagePayloadV1,
    tree: &PagePayloadTreeNode<'_>,
    kind: PageBufferKind,
) -> Result<Option<&'a [u8]>, CoveError> {
    tree.buffer_of_kind(kind)?
        .map(|descriptor| payload.buffer_bytes_for_descriptor(descriptor))
        .transpose()
}

fn validate_values_buffer(
    logical_type: CoveLogicalType,
    physical_kind: CovePhysicalKind,
    encoding_kind: CoveEncodingKind,
    row_count: u32,
    values: &[u8],
    dictionary: Option<&FileDictionaryView<'_>>,
) -> Result<(), CoveError> {
    match encoding_kind {
        CoveEncodingKind::FileCode => validate_filecodes(values, row_count, dictionary),
        CoveEncodingKind::NumCode => require_len(values.len(), fixed_rows_len(row_count, 8)?),
        CoveEncodingKind::PlainFixed => {
            let width = fixed_width_for(logical_type, physical_kind)?;
            require_len(values.len(), fixed_rows_len(row_count, width)?)?;
            if physical_kind == CovePhysicalKind::Boolean {
                validate_boolean_bytes(values)?;
            }
            Ok(())
        }
        CoveEncodingKind::VarBytes => validate_length_prefixed_u32_rows(values, row_count),
        CoveEncodingKind::PlainVarint => validate_varint_rows(values, row_count),
        CoveEncodingKind::Canonical => validate_canonical_rows(values, logical_type, row_count),
        CoveEncodingKind::Constant => {
            require_len(values.len(), ConstantPayload::ENCODED_LEN)?;
            let payload = ConstantPayload::parse(values)?;
            if payload.row_count != u64::from(row_count) {
                return Err(CoveError::PageCorrupt);
            }
            if physical_kind == CovePhysicalKind::Boolean && !matches!(payload.value, 0 | 1) {
                return Err(CoveError::PageCorrupt);
            }
            Ok(())
        }
        CoveEncodingKind::LocalCodebook => {
            let payload = LocalCodebookPayload::parse(values)?;
            require_len(values.len(), payload.encode().len())?;
            let decoded = payload.decode_values()?;
            if decoded.len() != row_count as usize {
                return Err(CoveError::PageCorrupt);
            }
            validate_local_codebook_values(&decoded, physical_kind, dictionary)
        }
        CoveEncodingKind::Rle => {
            let payload = RlePayload::parse(values)?;
            require_len(values.len(), payload.encode().len())?;
            validate_i64_values(
                Rle::fast_decode(&payload)?,
                physical_kind,
                row_count,
                dictionary,
            )
        }
        CoveEncodingKind::RunEnd => {
            let payload = RunEndPayload::parse(values)?;
            let expected = 4usize
                .checked_add(
                    payload
                        .values
                        .len()
                        .checked_mul(12)
                        .ok_or(CoveError::ArithOverflow)?,
                )
                .ok_or(CoveError::ArithOverflow)?;
            require_len(values.len(), expected)?;
            validate_i64_values(
                RunEnd::fast_decode(&payload)?,
                physical_kind,
                row_count,
                dictionary,
            )
        }
        CoveEncodingKind::BitPacked => {
            let payload = BitPackedPayload::parse(values)?;
            let expected = 9usize
                .checked_add(payload.bits.len())
                .ok_or(CoveError::ArithOverflow)?;
            require_len(values.len(), expected)?;
            if payload.row_count != row_count {
                return Err(CoveError::PageCorrupt);
            }
            validate_i64_values(
                BitPacked::fast_decode(&payload)?,
                physical_kind,
                row_count,
                dictionary,
            )
        }
        CoveEncodingKind::Delta => {
            let payload = DeltaPayload::parse(values)?;
            require_len(values.len(), payload.encode().len())?;
            validate_i64_values(
                Delta::fast_decode(&payload)?,
                physical_kind,
                row_count,
                dictionary,
            )
        }
        CoveEncodingKind::FrameOfReference => {
            let payload = ForPayload::parse(values)?;
            require_len(values.len(), payload.encode().len())?;
            validate_i64_values(
                FrameOfReference::fast_decode(&payload)?,
                physical_kind,
                row_count,
                dictionary,
            )
        }
        CoveEncodingKind::PatchedBase => {
            let payload = PatchedBasePayload::parse(values)?;
            let expected = 4usize
                .checked_add(
                    payload
                        .base
                        .len()
                        .checked_mul(8)
                        .ok_or(CoveError::ArithOverflow)?,
                )
                .and_then(|value| value.checked_add(4))
                .and_then(|value| {
                    payload
                        .patches
                        .len()
                        .checked_mul(12)
                        .and_then(|patch_len| value.checked_add(patch_len))
                })
                .ok_or(CoveError::ArithOverflow)?;
            require_len(values.len(), expected)?;
            validate_i64_values(
                PatchedBase::fast_decode(&payload)?,
                physical_kind,
                row_count,
                dictionary,
            )
        }
        CoveEncodingKind::Sparse => {
            let payload = SparsePayload::parse(values)?;
            let expected = 16usize
                .checked_add(
                    payload
                        .overrides
                        .len()
                        .checked_mul(12)
                        .ok_or(CoveError::ArithOverflow)?,
                )
                .ok_or(CoveError::ArithOverflow)?;
            require_len(values.len(), expected)?;
            validate_i64_values(
                Sparse::fast_decode(&payload)?,
                physical_kind,
                row_count,
                dictionary,
            )
        }
        CoveEncodingKind::Validity
        | CoveEncodingKind::Sequence
        | CoveEncodingKind::Lz4Block
        | CoveEncodingKind::ZstdBlock
        | CoveEncodingKind::RegisteredEncoding => {
            Err(CoveError::UnsupportedEncoding(format!("{encoding_kind:?}")))
        }
    }
}

fn validate_filecodes(
    values: &[u8],
    row_count: u32,
    dictionary: Option<&FileDictionaryView<'_>>,
) -> Result<(), CoveError> {
    require_len(values.len(), fixed_rows_len(row_count, 4)?)?;
    if let Some(dictionary) = dictionary {
        for chunk in values.chunks_exact(4) {
            let code = u32::from_le_bytes(chunk.try_into().unwrap());
            if code >= dictionary.len() {
                return Err(CoveError::BadFileCode);
            }
        }
    }
    Ok(())
}

fn validate_i64_values(
    values: Vec<i64>,
    physical_kind: CovePhysicalKind,
    row_count: u32,
    dictionary: Option<&FileDictionaryView<'_>>,
) -> Result<(), CoveError> {
    if values.len() != row_count as usize {
        return Err(CoveError::PageCorrupt);
    }
    match physical_kind {
        CovePhysicalKind::Boolean if values.iter().any(|value| !matches!(*value, 0 | 1)) => {
            return Err(CoveError::PageCorrupt);
        }
        CovePhysicalKind::Boolean => {}
        CovePhysicalKind::FileCode => {
            if let Some(dictionary) = dictionary {
                for value in &values {
                    let code = u32::try_from(*value).map_err(|_| CoveError::PageCorrupt)?;
                    if code >= dictionary.len() {
                        return Err(CoveError::BadFileCode);
                    }
                }
            }
        }
        CovePhysicalKind::NumCode => {
            for value in values {
                u64::try_from(value).map_err(|_| CoveError::PageCorrupt)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_local_codebook_values(
    values: &[LocalCodebookValue],
    physical_kind: CovePhysicalKind,
    dictionary: Option<&FileDictionaryView<'_>>,
) -> Result<(), CoveError> {
    for value in values {
        match (physical_kind, value) {
            (CovePhysicalKind::FileCode, LocalCodebookValue::FileCode(code)) => {
                if let Some(dictionary) = dictionary {
                    if *code >= dictionary.len() {
                        return Err(CoveError::BadFileCode);
                    }
                }
            }
            (CovePhysicalKind::NumCode, LocalCodebookValue::NumCode(_))
            | (CovePhysicalKind::Boolean, LocalCodebookValue::Boolean(_))
            | (CovePhysicalKind::VarBytes, LocalCodebookValue::VarBytes(_)) => {}
            _ => return Err(CoveError::PageCorrupt),
        }
    }
    Ok(())
}

fn validate_length_prefixed_u32_rows(values: &[u8], row_count: u32) -> Result<(), CoveError> {
    let mut pos = 0usize;
    for _ in 0..row_count {
        let len_end = pos.checked_add(4).ok_or(CoveError::ArithOverflow)?;
        if len_end > values.len() {
            return Err(CoveError::BufferTooShort);
        }
        let len = u32::from_le_bytes(values[pos..len_end].try_into().unwrap()) as usize;
        pos = len_end.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        if pos > values.len() {
            return Err(CoveError::BufferTooShort);
        }
    }
    require_len(pos, values.len())
}

fn validate_varint_rows(values: &[u8], row_count: u32) -> Result<(), CoveError> {
    let mut pos = 0usize;
    for _ in 0..row_count {
        let (_, consumed) = wire::decode_u64_leb128(&values[pos..])?;
        pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
    }
    require_len(pos, values.len())
}

fn validate_canonical_rows(
    values: &[u8],
    logical_type: CoveLogicalType,
    row_count: u32,
) -> Result<(), CoveError> {
    match logical_type {
        CoveLogicalType::Null | CoveLogicalType::Bool => require_len(values.len(), 0),
        CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json => {
            let mut pos = 0usize;
            for _ in 0..row_count {
                let (len, consumed) = wire::decode_u64_leb128(&values[pos..])?;
                let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
                pos = pos
                    .checked_add(consumed)
                    .and_then(|value| value.checked_add(len))
                    .ok_or(CoveError::ArithOverflow)?;
                if pos > values.len() {
                    return Err(CoveError::BufferTooShort);
                }
            }
            require_len(pos, values.len())
        }
        logical => {
            let width = fixed_width_for(logical, CovePhysicalKind::FixedBytes)?;
            require_len(values.len(), fixed_rows_len(row_count, width)?)
        }
    }
}

fn validate_boolean_bytes(values: &[u8]) -> Result<(), CoveError> {
    if values.iter().any(|value| !matches!(value, 0 | 1)) {
        return Err(CoveError::PageCorrupt);
    }
    Ok(())
}

fn fixed_width_for(
    logical_type: CoveLogicalType,
    physical_kind: CovePhysicalKind,
) -> Result<usize, CoveError> {
    match physical_kind {
        CovePhysicalKind::Boolean => Ok(1),
        CovePhysicalKind::FixedBytes | CovePhysicalKind::NumCode | CovePhysicalKind::FileCode => {
            logical_fixed_width(logical_type).ok_or_else(|| {
                CoveError::UnsupportedEncoding(format!(
                    "fixed-width page validation for {logical_type:?}"
                ))
            })
        }
        _ => logical_fixed_width(logical_type).ok_or_else(|| {
            CoveError::UnsupportedEncoding(format!(
                "fixed-width page validation for {logical_type:?}"
            ))
        }),
    }
}

fn logical_fixed_width(logical_type: CoveLogicalType) -> Option<usize> {
    match logical_type {
        CoveLogicalType::Bool | CoveLogicalType::Int8 | CoveLogicalType::UInt8 => Some(1),
        CoveLogicalType::Int16 | CoveLogicalType::UInt16 => Some(2),
        CoveLogicalType::Int32
        | CoveLogicalType::UInt32
        | CoveLogicalType::Float32
        | CoveLogicalType::DateDays => Some(4),
        CoveLogicalType::Int64
        | CoveLogicalType::UInt64
        | CoveLogicalType::Float64
        | CoveLogicalType::Decimal64
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos => Some(8),
        CoveLogicalType::Decimal128 | CoveLogicalType::Uuid => Some(16),
        _ => None,
    }
}

fn fixed_rows_len(row_count: u32, width: usize) -> Result<usize, CoveError> {
    (row_count as usize)
        .checked_mul(width)
        .ok_or(CoveError::ArithOverflow)
}

fn bitmap_len(row_count: u32) -> Result<usize, CoveError> {
    let len = row_count.checked_add(7).ok_or(CoveError::ArithOverflow)? / 8;
    usize::try_from(len).map_err(|_| CoveError::ArithOverflow)
}

fn require_len(actual: usize, expected: usize) -> Result<(), CoveError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CoveError::PageCorrupt)
    }
}

fn stat_kind_matches_logical(logical_type: CoveLogicalType, kind: StatKind) -> bool {
    match logical_type {
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64
        | CoveLogicalType::Decimal64 => kind == StatKind::Int64,
        CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64 => kind == StatKind::UInt64,
        CoveLogicalType::Float32 => false,
        CoveLogicalType::Float64 => kind == StatKind::Float64Bits,
        CoveLogicalType::Decimal128 => kind == StatKind::Decimal128,
        CoveLogicalType::DateDays => kind == StatKind::DateDays,
        CoveLogicalType::TimestampMicros => kind == StatKind::TimestampMicros,
        CoveLogicalType::TimestampNanos => kind == StatKind::TimestampNanos,
        CoveLogicalType::Uuid => kind == StatKind::FixedBytes,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::CoveEncodingKind,
        page::{
            PAGE_FLAG_ALL_NON_NULL, PAGE_FLAG_STATS_ONLY_CONSTANT, PAGE_FLAG_VALUE_STREAM_ELIDED,
        },
        page_payload::ColumnPagePayloadV1,
        zone_stats::{StatScalar, ZoneScope, ZoneStats},
    };

    fn base_page(row_count: u32, encoding: CoveEncodingKind) -> ColumnPageIndexEntryV1 {
        ColumnPageIndexEntryV1 {
            column_id: 7,
            morsel_id: 0,
            row_count,
            non_null_count: row_count,
            null_count: 0,
            encoding_root: encoding as u32,
            page_offset: 0,
            page_length: 1,
            uncompressed_length: 1,
            stats_ref: 0,
            flags: 0,
            checksum: 0,
        }
    }

    fn context<'a>(
        logical_type: CoveLogicalType,
        physical_kind: CovePhysicalKind,
        zone_stats: Option<&'a [ZoneStatsEntry]>,
    ) -> PageValidationContext<'a> {
        PageValidationContext {
            table_id: Some(3),
            segment_id: Some(5),
            column_id: 7,
            logical_type,
            physical_kind,
            dictionary: None,
            zone_stats,
            codec_descriptors: &[],
            nested_schema: None,
        }
    }

    #[test]
    fn rejects_short_numcode_values() {
        let payload = ColumnPagePayloadV1::build_single_node(
            2,
            CoveEncodingKind::NumCode,
            CoveLogicalType::UInt64,
            CovePhysicalKind::NumCode,
            None,
            1u64.to_le_bytes().to_vec(),
        )
        .unwrap();
        let payload = ColumnPagePayloadV1::parse(&payload).unwrap();
        let err = validate_column_page_payload(
            &context(CoveLogicalType::UInt64, CovePhysicalKind::NumCode, None),
            &base_page(2, CoveEncodingKind::NumCode),
            &payload,
        );
        assert_eq!(err, Err(CoveError::PageCorrupt));
    }

    #[test]
    fn rejects_invalid_boolean_byte() {
        let payload = ColumnPagePayloadV1::build_single_node(
            1,
            CoveEncodingKind::PlainFixed,
            CoveLogicalType::Bool,
            CovePhysicalKind::Boolean,
            None,
            vec![2],
        )
        .unwrap();
        let payload = ColumnPagePayloadV1::parse(&payload).unwrap();
        let err = validate_column_page_payload(
            &context(CoveLogicalType::Bool, CovePhysicalKind::Boolean, None),
            &base_page(1, CoveEncodingKind::PlainFixed),
            &payload,
        );
        assert_eq!(err, Err(CoveError::PageCorrupt));
    }

    #[test]
    fn rejects_null_bitmap_tail_bits() {
        let payload = ColumnPagePayloadV1::build_single_node(
            1,
            CoveEncodingKind::PlainFixed,
            CoveLogicalType::Bool,
            CovePhysicalKind::Boolean,
            Some(vec![0b1000_0001]),
            vec![0],
        )
        .unwrap();
        let payload = ColumnPagePayloadV1::parse(&payload).unwrap();
        let mut page = base_page(1, CoveEncodingKind::PlainFixed);
        page.non_null_count = 0;
        page.null_count = 1;
        let err = validate_column_page_payload(
            &context(CoveLogicalType::Bool, CovePhysicalKind::Boolean, None),
            &page,
            &payload,
        );
        assert_eq!(err, Err(CoveError::PageCorrupt));
    }

    #[test]
    fn accepts_mixed_value_stream_elided_constant_numcode() {
        let values = ConstantPayload {
            value: 42,
            row_count: 4,
        }
        .encode()
        .to_vec();
        let payload = ColumnPagePayloadV1::build_single_node(
            4,
            CoveEncodingKind::Constant,
            CoveLogicalType::Int64,
            CovePhysicalKind::NumCode,
            Some(vec![0b0000_1010]),
            values,
        )
        .unwrap();
        let payload = ColumnPagePayloadV1::parse(&payload).unwrap();
        let mut page = base_page(4, CoveEncodingKind::Constant);
        page.non_null_count = 2;
        page.null_count = 2;
        page.flags = PAGE_FLAG_VALUE_STREAM_ELIDED;
        assert_eq!(
            validate_column_page_payload(
                &context(CoveLogicalType::Int64, CovePhysicalKind::NumCode, None),
                &page,
                &payload,
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_value_stream_elided_non_constant_root() {
        let payload = ColumnPagePayloadV1::build_single_node(
            2,
            CoveEncodingKind::NumCode,
            CoveLogicalType::Int64,
            CovePhysicalKind::NumCode,
            None,
            [1u64.to_le_bytes(), 1u64.to_le_bytes()].concat(),
        )
        .unwrap();
        let payload = ColumnPagePayloadV1::parse(&payload).unwrap();
        let mut page = base_page(2, CoveEncodingKind::NumCode);
        page.flags = PAGE_FLAG_VALUE_STREAM_ELIDED;
        assert_eq!(
            validate_column_page_payload(
                &context(CoveLogicalType::Int64, CovePhysicalKind::NumCode, None),
                &page,
                &payload,
            ),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn rejects_all_null_value_stream_elided_page() {
        let values = ConstantPayload {
            value: 1,
            row_count: 2,
        }
        .encode()
        .to_vec();
        let payload = ColumnPagePayloadV1::build_single_node(
            2,
            CoveEncodingKind::Constant,
            CoveLogicalType::Int64,
            CovePhysicalKind::NumCode,
            Some(vec![0b0000_0011]),
            values,
        )
        .unwrap();
        let payload = ColumnPagePayloadV1::parse(&payload).unwrap();
        let mut page = base_page(2, CoveEncodingKind::Constant);
        page.non_null_count = 0;
        page.null_count = 2;
        page.flags = PAGE_FLAG_VALUE_STREAM_ELIDED;
        assert_eq!(
            validate_column_page_payload(
                &context(CoveLogicalType::Int64, CovePhysicalKind::NumCode, None),
                &page,
                &payload,
            ),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn validates_stats_only_all_non_null_constant() {
        let scalar = StatScalar {
            kind: StatKind::Int64,
            bytes: 9i64.to_le_bytes().to_vec(),
            truncated: false,
        };
        let stats = ZoneStatsEntry {
            table_id: 3,
            segment_id: 5,
            morsel_id: 0,
            column_id: 7,
            non_null_count: 2,
            distinct_count: 1,
            run_count: 1,
            stats: ZoneStats {
                scope: ZoneScope::Morsel,
                row_count: 2,
                null_count: 0,
                min: Some(scalar.clone()),
                max: Some(scalar),
                flags: ZoneStatFlags::HAS_MIN_MAX | ZoneStatFlags::CONSTANT,
            },
            min_domain_rank: 0,
            max_domain_rank: 0,
            exact_set_ref: u32::MAX,
            bloom_ref: u32::MAX,
        };
        let stats = [stats];
        let mut page = base_page(2, CoveEncodingKind::NumCode);
        page.encoding_root = u32::MAX;
        page.page_length = 0;
        page.uncompressed_length = 0;
        page.flags = PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NON_NULL;
        page.checksum = checksum::crc32c(&[]);
        assert_eq!(
            validate_stats_only_constant_page(
                &context(
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    Some(&stats)
                ),
                &page,
            ),
            Ok(())
        );
        let mut bad = stats[0].clone();
        bad.stats.flags = ZoneStatFlags::HAS_MIN_MAX;
        assert_eq!(
            validate_stats_only_constant_page(
                &context(
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    Some(&[bad])
                ),
                &page,
            ),
            Err(CoveError::PageCorrupt)
        );
    }
}
