use super::materialize::{
    encoded_array_for_page, materialize_page_payload, materialize_page_payload_from_wire,
};
use super::morsels::SegmentMetadata;
use super::predicates::{
    lookup_selection_for_morsel, plan_has_row_predicate, predicate_column_index,
    predicate_is_index_covered, try_apply_predicate_to_selection,
    try_apply_raw_predicate_to_selection,
};
use super::*;

fn apply_overlay_to_rows(
    state: &DatasetState,
    morsel_row_start: u64,
    selected_rows: &mut Vec<u32>,
    _stats: &mut DecodeStats,
) -> Result<(), CoveError> {
    let visibility = state.file(0)?.visibility();
    if visibility.is_all() || selected_rows.is_empty() {
        return Ok(());
    }
    let mut write = 0usize;
    for read in 0..selected_rows.len() {
        let row = selected_rows[read];
        let absolute = morsel_row_start
            .checked_add(u64::from(row))
            .ok_or(CoveError::ArithOverflow)?;
        if visibility.is_row_visible(absolute, state.table().row_count)? {
            selected_rows[write] = row;
            write += 1;
        }
    }
    selected_rows.truncate(write);
    Ok(())
}

pub(super) fn apply_overlay_to_selection(
    state: &DatasetState,
    morsel_row_start: u64,
    row_count: u32,
    scratch: &mut DecodeScratch,
    stats: &mut DecodeStats,
) -> Result<(), CoveError> {
    let visibility = state.file(0)?.visibility();
    if visibility.is_all() || scratch.selection.is_empty() {
        return Ok(());
    }
    scratch.selection.write_rows(&mut scratch.selected_rows)?;
    apply_overlay_to_rows(state, morsel_row_start, &mut scratch.selected_rows, stats)?;
    scratch.selection = Selection::from_rows(&scratch.selected_rows, row_count as usize);
    Ok(())
}

pub(super) fn selected_rows_for_morsel(
    state: &DatasetState,
    segment_bytes: &[u8],
    segment: &SegmentMetadata,
    segment_id: u32,
    morsel_id: u32,
    plan: &ScanPlan,
    stats: &mut DecodeStats,
    scratch: &mut DecodeScratch,
) -> Result<(), CoveError> {
    scratch.selected_rows.clear();
    scratch.selection = Selection::None;
    let morsel = segment.morsel(morsel_id)?;
    if !plan_has_row_predicate(plan) {
        scratch.selection = Selection::AllRows {
            len: morsel.row_count as usize,
        };
        return Ok(());
    }
    let skip_index_predicates = match lookup_selection_for_morsel(
        state,
        segment_id,
        morsel_id,
        morsel.row_count,
        plan,
        stats,
        scratch,
    )? {
        true => true,
        false => {
            scratch.selected_mask.fill_all(morsel.row_count as usize);
            false
        }
    };
    if scratch.selected_mask.all_zero() {
        scratch.selection = Selection::None;
        return Ok(());
    }
    for filter in &plan.filters {
        let Some(predicate) = &filter.predicate else {
            continue;
        };
        if matches!(predicate, CovePredicate::Null { .. }) {
            continue;
        }
        if skip_index_predicates && predicate_is_index_covered(state, predicate) {
            continue;
        }
        if matches!(
            predicate,
            CovePredicate::FileCodeIn { file_codes, .. } if file_codes.is_empty()
        ) {
            scratch.selection = Selection::None;
            return Ok(());
        }
        let Some(column_index) = predicate_column_index(predicate) else {
            continue;
        };
        let column = &state.table().columns[column_index];
        let segment_column = segment.column(column.column_id)?;
        let page = segment.page_for_morsel(segment_column, morsel_id)?;
        let payload = match materialize_page_payload(
            segment_bytes,
            column,
            &page,
            state.page_payload_validation_policy(),
        ) {
            Ok(payload) => payload,
            Err(CoveError::UnsupportedEncoding(_)) => {
                if filter.use_kind == CoveFilterUse::FullRowPredicateExact {
                    stats.exactness_fallbacks += 1;
                    return Err(CoveError::UnsupportedEncoding(format!(
                        "exact predicate {} cannot be evaluated for page encoding",
                        filter.display
                    )));
                }
                stats.kernel_fallbacks += 1;
                scratch.selection = Selection::AllRows {
                    len: morsel.row_count as usize,
                };
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        stats.pages_decoded += usize::from(page.page_length != 0);
        stats.data_bytes_read = stats
            .data_bytes_read
            .checked_add(usize::try_from(page.page_length).map_err(|_| CoveError::OffsetRange)?)
            .ok_or(CoveError::ArithOverflow)?;
        let dictionary = if matches!(predicate, CovePredicate::FileCodeIn { .. }) {
            None
        } else {
            state.mounted().dictionary.as_ref()
        };
        let array = encoded_array_for_page(&payload, &page, dictionary)?;
        let applied = match try_apply_raw_predicate_to_selection(
            predicate,
            &array,
            &mut scratch.selected_mask,
            &mut scratch.filter_mask,
        )? {
            Some(()) => true,
            None => {
                let prepared = array.prepare()?;
                try_apply_predicate_to_selection(
                    predicate,
                    &prepared,
                    &mut scratch.selected_mask,
                    &mut scratch.filter_mask,
                )?
            }
        };
        if !applied {
            if filter.use_kind == CoveFilterUse::FullRowPredicateExact {
                stats.exactness_fallbacks += 1;
                return Err(CoveError::UnsupportedEncoding(format!(
                    "exact predicate {} cannot be evaluated by Cove",
                    filter.display
                )));
            }
            stats.kernel_fallbacks += 1;
            scratch.selection = Selection::AllRows {
                len: morsel.row_count as usize,
            };
            return Ok(());
        }
        if scratch.selected_mask.all_zero() {
            scratch.selection = Selection::None;
            return Ok(());
        }
    }
    scratch.selection = Selection::from_mask(&scratch.selected_mask, &mut scratch.selected_rows)?;
    Ok(())
}

pub(super) async fn selected_rows_for_morsel_metadata<R: CoveRangeReader + ?Sized>(
    state: &DatasetState,
    segment: &SegmentMetadata,
    segment_ref: &TableSegmentIndexEntryV1,
    morsel_id: u32,
    plan: &ScanPlan,
    reader: &R,
    stats: &mut DecodeStats,
    scratch: &mut DecodeScratch,
) -> Result<(), CoveError> {
    scratch.selected_rows.clear();
    scratch.selection = Selection::None;
    let morsel = segment.morsel(morsel_id)?;
    if !plan_has_row_predicate(plan) {
        scratch.selection = Selection::AllRows {
            len: morsel.row_count as usize,
        };
        return Ok(());
    }
    let skip_index_predicates = match lookup_selection_for_morsel(
        state,
        segment_ref.segment_id,
        morsel_id,
        morsel.row_count,
        plan,
        stats,
        scratch,
    )? {
        true => true,
        false => {
            scratch.selected_mask.fill_all(morsel.row_count as usize);
            false
        }
    };
    if scratch.selected_mask.all_zero() {
        scratch.selection = Selection::None;
        return Ok(());
    }
    for filter in &plan.filters {
        let Some(predicate) = &filter.predicate else {
            continue;
        };
        if matches!(predicate, CovePredicate::Null { .. }) {
            continue;
        }
        if skip_index_predicates && predicate_is_index_covered(state, predicate) {
            continue;
        }
        if matches!(
            predicate,
            CovePredicate::FileCodeIn { file_codes, .. } if file_codes.is_empty()
        ) {
            scratch.selection = Selection::None;
            return Ok(());
        }
        let Some(column_index) = predicate_column_index(predicate) else {
            continue;
        };
        let column = &state.table().columns[column_index];
        let segment_column = segment.column(column.column_id)?;
        let page = segment.page_for_morsel(segment_column, morsel_id)?;
        let page_wire =
            read_page_wire(reader, state, segment_ref, page, stats, RangeReadKind::Data).await?;
        stats.pages_decoded += usize::from(page.page_length != 0);
        let payload = match materialize_page_payload_from_wire(
            column,
            page,
            page_wire,
            state.page_payload_validation_policy(),
        ) {
            Ok(payload) => payload,
            Err(CoveError::UnsupportedEncoding(_)) => {
                if filter.use_kind == CoveFilterUse::FullRowPredicateExact {
                    stats.exactness_fallbacks += 1;
                    return Err(CoveError::UnsupportedEncoding(format!(
                        "exact predicate {} cannot be evaluated for page encoding",
                        filter.display
                    )));
                }
                stats.kernel_fallbacks += 1;
                scratch.selection = Selection::AllRows {
                    len: morsel.row_count as usize,
                };
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let dictionary = if matches!(predicate, CovePredicate::FileCodeIn { .. }) {
            None
        } else {
            state.mounted().dictionary.as_ref()
        };
        let array = encoded_array_for_page(&payload, page, dictionary)?;
        let applied = match try_apply_raw_predicate_to_selection(
            predicate,
            &array,
            &mut scratch.selected_mask,
            &mut scratch.filter_mask,
        )? {
            Some(()) => true,
            None => {
                let prepared = array.prepare()?;
                try_apply_predicate_to_selection(
                    predicate,
                    &prepared,
                    &mut scratch.selected_mask,
                    &mut scratch.filter_mask,
                )?
            }
        };
        if !applied {
            if filter.use_kind == CoveFilterUse::FullRowPredicateExact {
                stats.exactness_fallbacks += 1;
                return Err(CoveError::UnsupportedEncoding(format!(
                    "exact predicate {} cannot be evaluated by Cove",
                    filter.display
                )));
            }
            stats.kernel_fallbacks += 1;
            scratch.selection = Selection::AllRows {
                len: morsel.row_count as usize,
            };
            return Ok(());
        }
        if scratch.selected_mask.all_zero() {
            scratch.selection = Selection::None;
            return Ok(());
        }
    }
    scratch.selection = Selection::from_mask(&scratch.selected_mask, &mut scratch.selected_rows)?;
    Ok(())
}

async fn read_page_wire<R: CoveRangeReader + ?Sized>(
    reader: &R,
    state: &DatasetState,
    segment_ref: &TableSegmentIndexEntryV1,
    page: &ColumnPageIndexEntryV1,
    stats: &mut DecodeStats,
    kind: RangeReadKind,
) -> Result<Option<RetainedBytes>, CoveError> {
    if page.page_length == 0 {
        return Ok(None);
    }
    let start = segment_ref
        .offset
        .checked_add(page.page_offset)
        .ok_or(CoveError::ArithOverflow)?;
    let end = start
        .checked_add(page.page_length)
        .ok_or(CoveError::ArithOverflow)?;
    let ranges = vec![start..end];
    let coalesced_plan = build_coalesced_range_plan(&ranges, state.range_coalescing())?;
    let range_stats = coalesced_plan.stats();
    stats.original_range_requests += range_stats.original_ranges;
    stats.range_requests += range_stats.coalesced_ranges;
    stats.range_bytes_requested = stats
        .range_bytes_requested
        .checked_add(range_stats.coalesced_bytes)
        .ok_or(CoveError::ArithOverflow)?;
    stats.range_bytes_used = stats
        .range_bytes_used
        .checked_add(range_stats.original_bytes)
        .ok_or(CoveError::ArithOverflow)?;
    if range_stats.coalesced_ranges < range_stats.original_ranges {
        stats.coalesced_range_requests += range_stats.coalesced_ranges;
    }
    let mut wires = read_coalesced_range_buffers_for_plan(reader, kind, &coalesced_plan).await?;
    stats.data_bytes_read = stats
        .data_bytes_read
        .checked_add(wires.iter().map(RetainedBytes::len).sum::<usize>())
        .ok_or(CoveError::ArithOverflow)?;
    Ok(wires.pop())
}

pub(super) fn should_prune_morsel(
    state: &DatasetState,
    segment: &SegmentMetadata,
    morsel_id: u32,
    plan: &ScanPlan,
    stats: &mut DecodeStats,
) -> Result<bool, CoveError> {
    for filter in &plan.filters {
        if filter.use_kind != CoveFilterUse::PruningOnly {
            continue;
        }
        let Some(CovePredicate::Null { column_index, kind }) = filter.predicate.as_ref() else {
            continue;
        };
        let column = &state.table().columns[*column_index];
        let segment_column = segment.column(column.column_id)?;
        let page = segment.page_for_morsel(segment_column, morsel_id)?;
        stats.predicate_pages_checked += 1;
        match *kind {
            NullPredicateKind::IsNull if page.null_count == 0 => return Ok(true),
            NullPredicateKind::IsNotNull if page.non_null_count == 0 => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

pub(super) fn should_prune_morsel_metadata(
    state: &DatasetState,
    segment: &SegmentMetadata,
    morsel_id: u32,
    plan: &ScanPlan,
    stats: &mut DecodeStats,
) -> Result<bool, CoveError> {
    for filter in &plan.filters {
        if filter.use_kind != CoveFilterUse::PruningOnly {
            continue;
        }
        let Some(CovePredicate::Null { column_index, kind }) = filter.predicate.as_ref() else {
            continue;
        };
        let column = &state.table().columns[*column_index];
        let segment_column = segment.column(column.column_id)?;
        let page = segment.page_for_morsel(segment_column, morsel_id)?;
        stats.predicate_pages_checked += 1;
        match *kind {
            NullPredicateKind::IsNull if page.null_count == 0 => return Ok(true),
            NullPredicateKind::IsNotNull if page.non_null_count == 0 => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}
