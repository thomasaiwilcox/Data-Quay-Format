//! In-memory row visibility overlays for COVE DataFusion registration.

use std::{borrow::Cow, sync::Arc};

use cove_core::CoveError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveOverlaySnapshot {
    pub snapshot_id: Arc<str>,
    pub files: Vec<OverlayFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFile {
    pub uri: Arc<str>,
    pub expected_identity: Option<OverlayFileIdentity>,
    pub visibility: RowVisibility,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFileIdentity {
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: u32,
    pub digest: Option<OverlayFileDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFileDigest {
    pub algorithm: u16,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowRange {
    pub start: u64,
    pub len: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RowVisibility {
    #[default]
    All,
    VisibleRanges(Vec<RowRange>),
    DeletedRanges(Vec<RowRange>),
    DeletedBitmap {
        row_count: u64,
        bits: Vec<u64>,
    },
}

impl RowVisibility {
    pub fn normalized(self, total_rows: u64) -> Result<Self, CoveError> {
        match self {
            Self::All => Ok(Self::All),
            Self::VisibleRanges(ranges) => {
                Ok(Self::VisibleRanges(normalize_ranges(ranges, total_rows)?))
            }
            Self::DeletedRanges(ranges) => {
                let ranges = normalize_ranges(ranges, total_rows)?;
                if ranges.is_empty() {
                    Ok(Self::All)
                } else {
                    Ok(Self::DeletedRanges(ranges))
                }
            }
            Self::DeletedBitmap { row_count, bits } => Ok(Self::DeletedBitmap {
                row_count,
                bits: normalized_bitmap_bits(row_count, bits, total_rows)?,
            }),
        }
    }

    pub fn is_explicitly_hidden(&self) -> bool {
        matches!(self, Self::VisibleRanges(ranges) if ranges.is_empty())
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    pub fn visible_count(&self, total_rows: u64) -> Result<u64, CoveError> {
        match self {
            Self::All => Ok(total_rows),
            Self::VisibleRanges(ranges) => Ok(total_rows.min(covered_rows_in_range(
                normalized_ranges(ranges, total_rows)?.as_ref(),
                0,
                total_rows,
            )?)),
            Self::DeletedRanges(ranges) => {
                let deleted = covered_rows_in_range(
                    normalized_ranges(ranges, total_rows)?.as_ref(),
                    0,
                    total_rows,
                )?;
                total_rows
                    .checked_sub(deleted)
                    .ok_or(CoveError::ArithOverflow)
            }
            Self::DeletedBitmap { row_count, bits } => {
                let bits = normalized_bitmap_bits(*row_count, bits.clone(), total_rows)?;
                let deleted = bitmap_count_ones(&bits, total_rows);
                total_rows
                    .checked_sub(deleted)
                    .ok_or(CoveError::ArithOverflow)
            }
        }
    }

    pub fn is_row_visible(&self, row: u64, total_rows: u64) -> Result<bool, CoveError> {
        if row >= total_rows {
            return Err(CoveError::OffsetRange);
        }
        match self {
            Self::All => Ok(true),
            Self::VisibleRanges(ranges) => Ok(range_set_contains(
                normalized_ranges(ranges, total_rows)?.as_ref(),
                row,
            )),
            Self::DeletedRanges(ranges) => Ok(!range_set_contains(
                normalized_ranges(ranges, total_rows)?.as_ref(),
                row,
            )),
            Self::DeletedBitmap { row_count, bits } => {
                validate_bitmap_shape(*row_count, bits, total_rows)?;
                Ok(!bitmap_contains(bits, row))
            }
        }
    }

    pub fn hidden_rows_in_range(
        &self,
        start: u64,
        len: u32,
        total_rows: u64,
    ) -> Result<u32, CoveError> {
        let end = start
            .checked_add(u64::from(len))
            .ok_or(CoveError::ArithOverflow)?;
        if end > total_rows {
            return Err(CoveError::OffsetRange);
        }
        let hidden = match self {
            Self::All => 0,
            Self::VisibleRanges(ranges) => {
                let visible = covered_rows_in_range(
                    normalized_ranges(ranges, total_rows)?.as_ref(),
                    start,
                    end,
                )?;
                u64::from(len)
                    .checked_sub(visible)
                    .ok_or(CoveError::ArithOverflow)?
            }
            Self::DeletedRanges(ranges) => {
                covered_rows_in_range(normalized_ranges(ranges, total_rows)?.as_ref(), start, end)?
            }
            Self::DeletedBitmap { row_count, bits } => {
                validate_bitmap_shape(*row_count, bits, total_rows)?;
                bitmap_count_deleted_in_range(bits, start, end)?
            }
        };
        u32::try_from(hidden).map_err(|_| CoveError::ArithOverflow)
    }

    pub fn morsel_all_hidden(
        &self,
        start: u64,
        len: u32,
        total_rows: u64,
    ) -> Result<bool, CoveError> {
        Ok(self.hidden_rows_in_range(start, len, total_rows)? == len)
    }
}

impl RowRange {
    pub fn end(self) -> Result<u64, CoveError> {
        self.start
            .checked_add(self.len)
            .ok_or(CoveError::ArithOverflow)
    }
}

fn validate_ranges(ranges: &[RowRange], total_rows: u64) -> Result<(), CoveError> {
    for range in ranges {
        if range.len == 0 {
            continue;
        }
        let end = range.end()?;
        if end > total_rows {
            return Err(CoveError::OffsetRange);
        }
    }
    Ok(())
}

fn normalized_ranges<'a>(
    ranges: &'a [RowRange],
    total_rows: u64,
) -> Result<Cow<'a, [RowRange]>, CoveError> {
    if ranges_are_normalized(ranges, total_rows)? {
        Ok(Cow::Borrowed(ranges))
    } else {
        Ok(Cow::Owned(normalize_ranges(ranges.to_vec(), total_rows)?))
    }
}

fn ranges_are_normalized(ranges: &[RowRange], total_rows: u64) -> Result<bool, CoveError> {
    let mut previous_end = 0u64;
    let mut saw_previous = false;
    for range in ranges {
        if range.len == 0 {
            return Ok(false);
        }
        let end = range.end()?;
        if end > total_rows {
            return Err(CoveError::OffsetRange);
        }
        if saw_previous && range.start <= previous_end {
            return Ok(false);
        }
        previous_end = end;
        saw_previous = true;
    }
    Ok(true)
}

fn normalize_ranges(
    mut ranges: Vec<RowRange>,
    total_rows: u64,
) -> Result<Vec<RowRange>, CoveError> {
    validate_ranges(&ranges, total_rows)?;
    ranges.retain(|range| range.len != 0);
    ranges.sort_by_key(|range| (range.start, range.len));
    let mut merged = Vec::with_capacity(ranges.len());
    for range in ranges {
        let Some(active) = merged.last_mut() else {
            merged.push(range);
            continue;
        };
        let active_end = active.end()?;
        let range_end = range.end()?;
        if range.start <= active_end {
            let merged_end = active_end.max(range_end);
            active.len = merged_end
                .checked_sub(active.start)
                .ok_or(CoveError::ArithOverflow)?;
        } else {
            merged.push(range);
        }
    }
    Ok(merged)
}

fn range_set_contains(ranges: &[RowRange], row: u64) -> bool {
    let candidate = ranges.partition_point(|range| match range.end() {
        Ok(end) => end <= row,
        Err(_) => false,
    });
    ranges
        .get(candidate)
        .map(|range| row >= range.start && row < range.start.saturating_add(range.len))
        .unwrap_or(false)
}

fn covered_rows_in_range(ranges: &[RowRange], start: u64, end: u64) -> Result<u64, CoveError> {
    let mut covered = 0u64;
    let mut index = ranges.partition_point(|range| match range.end() {
        Ok(range_end) => range_end <= start,
        Err(_) => false,
    });
    while let Some(range) = ranges.get(index) {
        if range.start >= end {
            break;
        }
        let range_end = range.end()?;
        let overlap_start = start.max(range.start);
        let overlap_end = end.min(range_end);
        if overlap_end > overlap_start {
            covered = covered
                .checked_add(overlap_end - overlap_start)
                .ok_or(CoveError::ArithOverflow)?;
        }
        index += 1;
    }
    Ok(covered)
}

fn validate_bitmap_shape(row_count: u64, bits: &[u64], total_rows: u64) -> Result<(), CoveError> {
    if row_count != total_rows {
        return Err(CoveError::BadSection(format!(
            "overlay bitmap row_count {row_count} does not match file row_count {total_rows}"
        )));
    }
    let required_words =
        usize::try_from(total_rows.div_ceil(64)).map_err(|_| CoveError::ArithOverflow)?;
    if bits.len() < required_words {
        return Err(CoveError::BadSection(
            "overlay deleted bitmap is shorter than row_count".into(),
        ));
    }
    Ok(())
}

fn normalized_bitmap_bits(
    row_count: u64,
    bits: Vec<u64>,
    total_rows: u64,
) -> Result<Vec<u64>, CoveError> {
    validate_bitmap_shape(row_count, &bits, total_rows)?;
    let required_words =
        usize::try_from(total_rows.div_ceil(64)).map_err(|_| CoveError::ArithOverflow)?;
    let mut normalized = bits.into_iter().take(required_words).collect::<Vec<_>>();
    if let Some(last) = normalized.last_mut() {
        let tail = total_rows % 64;
        if tail != 0 {
            *last &= (1u64 << tail) - 1;
        }
    }
    Ok(normalized)
}

fn bitmap_count_ones(bits: &[u64], total_rows: u64) -> u64 {
    let full_words = usize::try_from(total_rows / 64).unwrap_or(0);
    let mut total = bits
        .iter()
        .take(full_words)
        .map(|word| u64::from(word.count_ones()))
        .sum::<u64>();
    let tail = total_rows % 64;
    if tail != 0 {
        if let Some(word) = bits.get(full_words) {
            let mask = (1u64 << tail) - 1;
            total += u64::from((word & mask).count_ones());
        }
    }
    total
}

fn bitmap_count_deleted_in_range(bits: &[u64], start: u64, end: u64) -> Result<u64, CoveError> {
    if start >= end {
        return Ok(0);
    }
    let mut deleted = 0u64;
    let start_word = usize::try_from(start / 64).map_err(|_| CoveError::ArithOverflow)?;
    let end_word =
        usize::try_from((end.saturating_sub(1)) / 64).map_err(|_| CoveError::ArithOverflow)?;
    for word_index in start_word..=end_word {
        let word = bits.get(word_index).copied().unwrap_or(0);
        let word_start = u64::try_from(word_index)
            .map_err(|_| CoveError::ArithOverflow)?
            .checked_mul(64)
            .ok_or(CoveError::ArithOverflow)?;
        let word_end = word_start.checked_add(64).ok_or(CoveError::ArithOverflow)?;
        let overlap_start = start.max(word_start);
        let overlap_end = end.min(word_end);
        let start_bit = overlap_start
            .checked_sub(word_start)
            .ok_or(CoveError::ArithOverflow)?;
        let bit_len = overlap_end
            .checked_sub(overlap_start)
            .ok_or(CoveError::ArithOverflow)?;
        let mask = if bit_len == 64 {
            u64::MAX
        } else {
            ((1u64 << bit_len) - 1) << start_bit
        };
        deleted += u64::from((word & mask).count_ones());
    }
    Ok(deleted)
}

fn bitmap_contains(bits: &[u64], row: u64) -> bool {
    let word = row as usize / 64;
    let bit = row % 64;
    bits.get(word)
        .map(|value| value & (1u64 << bit) != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{RowRange, RowVisibility};

    #[test]
    fn visible_ranges_are_normalized_once() {
        let visibility = RowVisibility::VisibleRanges(vec![
            RowRange { start: 5, len: 2 },
            RowRange { start: 1, len: 2 },
            RowRange { start: 3, len: 2 },
            RowRange { start: 0, len: 0 },
        ])
        .normalized(10)
        .unwrap();
        assert_eq!(
            visibility,
            RowVisibility::VisibleRanges(vec![RowRange { start: 1, len: 6 }])
        );
        assert_eq!(visibility.visible_count(10).unwrap(), 6);
    }

    #[test]
    fn deleted_ranges_hidden_counts_use_interval_math() {
        let visibility = RowVisibility::DeletedRanges(vec![
            RowRange { start: 2, len: 3 },
            RowRange { start: 6, len: 2 },
        ])
        .normalized(10)
        .unwrap();
        assert_eq!(visibility.hidden_rows_in_range(0, 10, 10).unwrap(), 5);
        assert_eq!(visibility.hidden_rows_in_range(1, 3, 10).unwrap(), 2);
        assert!(visibility.morsel_all_hidden(2, 3, 10).unwrap());
        assert!(!visibility.morsel_all_hidden(1, 3, 10).unwrap());
    }

    #[test]
    fn deleted_bitmap_counts_only_requested_rows() {
        let visibility = RowVisibility::DeletedBitmap {
            row_count: 70,
            bits: vec![0b1011, 0b11, u64::MAX],
        }
        .normalized(70)
        .unwrap();
        assert_eq!(visibility.visible_count(70).unwrap(), 65);
        assert_eq!(visibility.hidden_rows_in_range(0, 4, 70).unwrap(), 3);
        assert_eq!(visibility.hidden_rows_in_range(64, 6, 70).unwrap(), 2);
        assert!(!visibility.is_row_visible(0, 70).unwrap());
        assert!(visibility.is_row_visible(2, 70).unwrap());
    }
}
