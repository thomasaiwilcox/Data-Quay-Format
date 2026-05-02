use std::{fs, path::Path, process};

use qf_core::{checksum, footer::QfFooter, postscript::QfPostscriptV1};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: qf-dump <file.quay> [--metadata | --section <id>] [--max-bytes <n>]");
        process::exit(2);
    }

    let path = &args[1];
    let mut mode = DumpMode::Metadata;
    let mut max_bytes: usize = 256;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--metadata" => mode = DumpMode::Metadata,
            "--section" => {
                if i + 1 >= args.len() {
                    eprintln!("--section requires an id");
                    process::exit(2);
                }
                let id = match args[i + 1].parse::<u32>() {
                    Ok(v) => v,
                    Err(_) => {
                        eprintln!("invalid --section id: {}", args[i + 1]);
                        process::exit(2);
                    }
                };
                mode = DumpMode::Section(id);
                i += 1;
            }
            "--max-bytes" => {
                if i + 1 >= args.len() {
                    eprintln!("--max-bytes requires a numeric value");
                    process::exit(2);
                }
                max_bytes = match args[i + 1].parse::<usize>() {
                    Ok(v) => v,
                    Err(_) => {
                        eprintln!("invalid --max-bytes value: {}", args[i + 1]);
                        process::exit(2);
                    }
                };
                i += 1;
            }
            other => {
                eprintln!("unknown argument: {}", other);
                process::exit(2);
            }
        }
        i += 1;
    }

    if let Err(e) = dump_file(Path::new(path), mode, max_bytes) {
        eprintln!("ERROR: {e}");
        process::exit(1);
    }
}

enum DumpMode {
    Metadata,
    Section(u32),
}

fn dump_file(path: &Path, mode: DumpMode, max_bytes: usize) -> Result<(), String> {
    let data = fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let postscript =
        QfPostscriptV1::parse_from_tail(&data).map_err(|e| format!("postscript parse: {e}"))?;

    // Spec §12: file_len MUST equal actual file length.
    if postscript.file_len != data.len() as u64 {
        return Err(format!(
            "postscript.file_len {} does not match actual file size {}",
            postscript.file_len,
            data.len()
        ));
    }

    let footer_start = postscript.footer.offset as usize;
    let footer_end = postscript
        .footer
        .end_offset()
        .map_err(|_| "footer offset overflow".to_string())? as usize;
    if footer_end > data.len() {
        return Err("footer outside file bounds".to_string());
    }

    // Spec §12: footer CRC32C MUST validate before footer contents are trusted.
    let footer_bytes = &data[footer_start..footer_end];
    let computed_crc = checksum::crc32c(footer_bytes);
    if computed_crc != postscript.footer.crc32c {
        return Err(format!(
            "footer CRC mismatch: stored 0x{:08x}, computed 0x{:08x}",
            postscript.footer.crc32c, computed_crc
        ));
    }

    let footer = QfFooter::parse(footer_bytes)
        .map_err(|e| format!("footer parse: {e}"))?;

    match mode {
        DumpMode::Metadata => {
            if footer.metadata_json.is_empty() {
                println!("(metadata is empty)");
                return Ok(());
            }
            let n = footer.metadata_json.len().min(max_bytes);
            println!(
                "metadata_len={} showing={} bytes",
                footer.metadata_json.len(),
                n
            );
            print_hex(&footer.metadata_json[..n]);
        }
        DumpMode::Section(section_id) => {
            let entry = footer
                .sections
                .iter()
                .find(|s| s.section_id == section_id)
                .ok_or_else(|| format!("section id {} not found", section_id))?;
            let end = entry
                .end_offset()
                .map_err(|_| "section offset overflow".to_string())? as usize;
            if end > data.len() {
                return Err(format!("section {} outside file bounds", section_id));
            }
            let section = &data[entry.offset as usize..end];
            let n = section.len().min(max_bytes);
            println!(
                "section_id={} len={} showing={} bytes",
                section_id,
                section.len(),
                n
            );
            print_hex(&section[..n]);
        }
    }

    Ok(())
}

fn print_hex(bytes: &[u8]) {
    for (line_idx, chunk) in bytes.chunks(16).enumerate() {
        print!("{:08x}: ", line_idx * 16);
        for b in chunk {
            print!("{:02x} ", b);
        }
        println!();
    }
}
