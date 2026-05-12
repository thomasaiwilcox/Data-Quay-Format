use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use arrow_array::ArrayRef;
use cove_arrow::arrow::{
    ArrowDictionaryPolicy, ArrowExportOptions, ArrowStringValidationPolicy,
    ArrowVarBytesExportPolicy,
};
use cove_core::constants::CoveLogicalType;

use super::morsels::SegmentMetadata;
use super::*;

#[derive(Debug, Default)]
pub(crate) struct ScanExecutionCache {
    local_readers: Mutex<HashMap<LocalReaderCacheKey, Arc<dyn CoveRangeReader>>>,
    segment_metadata: Mutex<HashMap<SegmentMetadataCacheKey, Arc<SegmentMetadata>>>,
    arrow_dictionary_values:
        Mutex<HashMap<ArrowDictionaryValuesCacheKey, Arc<Mutex<Option<ArrayRef>>>>>,
}

#[derive(Debug)]
pub(crate) struct Utf8ProofCache {
    entries: Mutex<HashMap<Utf8ProofKey, u64>>,
    next_epoch: Mutex<u64>,
    capacity: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LocalReaderCacheKey {
    file_ordinal: usize,
    policy: LocalFileReadPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SegmentMetadataCacheKey {
    file_ordinal: usize,
    table_id: u32,
    segment_id: u32,
    row_start: u64,
    offset: u64,
    length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Utf8ProofKey {
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: u32,
    pub column_id: u32,
    pub logical: u16,
    pub row_count: u32,
    pub non_null_count: u32,
    pub page_offset: u64,
    pub page_length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ArrowDictionaryValuesCacheKey {
    file_id: [u8; 16],
    file_len: u64,
    footer_crc32c: u32,
    logical: u16,
    dictionary_policy: u8,
    varbytes_policy: u8,
    string_validation_policy: u8,
    decimal: Option<(u8, i8)>,
    emit_uuid_extension_metadata: bool,
    emit_json_extension_metadata: bool,
}

impl ArrowDictionaryValuesCacheKey {
    pub(crate) fn new(
        identity: &crate::dataset_state::FileIdentity,
        logical: CoveLogicalType,
        options: ArrowExportOptions,
    ) -> Self {
        Self {
            file_id: identity.file_id,
            file_len: identity.file_len,
            footer_crc32c: identity.footer_crc32c,
            logical: logical as u16,
            dictionary_policy: match options.dictionary_policy {
                ArrowDictionaryPolicy::DecodeValues => 0,
                ArrowDictionaryPolicy::DictionaryKeys => 1,
                _ => u8::MAX,
            },
            varbytes_policy: match options.varbytes_policy {
                ArrowVarBytesExportPolicy::Standard => 0,
                ArrowVarBytesExportPolicy::View => 1,
                _ => u8::MAX,
            },
            string_validation_policy: match options.string_validation_policy {
                ArrowStringValidationPolicy::Strict => 0,
                ArrowStringValidationPolicy::StrictOrCachedProof => 1,
                ArrowStringValidationPolicy::TrustedPageProof => 2,
                _ => u8::MAX,
            },
            decimal: options
                .decimal
                .map(|decimal| (decimal.precision, decimal.scale)),
            emit_uuid_extension_metadata: options.emit_uuid_extension_metadata,
            emit_json_extension_metadata: options.emit_json_extension_metadata,
        }
    }
}

impl Utf8ProofKey {
    pub(crate) fn new(
        identity: &crate::dataset_state::FileIdentity,
        column: &ColumnEntry,
        page: &ColumnPageIndexEntryV1,
    ) -> Option<Self> {
        if !matches!(
            column.logical,
            CoveLogicalType::Utf8 | CoveLogicalType::Json
        ) || column.physical != CovePhysicalKind::VarBytes
            || page.page_length == 0
            || page.non_null_count == 0
        {
            return None;
        }
        Some(Self {
            file_id: identity.file_id,
            file_len: identity.file_len,
            footer_crc32c: identity.footer_crc32c,
            column_id: column.column_id,
            logical: column.logical as u16,
            row_count: page.row_count,
            non_null_count: page.non_null_count,
            page_offset: page.page_offset,
            page_length: page.page_length,
        })
    }
}

impl SegmentMetadataCacheKey {
    pub(super) fn new(file_ordinal: usize, segment_ref: &TableSegmentIndexEntryV1) -> Self {
        Self {
            file_ordinal,
            table_id: segment_ref.table_id,
            segment_id: segment_ref.segment_id,
            row_start: u64::from(segment_ref.row_start),
            offset: segment_ref.offset,
            length: segment_ref.length,
        }
    }
}

impl ScanExecutionCache {
    pub(super) fn local_reader(
        &self,
        file_ordinal: usize,
        policy: LocalFileReadPolicy,
        path: impl AsRef<Path>,
    ) -> Result<Arc<dyn CoveRangeReader>, CoveError> {
        let path = path.as_ref().to_path_buf();
        let mut readers = self.local_readers.lock().map_err(|_| {
            CoveError::BadSection("scan execution local-reader cache lock poisoned".into())
        })?;
        let key = LocalReaderCacheKey {
            file_ordinal,
            policy,
        };
        Ok(Arc::clone(readers.entry(key).or_insert_with(
            || match policy {
                LocalFileReadPolicy::PositionedReads => Arc::new(LocalFileRangeReader::new(&path)),
                LocalFileReadPolicy::Mmap => Arc::new(MmapFileRangeReader::new(&path)),
            },
        )))
    }

    pub(super) fn get_segment_metadata(
        &self,
        key: SegmentMetadataCacheKey,
    ) -> Result<Option<Arc<SegmentMetadata>>, CoveError> {
        let metadata = self.segment_metadata.lock().map_err(|_| {
            CoveError::BadSection("scan execution segment-metadata cache lock poisoned".into())
        })?;
        Ok(metadata.get(&key).cloned())
    }

    pub(super) fn insert_segment_metadata(
        &self,
        key: SegmentMetadataCacheKey,
        segment: Arc<SegmentMetadata>,
    ) -> Result<Arc<SegmentMetadata>, CoveError> {
        let mut metadata = self.segment_metadata.lock().map_err(|_| {
            CoveError::BadSection("scan execution segment-metadata cache lock poisoned".into())
        })?;
        Ok(Arc::clone(metadata.entry(key).or_insert(segment)))
    }

    pub(super) fn get_or_build_arrow_dictionary_values(
        &self,
        key: ArrowDictionaryValuesCacheKey,
        build: impl FnOnce() -> Result<ArrayRef, CoveError>,
    ) -> Result<(ArrayRef, bool), CoveError> {
        let cell = {
            let mut values = self.arrow_dictionary_values.lock().map_err(|_| {
                CoveError::BadSection("scan execution Arrow dictionary cache lock poisoned".into())
            })?;
            Arc::clone(
                values
                    .entry(key)
                    .or_insert_with(|| Arc::new(Mutex::new(None))),
            )
        };
        let mut slot = cell.lock().map_err(|_| {
            CoveError::BadSection(
                "scan execution Arrow dictionary cache value lock poisoned".into(),
            )
        })?;
        if let Some(value) = slot.as_ref() {
            return Ok((Arc::clone(value), true));
        }
        let value = build()?;
        *slot = Some(Arc::clone(&value));
        Ok((value, false))
    }
}

impl Default for Utf8ProofCache {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            next_epoch: Mutex::new(0),
            capacity: 4_096,
        }
    }
}

impl Utf8ProofCache {
    pub(crate) fn contains(&self, key: &Utf8ProofKey) -> Result<bool, CoveError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| CoveError::BadSection("utf8 proof cache lock poisoned".into()))?;
        let mut next_epoch = self
            .next_epoch
            .lock()
            .map_err(|_| CoveError::BadSection("utf8 proof cache clock lock poisoned".into()))?;
        if let Some(epoch) = entries.get_mut(key) {
            *next_epoch = next_epoch.checked_add(1).ok_or(CoveError::ArithOverflow)?;
            *epoch = *next_epoch;
            return Ok(true);
        }
        Ok(false)
    }

    pub(crate) fn insert(&self, key: Utf8ProofKey) -> Result<bool, CoveError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| CoveError::BadSection("utf8 proof cache lock poisoned".into()))?;
        let mut next_epoch = self
            .next_epoch
            .lock()
            .map_err(|_| CoveError::BadSection("utf8 proof cache clock lock poisoned".into()))?;
        *next_epoch = next_epoch.checked_add(1).ok_or(CoveError::ArithOverflow)?;
        let epoch = *next_epoch;
        let was_new = entries.insert(key, epoch).is_none();
        if entries.len() > self.capacity {
            if let Some((&oldest_key, _)) = entries.iter().min_by_key(|(_, epoch)| **epoch) {
                entries.remove(&oldest_key);
            }
        }
        Ok(was_new)
    }
}
