use std::{fs, path::Path, process};

use qf_core::{
    constants::MAGIC_QF, footer::QfFooter, header::QfHeaderV1, postscript::QfPostscriptV1,
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

    let header = QfHeaderV1::parse(&data, false).map_err(|e| format!("header parse: {e}"))?;
    let postscript =
        QfPostscriptV1::parse_from_tail(&data).map_err(|e| format!("postscript parse: {e}"))?;

    let footer_start = postscript.footer.offset as usize;
    let footer_end = postscript
        .footer
        .end_offset()
        .map_err(|_| "footer offset overflow".to_string())? as usize;

    if footer_end > data.len() {
        return Err("footer outside file bounds".to_string());
    }

    let footer = QfFooter::parse(&data[footer_start..footer_end])
        .map_err(|e| format!("footer parse: {e}"))?;

    println!("File: {}", path.display());
    println!("  Size            : {}", data.len());
    println!(
        "  Version         : {}.{}",
        header.version_major, header.version_minor
    );
    println!("  Primary Profile : {}", header.primary_profile);
    println!("  Required Feat   : 0x{:016x}", header.required_features);
    println!("  Optional Feat   : 0x{:016x}", header.optional_features);
    println!(
        "  Footer          : offset={} len={} sections={}",
        postscript.footer.offset,
        postscript.footer.length,
        footer.sections.len()
    );

    for s in &footer.sections {
        println!(
            "    - id={} kind={} offset={} len={} rows={} items={} comp={}",
            s.section_id,
            s.section_kind,
            s.offset,
            s.length,
            s.row_count,
            s.item_count,
            s.compression
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
