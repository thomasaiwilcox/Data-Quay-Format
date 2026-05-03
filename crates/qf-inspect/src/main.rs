use std::{fs, path::Path, process};

use qf_core::{
    constants::MAGIC_QF,
    constants::{
        SectionKind, FEATURE_AGGREGATE_SYNOPSES, FEATURE_ARCHIVE_PROFILE,
        FEATURE_ARROW_INTEROP_HINTS, FEATURE_BLOOM_FILTERS, FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD,
        FEATURE_COLUMN_DOMAINS, FEATURE_COMPOSITE_ZONES, FEATURE_DIGEST_MANIFEST,
        FEATURE_ENGINE_PROFILE, FEATURE_EXACT_SETS, FEATURE_EXTENSION_REGISTRY,
        FEATURE_FILE_DICTIONARY, FEATURE_HARBOR_PROFILE, FEATURE_INVERTED_INDEXES,
        FEATURE_LAKEHOUSE_HINTS, FEATURE_LOOKUP_INDEXES, FEATURE_NESTED_COLUMNS, FEATURE_NUMCODES,
        FEATURE_OBJECT_PROFILE, FEATURE_REDACTIONS, FEATURE_TABLE_PROFILE, FEATURE_TOPN_SUMMARIES,
        FEATURE_TRUST_CHAIN,
    },
    reader,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: qf-inspect <file.quay> [<file2.quay> ...]");
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

    if data.len() < 4 || data[data.len() - 4..] != MAGIC_QF {
        return Err(format!("{}: invalid trailing magic", path.display()));
    }

    let parsed = reader::validate_bytes(&data).map_err(|e| format!("validation: {e}"))?;
    let header = parsed.header;
    let postscript = parsed.postscript;
    let footer = parsed.footer;

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

    Ok(())
}

fn profile_name(code: u8) -> String {
    match code {
        0 => "Mixed/Unknown".into(),
        1 => "QF-O (Object Temporal)".into(),
        2 => "QF-T (Table Scan)".into(),
        3 => "QF-A (Archive Acceleration)".into(),
        4 => "QF-E (Engine Execution)".into(),
        5 => "QF-H (Harbor Execution)".into(),
        _ => format!("Unknown({code})"),
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
    ];
    all.iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| *name)
        .collect()
}
