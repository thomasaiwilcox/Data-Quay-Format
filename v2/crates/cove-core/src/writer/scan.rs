use super::*;

impl ScanPageSpec {
    pub fn new(row_count: u32, payload: Vec<u8>) -> Self {
        Self {
            row_count,
            non_null_count: row_count,
            null_count: 0,
            encoding_root: 0,
            compression: CompressionCodec::None,
            flags: 0,
            stats_ref: 0,
            payload,
        }
    }

    pub fn with_compression(mut self, compression: CompressionCodec) -> Self {
        self.compression = compression;
        self
    }

    pub fn with_counts(mut self, non_null_count: u32, null_count: u32) -> Self {
        self.non_null_count = non_null_count;
        self.null_count = null_count;
        self
    }

    pub fn with_encoding_root(mut self, encoding_root: u32) -> Self {
        self.encoding_root = encoding_root;
        self
    }

    pub fn with_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }
}

impl ScanSegment {
    pub fn new(
        table_id: u32,
        segment_id: u32,
        row_start: u64,
        row_count: u32,
        column_count: u32,
    ) -> Self {
        Self {
            table_id,
            segment_id,
            row_start,
            row_count,
            morsel_row_count: 4096,
            column_count,
            stats_ref: 0,
            flags: 0,
            column_page_specs: Vec::new(),
        }
    }

    pub fn set_column_pages(&mut self, column_id: u32, pages: Vec<ScanPageSpec>) {
        if let Some(existing) = self
            .column_page_specs
            .iter_mut()
            .find(|spec| spec.column_id == column_id)
        {
            existing.pages = pages;
        } else {
            self.column_page_specs
                .push(ScanColumnPageSpec { column_id, pages });
        }
    }

    fn morsel_count(&self) -> Result<u32, CoveError> {
        if self.row_count == 0 {
            return Ok(0);
        }
        if self.morsel_row_count == 0 {
            return Err(CoveError::SegmentCorrupt);
        }
        let count = self
            .row_count
            .checked_add(self.morsel_row_count - 1)
            .ok_or(CoveError::ArithOverflow)?
            / self.morsel_row_count;
        Ok(count)
    }

    fn payload(&self, columns: &[ColumnEntry]) -> Result<Vec<u8>, CoveError> {
        let morsel_count = self.morsel_count()?;
        let morsel_dir_len = (morsel_count as usize)
            .checked_mul(ROW_MORSEL_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let column_dir_len = columns
            .len()
            .checked_mul(TABLE_COLUMN_DIRECTORY_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let per_column_page_index_len = (morsel_count as usize)
            .checked_mul(crate::page::COLUMN_PAGE_INDEX_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let total_page_index_len = columns
            .len()
            .checked_mul(per_column_page_index_len)
            .ok_or(CoveError::ArithOverflow)?;
        let column_directory_offset = TABLE_SEGMENT_HEADER_LEN
            .checked_add(morsel_dir_len)
            .ok_or(CoveError::ArithOverflow)? as u64;
        let page_index_offset = column_directory_offset
            .checked_add(column_dir_len as u64)
            .ok_or(CoveError::ArithOverflow)?;
        let data_offset = page_index_offset
            .checked_add(total_page_index_len as u64)
            .ok_or(CoveError::ArithOverflow)?;
        let header = TableSegmentHeaderV1 {
            table_id: self.table_id,
            segment_id: self.segment_id,
            row_start: self.row_start,
            row_count: self.row_count,
            morsel_count,
            morsel_row_count: self.morsel_row_count,
            column_count: columns.len() as u32,
            morsel_directory_offset: TABLE_SEGMENT_HEADER_LEN as u64,
            column_directory_offset,
            page_index_offset,
            data_offset,
            flags: self.flags,
            checksum: 0,
        };
        let mut morsels = Vec::with_capacity(morsel_count as usize);
        let mut first_row = 0u32;
        for morsel_id in 0..morsel_count {
            let remaining = self.row_count - first_row;
            let row_count = remaining.min(self.morsel_row_count);
            morsels.push(RowMorselEntryV1 {
                morsel_id,
                first_row_in_segment: first_row,
                row_count,
                flags: 0,
                stats_ref: 0,
                checksum: 0,
            });
            first_row = first_row
                .checked_add(row_count)
                .ok_or(CoveError::ArithOverflow)?;
        }
        let morsel_dir = RowMorselDirectory { entries: morsels };
        let known_columns = columns
            .iter()
            .map(|column| column.column_id)
            .collect::<std::collections::BTreeSet<_>>();
        for spec in &self.column_page_specs {
            if !known_columns.contains(&spec.column_id) {
                return Err(CoveError::BadSchema(format!(
                    "segment {} page spec references unknown column_id {}",
                    self.segment_id, spec.column_id
                )));
            }
        }
        let mut column_directory = Vec::with_capacity(columns.len());
        let mut page_index_bytes = Vec::with_capacity(total_page_index_len);
        let mut page_payload_bytes = Vec::new();
        let mut next_page_index_offset = page_index_offset;
        let mut next_data_offset = data_offset;
        for column in columns {
            let column_page_index_offset = next_page_index_offset;
            let column_data_offset = next_data_offset;
            let mut column_page_count = 0usize;
            let custom_pages = self.page_specs_for_column(column.column_id)?;
            if let Some(custom_pages) = custom_pages {
                if custom_pages.len() != morsel_dir.entries.len() {
                    return Err(CoveError::BadSection(format!(
                        "segment {} column {} has {} page specs, expected {}",
                        self.segment_id,
                        column.column_id,
                        custom_pages.len(),
                        morsel_dir.entries.len()
                    )));
                }
                for (morsel, spec) in morsel_dir.entries.iter().zip(custom_pages.iter()) {
                    if spec.row_count != morsel.row_count {
                        return Err(CoveError::PageCorrupt);
                    }
                    if spec.flags & PAGE_FLAG_CODEC_MASK != 0 {
                        return Err(CoveError::BadSection(
                            "ScanPageSpec flags must not set codec bits directly".into(),
                        ));
                    }
                    if spec
                        .non_null_count
                        .checked_add(spec.null_count)
                        .ok_or(CoveError::ArithOverflow)?
                        != spec.row_count
                    {
                        return Err(CoveError::PageCorrupt);
                    }
                    if spec.payload.is_empty() && spec.compression != CompressionCodec::None {
                        return Err(CoveError::BadSection(
                            "compressed page payload must be non-empty".into(),
                        ));
                    }
                    let stats_only_constant = spec.flags & PAGE_FLAG_STATS_ONLY_CONSTANT != 0;
                    if stats_only_constant {
                        if !spec.payload.is_empty() {
                            return Err(CoveError::BadSection(
                                "stats-only constant page specs must use an empty payload".into(),
                            ));
                        }
                        if spec.compression != CompressionCodec::None {
                            return Err(CoveError::BadSection(
                                "stats-only constant page specs must use compression=None".into(),
                            ));
                        }
                        if spec.encoding_root != u32::MAX {
                            return Err(CoveError::BadSection(
                                "stats-only constant page specs must use encoding_root=u32::MAX"
                                    .into(),
                            ));
                        }
                    } else if spec.payload.is_empty() {
                        return Err(CoveError::BadSection(
                            "empty page payload requires PAGE_FLAG_STATS_ONLY_CONSTANT".into(),
                        ));
                    }
                    let encoded_payload = if stats_only_constant {
                        Vec::new()
                    } else {
                        encode_scan_page_payload(column, spec)?
                    };
                    let wire_payload =
                        compression::encode_page_payload(&encoded_payload, spec.compression)?;
                    let page_length = wire_payload.len() as u64;
                    let page_offset = if stats_only_constant {
                        0
                    } else {
                        next_data_offset
                    };
                    let page_checksum = checksum::crc32c(&wire_payload);
                    let page = ColumnPageIndexEntryV1 {
                        column_id: column.column_id,
                        morsel_id: morsel.morsel_id,
                        row_count: spec.row_count,
                        non_null_count: spec.non_null_count,
                        null_count: spec.null_count,
                        encoding_root: spec.encoding_root,
                        page_offset,
                        page_length,
                        uncompressed_length: encoded_payload.len() as u64,
                        stats_ref: spec.stats_ref,
                        flags: spec.flags | spec.compression as u32,
                        checksum: page_checksum,
                    };
                    page_index_bytes.extend_from_slice(&page.serialize());
                    if page_length != 0 {
                        page_payload_bytes.extend_from_slice(&wire_payload);
                        next_data_offset = next_data_offset
                            .checked_add(page_length)
                            .ok_or(CoveError::ArithOverflow)?;
                    }
                    column_page_count += 1;
                }
            } else {
                if column_uses_nested_feature(column) {
                    return Err(CoveError::BadSection(format!(
                        "segment {} nested column {} requires explicit page specs",
                        self.segment_id, column.column_id
                    )));
                }
                for morsel in &morsel_dir.entries {
                    let payload = default_page_payload(column, morsel.row_count)?;
                    let page_length = payload.len() as u64;
                    let page_offset = next_data_offset;
                    let page_checksum = checksum::crc32c(&payload);
                    let page = ColumnPageIndexEntryV1 {
                        column_id: column.column_id,
                        morsel_id: morsel.morsel_id,
                        row_count: morsel.row_count,
                        non_null_count: morsel.row_count,
                        null_count: 0,
                        encoding_root: default_encoding_kind(column) as u32,
                        page_offset,
                        page_length,
                        uncompressed_length: page_length,
                        stats_ref: 0,
                        flags: 0,
                        checksum: page_checksum,
                    };
                    page_index_bytes.extend_from_slice(&page.serialize());
                    if page_length != 0 {
                        page_payload_bytes.extend_from_slice(&payload);
                        next_data_offset = next_data_offset
                            .checked_add(page_length)
                            .ok_or(CoveError::ArithOverflow)?;
                    }
                    column_page_count += 1;
                }
            }
            let page_index_length = (column_page_count
                .checked_mul(crate::page::COLUMN_PAGE_INDEX_ENTRY_LEN)
                .ok_or(CoveError::ArithOverflow)?) as u64;
            next_page_index_offset = next_page_index_offset
                .checked_add(page_index_length)
                .ok_or(CoveError::ArithOverflow)?;
            column_directory.push(TableColumnDirectoryEntryV1 {
                column_id: column.column_id,
                logical_type: column.logical,
                physical_kind: column.physical,
                flags: segment_column_flags(column),
                page_index_offset: column_page_index_offset,
                page_index_length,
                data_offset: column_data_offset,
                data_length: next_data_offset - column_data_offset,
                stats_ref: 0,
                domain_ref: 0,
                checksum: 0,
            });
        }
        let mut out = Vec::with_capacity(
            TABLE_SEGMENT_HEADER_LEN + morsel_dir_len + column_dir_len + total_page_index_len,
        );
        out.extend_from_slice(&header.serialize());
        out.extend_from_slice(&morsel_dir.serialize());
        for entry in &column_directory {
            out.extend_from_slice(&entry.serialize());
        }
        out.extend_from_slice(&page_index_bytes);
        out.extend_from_slice(&page_payload_bytes);
        Ok(out)
    }

    fn index_entry(&self, offset: u64, length: u64) -> Result<TableSegmentIndexEntryV1, CoveError> {
        Ok(TableSegmentIndexEntryV1 {
            table_id: self.table_id,
            segment_id: self.segment_id,
            row_start: self.row_start,
            row_count: self.row_count,
            morsel_count: self.morsel_count()?,
            morsel_row_count: self.morsel_row_count,
            column_count: self.column_count,
            offset,
            length,
            stats_ref: self.stats_ref,
            flags: self.flags,
            checksum: 0,
        })
    }

    fn page_specs_for_column(&self, column_id: u32) -> Result<Option<&[ScanPageSpec]>, CoveError> {
        let mut matches = self
            .column_page_specs
            .iter()
            .filter(|spec| spec.column_id == column_id);
        let first = matches.next();
        if matches.next().is_some() {
            return Err(CoveError::BadSection(format!(
                "segment {} defines duplicate page specs for column {}",
                self.segment_id, column_id
            )));
        }
        Ok(first.map(|spec| spec.pages.as_slice()))
    }

    fn page_codec_features(&self) -> u64 {
        self.column_page_specs
            .iter()
            .flat_map(|spec| spec.pages.iter())
            .fold(0u64, |bits, page| {
                bits | codec_feature_bit(page.compression)
            })
    }

    fn page_required_features(&self) -> u64 {
        self.column_page_specs
            .iter()
            .flat_map(|spec| spec.pages.iter())
            .fold(0u64, |bits, page| {
                bits | if page_uses_payload_elision(page.flags) {
                    FEATURE_PAGE_PAYLOAD_ELISION
                } else {
                    0
                }
            })
    }
}

fn codec_feature_bit(codec: CompressionCodec) -> u64 {
    match codec {
        CompressionCodec::None => 0,
        CompressionCodec::Lz4 => FEATURE_CODEC_LZ4,
        CompressionCodec::Zstd => FEATURE_CODEC_ZSTD,
    }
}

fn column_uses_nested_feature(column: &ColumnEntry) -> bool {
    matches!(
        column.physical,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map
    )
}

fn encode_scan_page_payload(
    column: &ColumnEntry,
    spec: &ScanPageSpec,
) -> Result<Vec<u8>, CoveError> {
    let encoding_raw = u16::try_from(spec.encoding_root).map_err(|_| {
        CoveError::UnsupportedEncoding(format!(
            "encoding_root {} does not fit a v1 encoding kind",
            spec.encoding_root
        ))
    })?;
    let encoding_kind = CoveEncodingKind::from_u16(encoding_raw).ok_or_else(|| {
        CoveError::UnsupportedEncoding(format!("unknown page encoding kind {encoding_raw}"))
    })?;
    let (null_bitmap, values) = if spec.null_count == 0 {
        (None, spec.payload.clone())
    } else {
        let validity_len = (spec.row_count as usize)
            .checked_add(7)
            .ok_or(CoveError::ArithOverflow)?
            / 8;
        if spec.payload.len() < validity_len {
            return Err(CoveError::PageCorrupt);
        }
        let bitmap = &spec.payload[..validity_len];
        // INVARIANT: a writer-created non-elided mixed/null page must carry an
        // exact §27 null bitmap prefix; counts and tail bits are part of the
        // decode contract, not optional metadata.
        if spec.row_count % 8 != 0 && validity_len != 0 {
            let valid_bits = spec.row_count % 8;
            let mask = (1u8 << valid_bits) - 1;
            if bitmap[validity_len - 1] & !mask != 0 {
                return Err(CoveError::PageCorrupt);
            }
        }
        let counted = bitmap.iter().try_fold(0u32, |acc, byte| {
            acc.checked_add(byte.count_ones())
                .ok_or(CoveError::ArithOverflow)
        })?;
        if counted != spec.null_count {
            return Err(CoveError::PageCorrupt);
        }
        (Some(bitmap.to_vec()), spec.payload[validity_len..].to_vec())
    };
    ColumnPagePayloadV1::build_single_node(
        spec.row_count,
        encoding_kind,
        column.logical,
        column.physical,
        null_bitmap,
        values,
    )
}

fn default_encoding_kind(column: &ColumnEntry) -> CoveEncodingKind {
    match column.physical {
        CovePhysicalKind::FileCode => CoveEncodingKind::FileCode,
        CovePhysicalKind::NumCode => CoveEncodingKind::NumCode,
        CovePhysicalKind::Boolean | CovePhysicalKind::FixedBytes => CoveEncodingKind::PlainFixed,
        CovePhysicalKind::VarBytes => CoveEncodingKind::VarBytes,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            CoveEncodingKind::Canonical
        }
    }
}

fn segment_column_flags(column: &ColumnEntry) -> u8 {
    if column.flags & COLUMN_FLAG_BOOL_DECLARED_NUMERIC != 0 {
        SEGMENT_COLUMN_FLAG_BOOL_DECLARED_NUMERIC
    } else {
        0
    }
}

fn default_page_payload(column: &ColumnEntry, row_count: u32) -> Result<Vec<u8>, CoveError> {
    let values = default_physical_payload(column, row_count)?;
    ColumnPagePayloadV1::build_single_node(
        row_count,
        default_encoding_kind(column),
        column.logical,
        column.physical,
        None,
        values,
    )
}

fn default_physical_payload(column: &ColumnEntry, row_count: u32) -> Result<Vec<u8>, CoveError> {
    let rows = row_count as usize;
    match column.physical {
        CovePhysicalKind::Boolean => Ok(vec![0u8; rows]),
        CovePhysicalKind::FileCode => rows
            .checked_mul(4)
            .map(|len| vec![0u8; len])
            .ok_or(CoveError::ArithOverflow),
        CovePhysicalKind::NumCode => rows
            .checked_mul(8)
            .map(|len| vec![0u8; len])
            .ok_or(CoveError::ArithOverflow),
        CovePhysicalKind::FixedBytes => {
            let width = match column.logical {
                CoveLogicalType::Decimal64 => 8,
                CoveLogicalType::Decimal128 | CoveLogicalType::Uuid => 16,
                _ => 0,
            };
            rows.checked_mul(width)
                .map(|len| vec![0u8; len])
                .ok_or(CoveError::ArithOverflow)
        }
        CovePhysicalKind::VarBytes => {
            let len = rows.checked_mul(4).ok_or(CoveError::ArithOverflow)?;
            Ok(vec![0u8; len])
        }
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => Err(
            CoveError::BadSection("nested columns require explicit page payloads".into()),
        ),
    }
}

fn columns_feature_bits(columns: &[ColumnEntry]) -> u64 {
    columns.iter().fold(0u64, |bits, column| {
        bits | if column_uses_nested_feature(column) {
            FEATURE_NESTED_COLUMNS
        } else {
            0
        }
    })
}

fn nested_column_features_for_catalog(catalog: &TableCatalog) -> u64 {
    catalog.tables.iter().fold(0u64, |bits, table| {
        bits | columns_feature_bits(&table.columns)
    })
}

fn section_kind_feature_bits(section_kind: u16) -> u64 {
    match SectionKind::from_u16(section_kind) {
        Some(SectionKind::FileDictionaryIndex | SectionKind::FileDictionaryPayload) => {
            FEATURE_FILE_DICTIONARY
        }
        Some(SectionKind::ColumnDomain) => FEATURE_COLUMN_DOMAINS,
        Some(SectionKind::ExactSetIndex) => FEATURE_EXACT_SETS,
        Some(SectionKind::BloomIndex) => FEATURE_BLOOM_FILTERS,
        Some(SectionKind::InvertedMorselIndex) => FEATURE_INVERTED_INDEXES,
        Some(SectionKind::LookupIndex) => FEATURE_LOOKUP_INDEXES,
        Some(SectionKind::AggregateSynopsis) => FEATURE_AGGREGATE_SYNOPSES,
        Some(SectionKind::CompositeZoneIndex) => FEATURE_COMPOSITE_ZONES,
        Some(SectionKind::TopNZoneSummary) => FEATURE_TOPN_SUMMARIES,
        _ => 0,
    }
}

fn profile_feature_bit(profile: u8) -> u64 {
    match PrimaryProfile::from_u8(profile) {
        Some(PrimaryProfile::Mixed) | None => 0,
        Some(PrimaryProfile::ObjectTemporal) => FEATURE_OBJECT_PROFILE,
        Some(PrimaryProfile::TableScan) => FEATURE_TABLE_PROFILE,
        Some(PrimaryProfile::ArchiveAcceleration) => FEATURE_ARCHIVE_PROFILE,
        Some(PrimaryProfile::EngineExecution) => FEATURE_ENGINE_PROFILE,
        Some(PrimaryProfile::HarborExecution) => FEATURE_HARBOR_PROFILE,
        Some(PrimaryProfile::SemanticMapping) => FEATURE_SEMANTIC_MAP,
        Some(PrimaryProfile::CodecExtension) => FEATURE_CODEC_EXTENSION_REGISTRY,
        Some(PrimaryProfile::LayoutPlanning) => FEATURE_LAYOUT_PLAN,
        Some(PrimaryProfile::RuntimeCompatibility) => FEATURE_RUNTIME_COMPATIBILITY_HINTS,
        Some(PrimaryProfile::CoverageMetadata) => FEATURE_COVERAGE_METADATA,
        Some(PrimaryProfile::SecondaryIndex) => FEATURE_SECONDARY_INDEX_ARTIFACT,
    }
}

fn section_encoded_len(section: &SectionPayload) -> Result<usize, CoveError> {
    compression::encode_payload_for_codec(&section.data, section.compression)
        .map(|bytes| bytes.len())
}

impl ScanProfileCoveWriter {
    /// Serialize and durably publish the file to `path` using Spec §75.
    pub fn publish_durable(&self, path: &Path) -> Result<PathBuf, CoveError> {
        durable::durable_replace_with_writer(path, |file| self.write_to(file))
    }

    pub fn new(table_catalog: TableCatalog) -> Self {
        Self {
            created_at_us: 0,
            file_id: [0; 16],
            producer_scope_id: [0; 16],
            producer_scope_kind: 0,
            metadata_json: Vec::new(),
            table_catalog,
            extra_sections: Vec::new(),
            segments: Vec::new(),
        }
    }

    pub fn push_segment(&mut self, segment: ScanSegment) {
        self.segments.push(segment);
    }

    pub fn push_extra_section(&mut self, section: SectionPayload) {
        self.extra_sections.push(section);
    }

    pub fn push_file_dictionary(&mut self, dictionary: &FileDictionary) {
        let mut index = Vec::with_capacity(
            crate::dictionary::DICT_HEADER_SIZE
                + dictionary.entries.len() * crate::dictionary::DICT_INDEX_ENTRY_SIZE,
        );
        index.extend_from_slice(&dictionary.header.serialize());
        for entry in &dictionary.entries {
            index.extend_from_slice(&entry.serialize());
        }
        self.extra_sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: PrimaryProfile::Mixed as u8,
            flags: 0,
            item_count: dictionary.len() as u64,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: index,
        });
        if !dictionary.payload.is_empty() {
            self.extra_sections.push(SectionPayload {
                section_kind: SectionKind::FileDictionaryPayload as u16,
                profile: PrimaryProfile::Mixed as u8,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_FILE_DICTIONARY,
                optional_features: 0,
                data: dictionary.payload.clone(),
            });
        }
    }

    pub fn push_column_domain(&mut self, domain: &ColumnDomain) -> Result<(), CoveError> {
        self.extra_sections.push(SectionPayload {
            section_kind: SectionKind::ColumnDomain as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: FEATURE_COLUMN_DOMAINS,
            data: domain.serialize()?,
        });
        Ok(())
    }

    pub fn push_zone_stats(&mut self, zone_stats: &ZoneStatsSection) -> Result<(), CoveError> {
        self.extra_sections.push(SectionPayload {
            section_kind: SectionKind::ZoneStats as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: zone_stats.entries.len() as u64,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: zone_stats.serialize()?,
        });
        Ok(())
    }

    pub fn push_exact_set_index(&mut self, index: &ExactSetIndex) {
        self.push_serialized_scan_artifact(
            SectionKind::ExactSetIndex,
            PrimaryProfile::TableScan,
            FEATURE_EXACT_SETS,
            index.serialize(),
        );
    }

    pub fn push_bloom_index(&mut self, index: &BloomFilterIndex) {
        self.push_serialized_scan_artifact(
            SectionKind::BloomIndex,
            PrimaryProfile::TableScan,
            FEATURE_BLOOM_FILTERS,
            index.serialize(),
        );
    }

    pub fn push_inverted_morsel_index(&mut self, index: &InvertedMorselIndex) {
        self.push_serialized_scan_artifact(
            SectionKind::InvertedMorselIndex,
            PrimaryProfile::TableScan,
            FEATURE_INVERTED_INDEXES,
            index.serialize(),
        );
    }

    pub fn push_lookup_index(&mut self, index: &LookupIndex) -> Result<(), CoveError> {
        self.push_serialized_scan_artifact(
            SectionKind::LookupIndex,
            PrimaryProfile::ArchiveAcceleration,
            FEATURE_LOOKUP_INDEXES,
            index.serialize()?,
        );
        Ok(())
    }

    pub fn push_aggregate_synopsis(&mut self, synopsis: &AggregateSynopsis) {
        self.push_serialized_scan_artifact(
            SectionKind::AggregateSynopsis,
            PrimaryProfile::ArchiveAcceleration,
            FEATURE_AGGREGATE_SYNOPSES,
            synopsis.serialize(),
        );
    }

    pub fn push_composite_zone_index(&mut self, index: &CompositeIndex) {
        self.push_serialized_scan_artifact(
            SectionKind::CompositeZoneIndex,
            PrimaryProfile::ArchiveAcceleration,
            FEATURE_COMPOSITE_ZONES,
            index.serialize(),
        );
    }

    pub fn push_topn_summary(&mut self, summary: &TopNSummary) {
        self.push_serialized_scan_artifact(
            SectionKind::TopNZoneSummary,
            PrimaryProfile::ArchiveAcceleration,
            FEATURE_TOPN_SUMMARIES,
            summary.serialize(),
        );
    }

    fn push_serialized_scan_artifact(
        &mut self,
        kind: SectionKind,
        profile: PrimaryProfile,
        feature: u64,
        data: Vec<u8>,
    ) {
        self.extra_sections.push(SectionPayload {
            section_kind: kind as u16,
            profile: profile as u8,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: feature,
            data,
        });
    }

    pub fn write_to<W: Write + Seek>(&self, writer: &mut W) -> Result<(), CoveError> {
        let inner = self.prepare_inner_writer()?;
        inner.write_to(writer)
    }

    pub fn write(&self) -> Result<Vec<u8>, CoveError> {
        let mut cursor = Cursor::new(Vec::new());
        self.write_to(&mut cursor)?;
        Ok(cursor.into_inner())
    }

    fn prepare_inner_writer(&self) -> Result<MinimalCoveWriter, CoveError> {
        self.table_catalog.validate()?;
        self.validate_segments_against_catalog()?;

        let tables_by_id = self
            .table_catalog
            .tables
            .iter()
            .map(|table| (table.table_id, table))
            .collect::<std::collections::BTreeMap<_, _>>();

        let table_catalog_payload = self.table_catalog.serialize()?;
        let table_catalog_section = SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: self.table_catalog.tables.len() as u64,
            row_count: self.table_catalog.tables.iter().map(|t| t.row_count).sum(),
            compression: 0,
            alignment_log2: 0,
            required_features: nested_column_features_for_catalog(&self.table_catalog),
            optional_features: 0,
            data: table_catalog_payload,
        };
        let segment_index_len = 8usize
            .checked_add(
                self.segments
                    .len()
                    .checked_mul(TABLE_SEGMENT_INDEX_ENTRY_LEN)
                    .ok_or(CoveError::ArithOverflow)?,
            )
            .ok_or(CoveError::ArithOverflow)?;
        let segment_payloads = self
            .segments
            .iter()
            .map(|segment| {
                let table = tables_by_id.get(&segment.table_id).ok_or_else(|| {
                    CoveError::BadSchema(format!(
                        "segment references unknown table_id {}",
                        segment.table_id
                    ))
                })?;
                segment.payload(&table.columns)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let table_catalog_len = section_encoded_len(&table_catalog_section)?;
        let extra_sections_len = self
            .extra_sections
            .iter()
            .try_fold(0usize, |acc, section| {
                section_encoded_len(section)
                    .and_then(|len| acc.checked_add(len).ok_or(CoveError::ArithOverflow))
            })?;
        let pre_segment_len = HEADER_SIZE
            .checked_add(table_catalog_len)
            .and_then(|len| len.checked_add(extra_sections_len))
            .and_then(|len| len.checked_add(segment_index_len))
            .ok_or(CoveError::ArithOverflow)?;
        let mut offset = pre_segment_len as u64;
        let mut index_entries = Vec::with_capacity(self.segments.len());
        for (segment, payload) in self.segments.iter().zip(segment_payloads.iter()) {
            let length = payload.len() as u64;
            index_entries.push(segment.index_entry(offset, length)?);
            offset = offset.checked_add(length).ok_or(CoveError::ArithOverflow)?;
        }
        let segment_index = TableSegmentIndex {
            flags: 0,
            entries: index_entries,
        };
        segment_index.validate()?;
        let segment_index_payload = segment_index.serialize()?;
        let page_codec_features = self
            .segments
            .iter()
            .fold(0u64, |bits, segment| bits | segment.page_codec_features());
        let page_required_features = self.segments.iter().fold(0u64, |bits, segment| {
            bits | segment.page_required_features()
        });
        let nested_column_features = nested_column_features_for_catalog(&self.table_catalog);
        let table_nested_features = self
            .table_catalog
            .tables
            .iter()
            .map(|table| (table.table_id, columns_feature_bits(&table.columns)))
            .collect::<std::collections::BTreeMap<_, _>>();

        let mut inner = MinimalCoveWriter::new();
        inner.created_at_us = self.created_at_us;
        inner.file_id = self.file_id;
        inner.producer_scope_id = self.producer_scope_id;
        inner.producer_scope_kind = self.producer_scope_kind;
        inner.metadata_json = self.metadata_json.clone();
        let extra_required_features = self.extra_sections.iter().fold(0u64, |bits, section| {
            bits | section.required_features
                | profile_feature_bit(section.profile)
                | if matches!(
                    SectionKind::from_u16(section.section_kind),
                    Some(SectionKind::FileDictionaryIndex | SectionKind::FileDictionaryPayload)
                ) {
                    FEATURE_FILE_DICTIONARY
                } else {
                    0
                }
        });
        let extra_optional_features = self.extra_sections.iter().fold(0u64, |bits, section| {
            let kind_bits = if matches!(
                SectionKind::from_u16(section.section_kind),
                Some(SectionKind::FileDictionaryIndex | SectionKind::FileDictionaryPayload)
            ) {
                0
            } else {
                section_kind_feature_bits(section.section_kind)
            };
            bits | section.optional_features | kind_bits
        });
        inner.required_features = FEATURE_TABLE_PROFILE
            | nested_column_features
            | page_required_features
            | extra_required_features;
        inner.optional_features = page_codec_features | extra_optional_features;
        inner.sections.push(table_catalog_section);
        inner.sections.extend(self.extra_sections.iter().cloned());
        inner.sections.push(SectionPayload {
            section_kind: SectionKind::TableSegmentIndex as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: self.segments.len() as u64,
            row_count: self.segments.iter().map(|s| s.row_count as u64).sum(),
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: segment_index_payload,
        });
        for (segment, payload) in self.segments.iter().zip(segment_payloads) {
            inner.sections.push(SectionPayload {
                section_kind: SectionKind::TableSegmentData as u16,
                profile: PrimaryProfile::TableScan as u8,
                flags: 0,
                item_count: 1,
                row_count: segment.row_count as u64,
                compression: 0,
                alignment_log2: 0,
                required_features: table_nested_features
                    .get(&segment.table_id)
                    .copied()
                    .unwrap_or(0)
                    | segment.page_required_features(),
                optional_features: segment.page_codec_features(),
                data: payload,
            });
        }
        Ok(inner)
    }

    fn validate_segments_against_catalog(&self) -> Result<(), CoveError> {
        use std::collections::BTreeMap;

        let mut tables = BTreeMap::new();
        for table in &self.table_catalog.tables {
            tables.insert(
                table.table_id,
                (table.row_count, table.columns.len() as u32),
            );
        }
        let mut rows_by_table: BTreeMap<u32, u64> = BTreeMap::new();
        for segment in &self.segments {
            let Some((_declared_rows, column_count)) = tables.get(&segment.table_id) else {
                return Err(CoveError::BadSchema(format!(
                    "segment references unknown table_id {}",
                    segment.table_id
                )));
            };
            if segment.column_count != *column_count {
                return Err(CoveError::BadSchema(format!(
                    "segment {} column_count {} does not match table {} column count {}",
                    segment.segment_id, segment.column_count, segment.table_id, column_count
                )));
            }
            *rows_by_table.entry(segment.table_id).or_default() += segment.row_count as u64;
        }
        for (table_id, (declared_rows, _column_count)) in tables {
            let segment_rows = rows_by_table.get(&table_id).copied().unwrap_or(0);
            if segment_rows != declared_rows {
                return Err(CoveError::BadSchema(format!(
                    "table {} declares row_count {}, but segments cover {} rows",
                    table_id, declared_rows, segment_rows
                )));
            }
        }
        Ok(())
    }
}
