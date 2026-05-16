//! Footer and dataset bootstrap helpers for COVE-backed DataFusion datasets.

#[cfg(feature = "covi")]
mod covi;
#[cfg(feature = "covm")]
mod covm;
mod local;
mod overlay;
mod parse;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{dataset_state::DatasetState, options::CoveTableSelection};

#[cfg(feature = "covm")]
pub use covm::{
    bootstrap_covm_local_file_with_options, bootstrap_covm_local_file_with_options_async,
};
#[cfg(feature = "covi")]
pub use local::bootstrap_bytes_with_covi_artifacts;
pub use local::{
    bootstrap_bytes, bootstrap_bytes_with_options, bootstrap_local_file,
    bootstrap_local_file_async, bootstrap_local_file_with_options,
    bootstrap_local_file_with_options_async, bootstrap_range_reader_with_options,
};
pub use overlay::{
    bootstrap_overlay_snapshot_with_options, bootstrap_overlay_snapshot_with_options_async,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CoveMetadataCacheKey {
    pub source: Arc<str>,
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: u32,
    pub table_selection: Option<CoveTableSelection>,
}

#[derive(Debug, Default)]
pub struct CoveMetadataCache {
    entries: Mutex<HashMap<CoveMetadataCacheKey, Arc<DatasetState>>>,
}

impl CoveMetadataCache {
    fn entries(&self) -> MutexGuard<'_, HashMap<CoveMetadataCacheKey, Arc<DatasetState>>> {
        match self.entries.lock() {
            Ok(entries) => entries,
            // INVARIANT: cache poisoning must not silently disable metadata reuse.
            // The cache only stores immutable DatasetState values, so recovering
            // the guard is deterministic and keeps fallback behavior visible in
            // tests instead of degrading to repeated reparsing.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn get(&self, key: &CoveMetadataCacheKey) -> Option<Arc<DatasetState>> {
        self.entries().get(key).cloned()
    }

    pub fn insert(&self, key: CoveMetadataCacheKey, state: Arc<DatasetState>) {
        self.entries().insert(key, state);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        panic::{catch_unwind, AssertUnwindSafe},
        sync::Arc,
    };

    use super::{bootstrap_range_reader_with_options, CoveMetadataCache, CoveMetadataCacheKey};
    use crate::{options::CoveTableOptions, range_reader::MemoryRangeReader};
    use cove_core::{
        constants::{CoveLogicalType, CovePhysicalKind},
        table::{ColumnEntry, TableCatalog, TableEntry},
        writer::ScanProfileCoveWriter,
    };

    #[test]
    fn metadata_cache_reuses_bootstrapped_state() {
        let bytes = cache_test_bytes();
        let reader = MemoryRangeReader::new(bytes.clone());
        let cache = CoveMetadataCache::default();

        let first = futures::executor::block_on(bootstrap_range_reader_with_options(
            "memory://cache-hit",
            bytes.len() as u64,
            &reader,
            CoveTableOptions::default(),
            Some(&cache),
        ))
        .unwrap();
        let second = futures::executor::block_on(bootstrap_range_reader_with_options(
            "memory://cache-hit",
            bytes.len() as u64,
            &reader,
            CoveTableOptions::default(),
            Some(&cache),
        ))
        .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn metadata_cache_recovers_after_poison() {
        let bytes = cache_test_bytes();
        let reader = MemoryRangeReader::new(bytes.clone());
        let cache = CoveMetadataCache::default();
        let state = futures::executor::block_on(bootstrap_range_reader_with_options(
            "memory://cache-poison",
            bytes.len() as u64,
            &reader,
            CoveTableOptions::default(),
            None,
        ))
        .unwrap();
        let key = CoveMetadataCacheKey {
            source: Arc::from("memory://cache-poison"),
            file_id: *state.file_id(),
            file_len: state.file_len(),
            footer_crc32c: state.footer_crc32c(),
            table_selection: None,
        };

        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = cache.entries.lock().unwrap();
            panic!("poison cache lock for recovery test");
        }));
        assert!(cache.entries.is_poisoned());

        cache.insert(key.clone(), Arc::clone(&state));
        let cached = cache.get(&key).unwrap();
        assert!(Arc::ptr_eq(&cached, &state));
    }

    fn cache_test_bytes() -> Vec<u8> {
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "events".into(),
                row_count: 0,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![ColumnEntry {
                    column_id: 1,
                    name: "id".into(),
                    logical: CoveLogicalType::Int64,
                    physical: CovePhysicalKind::NumCode,
                    nullable: false,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        };
        ScanProfileCoveWriter::new(catalog).write().unwrap()
    }
}
