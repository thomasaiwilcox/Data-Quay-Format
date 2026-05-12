use cove_core::{
    compression,
    constants::SectionKind,
    dictionary::FileDictionary,
    footer::CoveFooter,
    header::CoveHeaderV1,
    mount::{
        build_reverse_lookup, EngineMetadata, MountedColumn, MountedCoveFile, MountedTable,
        OutputRepresentation, SidecarValidationStatus,
    },
    segment::{TableSegmentIndex, TableSegmentIndexEntryV1},
    table::{TableCatalog, TableEntry},
    CoveError,
};

pub(super) fn mounted_from_metadata(
    header: CoveHeaderV1,
    footer: CoveFooter,
    table: TableEntry,
    dictionary: Option<FileDictionary>,
    engine_metadata: EngineMetadata,
) -> Result<MountedCoveFile, CoveError> {
    let representation = OutputRepresentation::DecodeToValue;
    let reverse_lookup = dictionary.as_ref().map(build_reverse_lookup).transpose()?;
    let mounted_table = MountedTable {
        table_id: table.table_id,
        namespace: table.namespace.clone(),
        name: table.name.clone(),
        row_count: table.row_count,
        columns: table
            .columns
            .iter()
            .map(|column| MountedColumn {
                column_id: column.column_id,
                name: column.name.clone(),
                logical: column.logical,
                physical: column.physical,
                nullable: column.nullable,
                representation,
            })
            .collect(),
    };
    Ok(MountedCoveFile {
        header,
        footer,
        table_catalog: Some(TableCatalog {
            flags: 0,
            tables: vec![table],
        }),
        tables: vec![mounted_table],
        dictionary,
        representation,
        reverse_lookup,
        execution_code_map: None,
        execution_descriptors: engine_metadata.execution_descriptors.clone(),
        execution_scopes: engine_metadata.execution_scopes.clone(),
        code_spaces: engine_metadata.code_spaces.clone(),
        engine_profile_registries: engine_metadata.engine_profile_registries.clone(),
        engine_mount_policies: engine_metadata.engine_mount_policies.clone(),
        engine_metadata,
        column_domains: Vec::new(),
        zone_stats: Vec::new(),
        scan_indexes: Vec::new(),
        ignored_optional_sections: Vec::new(),
        covx_status: SidecarValidationStatus::NotProvided,
        covm_status: SidecarValidationStatus::NotProvided,
    })
}

pub(super) fn parse_segment_index(
    bytes: &[u8],
    mounted: &MountedCoveFile,
) -> Result<TableSegmentIndex, CoveError> {
    let mut indexes = mounted
        .footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::TableSegmentIndex as u16);
    let Some(entry) = indexes.next() else {
        return Ok(TableSegmentIndex::default());
    };
    if indexes.next().is_some() {
        return Err(CoveError::SegmentCorrupt);
    }
    let payload = compression::section_payload(bytes, entry)?;
    TableSegmentIndex::parse(&payload)
}

pub(super) fn validate_table_segments(
    table: &TableEntry,
    segments: &[TableSegmentIndexEntryV1],
) -> Result<(), CoveError> {
    let rows = segments.iter().try_fold(0u64, |acc, segment| {
        if segment.column_count != table.columns.len() as u32 {
            return Err(CoveError::SegmentCorrupt);
        }
        acc.checked_add(segment.row_count as u64)
            .ok_or(CoveError::ArithOverflow)
    })?;
    if rows != table.row_count {
        return Err(CoveError::SegmentCorrupt);
    }
    Ok(())
}
