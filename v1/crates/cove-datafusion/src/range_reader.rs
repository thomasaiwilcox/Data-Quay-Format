//! DataFusion-agnostic byte-range reader abstractions.

use std::{
    fs::File,
    mem::MaybeUninit,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use cove_core::{
    retained_bytes::{RetainedByteOwner, RetainedByteSource, RetainedBytes},
    CoveError,
};
use memmap2::Mmap;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoalescedRangeStats {
    pub original_ranges: usize,
    pub coalesced_ranges: usize,
    pub original_bytes: usize,
    pub coalesced_bytes: usize,
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
pub trait CoveRangeReader: std::fmt::Debug + Send + Sync {
    async fn read_ranges(
        &self,
        ranges: &[Range<u64>],
        kind: RangeReadKind,
    ) -> Result<Vec<Vec<u8>>, CoveError>;

    fn record_coalescing(&self, _original_ranges: usize, _coalesced_ranges: usize) {}

    async fn read_range_buffers(
        &self,
        ranges: &[Range<u64>],
        kind: RangeReadKind,
    ) -> Result<Vec<RetainedBytes>, CoveError> {
        Ok(self
            .read_ranges(ranges, kind)
            .await?
            .into_iter()
            .map(RetainedBytes::from_vec)
            .collect())
    }

    async fn read_range(
        &self,
        range: Range<u64>,
        kind: RangeReadKind,
    ) -> Result<Vec<u8>, CoveError> {
        let mut ranges = self.read_ranges(&[range], kind).await?;
        ranges.pop().ok_or(CoveError::BufferTooShort)
    }

    async fn read_range_buffer(
        &self,
        range: Range<u64>,
        kind: RangeReadKind,
    ) -> Result<RetainedBytes, CoveError> {
        let mut ranges = self.read_range_buffers(&[range], kind).await?;
        ranges.pop().ok_or(CoveError::BufferTooShort)
    }
}

#[derive(Debug)]
pub struct LocalFileRangeReader {
    path: PathBuf,
    file: Mutex<Option<Arc<File>>>,
}

impl LocalFileRangeReader {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            file: Mutex::new(None),
        }
    }

    fn file(&self) -> Result<Arc<File>, CoveError> {
        let mut guard = self
            .file
            .lock()
            .map_err(|_| CoveError::BadSection("local range reader file lock poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(Arc::new(File::open(&self.path)?));
        }
        guard.as_ref().cloned().ok_or(CoveError::BufferTooShort)
    }
}

#[async_trait]
impl CoveRangeReader for LocalFileRangeReader {
    async fn read_ranges(
        &self,
        ranges: &[Range<u64>],
        kind: RangeReadKind,
    ) -> Result<Vec<Vec<u8>>, CoveError> {
        Ok(self
            .read_range_buffers(ranges, kind)
            .await?
            .into_iter()
            .map(|bytes| bytes.to_vec())
            .collect())
    }

    async fn read_range_buffers(
        &self,
        ranges: &[Range<u64>],
        _kind: RangeReadKind,
    ) -> Result<Vec<RetainedBytes>, CoveError> {
        // INVARIANT: a LocalFileRangeReader is bound to one immutable COVE file
        // snapshot. Reusing the descriptor avoids repeated open/close overhead;
        // positioned reads avoid mutating shared file cursor state.
        let file = self.file()?;
        let mut out = Vec::with_capacity(ranges.len());
        for range in ranges {
            out.push(RetainedBytes::from_vec(read_file_range(&file, range)?));
        }
        Ok(out)
    }
}

#[derive(Debug)]
struct MmapByteSource {
    mmap: Mmap,
}

impl RetainedByteSource for MmapByteSource {
    fn as_slice(&self) -> &[u8] {
        self.mmap.as_ref()
    }
}

/// Read-only mmap-backed local range reader.
///
/// INVARIANT: callers must only select this reader for immutable local COVE
/// files. If a file may be concurrently replaced, truncated, or modified, use
/// `LocalFileRangeReader`.
#[derive(Debug)]
pub struct MmapFileRangeReader {
    path: PathBuf,
    owner: Mutex<Option<Arc<RetainedByteOwner>>>,
}

impl MmapFileRangeReader {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            owner: Mutex::new(None),
        }
    }

    fn owner(&self) -> Result<Arc<RetainedByteOwner>, CoveError> {
        let mut guard = self
            .owner
            .lock()
            .map_err(|_| CoveError::BadSection("local mmap reader lock poisoned".into()))?;
        if let Some(owner) = guard.as_ref() {
            return Ok(Arc::clone(owner));
        }

        let file = File::open(&self.path)?;
        let len = file.metadata()?.len();
        let owner = if len == 0 {
            Arc::new(RetainedByteOwner::from_vec(Vec::new()))
        } else {
            // SAFETY: mmap mode is only sound for immutable local COVE files.
            // The returned read-only map is retained by the owner for every
            // slice built from it, and positioned reads remain available for
            // files that may be concurrently modified or truncated.
            let mmap = unsafe { Mmap::map(&file)? };
            Arc::new(RetainedByteOwner::from_external(Arc::new(MmapByteSource {
                mmap,
            })))
        };
        *guard = Some(Arc::clone(&owner));
        Ok(owner)
    }
}

#[async_trait]
impl CoveRangeReader for MmapFileRangeReader {
    async fn read_ranges(
        &self,
        ranges: &[Range<u64>],
        kind: RangeReadKind,
    ) -> Result<Vec<Vec<u8>>, CoveError> {
        Ok(self
            .read_range_buffers(ranges, kind)
            .await?
            .into_iter()
            .map(|bytes| bytes.to_vec())
            .collect())
    }

    async fn read_range_buffers(
        &self,
        ranges: &[Range<u64>],
        _kind: RangeReadKind,
    ) -> Result<Vec<RetainedBytes>, CoveError> {
        let owner = self.owner()?;
        ranges
            .iter()
            .map(|range| {
                let start = usize::try_from(range.start).map_err(|_| CoveError::OffsetRange)?;
                let len = range_len(range)?;
                RetainedBytes::from_owner_slice(Arc::clone(&owner), start, len)
            })
            .collect()
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
        kind: RangeReadKind,
    ) -> Result<Vec<Vec<u8>>, CoveError> {
        Ok(self
            .read_range_buffers(ranges, kind)
            .await?
            .into_iter()
            .map(|bytes| bytes.to_vec())
            .collect())
    }

    async fn read_range_buffers(
        &self,
        ranges: &[Range<u64>],
        _kind: RangeReadKind,
    ) -> Result<Vec<RetainedBytes>, CoveError> {
        ranges
            .iter()
            .map(|range| {
                let start = usize::try_from(range.start).map_err(|_| CoveError::OffsetRange)?;
                let len = range_len(range)?;
                let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
                if end > self.bytes.len() {
                    return Err(CoveError::OffsetRange);
                }
                RetainedBytes::from_arc_slice(Arc::clone(&self.bytes), start, len)
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

#[derive(Debug, Clone)]
pub(crate) struct CoalescedRangePlan {
    stats: CoalescedRangeStats,
    coalesced: Vec<CoalescedRange>,
}

impl CoalescedRangePlan {
    pub(crate) fn stats(&self) -> CoalescedRangeStats {
        self.stats
    }
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
    Ok(
        read_coalesced_range_buffers_with_options(reader, ranges, kind, options)
            .await?
            .into_iter()
            .map(|bytes| bytes.to_vec())
            .collect(),
    )
}

pub async fn read_coalesced_range_buffers_with_options<R: CoveRangeReader + ?Sized>(
    reader: &R,
    ranges: &[Range<u64>],
    kind: RangeReadKind,
    options: RangeCoalescingOptions,
) -> Result<Vec<RetainedBytes>, CoveError> {
    let plan = build_coalesced_range_plan(ranges, options)?;
    read_coalesced_range_buffers_for_plan(reader, kind, &plan).await
}

pub(crate) async fn read_coalesced_range_buffers_for_plan<R: CoveRangeReader + ?Sized>(
    reader: &R,
    kind: RangeReadKind,
    plan: &CoalescedRangePlan,
) -> Result<Vec<RetainedBytes>, CoveError> {
    if plan.stats.original_ranges == 0 {
        return Ok(Vec::new());
    }
    reader.record_coalescing(plan.stats.original_ranges, plan.stats.coalesced_ranges);
    let reads = plan
        .coalesced
        .iter()
        .map(|range| range.start..range.end)
        .collect::<Vec<_>>();
    let coalesced_bytes = reader.read_range_buffers(&reads, kind).await?;
    if coalesced_bytes.len() != plan.coalesced.len() {
        return Err(CoveError::BufferTooShort);
    }

    let mut out = vec![None; plan.stats.original_ranges];
    for (group, bytes) in plan.coalesced.iter().zip(coalesced_bytes.iter()) {
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
            out[original.index] = Some(bytes.slice(offset, len)?);
        }
    }
    out.into_iter()
        .map(|bytes| bytes.ok_or(CoveError::BufferTooShort))
        .collect()
}

pub fn coalesced_range_count(
    ranges: &[Range<u64>],
    options: RangeCoalescingOptions,
) -> Result<usize, CoveError> {
    build_coalesced_range_plan(ranges, options).map(|plan| plan.stats.coalesced_ranges)
}

pub fn coalesced_range_stats(
    ranges: &[Range<u64>],
    options: RangeCoalescingOptions,
) -> Result<CoalescedRangeStats, CoveError> {
    build_coalesced_range_plan(ranges, options).map(|plan| plan.stats)
}

pub(crate) fn build_coalesced_range_plan(
    ranges: &[Range<u64>],
    options: RangeCoalescingOptions,
) -> Result<CoalescedRangePlan, CoveError> {
    let coalesced = coalesce_ranges(ranges, options.max_gap, options.max_span)?;
    let original_bytes = ranges.iter().try_fold(0usize, |total, range| {
        total
            .checked_add(range_len(range)?)
            .ok_or(CoveError::ArithOverflow)
    })?;
    let coalesced_bytes = coalesced.iter().try_fold(0usize, |total, range| {
        let len = usize::try_from(
            range
                .end
                .checked_sub(range.start)
                .ok_or(CoveError::OffsetRange)?,
        )
        .map_err(|_| CoveError::OffsetRange)?;
        total.checked_add(len).ok_or(CoveError::ArithOverflow)
    })?;
    Ok(CoalescedRangePlan {
        stats: CoalescedRangeStats {
            original_ranges: ranges.len(),
            coalesced_ranges: coalesced.len(),
            original_bytes,
            coalesced_bytes,
        },
        coalesced,
    })
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

fn read_file_range(file: &File, range: &Range<u64>) -> Result<Vec<u8>, CoveError> {
    let len = range_len(range)?;
    let mut bytes = Vec::<u8>::with_capacity(len);
    read_file_exact_at_uninit(file, range.start, &mut bytes.spare_capacity_mut()[..len])?;
    // INVARIANT: `read_file_exact_at_uninit` returns `Ok(())` only after every
    // byte in the spare range has been initialized by the positioned read loop.
    // SAFETY: `bytes` has capacity `len`, and all elements in `0..len` have
    // been initialized before publishing the vector length.
    unsafe {
        bytes.set_len(len);
    }
    Ok(bytes)
}

#[cfg(unix)]
fn read_file_exact_at_uninit(
    file: &File,
    mut offset: u64,
    mut bytes: &mut [MaybeUninit<u8>],
) -> Result<(), CoveError> {
    use std::os::unix::fs::FileExt;

    while !bytes.is_empty() {
        // INVARIANT: the OS positioned read initializes at most `bytes.len()`
        // bytes in the supplied memory and never reads from the destination.
        // SAFETY: `MaybeUninit<u8>` and `u8` have compatible layout, and the
        // returned byte count is used to advance the uninitialized tail before
        // the caller publishes the vector length.
        let initialized =
            unsafe { std::slice::from_raw_parts_mut(bytes.as_mut_ptr().cast::<u8>(), bytes.len()) };
        let read = file.read_at(initialized, offset)?;
        if read == 0 {
            return Err(CoveError::BufferTooShort);
        }
        offset = offset
            .checked_add(u64::try_from(read).map_err(|_| CoveError::ArithOverflow)?)
            .ok_or(CoveError::ArithOverflow)?;
        let tmp = bytes;
        bytes = &mut tmp[read..];
    }
    Ok(())
}

#[cfg(not(unix))]
fn read_file_exact_at_uninit(
    file: &File,
    offset: u64,
    bytes: &mut [MaybeUninit<u8>],
) -> Result<(), CoveError> {
    use std::io::{ErrorKind, Read, Seek, SeekFrom};

    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    // INVARIANT: `read_exact` returns `Ok(())` only after filling the whole
    // destination slice.
    // SAFETY: `MaybeUninit<u8>` and `u8` have compatible layout, and `read`
    // implementations must not read from the destination buffer.
    let initialized =
        unsafe { std::slice::from_raw_parts_mut(bytes.as_mut_ptr().cast::<u8>(), bytes.len()) };
    match file.read_exact(initialized) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => Err(CoveError::BufferTooShort),
        Err(err) => Err(err.into()),
    }
}

fn range_len(range: &Range<u64>) -> Result<usize, CoveError> {
    let len = range
        .end
        .checked_sub(range.start)
        .ok_or(CoveError::OffsetRange)?;
    usize::try_from(len).map_err(|_| CoveError::OffsetRange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process, time::SystemTime};

    #[test]
    fn coalesced_range_buffers_split_without_copying() {
        let reader = MemoryRangeReader::new((0u8..64).collect());
        let ranges = vec![10..14, 16..20];
        let out = futures::executor::block_on(read_coalesced_range_buffers_with_options(
            &reader,
            &ranges,
            RangeReadKind::Data,
            RangeCoalescingOptions {
                max_gap: 4,
                max_span: 16,
            },
        ))
        .unwrap();

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].as_slice(), &[10, 11, 12, 13]);
        assert_eq!(out[1].as_slice(), &[16, 17, 18, 19]);
        assert!(out[0].shares_owner(&out[1]));
        assert_eq!(out[0].owner_offset(), 10);
        assert_eq!(out[1].owner_offset(), 16);
    }

    #[test]
    fn coalesced_range_stats_report_requested_and_used_bytes() {
        let ranges = vec![10..14, 16..20, 40..44];
        let stats = coalesced_range_stats(
            &ranges,
            RangeCoalescingOptions {
                max_gap: 4,
                max_span: 16,
            },
        )
        .unwrap();

        assert_eq!(stats.original_ranges, 3);
        assert_eq!(stats.coalesced_ranges, 2);
        assert_eq!(stats.original_bytes, 12);
        assert_eq!(stats.coalesced_bytes, 14);
    }

    #[test]
    fn local_file_range_reader_reads_exact_positioned_ranges() {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("cove-range-reader-{}-{stamp}.bin", process::id()));
        fs::write(&path, b"abcdefghijklmnopqrstuvwxyz").unwrap();
        let reader = LocalFileRangeReader::new(&path);

        let first =
            futures::executor::block_on(reader.read_range_buffer(5..10, RangeReadKind::Data))
                .unwrap();
        let second =
            futures::executor::block_on(reader.read_range_buffer(0..3, RangeReadKind::Data))
                .unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(first.as_slice(), b"fghij");
        assert_eq!(second.as_slice(), b"abc");
    }

    #[test]
    fn mmap_file_range_reader_returns_shared_retained_slices() {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "cove-mmap-range-reader-{}-{stamp}.bin",
            process::id()
        ));
        fs::write(&path, b"abcdefghijklmnopqrstuvwxyz").unwrap();
        let reader = MmapFileRangeReader::new(&path);

        let ranges = vec![2..7, 10..15];
        let out =
            futures::executor::block_on(reader.read_range_buffers(&ranges, RangeReadKind::Data))
                .unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(out[0].as_slice(), b"cdefg");
        assert_eq!(out[1].as_slice(), b"klmno");
        assert!(out[0].shares_owner(&out[1]));
        assert_eq!(out[0].owner_offset(), 2);
        assert_eq!(out[1].owner_offset(), 10);
    }

    #[test]
    fn local_file_range_reader_handles_empty_and_short_reads() {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "cove-range-reader-empty-short-{}-{stamp}.bin",
            process::id()
        ));
        fs::write(&path, b"abc").unwrap();
        let reader = LocalFileRangeReader::new(&path);

        let empty =
            futures::executor::block_on(reader.read_range_buffer(1..1, RangeReadKind::Data))
                .unwrap();
        let short =
            futures::executor::block_on(reader.read_range_buffer(0..10, RangeReadKind::Data));
        fs::remove_file(&path).unwrap();

        assert!(empty.is_empty());
        assert!(matches!(short, Err(CoveError::BufferTooShort)));
    }
}
