//! DataFusion-agnostic byte-range reader abstractions.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Range,
    path::{Path, PathBuf},
    sync::Mutex,
};

use async_trait::async_trait;
use cove_core::CoveError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeCoalescingOptions {
    pub max_gap: u64,
    pub max_span: u64,
}

impl Default for RangeCoalescingOptions {
    fn default() -> Self {
        Self {
            max_gap: 4096,
            max_span: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeReadKind {
    Metadata,
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeReadMode {
    Sparse,
    Mixed,
    Dense,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeReadPlan {
    pub mode: RangeReadMode,
    pub original_ranges: usize,
    pub coalesced_ranges: usize,
}

impl RangeReadPlan {
    pub fn choose(
        selected_rows: usize,
        row_count: usize,
        original_ranges: usize,
        coalesced_ranges: usize,
    ) -> Self {
        let mode =
            if row_count != 0 && selected_rows.saturating_mul(5) >= row_count.saturating_mul(4) {
                RangeReadMode::Dense
            } else if coalesced_ranges < original_ranges {
                RangeReadMode::Mixed
            } else {
                RangeReadMode::Sparse
            };
        Self {
            mode,
            original_ranges,
            coalesced_ranges,
        }
    }
}

#[async_trait]
pub trait CoveRangeReader: Send + Sync {
    async fn read_ranges(
        &self,
        ranges: &[Range<u64>],
        kind: RangeReadKind,
    ) -> Result<Vec<Vec<u8>>, CoveError>;

    fn record_coalescing(&self, _original_ranges: usize, _coalesced_ranges: usize) {}

    async fn read_range(
        &self,
        range: Range<u64>,
        kind: RangeReadKind,
    ) -> Result<Vec<u8>, CoveError> {
        let mut ranges = self.read_ranges(&[range], kind).await?;
        ranges.pop().ok_or(CoveError::BufferTooShort)
    }
}

#[derive(Debug)]
pub struct LocalFileRangeReader {
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl LocalFileRangeReader {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            file: Mutex::new(None),
        }
    }
}

#[async_trait]
impl CoveRangeReader for LocalFileRangeReader {
    async fn read_ranges(
        &self,
        ranges: &[Range<u64>],
        _kind: RangeReadKind,
    ) -> Result<Vec<Vec<u8>>, CoveError> {
        // INVARIANT: a LocalFileRangeReader is bound to one immutable COVE file
        // snapshot. Reusing the descriptor avoids repeated open/close overhead
        // while preserving checked seek/read bounds for every requested range.
        let mut guard = self
            .file
            .lock()
            .map_err(|_| CoveError::BadSection("local range reader file lock poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(File::open(&self.path)?);
        }
        let file = guard.as_mut().ok_or(CoveError::BufferTooShort)?;
        let mut out = Vec::with_capacity(ranges.len());
        for range in ranges {
            out.push(read_file_range(file, range)?);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone)]
pub struct MemoryRangeReader {
    bytes: std::sync::Arc<Vec<u8>>,
}

impl MemoryRangeReader {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: std::sync::Arc::new(bytes),
        }
    }

    pub fn from_arc(bytes: std::sync::Arc<Vec<u8>>) -> Self {
        Self { bytes }
    }
}

#[async_trait]
impl CoveRangeReader for MemoryRangeReader {
    async fn read_ranges(
        &self,
        ranges: &[Range<u64>],
        _kind: RangeReadKind,
    ) -> Result<Vec<Vec<u8>>, CoveError> {
        ranges
            .iter()
            .map(|range| {
                let start = usize::try_from(range.start).map_err(|_| CoveError::OffsetRange)?;
                let len = range_len(range)?;
                let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
                if end > self.bytes.len() {
                    return Err(CoveError::OffsetRange);
                }
                Ok(self.bytes[start..end].to_vec())
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct OriginalRange {
    index: usize,
    start: u64,
    end: u64,
}

#[derive(Debug, Clone)]
struct CoalescedRange {
    start: u64,
    end: u64,
    originals: Vec<OriginalRange>,
}

pub async fn read_coalesced_ranges<R: CoveRangeReader + ?Sized>(
    reader: &R,
    ranges: &[Range<u64>],
    kind: RangeReadKind,
) -> Result<Vec<Vec<u8>>, CoveError> {
    read_coalesced_ranges_with_options(reader, ranges, kind, RangeCoalescingOptions::default())
        .await
}

pub async fn read_coalesced_ranges_with_options<R: CoveRangeReader + ?Sized>(
    reader: &R,
    ranges: &[Range<u64>],
    kind: RangeReadKind,
    options: RangeCoalescingOptions,
) -> Result<Vec<Vec<u8>>, CoveError> {
    if ranges.is_empty() {
        return Ok(Vec::new());
    }
    let coalesced = coalesce_ranges(ranges, options.max_gap, options.max_span)?;
    reader.record_coalescing(ranges.len(), coalesced.len());
    let reads = coalesced
        .iter()
        .map(|range| range.start..range.end)
        .collect::<Vec<_>>();
    let coalesced_bytes = reader.read_ranges(&reads, kind).await?;
    if coalesced_bytes.len() != coalesced.len() {
        return Err(CoveError::BufferTooShort);
    }

    let mut out = vec![Vec::new(); ranges.len()];
    for (group, bytes) in coalesced.iter().zip(coalesced_bytes.iter()) {
        for original in &group.originals {
            let offset = usize::try_from(
                original
                    .start
                    .checked_sub(group.start)
                    .ok_or(CoveError::OffsetRange)?,
            )
            .map_err(|_| CoveError::OffsetRange)?;
            let len = usize::try_from(
                original
                    .end
                    .checked_sub(original.start)
                    .ok_or(CoveError::OffsetRange)?,
            )
            .map_err(|_| CoveError::OffsetRange)?;
            let end = offset.checked_add(len).ok_or(CoveError::ArithOverflow)?;
            if end > bytes.len() {
                return Err(CoveError::OffsetRange);
            }
            out[original.index] = bytes[offset..end].to_vec();
        }
    }
    Ok(out)
}

pub fn coalesced_range_count(
    ranges: &[Range<u64>],
    options: RangeCoalescingOptions,
) -> Result<usize, CoveError> {
    coalesce_ranges(ranges, options.max_gap, options.max_span).map(|ranges| ranges.len())
}

fn coalesce_ranges(
    ranges: &[Range<u64>],
    max_gap: u64,
    max_span: u64,
) -> Result<Vec<CoalescedRange>, CoveError> {
    let mut sorted = ranges
        .iter()
        .enumerate()
        .map(|(index, range)| {
            if range.start > range.end {
                return Err(CoveError::OffsetRange);
            }
            Ok(OriginalRange {
                index,
                start: range.start,
                end: range.end,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    sorted.sort_by_key(|range| (range.start, range.end));

    let mut groups: Vec<CoalescedRange> = Vec::new();
    for range in sorted {
        let Some(last) = groups.last_mut() else {
            groups.push(CoalescedRange {
                start: range.start,
                end: range.end,
                originals: vec![range],
            });
            continue;
        };
        let gap = range.start.saturating_sub(last.end);
        let merged_end = last.end.max(range.end);
        let merged_span = merged_end
            .checked_sub(last.start)
            .ok_or(CoveError::OffsetRange)?;
        if gap <= max_gap && merged_span <= max_span {
            last.end = merged_end;
            last.originals.push(range);
        } else {
            groups.push(CoalescedRange {
                start: range.start,
                end: range.end,
                originals: vec![range],
            });
        }
    }
    Ok(groups)
}

fn read_file_range(file: &mut File, range: &Range<u64>) -> Result<Vec<u8>, CoveError> {
    let len = range_len(range)?;
    let mut bytes = vec![0u8; len];
    file.seek(SeekFrom::Start(range.start))?;
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn range_len(range: &Range<u64>) -> Result<usize, CoveError> {
    let len = range
        .end
        .checked_sub(range.start)
        .ok_or(CoveError::OffsetRange)?;
    usize::try_from(len).map_err(|_| CoveError::OffsetRange)
}
