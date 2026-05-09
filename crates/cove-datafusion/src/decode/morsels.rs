use super::*;

pub(super) fn ordered_morsels<'a>(
    state: &DatasetState,
    segment_id: u32,
    entries: &'a [RowMorselEntryV1],
    plan: &ScanPlan,
) -> Vec<&'a RowMorselEntryV1> {
    let mut ordered = entries.iter().collect::<Vec<_>>();
    let Some(hint) = plan.topn_hint else {
        return ordered;
    };
    let Some(column) = state.table().columns.get(hint.column_index) else {
        return ordered;
    };
    let wanted_direction = if hint.descending {
        TopNDirection::Largest
    } else {
        TopNDirection::Smallest
    };
    ordered.sort_by_key(|morsel| {
        let rank = state
            .topn_for(column.column_id)
            .into_iter()
            .find(|summary| {
                summary.segment_id == segment_id
                    && summary.morsel_id == morsel.morsel_id
                    && summary.direction == wanted_direction
            })
            .and_then(topn_score)
            .map(|score| {
                if hint.descending {
                    u64::MAX.saturating_sub(score)
                } else {
                    score
                }
            })
            .unwrap_or(u64::MAX);
        (rank, morsel.morsel_id)
    });
    ordered
}

fn topn_score(summary: &cove_core::index::topn::TopNSummary) -> Option<u64> {
    summary
        .payload
        .chunks_exact(16)
        .next()
        .and_then(|chunk| chunk[0..8].try_into().ok().map(u64::from_le_bytes))
}

#[derive(Debug)]
pub(super) struct SegmentMetadata {
    morsels: RowMorselDirectory,
    morsel_positions_by_id: Vec<Option<usize>>,
    columns: Vec<PreparedSegmentColumn>,
    column_positions: Vec<(u32, usize)>,
}

#[derive(Debug)]
pub(super) struct PreparedSegmentColumn {
    page_index: ColumnPageIndex,
    page_positions_by_morsel: Vec<Option<usize>>,
}

impl SegmentMetadata {
    fn new(
        morsels: RowMorselDirectory,
        columns: Vec<TableColumnDirectoryEntryV1>,
        page_indexes: Vec<ColumnPageIndex>,
    ) -> Result<Self, CoveError> {
        if columns.len() != page_indexes.len() {
            return Err(CoveError::SegmentCorrupt);
        }
        let max_morsel_id = morsels
            .entries
            .iter()
            .map(|entry| entry.morsel_id as usize)
            .max()
            .unwrap_or(0);
        let mut morsel_positions_by_id = vec![None; max_morsel_id.saturating_add(1)];
        for (position, morsel) in morsels.entries.iter().enumerate() {
            let slot = morsel.morsel_id as usize;
            if morsel_positions_by_id[slot].replace(position).is_some() {
                return Err(CoveError::SegmentCorrupt);
            }
        }

        let mut prepared_columns = Vec::with_capacity(columns.len());
        let mut column_positions = Vec::with_capacity(columns.len());
        for (position, (directory, page_index)) in columns
            .into_iter()
            .zip(page_indexes.into_iter())
            .enumerate()
        {
            let mut page_positions_by_morsel = vec![None; morsel_positions_by_id.len()];
            for (page_position, page) in page_index.entries.iter().enumerate() {
                let Some(&Some(morsel_position)) =
                    morsel_positions_by_id.get(page.morsel_id as usize)
                else {
                    return Err(CoveError::PageCorrupt);
                };
                if morsels.entries[morsel_position].row_count != page.row_count {
                    return Err(CoveError::PageCorrupt);
                }
                let slot = &mut page_positions_by_morsel[page.morsel_id as usize];
                if slot.replace(page_position).is_some() {
                    return Err(CoveError::PageCorrupt);
                }
            }
            column_positions.push((directory.column_id, position));
            prepared_columns.push(PreparedSegmentColumn {
                page_index,
                page_positions_by_morsel,
            });
        }
        column_positions.sort_unstable_by_key(|(column_id, _)| *column_id);
        for pair in column_positions.windows(2) {
            if pair[0].0 == pair[1].0 {
                return Err(CoveError::SegmentCorrupt);
            }
        }
        Ok(Self {
            morsels,
            morsel_positions_by_id,
            columns: prepared_columns,
            column_positions,
        })
    }

    pub(super) fn morsel_entries(&self) -> &[RowMorselEntryV1] {
        &self.morsels.entries
    }

    pub(super) fn morsel(&self, morsel_id: u32) -> Result<&RowMorselEntryV1, CoveError> {
        let Some(&Some(position)) = self.morsel_positions_by_id.get(morsel_id as usize) else {
            return Err(CoveError::SegmentCorrupt);
        };
        self.morsels
            .entries
            .get(position)
            .ok_or(CoveError::SegmentCorrupt)
    }

    pub(super) fn column(&self, column_id: u32) -> Result<&PreparedSegmentColumn, CoveError> {
        let position = self
            .column_positions
            .binary_search_by_key(&column_id, |(candidate, _)| *candidate)
            .map_err(|_| CoveError::SegmentCorrupt)?;
        self.columns
            .get(self.column_positions[position].1)
            .ok_or(CoveError::SegmentCorrupt)
    }

    pub(super) fn page_for_morsel<'a>(
        &'a self,
        column: &'a PreparedSegmentColumn,
        morsel_id: u32,
    ) -> Result<&'a ColumnPageIndexEntryV1, CoveError> {
        let Some(&Some(page_position)) = column.page_positions_by_morsel.get(morsel_id as usize)
        else {
            return Err(CoveError::PageCorrupt);
        };
        column
            .page_index
            .entries
            .get(page_position)
            .ok_or(CoveError::PageCorrupt)
    }
}

pub(super) async fn read_segment_metadata<R: CoveRangeReader + ?Sized>(
    reader: &R,
    state: &DatasetState,
    segment_ref: &TableSegmentIndexEntryV1,
    stats: &mut DecodeStats,
    cache: Option<&ScanExecutionCache>,
    file_ordinal: usize,
) -> Result<Arc<SegmentMetadata>, CoveError> {
    let key = SegmentMetadataCacheKey::new(file_ordinal, segment_ref);
    if let Some(cache) = cache {
        if let Some(segment) = cache.get_segment_metadata(key)? {
            return Ok(segment);
        }
    }

    let header_end = segment_ref
        .offset
        .checked_add(TABLE_SEGMENT_HEADER_LEN as u64)
        .ok_or(CoveError::ArithOverflow)?;
    let header_bytes = reader
        .read_range(segment_ref.offset..header_end, RangeReadKind::Metadata)
        .await?;
    stats.metadata_bytes_read = stats
        .metadata_bytes_read
        .checked_add(header_bytes.len())
        .ok_or(CoveError::ArithOverflow)?;
    let header = TableSegmentHeaderV1::parse(&header_bytes)?;
    if header.table_id != segment_ref.table_id
        || header.segment_id != segment_ref.segment_id
        || header.row_start != segment_ref.row_start
        || header.row_count != segment_ref.row_count
        || header.column_count != segment_ref.column_count
    {
        return Err(CoveError::SegmentCorrupt);
    }
    if header.data_offset > segment_ref.length {
        return Err(CoveError::SegmentCorrupt);
    }
    let metadata_end = segment_ref
        .offset
        .checked_add(header.data_offset)
        .ok_or(CoveError::ArithOverflow)?;
    let metadata = reader
        .read_range(segment_ref.offset..metadata_end, RangeReadKind::Metadata)
        .await?;
    stats.metadata_bytes_read = stats
        .metadata_bytes_read
        .checked_add(metadata.len())
        .ok_or(CoveError::ArithOverflow)?;
    let segment = Arc::new(parse_segment_metadata(
        &metadata,
        segment_ref.length,
        state.mounted().header.required_features,
    )?);
    if let Some(cache) = cache {
        cache.insert_segment_metadata(key, segment)
    } else {
        Ok(segment)
    }
}

fn parse_segment_metadata(
    bytes: &[u8],
    segment_len: u64,
    required_features: u64,
) -> Result<SegmentMetadata, CoveError> {
    let header = TableSegmentHeaderV1::parse(bytes)?;
    if header.row_count == 0 && header.morsel_count != 0 {
        return Err(CoveError::SegmentCorrupt);
    }
    if header.row_count != 0 && header.morsel_row_count == 0 {
        return Err(CoveError::SegmentCorrupt);
    }
    let morsel_offset =
        usize::try_from(header.morsel_directory_offset).map_err(|_| CoveError::OffsetRange)?;
    if morsel_offset < TABLE_SEGMENT_HEADER_LEN || morsel_offset > bytes.len() {
        return Err(CoveError::SegmentCorrupt);
    }
    let morsel_dir_len = (header.morsel_count as usize)
        .checked_mul(cove_core::segment::ROW_MORSEL_ENTRY_LEN)
        .ok_or(CoveError::ArithOverflow)?;
    let morsel_end = morsel_offset
        .checked_add(morsel_dir_len)
        .ok_or(CoveError::ArithOverflow)?;
    if morsel_end > bytes.len() {
        return Err(CoveError::SegmentCorrupt);
    }
    let column_directory_offset =
        usize::try_from(header.column_directory_offset).map_err(|_| CoveError::OffsetRange)?;
    let page_index_offset =
        usize::try_from(header.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
    let data_offset = usize::try_from(header.data_offset).map_err(|_| CoveError::OffsetRange)?;
    if column_directory_offset < morsel_end
        || page_index_offset < column_directory_offset
        || data_offset < page_index_offset
        || data_offset > bytes.len()
    {
        return Err(CoveError::SegmentCorrupt);
    }
    let morsels =
        RowMorselDirectory::parse(&bytes[morsel_offset..morsel_end], header.morsel_count)?;
    if morsels.sum_rows() != header.row_count as u64 {
        return Err(CoveError::SegmentCorrupt);
    }
    let column_dir_len = (header.column_count as usize)
        .checked_mul(TABLE_COLUMN_DIRECTORY_ENTRY_LEN)
        .ok_or(CoveError::ArithOverflow)?;
    let column_dir_end = column_directory_offset
        .checked_add(column_dir_len)
        .ok_or(CoveError::ArithOverflow)?;
    if column_dir_end > page_index_offset {
        return Err(CoveError::SegmentCorrupt);
    }
    let mut columns = Vec::with_capacity(header.column_count as usize);
    let mut page_indexes = Vec::with_capacity(header.column_count as usize);
    let mut pos = column_directory_offset;
    for _ in 0..header.column_count {
        columns.push(TableColumnDirectoryEntryV1::parse(
            &bytes[pos..pos + TABLE_COLUMN_DIRECTORY_ENTRY_LEN],
        )?);
        pos += TABLE_COLUMN_DIRECTORY_ENTRY_LEN;
    }
    for column in &columns {
        let column_page_index_offset =
            usize::try_from(column.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
        let column_page_index_length =
            usize::try_from(column.page_index_length).map_err(|_| CoveError::OffsetRange)?;
        let column_page_index_end = column_page_index_offset
            .checked_add(column_page_index_length)
            .ok_or(CoveError::ArithOverflow)?;
        if column_page_index_offset < page_index_offset || column_page_index_end > data_offset {
            return Err(CoveError::SegmentCorrupt);
        }
        let column_data_end = column
            .data_offset
            .checked_add(column.data_length)
            .ok_or(CoveError::ArithOverflow)?;
        if column.data_offset < header.data_offset || column_data_end > segment_len {
            return Err(CoveError::PageCorrupt);
        }
        let page_index = column_page_index(bytes, column)?;
        for page in &page_index.entries {
            if page.column_id != column.column_id {
                return Err(CoveError::PageCorrupt);
            }
            let morsel = morsels
                .entries
                .get(page.morsel_id as usize)
                .ok_or(CoveError::SegmentCorrupt)?;
            if page.row_count != morsel.row_count {
                return Err(CoveError::PageCorrupt);
            }
            if page_uses_payload_elision(page.flags)
                && required_features & cove_core::constants::FEATURE_PAGE_PAYLOAD_ELISION == 0
            {
                return Err(CoveError::BadSection(
                    "page payload-elision flags require FEATURE_PAGE_PAYLOAD_ELISION in required_features"
                        .into(),
                ));
            }
            if page.page_length != 0 {
                let page_end = page
                    .page_offset
                    .checked_add(page.page_length)
                    .ok_or(CoveError::ArithOverflow)?;
                if page.page_offset < column.data_offset || page_end > column_data_end {
                    return Err(CoveError::PageCorrupt);
                }
            }
        }
        page_indexes.push(page_index);
    }
    SegmentMetadata::new(morsels, columns, page_indexes)
}

fn column_page_index(
    segment_bytes: &[u8],
    column: &cove_core::segment::TableColumnDirectoryEntryV1,
) -> Result<ColumnPageIndex, CoveError> {
    let start = usize::try_from(column.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
    let len = usize::try_from(column.page_index_length).map_err(|_| CoveError::OffsetRange)?;
    let bytes = wire::read_range_checked(segment_bytes, start, len)?;
    ColumnPageIndex::parse(bytes)
}

pub(super) fn prepare_segment_payload(
    segment_bytes: &[u8],
    segment: &TableSegmentPayloadV1,
) -> Result<SegmentMetadata, CoveError> {
    let mut page_indexes = Vec::with_capacity(segment.columns.len());
    for column in &segment.columns {
        page_indexes.push(column_page_index(segment_bytes, column)?);
    }
    SegmentMetadata::new(
        segment.morsels.clone(),
        segment.columns.clone(),
        page_indexes,
    )
}
