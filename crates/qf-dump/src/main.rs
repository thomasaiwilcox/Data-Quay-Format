use std::{fs, path::Path, process};

use qf_core::{
    constants::{SectionKind, StorageClass, FEATURE_FILE_DICTIONARY},
    dictionary::FileDictionary,
    reader::{self, ValidationOptions},
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: qf-dump <file.quay> [--metadata | --section <id> | --dictionary | --dictionary-entry <code>] [--max-bytes <n>]"
        );
        process::exit(2);
    }

    let path = &args[1];
    let mut mode = DumpMode::Metadata;
    let mut max_bytes: usize = 256;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--metadata" => mode = DumpMode::Metadata,
            "--section" | "--decode-section" => {
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
            "--dictionary" => mode = DumpMode::Dictionary,
            "--dictionary-entry" => {
                if i + 1 >= args.len() {
                    eprintln!("--dictionary-entry requires a filecode");
                    process::exit(2);
                }
                let code = match args[i + 1].parse::<u64>() {
                    Ok(v) => v,
                    Err(_) => {
                        eprintln!("invalid --dictionary-entry code: {}", args[i + 1]);
                        process::exit(2);
                    }
                };
                mode = DumpMode::DictionaryEntry(code);
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
    Dictionary,
    DictionaryEntry(u64),
}

fn dump_file(path: &Path, mode: DumpMode, max_bytes: usize) -> Result<(), String> {
    let data = fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;

    match mode {
        DumpMode::Metadata | DumpMode::Section(_) => {
            // Use simple structural validation for these modes
            let parsed = reader::validate_bytes(&data).map_err(|e| format!("validation: {e}"))?;
            let footer = parsed.footer;
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
                        .map_err(|_| "section offset overflow".to_string())?
                        as usize;
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
                _ => unreachable!(),
            }
        }
        DumpMode::Dictionary => {
            let opts = ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
            };
            let report = reader::validate_bytes_with_options(&data, opts)
                .map_err(|e| format!("validation: {e}"))?;
            let validated = &report.validated;

            if validated.header.required_features & FEATURE_FILE_DICTIONARY == 0 {
                println!("(no file dictionary in this file)");
                return Ok(());
            }

            let dict =
                parse_dictionary(&data, validated).map_err(|e| format!("dictionary parse: {e}"))?;

            println!("dictionary_entries={}", dict.len());
            // For --dictionary, max_bytes is reused as a max-entries limit.
            let max_entries = (dict.len() as usize).min(max_bytes.max(4));
            for filecode in 0..max_entries as u32 {
                let entry = dict.get_entry(filecode).map_err(|e| e.to_string())?;
                let storage = match StorageClass::from_u8(entry.storage_class) {
                    Some(StorageClass::Inline) => "inline",
                    Some(StorageClass::Payload) => "payload",
                    Some(StorageClass::Redacted) => "redacted",
                    None => "unknown",
                };
                let tag_str = format!("{}", entry.value_tag);
                println!("  filecode={filecode} tag={tag_str} storage={storage}");
            }
        }
        DumpMode::DictionaryEntry(code) => {
            let opts = ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
            };
            let report = reader::validate_bytes_with_options(&data, opts)
                .map_err(|e| format!("validation: {e}"))?;
            let validated = &report.validated;

            if validated.header.required_features & FEATURE_FILE_DICTIONARY == 0 {
                return Err("no file dictionary in this file".into());
            }

            let dict =
                parse_dictionary(&data, validated).map_err(|e| format!("dictionary parse: {e}"))?;

            let code32 =
                u32::try_from(code).map_err(|_| "filecode out of u32 range".to_string())?;
            let value = dict.decode_value(code32).map_err(|e| e.to_string())?;

            match value {
                qf_core::dictionary::DictionaryValue::RedactedPresent => {
                    println!("filecode={code} value=REDACTED");
                }
                qf_core::dictionary::DictionaryValue::RawBytes(bytes) => {
                    println!("filecode={code} value_len={}", bytes.len());
                    let n = bytes.len().min(max_bytes);
                    print_hex(&bytes[..n]);
                }
            }
        }
    }

    Ok(())
}

fn parse_dictionary(
    data: &[u8],
    validated: &qf_core::reader::ValidatedQfFile,
) -> Result<FileDictionary, qf_core::QfError> {
    use qf_core::compression;

    let index_entry = validated
        .footer
        .sections
        .iter()
        .find(|s| s.section_kind == SectionKind::FileDictionaryIndex as u16)
        .ok_or_else(|| {
            qf_core::QfError::BadSection("FILE_DICTIONARY_INDEX section missing".into())
        })?;
    let payload_entry = validated
        .footer
        .sections
        .iter()
        .find(|s| s.section_kind == SectionKind::FileDictionaryPayload as u16);

    let index_bytes = compression::section_payload(data, index_entry)?;
    let payload_bytes = match payload_entry {
        Some(pe) => compression::section_payload(data, pe)?,
        None => std::borrow::Cow::Borrowed(&[][..]),
    };

    FileDictionary::parse(&index_bytes, &payload_bytes)
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
