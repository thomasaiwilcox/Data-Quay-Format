//! `qf-validate` — Quay Format (QF) v1.0 file validator.
//!
//! Validates the structural integrity of a QF file by checking:
//!
//! 1. Trailing magic bytes.
//! 2. Postscript (checksum, file_len, footer bounds).
//! 3. Footer CRC.
//! 4. Footer header (magic, version, section entry length).
//! 5. Every section directory entry (bounds, CRC, reserved fields).
//! 6. Header (checksum, magic, version, endianness, reserved fields).
//!
//! Usage:
//! ```text
//! qf-validate <file.quay> [<file2.quay> ...]
//! ```
//!
//! Exit codes:
//! - 0 — all files are valid.
//! - 1 — one or more validation errors were found.
//! - 2 — usage error (no files specified).

use std::{io::Read, path::Path, process};

use qf_core::{
    checksum, constants::MAGIC_QF, footer::QfFooter, header::QfHeaderV1,
    postscript::QfPostscriptV1, QfError,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: qf-validate <file.quay> [<file2.quay> ...]");
        process::exit(2);
    }

    let mut all_ok = true;
    for path in &args[1..] {
        let ok = validate_file(Path::new(path));
        if !ok {
            all_ok = false;
        }
    }

    process::exit(if all_ok { 0 } else { 1 });
}

/// Validate a single QF file.  Returns `true` if valid.
fn validate_file(path: &Path) -> bool {
    let display = path.display();
    print!("Validating {display} ... ");

    let data = match read_file(path) {
        Ok(d) => d,
        Err(e) => {
            println!("ERROR");
            eprintln!("  [I/O] {e}");
            return false;
        }
    };

    match validate_bytes(&data) {
        Ok(info) => {
            println!("OK");
            println!("  QF v{}.{}", info.version_major, info.version_minor);
            println!("  file_len        : {} bytes", data.len());
            println!("  primary_profile : {}", info.primary_profile);
            println!("  section_count   : {}", info.section_count);
            if !info.metadata_json_preview.is_empty() {
                println!(
                    "  metadata (first 120 chars): {}",
                    &info.metadata_json_preview
                );
            }
            true
        }
        Err(errors) => {
            println!("INVALID");
            for e in &errors {
                eprintln!("  [ERR] {e}");
            }
            false
        }
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    let mut f = std::fs::File::open(path)?;
    let mut data = Vec::new();
    f.read_to_end(&mut data)?;
    Ok(data)
}

/// Summary of a successfully validated QF file.
struct ValidatedInfo {
    version_major: u16,
    version_minor: u16,
    primary_profile: String,
    section_count: usize,
    metadata_json_preview: String,
}

/// Validate raw QF file bytes.
///
/// Returns `Ok(info)` on success or `Err(vec_of_errors)` on failure.
/// Collects all errors rather than stopping at the first.
fn validate_bytes(data: &[u8]) -> Result<ValidatedInfo, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();

    // ── Step 1: Trailing magic ───────────────────────────────────────────────
    if data.len() < 4 {
        errors.push("file too short to contain trailing magic".into());
        return Err(errors);
    }
    let trailing_magic: [u8; 4] = data[data.len() - 4..].try_into().unwrap();
    if trailing_magic != MAGIC_QF {
        errors.push(format!(
            "trailing magic is {:?}, expected {:?}",
            trailing_magic, MAGIC_QF
        ));
        // Cannot continue without valid tail.
        return Err(errors);
    }

    // ── Step 2: Postscript ───────────────────────────────────────────────────
    let postscript = match QfPostscriptV1::parse_from_tail(data) {
        Ok(ps) => ps,
        Err(e) => {
            errors.push(format!("postscript: {e}"));
            return Err(errors);
        }
    };

    // Validate file_len.
    if postscript.file_len != data.len() as u64 {
        errors.push(format!(
            "postscript.file_len {} does not match actual file size {}",
            postscript.file_len,
            data.len()
        ));
    }

    // Validate footer bounds.
    let footer_end = match postscript.footer.end_offset() {
        Ok(e) => e,
        Err(_) => {
            errors.push("postscript footer offset/length arithmetic overflow".into());
            return Err(errors);
        }
    };
    if footer_end > data.len() as u64 {
        errors.push(format!(
            "footer end offset {} exceeds file length {}",
            footer_end,
            data.len()
        ));
        return Err(errors);
    }

    // ── Step 3: Footer CRC ───────────────────────────────────────────────────
    let footer_start = postscript.footer.offset as usize;
    let footer_bytes = &data[footer_start..footer_end as usize];
    let computed_footer_crc = checksum::crc32c(footer_bytes);
    if computed_footer_crc != postscript.footer.crc32c {
        errors.push(format!(
            "footer CRC mismatch: stored 0x{:08x}, computed 0x{:08x}",
            postscript.footer.crc32c, computed_footer_crc
        ));
        return Err(errors);
    }

    // ── Step 4: Parse footer ─────────────────────────────────────────────────
    let footer = match QfFooter::parse(footer_bytes) {
        Ok(f) => f,
        Err(e) => {
            errors.push(format!("footer parse: {e}"));
            return Err(errors);
        }
    };

    // ── Step 5: Validate section directory ───────────────────────────────────
    let mut section_ids_seen = std::collections::HashSet::new();
    for entry in &footer.sections {
        // Unique section_id check.
        if !section_ids_seen.insert(entry.section_id) {
            errors.push(format!("duplicate section_id {}", entry.section_id));
        }

        // Bounds check.
        let section_end = match entry.end_offset() {
            Ok(e) => e,
            Err(_) => {
                errors.push(format!(
                    "section {}: offset/length arithmetic overflow",
                    entry.section_id
                ));
                continue;
            }
        };
        if section_end > data.len() as u64 {
            errors.push(format!(
                "section {}: end offset {} exceeds file length {}",
                entry.section_id,
                section_end,
                data.len()
            ));
            continue;
        }

        // CRC validation.
        let section_bytes = &data[entry.offset as usize..section_end as usize];
        let computed_crc = checksum::crc32c(section_bytes);
        if computed_crc != entry.crc32c {
            errors.push(format!(
                "section {}: CRC mismatch (stored 0x{:08x}, computed 0x{:08x})",
                entry.section_id, entry.crc32c, computed_crc
            ));
        }
    }

    // ── Step 6: Parse and validate header ────────────────────────────────────
    let header = match QfHeaderV1::parse(data, false) {
        Ok(h) => h,
        Err(QfError::ChecksumMismatch) => {
            errors.push("header checksum mismatch".into());
            return Err(errors);
        }
        Err(e) => {
            errors.push(format!("header: {e}"));
            return Err(errors);
        }
    };

    // ── Step 7: Cross-check feature bits ─────────────────────────────────────
    if header.required_features != postscript.required_features {
        errors.push(format!(
            "required_features mismatch: header 0x{:016x} vs postscript 0x{:016x}",
            header.required_features, postscript.required_features
        ));
    }
    if header.optional_features != postscript.optional_features {
        errors.push(format!(
            "optional_features mismatch: header 0x{:016x} vs postscript 0x{:016x}",
            header.optional_features, postscript.optional_features
        ));
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    // ── Build summary ─────────────────────────────────────────────────────────
    let primary_profile = profile_name(header.primary_profile);
    let metadata_json_preview = String::from_utf8_lossy(&footer.metadata_json)
        .chars()
        .take(120)
        .collect::<String>()
        .replace('\n', " ");

    Ok(ValidatedInfo {
        version_major: header.version_major,
        version_minor: header.version_minor,
        primary_profile,
        section_count: footer.sections.len(),
        metadata_json_preview,
    })
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
