use std::{fs, path::Path};

use cove_core::{
    artifact::covemap::CovemapFile,
    compression,
    constants::{SectionKind, MAGIC_COVE, MAGIC_COVEMAP},
    reader,
    segment::TableSegmentIndex,
    table::TableCatalog,
};

use crate::format::{comp_name, feature_names, profile_name, section_kind_name};

pub(crate) fn inspect_file(path: &Path) -> Result<(), String> {
    let data = fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;

    if data.len() < 4 {
        return Err(format!("{}: invalid trailing magic", path.display()));
    }

    if data[data.len() - 4..] == MAGIC_COVEMAP {
        return inspect_covemap_file(path, &data);
    }

    if data[data.len() - 4..] != MAGIC_COVE {
        return Err(format!("{}: invalid trailing magic", path.display()));
    }

    inspect_cove_file(path, &data)
}

fn inspect_cove_file(path: &Path, data: &[u8]) -> Result<(), String> {
    let parsed = reader::validate_bytes(data).map_err(|e| format!("validation: {e}"))?;
    let header = &parsed.header;
    let postscript = &parsed.postscript;
    let footer = &parsed.footer;

    println!("File: {}", path.display());
    println!("  Size            : {}", data.len());
    println!(
        "  Version         : {}.{}",
        header.version_major, header.version_minor
    );
    println!(
        "  Primary Profile : {}",
        profile_name(header.primary_profile)
    );

    let req_names = feature_names(header.required_features);
    println!("  Required Feat   : 0x{:016x}", header.required_features);
    if !req_names.is_empty() {
        println!("    flags: {}", req_names.join(", "));
    }

    let opt_names = feature_names(header.optional_features);
    println!("  Optional Feat   : 0x{:016x}", header.optional_features);
    if !opt_names.is_empty() {
        println!("    flags: {}", opt_names.join(", "));
    }

    println!(
        "  Footer          : offset={} len={} sections={}",
        postscript.footer.offset,
        postscript.footer.length,
        footer.sections.len()
    );

    for section in &footer.sections {
        let kind_name = SectionKind::from_u16(section.section_kind)
            .map(|kind| format!("{kind:?}"))
            .unwrap_or_else(|| format!("Unknown({})", section.section_kind));
        println!(
            "    - id={} kind={} offset={} len={} rows={} items={} comp={}",
            section.section_id,
            kind_name,
            section.offset,
            section.length,
            section.row_count,
            section.item_count,
            comp_name(section.compression),
        );
    }

    if !footer.metadata_json.is_empty() {
        let preview = String::from_utf8_lossy(&footer.metadata_json)
            .chars()
            .take(120)
            .collect::<String>()
            .replace('\n', " ");
        println!("  Metadata Preview: {}", preview);
    }

    print_table_summary(data, &parsed)?;
    Ok(())
}

fn print_table_summary(
    data: &[u8],
    parsed: &cove_core::reader::ValidatedCoveFile,
) -> Result<(), String> {
    let Some(catalog_entry) = parsed
        .footer
        .sections
        .iter()
        .find(|entry| entry.section_kind == SectionKind::TableCatalog as u16)
    else {
        return Ok(());
    };

    let catalog_payload = compression::section_payload(data, catalog_entry)
        .map_err(|e| format!("table catalog payload: {e}"))?;
    let catalog = TableCatalog::parse(catalog_payload.as_ref())
        .map_err(|e| format!("table catalog parse: {e}"))?;
    println!("  Tables          : {}", catalog.tables.len());
    for table in &catalog.tables {
        println!(
            "    - table={} {}.{} rows={} columns={}",
            table.table_id,
            table.namespace,
            table.name,
            table.row_count,
            table.columns.len()
        );
        for column in &table.columns {
            println!(
                "      column={} name={} logical={:?} physical={:?} nullable={}",
                column.column_id, column.name, column.logical, column.physical, column.nullable
            );
        }
    }

    if let Some(index_entry) = parsed
        .footer
        .sections
        .iter()
        .find(|entry| entry.section_kind == SectionKind::TableSegmentIndex as u16)
    {
        let index_payload = compression::section_payload(data, index_entry)
            .map_err(|e| format!("table segment index payload: {e}"))?;
        let index = TableSegmentIndex::parse(index_payload.as_ref())
            .map_err(|e| format!("table segment index parse: {e}"))?;
        println!("  Segments        : {}", index.entries.len());
        for segment in &index.entries {
            println!(
                "    - table={} segment={} row_start={} rows={} morsels={} columns={}",
                segment.table_id,
                segment.segment_id,
                segment.row_start,
                segment.row_count,
                segment.morsel_count,
                segment.column_count
            );
        }
    }
    Ok(())
}

fn inspect_covemap_file(path: &Path, data: &[u8]) -> Result<(), String> {
    let file = CovemapFile::parse_validated(data).map_err(|e| format!("validation: {e}"))?;

    println!("File: {}", path.display());
    println!("  Artifact        : COVEMAP");
    println!("  Size            : {}", data.len());
    println!(
        "  Version         : {}.{}",
        file.header.version_major, file.header.version_minor
    );
    println!("  Mapping Version : {}", file.mapping_version);
    println!("  Section Count   : {}", file.sections.len());

    let req_names = feature_names(file.header.required_features);
    println!(
        "  Required Feat   : 0x{:016x}",
        file.header.required_features
    );
    if !req_names.is_empty() {
        println!("    flags: {}", req_names.join(", "));
    }

    let opt_names = feature_names(file.header.optional_features);
    println!(
        "  Optional Feat   : 0x{:016x}",
        file.header.optional_features
    );
    if !opt_names.is_empty() {
        println!("    flags: {}", opt_names.join(", "));
    }

    println!(
        "  Header          : offset={} len={}",
        file.postscript.header_offset, file.postscript.header_length
    );

    for section in &file.sections {
        println!(
            "    - kind={} offset={} len={} raw_len={} comp={} required={}",
            section_kind_name(section.entry.section_id),
            section.entry.offset,
            section.entry.uncompressed_length,
            section.entry.length,
            comp_name(section.entry.compression),
            section.entry.required,
        );
    }

    Ok(())
}
