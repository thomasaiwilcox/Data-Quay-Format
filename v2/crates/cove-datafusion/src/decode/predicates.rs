use super::*;

pub(super) fn plan_has_row_predicate(plan: &ScanPlan) -> bool {
    plan.filters.iter().any(|filter| {
        matches!(
            filter.predicate,
            Some(
                CovePredicate::Numeric { .. }
                    | CovePredicate::FileCodeIn { .. }
                    | CovePredicate::VarBytesEq { .. }
            )
        )
    })
}

pub(super) fn plan_has_exact_row_predicate(plan: &ScanPlan) -> bool {
    plan.filters.iter().any(|filter| {
        filter.use_kind == CoveFilterUse::FullRowPredicateExact
            && matches!(
                filter.predicate,
                Some(
                    CovePredicate::Numeric { .. }
                        | CovePredicate::FileCodeIn { .. }
                        | CovePredicate::VarBytesEq { .. }
                )
            )
    })
}

pub(super) fn lookup_selection_for_morsel(
    state: &DatasetState,
    segment_id: u32,
    morsel_id: u32,
    row_count: u32,
    plan: &ScanPlan,
    stats: &mut DecodeStats,
    scratch: &mut DecodeScratch,
) -> Result<bool, CoveError> {
    let mut saw_lookup_filter = false;
    scratch.selected_mask.fill_all(row_count as usize);
    for filter in &plan.filters {
        let (column_index, key_kind, keys) = match &filter.predicate {
            Some(CovePredicate::FileCodeIn {
                column_index,
                file_codes,
                ..
            }) => (
                *column_index,
                LookupKeyKind::FileCode,
                file_codes
                    .iter()
                    .copied()
                    .map(u64::from)
                    .collect::<Vec<_>>(),
            ),
            Some(CovePredicate::Numeric {
                column_index,
                op: NumericPredicateOp::Eq,
                literal,
            }) => {
                let Some(key) = numeric_lookup_key(*literal) else {
                    continue;
                };
                (*column_index, LookupKeyKind::NumCode, vec![key])
            }
            _ => continue,
        };
        let column = &state.table().columns[column_index];
        let Some(index) = state.lookup_for(column.column_id) else {
            if saw_lookup_filter && key_kind == LookupKeyKind::FileCode {
                stats.index_fallbacks += 1;
                return Ok(false);
            }
            continue;
        };
        if index.header.key_kind != key_kind {
            stats.index_fallbacks += 1;
            return Ok(false);
        }
        saw_lookup_filter = true;
        scratch.filter_mask.fill_none(row_count as usize);
        for key in keys {
            match index.rows_for(key) {
                Some(rows) if !rows.is_empty() => {
                    stats.lookup_index_hits += 1;
                    for row in rows {
                        if row.table_id != state.table().table_id
                            || row.segment_id != segment_id
                            || row.morsel_id != morsel_id
                        {
                            continue;
                        }
                        let row_index = usize::from(row.row_in_morsel);
                        if row_index >= scratch.filter_mask.len {
                            stats.index_fallbacks += 1;
                            return Ok(false);
                        }
                        scratch.filter_mask.set(row_index);
                    }
                }
                _ => stats.lookup_index_misses += 1,
            }
        }
        scratch.selected_mask.and_inplace(&scratch.filter_mask);
        if scratch.selected_mask.all_zero() {
            break;
        }
    }
    if saw_lookup_filter {
        stats.index_rows_selected += scratch.selected_mask.count_ones();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[inline]
pub(super) fn predicate_is_index_covered(state: &DatasetState, predicate: &CovePredicate) -> bool {
    match predicate {
        CovePredicate::FileCodeIn { column_index, .. } => {
            let column = &state.table().columns[*column_index];
            state
                .lookup_for(column.column_id)
                .map(|index| index.header.key_kind == LookupKeyKind::FileCode)
                .unwrap_or(false)
        }
        CovePredicate::Numeric {
            column_index,
            op: NumericPredicateOp::Eq,
            literal,
        } if numeric_lookup_key(*literal).is_some() => {
            let column = &state.table().columns[*column_index];
            state
                .lookup_for(column.column_id)
                .map(|index| index.header.key_kind == LookupKeyKind::NumCode)
                .unwrap_or(false)
        }
        _ => false,
    }
}

#[inline]
pub(crate) fn numeric_lookup_key(literal: PredicateLiteral) -> Option<u64> {
    match literal {
        PredicateLiteral::Int64(value) => u64::try_from(value).ok(),
        PredicateLiteral::UInt64(value) => Some(value),
        PredicateLiteral::Float64(value) if value.is_finite() && value.fract() == 0.0 => {
            if value >= 0.0 && value <= u64::MAX as f64 {
                Some(value as u64)
            } else {
                None
            }
        }
        PredicateLiteral::Float64(_) => None,
    }
}

pub(super) fn plan_has_residual(plan: &ScanPlan) -> bool {
    plan.filters
        .iter()
        .any(|filter| filter.use_kind == CoveFilterUse::PruningOnly)
}

#[inline]
pub(super) fn predicate_column_index(predicate: &CovePredicate) -> Option<usize> {
    match predicate {
        CovePredicate::Null { column_index, .. }
        | CovePredicate::Numeric { column_index, .. }
        | CovePredicate::FileCodeIn { column_index, .. }
        | CovePredicate::VarBytesEq { column_index, .. } => Some(*column_index),
    }
}

pub(super) fn apply_predicate_to_selection(
    predicate: &CovePredicate,
    prepared: &PreparedEncodedArray<'_>,
    selected: &mut SelectionMask,
    scratch: &mut SelectionMask,
) -> Result<bool, CoveError> {
    if let Some(()) =
        try_apply_numcode_predicate_to_selection(predicate, prepared.array(), selected, scratch)?
    {
        return Ok(true);
    }
    if let Some(()) = try_apply_varbytes_eq_predicate_to_selection(
        predicate,
        prepared.array(),
        selected,
        scratch,
    )? {
        return Ok(true);
    }
    for word_index in 0..selected.words.len() {
        let mut remaining = selected.words[word_index];
        while remaining != 0 {
            let bit = remaining.trailing_zeros() as usize;
            let index = word_index
                .checked_mul(64)
                .and_then(|base| base.checked_add(bit))
                .ok_or_else(|| CoveError::ArithOverflow)?;
            if index >= selected.len {
                break;
            }
            let row = u64::try_from(index).map_err(|_| CoveError::ArithOverflow)?;
            let array = prepared.array();
            let keep = match predicate {
                CovePredicate::Null { kind, .. } => {
                    let is_null = array.is_null(row)?;
                    match kind {
                        NullPredicateKind::IsNull => is_null,
                        NullPredicateKind::IsNotNull => !is_null,
                    }
                }
                CovePredicate::Numeric { op, literal, .. } => {
                    let value = prepared.decode_row(row)?;
                    match compare_numeric_value(&value, *op, *literal)? {
                        Some(value) => value,
                        None => return Ok(false),
                    }
                }
                CovePredicate::FileCodeIn { file_codes, .. } => {
                    if array.is_null(row)? {
                        false
                    } else {
                        let code = match raw_file_code_at(array, row)? {
                            Some(code) => Some(code),
                            None => match prepared.decode_row(row)? {
                                CoveArrayValue::FileCode(code) => Some(code),
                                _ => None,
                            },
                        };
                        match code {
                            Some(code) => file_codes.binary_search(&code).is_ok(),
                            None => return Ok(false),
                        }
                    }
                }
                CovePredicate::VarBytesEq { literal, .. } => {
                    if array.is_null(row)? {
                        false
                    } else {
                        match prepared.decode_row(row)? {
                            CoveArrayValue::Bytes(bytes) => bytes == literal.as_slice(),
                            CoveArrayValue::OwnedBytes(bytes) => {
                                bytes.as_slice() == literal.as_slice()
                            }
                            _ => return Ok(false),
                        }
                    }
                }
            };
            if !keep {
                selected.clear_bit(index);
            }
            remaining &= remaining - 1;
        }
    }
    Ok(true)
}

pub(super) fn try_apply_raw_predicate_to_selection(
    predicate: &CovePredicate,
    array: &EncodedArray<'_>,
    selected: &mut SelectionMask,
    scratch: &mut SelectionMask,
) -> Result<Option<()>, CoveError> {
    if let Some(()) = try_apply_numcode_predicate_to_selection(predicate, array, selected, scratch)?
    {
        return Ok(Some(()));
    }
    if let Some(()) =
        try_apply_varbytes_eq_predicate_to_selection(predicate, array, selected, scratch)?
    {
        return Ok(Some(()));
    }
    Ok(None)
}

fn try_apply_numcode_predicate_to_selection(
    predicate: &CovePredicate,
    array: &EncodedArray<'_>,
    selected: &mut SelectionMask,
    scratch: &mut SelectionMask,
) -> Result<Option<()>, CoveError> {
    let CovePredicate::Numeric { op, literal, .. } = predicate else {
        return Ok(None);
    };
    if array.encoding != CoveEncodingKind::NumCode || array.physical != CovePhysicalKind::NumCode {
        return Ok(None);
    }

    scratch.clone_from_mask(selected);
    for word_index in 0..selected.words.len() {
        let mut remaining = selected.words[word_index];
        while remaining != 0 {
            let bit = remaining.trailing_zeros() as usize;
            let index = word_index
                .checked_mul(64)
                .and_then(|base| base.checked_add(bit))
                .ok_or_else(|| CoveError::ArithOverflow)?;
            if index >= selected.len {
                break;
            }
            let row = u64::try_from(index).map_err(|_| CoveError::ArithOverflow)?;
            let keep = if array.is_null(row)? {
                false
            } else {
                let code = raw_numcode_at(array, row)?;
                match compare_numcode_value(code, *op, *literal) {
                    Ok(value) => value,
                    Err(CoveError::UnsupportedEncoding(_)) => return Ok(None),
                    Err(error) => return Err(error),
                }
            };
            if !keep {
                scratch.clear_bit(index);
            }
            remaining &= remaining - 1;
        }
    }
    std::mem::swap(selected, scratch);
    Ok(Some(()))
}

fn try_apply_varbytes_eq_predicate_to_selection(
    predicate: &CovePredicate,
    array: &EncodedArray<'_>,
    selected: &mut SelectionMask,
    scratch: &mut SelectionMask,
) -> Result<Option<()>, CoveError> {
    let CovePredicate::VarBytesEq { literal, .. } = predicate else {
        return Ok(None);
    };
    if array.encoding != CoveEncodingKind::VarBytes || array.physical != CovePhysicalKind::VarBytes
    {
        return Ok(None);
    }
    let row_count = usize::try_from(array.row_count).map_err(|_| CoveError::ArithOverflow)?;
    if selected.len != row_count {
        return Err(CoveError::BadSection(format!(
            "selection length {} does not match VarBytes row count {row_count}",
            selected.len
        )));
    }

    scratch.clone_from_mask(selected);
    let all_non_null = array.validity.is_none();
    let all_selected = selected.count_ones() == row_count;
    let data = array.data;
    let mut offset = 0usize;
    for row in 0..row_count {
        let header_end = offset
            .checked_add(4)
            .ok_or_else(|| CoveError::ArithOverflow)?;
        if header_end > data.len() {
            return Err(CoveError::OffsetRange);
        }
        let len = u32::from_le_bytes(data[offset..header_end].try_into().unwrap()) as usize;
        offset = header_end;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| CoveError::ArithOverflow)?;
        if end > data.len() {
            return Err(CoveError::OffsetRange);
        }
        let value = &data[offset..end];

        let is_selected = if all_selected {
            true
        } else {
            let word = selected.words.get(row / 64).copied().unwrap_or(0);
            (word & (1u64 << (row % 64))) != 0
        };
        if is_selected {
            let is_non_null = if all_non_null {
                true
            } else {
                let row_u64 = u64::try_from(row).map_err(|_| CoveError::ArithOverflow)?;
                !array.is_null(row_u64)?
            };
            let keep = is_non_null && value.len() == literal.len() && value == literal.as_slice();
            if !keep {
                scratch.clear_bit(row);
            }
        }
        offset = end;
    }
    if offset != array.data.len() {
        return Err(CoveError::PageCorrupt);
    }
    std::mem::swap(selected, scratch);
    Ok(Some(()))
}

pub(super) fn try_apply_predicate_to_selection(
    predicate: &CovePredicate,
    prepared: &PreparedEncodedArray<'_>,
    selected: &mut SelectionMask,
    scratch: &mut SelectionMask,
) -> Result<bool, CoveError> {
    match apply_predicate_to_selection(predicate, prepared, selected, scratch) {
        Ok(value) => Ok(value),
        Err(CoveError::UnsupportedEncoding(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

fn compare_numcode_value(
    value: u64,
    op: NumericPredicateOp,
    literal: PredicateLiteral,
) -> Result<bool, CoveError> {
    match literal {
        PredicateLiteral::Int64(literal) => {
            let value = i64::try_from(value)
                .map_err(|_| CoveError::UnsupportedEncoding("NumCode value exceeds i64".into()))?;
            Ok(compare_ordered(value, op, literal))
        }
        PredicateLiteral::UInt64(literal) => Ok(compare_ordered(value, op, literal)),
        PredicateLiteral::Float64(literal) => Ok(compare_ordered(value as f64, op, literal)),
    }
}

fn compare_numeric_value(
    value: &CoveArrayValue<'_>,
    op: NumericPredicateOp,
    literal: PredicateLiteral,
) -> Result<Option<bool>, CoveError> {
    if matches!(value, CoveArrayValue::Null) {
        return Ok(Some(false));
    }
    match literal {
        PredicateLiteral::Int64(literal) => {
            let Some(value) = value_as_i64(value)? else {
                return Ok(None);
            };
            Ok(Some(compare_ordered(value, op, literal)))
        }
        PredicateLiteral::UInt64(literal) => {
            let Some(value) = value_as_u64(value)? else {
                return Ok(None);
            };
            Ok(Some(compare_ordered(value, op, literal)))
        }
        PredicateLiteral::Float64(literal) => {
            let Some(value) = value_as_f64(value)? else {
                return Ok(None);
            };
            Ok(Some(compare_ordered(value, op, literal)))
        }
    }
}

fn compare_ordered<T: PartialOrd + PartialEq>(left: T, op: NumericPredicateOp, right: T) -> bool {
    match op {
        NumericPredicateOp::Eq => left == right,
        NumericPredicateOp::Lt => left < right,
        NumericPredicateOp::LtEq => left <= right,
        NumericPredicateOp::Gt => left > right,
        NumericPredicateOp::GtEq => left >= right,
    }
}

fn value_as_i64(value: &CoveArrayValue<'_>) -> Result<Option<i64>, CoveError> {
    match value {
        CoveArrayValue::NumCode(value) | CoveArrayValue::Varint(value) => i64::try_from(*value)
            .map(Some)
            .map_err(|_| CoveError::UnsupportedEncoding("NumCode value exceeds i64".into())),
        CoveArrayValue::Int64(value) => Ok(Some(*value)),
        CoveArrayValue::Bytes(bytes) if bytes.len() == 8 => {
            Ok(Some(i64::from_le_bytes((*bytes).try_into().unwrap())))
        }
        _ => Ok(None),
    }
}

fn value_as_u64(value: &CoveArrayValue<'_>) -> Result<Option<u64>, CoveError> {
    match value {
        CoveArrayValue::NumCode(value) | CoveArrayValue::Varint(value) => Ok(Some(*value)),
        CoveArrayValue::Int64(value) => u64::try_from(*value).map(Some).map_err(|_| {
            CoveError::UnsupportedEncoding("negative value cannot compare as u64".into())
        }),
        CoveArrayValue::Bytes(bytes) if bytes.len() == 8 => {
            Ok(Some(u64::from_le_bytes((*bytes).try_into().unwrap())))
        }
        _ => Ok(None),
    }
}

fn value_as_f64(value: &CoveArrayValue<'_>) -> Result<Option<f64>, CoveError> {
    match value {
        CoveArrayValue::NumCode(value) | CoveArrayValue::Varint(value) => Ok(Some(*value as f64)),
        CoveArrayValue::Int64(value) => Ok(Some(*value as f64)),
        CoveArrayValue::Bytes(bytes) if bytes.len() == 8 => {
            let value = f64::from_bits(u64::from_le_bytes((*bytes).try_into().unwrap()));
            if value.is_nan() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        }
        _ => Ok(None),
    }
}

fn raw_file_code_at(array: &EncodedArray<'_>, row: u64) -> Result<Option<u32>, CoveError> {
    if array.encoding != CoveEncodingKind::FileCode {
        return Ok(None);
    }
    let offset = usize::try_from(row)
        .map_err(|_| CoveError::ArithOverflow)?
        .checked_mul(4)
        .ok_or_else(|| CoveError::ArithOverflow)?;
    let bytes = wire::read_range_checked(array.data, offset, 4)?;
    Ok(Some(u32::from_le_bytes(bytes.try_into().unwrap())))
}

fn raw_numcode_at(array: &EncodedArray<'_>, row: u64) -> Result<u64, CoveError> {
    let offset = usize::try_from(row)
        .map_err(|_| CoveError::ArithOverflow)?
        .checked_mul(8)
        .ok_or_else(|| CoveError::ArithOverflow)?;
    let bytes = wire::read_range_checked(array.data, offset, 8)?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}
