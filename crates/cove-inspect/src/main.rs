use std::{fs, path::Path, process};

use cove_core::{
    artifact::covemap::CovemapFile,
    compression,
    constants::{
        PrimaryProfile, SectionKind, FEATURE_AGGREGATE_SYNOPSES, FEATURE_ARCHIVE_PROFILE,
        FEATURE_ARROW_INTEROP_HINTS, FEATURE_BLOOM_FILTERS, FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD,
        FEATURE_COLUMN_DOMAINS, FEATURE_COMPOSITE_ZONES, FEATURE_DIGEST_MANIFEST,
        FEATURE_ENGINE_PROFILE, FEATURE_EXACT_SETS, FEATURE_EXTENSION_REGISTRY,
        FEATURE_FILE_DICTIONARY, FEATURE_HARBOR_PROFILE, FEATURE_INVERTED_INDEXES,
        FEATURE_LAKEHOUSE_HINTS, FEATURE_LOOKUP_INDEXES, FEATURE_NESTED_COLUMNS, FEATURE_NUMCODES,
        FEATURE_OBJECT_PROFILE, FEATURE_PAGE_PAYLOAD_ELISION, FEATURE_REDACTIONS,
        FEATURE_SEMANTIC_MAP, FEATURE_TABLE_PROFILE, FEATURE_TOPN_SUMMARIES, FEATURE_TRUST_CHAIN,
        MAGIC_COVE, MAGIC_COVEMAP,
    },
    reader,
    segment::TableSegmentIndex,
    table::TableCatalog,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: cove-inspect <file.cove> [<file2.cove> ...]");
        process::exit(2);
    }

    let mut all_ok = true;
    for path in &args[1..] {
        if let Err(e) = inspect_file(Path::new(path)) {
            all_ok = false;
            eprintln!("ERROR: {}", e);
        }
    }

    process::exit(if all_ok { 0 } else { 1 });
}

fn inspect_file(path: &Path) -> Result<(), String> {
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
    let parsed = reader::validate_bytes(&data).map_err(|e| format!("validation: {e}"))?;
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

    for s in &footer.sections {
        let kind_name = SectionKind::from_u16(s.section_kind)
            .map(|k| format!("{k:?}"))
            .unwrap_or_else(|| format!("Unknown({})", s.section_kind));
        println!(
            "    - id={} kind={} offset={} len={} rows={} items={} comp={}",
            s.section_id,
            kind_name,
            s.offset,
            s.length,
            s.row_count,
            s.item_count,
            comp_name(s.compression),
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

fn section_kind_name(section_id: u32) -> String {
    u16::try_from(section_id)
        .ok()
        .and_then(SectionKind::from_u16)
        .map(|kind| format!("{kind:?}"))
        .unwrap_or_else(|| format!("Unknown({section_id})"))
}

fn profile_name(code: u8) -> String {
    match PrimaryProfile::from_u8(code) {
        Some(PrimaryProfile::Mixed) => "Mixed/Unknown".into(),
        Some(PrimaryProfile::ObjectTemporal) => "COVE-O (Object Temporal)".into(),
        Some(PrimaryProfile::TableScan) => "COVE-T (Table Scan)".into(),
        Some(PrimaryProfile::ArchiveAcceleration) => "COVE-A (Archive Acceleration)".into(),
        Some(PrimaryProfile::EngineExecution) => "COVE-E (Engine Execution)".into(),
        Some(PrimaryProfile::HarborExecution) => "COVE-H (Harbor Execution)".into(),
        Some(PrimaryProfile::SemanticMapping) => "COVE-MAP (Semantic Mapping)".into(),
        Some(other) => format!("{other:?}"),
        None => format!("Unknown({code})"),
    }
}

fn comp_name(codec: u8) -> String {
    match codec {
        0 => "None".into(),
        1 => "LZ4".into(),
        2 => "Zstd".into(),
        other => format!("Unknown({other})"),
    }
}

fn feature_names(bits: u64) -> Vec<&'static str> {
    let all: &[(u64, &str)] = &[
        (FEATURE_OBJECT_PROFILE, "OBJECT_PROFILE"),
        (FEATURE_TABLE_PROFILE, "TABLE_PROFILE"),
        (FEATURE_ARCHIVE_PROFILE, "ARCHIVE_PROFILE"),
        (FEATURE_ENGINE_PROFILE, "ENGINE_PROFILE"),
        (FEATURE_HARBOR_PROFILE, "HARBOR_PROFILE"),
        (FEATURE_FILE_DICTIONARY, "FILE_DICTIONARY"),
        (FEATURE_NUMCODES, "NUMCODES"),
        (FEATURE_COLUMN_DOMAINS, "COLUMN_DOMAINS"),
        (FEATURE_EXACT_SETS, "EXACT_SETS"),
        (FEATURE_BLOOM_FILTERS, "BLOOM_FILTERS"),
        (FEATURE_INVERTED_INDEXES, "INVERTED_INDEXES"),
        (FEATURE_LOOKUP_INDEXES, "LOOKUP_INDEXES"),
        (FEATURE_AGGREGATE_SYNOPSES, "AGGREGATE_SYNOPSES"),
        (FEATURE_COMPOSITE_ZONES, "COMPOSITE_ZONES"),
        (FEATURE_TOPN_SUMMARIES, "TOPN_SUMMARIES"),
        (FEATURE_TRUST_CHAIN, "TRUST_CHAIN"),
        (FEATURE_REDACTIONS, "REDACTIONS"),
        (FEATURE_NESTED_COLUMNS, "NESTED_COLUMNS"),
        (FEATURE_DIGEST_MANIFEST, "DIGEST_MANIFEST"),
        (FEATURE_ARROW_INTEROP_HINTS, "ARROW_INTEROP_HINTS"),
        (FEATURE_LAKEHOUSE_HINTS, "LAKEHOUSE_HINTS"),
        (FEATURE_EXTENSION_REGISTRY, "EXTENSION_REGISTRY"),
        (FEATURE_CODEC_LZ4, "CODEC_LZ4"),
        (FEATURE_CODEC_ZSTD, "CODEC_ZSTD"),
        (FEATURE_SEMANTIC_MAP, "SEMANTIC_MAP"),
        (FEATURE_PAGE_PAYLOAD_ELISION, "PAGE_PAYLOAD_ELISION"),
    ];
    all.iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| *name)
        .collect()
}
