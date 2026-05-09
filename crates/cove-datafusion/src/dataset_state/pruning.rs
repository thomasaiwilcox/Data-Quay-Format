use std::collections::BTreeMap;

use cove_core::{
    constants::CovePhysicalKind,
    domain::ColumnDomain,
    index::{
        aggregate::{AggregateEntry, SynopsisAccuracy, SynopsisKind},
        bloom::BloomFilterIndex,
        composite::{CompositeIndex, CompositeTransformKind},
        exact_set::ExactSetIndex,
        inverted::InvertedMorselIndex,
        lookup::LookupIndex,
        topn::TopNSummary,
    },
    zone_stats::ZoneStatsEntry,
    CoveError,
};

use super::{DatasetState, FileMetadata, PruningMetadata, TableEntry};

#[derive(Debug, Clone, Default)]
pub(super) struct PlanningCache {
    column_by_id: BTreeMap<u32, usize>,
    zone_stat_by_key: BTreeMap<(u32, u32, u32), (usize, usize)>,
    column_domain_by_id: BTreeMap<u32, usize>,
    exact_set_by_id: BTreeMap<u32, usize>,
    bloom_by_id: BTreeMap<u32, usize>,
    lookup_by_id: BTreeMap<u32, usize>,
    inverted_by_id: BTreeMap<u32, usize>,
}

impl PlanningCache {
    pub(super) fn build(table: &TableEntry, pruning: &PruningMetadata) -> Self {
        let mut cache = Self::default();
        for (index, column) in table.columns.iter().enumerate() {
            cache.column_by_id.entry(column.column_id).or_insert(index);
        }
        for (section_index, section) in pruning.zone_stats.iter().enumerate() {
            for (entry_index, entry) in section.entries.iter().enumerate() {
                if entry.table_id == table.table_id {
                    cache
                        .zone_stat_by_key
                        .entry((entry.segment_id, entry.morsel_id, entry.column_id))
                        .or_insert((section_index, entry_index));
                }
            }
        }
        for (index, domain) in pruning.column_domains.iter().enumerate() {
            if domain.header.table_or_object_id == table.table_id && domain.is_safe() {
                cache
                    .column_domain_by_id
                    .entry(domain.header.column_or_property_id)
                    .or_insert(index);
            }
        }
        for (index, exact_set) in pruning.exact_sets.iter().enumerate() {
            if exact_set.header.table_id == table.table_id {
                cache
                    .exact_set_by_id
                    .entry(exact_set.header.column_id)
                    .or_insert(index);
            }
        }
        for (index, bloom) in pruning.blooms.iter().enumerate() {
            if bloom.header.table_id == table.table_id {
                cache
                    .bloom_by_id
                    .entry(bloom.header.column_id)
                    .or_insert(index);
            }
        }
        for (index, lookup) in pruning.lookups.iter().enumerate() {
            if lookup.header.table_id == table.table_id {
                cache
                    .lookup_by_id
                    .entry(lookup.header.column_id)
                    .or_insert(index);
            }
        }
        for (index, inverted) in pruning.inverted.iter().enumerate() {
            if inverted.header.table_id == table.table_id {
                cache
                    .inverted_by_id
                    .entry(inverted.header.column_id)
                    .or_insert(index);
            }
        }
        cache
    }
}

impl DatasetState {
    pub fn zone_stats_for(
        &self,
        segment_id: u32,
        morsel_id: u32,
        column_id: u32,
    ) -> Option<&ZoneStatsEntry> {
        let (section_index, entry_index) = self
            .planning_cache
            .zone_stat_by_key
            .get(&(segment_id, morsel_id, column_id))
            .copied()?;
        self.pruning
            .zone_stats
            .get(section_index)?
            .entries
            .get(entry_index)
    }

    pub fn segment_zone_stats_for(
        &self,
        segment_id: u32,
        column_id: u32,
    ) -> Option<&ZoneStatsEntry> {
        self.zone_stats_for(segment_id, u32::MAX, column_id)
    }

    pub fn column_domain_for(&self, column_id: u32) -> Option<&ColumnDomain> {
        self.planning_cache
            .column_domain_by_id
            .get(&column_id)
            .and_then(|index| self.pruning.column_domains.get(*index))
    }

    pub fn exact_set_for(&self, column_id: u32) -> Option<&ExactSetIndex> {
        self.planning_cache
            .exact_set_by_id
            .get(&column_id)
            .and_then(|index| self.pruning.exact_sets.get(*index))
    }

    pub fn bloom_for(&self, column_id: u32) -> Option<&BloomFilterIndex> {
        self.planning_cache
            .bloom_by_id
            .get(&column_id)
            .and_then(|index| self.pruning.blooms.get(*index))
    }

    pub fn lookup_for(&self, column_id: u32) -> Option<&LookupIndex> {
        self.planning_cache
            .lookup_by_id
            .get(&column_id)
            .and_then(|index| self.pruning.lookups.get(*index))
    }

    pub fn inverted_for(&self, column_id: u32) -> Option<&InvertedMorselIndex> {
        self.planning_cache
            .inverted_by_id
            .get(&column_id)
            .and_then(|index| self.pruning.inverted.get(*index))
    }

    pub fn aggregate_entries_for(&self, column_id: u32) -> Vec<&AggregateEntry> {
        self.pruning
            .aggregates
            .iter()
            .flat_map(|synopsis| synopsis.entries.iter())
            .filter(|entry| entry.table_id == self.table.table_id && entry.column_id == column_id)
            .collect()
    }

    pub fn composite_indexes(&self) -> impl Iterator<Item = &CompositeIndex> {
        self.pruning.composites.iter().filter(|index| {
            index.header.table_id == self.table.table_id
                && index.header.transform_kind == CompositeTransformKind::Tuple
        })
    }

    pub fn topn_for(&self, column_id: u32) -> Vec<&TopNSummary> {
        self.pruning
            .topn
            .iter()
            .filter(|summary| {
                summary.table_id == self.table.table_id && summary.column_id == column_id
            })
            .collect()
    }

    pub fn exact_global_count(
        &self,
        column_index: Option<usize>,
    ) -> Result<Option<u64>, CoveError> {
        let mut total = 0u64;
        for file in self.files() {
            let visible = file.visibility().visible_count(file.table().row_count)?;
            if visible == 0 {
                continue;
            }
            match column_index {
                None => {
                    total = total.checked_add(visible).ok_or(CoveError::ArithOverflow)?;
                }
                Some(index) => {
                    let column = file.table().columns.get(index).ok_or_else(|| {
                        CoveError::BadSchema(format!(
                            "COUNT column index {index} is out of bounds for {} columns",
                            file.table().columns.len()
                        ))
                    })?;
                    if column.physical == CovePhysicalKind::FileCode && file_has_redaction(file) {
                        return Ok(None);
                    }
                    if !column.nullable {
                        total = total.checked_add(visible).ok_or(CoveError::ArithOverflow)?;
                        continue;
                    }
                    if !file.visibility().is_all() {
                        return Ok(None);
                    }
                    let Some(file_count) = exact_count_for_file_column(file, column.column_id)?
                    else {
                        return Ok(None);
                    };
                    total = total
                        .checked_add(file_count)
                        .ok_or(CoveError::ArithOverflow)?;
                }
            }
        }
        Ok(Some(total))
    }

    pub fn exact_visible_row_count(&self) -> Result<u64, CoveError> {
        self.exact_global_count(None)?
            .ok_or(CoveError::ArithOverflow)
    }
}

impl FileMetadata {
    pub fn has_redaction(&self) -> bool {
        file_has_redaction(self)
    }
}

fn file_has_redaction(file: &FileMetadata) -> bool {
    file.mounted()
        .reverse_lookup
        .as_ref()
        .map(|lookup| !lookup.redacted_filecodes.is_empty())
        .unwrap_or(false)
}

fn exact_count_for_file_column(
    file: &FileMetadata,
    column_id: u32,
) -> Result<Option<u64>, CoveError> {
    let entries = file
        .pruning()
        .aggregates
        .iter()
        .flat_map(|synopsis| synopsis.entries.iter())
        .filter(|entry| {
            entry.table_id == file.table().table_id
                && entry.column_id == column_id
                && entry.synopsis_kind == SynopsisKind::Count
                && entry.accuracy == SynopsisAccuracy::Exact
        })
        .collect::<Vec<_>>();

    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.segment_id == u32::MAX && entry.morsel_id == u32::MAX)
    {
        return entry_count(entry).map(Some);
    }

    let segment_entries = entries
        .iter()
        .copied()
        .filter(|entry| entry.segment_id != u32::MAX && entry.morsel_id == u32::MAX)
        .collect::<Vec<_>>();
    if let Some(count) = exact_count_from_entries(file.table().row_count, &segment_entries)? {
        return Ok(Some(count));
    }

    let morsel_entries = entries
        .iter()
        .copied()
        .filter(|entry| entry.segment_id != u32::MAX && entry.morsel_id != u32::MAX)
        .collect::<Vec<_>>();
    exact_count_from_entries(file.table().row_count, &morsel_entries)
}

fn exact_count_from_entries(
    expected_rows: u64,
    entries: &[&AggregateEntry],
) -> Result<Option<u64>, CoveError> {
    if entries.is_empty() {
        return Ok(None);
    }
    let mut rows = 0u64;
    let mut count = 0u64;
    for entry in entries {
        rows = rows
            .checked_add(u64::from(entry.row_count))
            .ok_or(CoveError::ArithOverflow)?;
        count = count
            .checked_add(entry_count(entry)?)
            .ok_or(CoveError::ArithOverflow)?;
    }
    if rows == expected_rows {
        Ok(Some(count))
    } else {
        Ok(None)
    }
}

fn entry_count(entry: &AggregateEntry) -> Result<u64, CoveError> {
    entry
        .row_count
        .checked_sub(entry.null_count)
        .map(u64::from)
        .ok_or(CoveError::BadIndex)
}
