use std::path::Path;

use cove_core::{constants::DigestAlgorithm, digest::compute_digest, CoveError};
use serde_json::{json, Value};

use crate::manifest::Entry;

pub(crate) fn run_entries(
    corpus: &Path,
    entries: &[Entry],
    validate_fixture: fn(&Entry, &Path, &[u8]) -> Result<(), CoveError>,
) -> bool {
    let mut total = 0usize;
    let mut passed = 0usize;
    for entry in entries {
        total += 1;
        let path = corpus.join(&entry.path);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("FAIL {} (read error: {})", entry.path, err);
                continue;
            }
        };
        let metadata_result = validate_manifest_metadata(entry, &bytes);
        let result = metadata_result.and_then(|_| validate_fixture(entry, corpus, &bytes));
        let ok = expected_result_matches(entry, &result);
        if ok {
            passed += 1;
            println!("PASS {}", entry.path);
        } else {
            let actual = match &result {
                Ok(()) => "accept".to_string(),
                Err(error) => format!("reject ({error})"),
            };
            println!(
                "FAIL {} (kind {}, expected {}, got {})",
                entry.path, entry.kind, entry.expect, actual
            );
        }
    }
    println!("\n{passed}/{total} fixtures passed");
    passed == total
}

fn validate_manifest_metadata(entry: &Entry, bytes: &[u8]) -> Result<(), CoveError> {
    require_field(entry, "conformance_level")?;
    require_field(entry, "feature_bits")?;
    require_field(entry, "producer_version")?;
    require_field(entry, "vector_version")?;
    require_field(entry, "source_digest")?;
    require_field(entry, "expected_inspect")?;
    require_field(entry, "expected_dump")?;
    let expected_digest = entry
        .raw
        .get("source_digest")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSchema("manifest source_digest must be a string".into()))?;
    let actual_digest = compute_digest(DigestAlgorithm::Sha256, bytes)
        .map(|digest| format!("sha256:{}", hex_lower(&digest)))?;
    if expected_digest != actual_digest {
        return Err(CoveError::BadSchema(format!(
            "manifest source_digest mismatch for {}",
            entry.path
        )));
    }
    let expected_summary = compact_summary(entry, bytes);
    if entry.raw.get("expected_inspect") != Some(&expected_summary) {
        return Err(CoveError::BadSchema(format!(
            "manifest expected_inspect mismatch for {}",
            entry.path
        )));
    }
    if entry.raw.get("expected_dump") != Some(&expected_summary) {
        return Err(CoveError::BadSchema(format!(
            "manifest expected_dump mismatch for {}",
            entry.path
        )));
    }
    Ok(())
}

fn require_field(entry: &Entry, field: &str) -> Result<(), CoveError> {
    if entry.raw.get(field).is_none() {
        return Err(CoveError::BadSchema(format!(
            "manifest entry {} missing {field}",
            entry.path
        )));
    }
    Ok(())
}

fn compact_summary(entry: &Entry, bytes: &[u8]) -> Value {
    json!({
        "kind": entry.kind.as_str(),
        "expect": entry.expect.as_str(),
        "bytes": bytes.len(),
        "error_code": entry.error_code.as_deref(),
    })
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn expected_result_matches(entry: &Entry, result: &Result<(), CoveError>) -> bool {
    match (entry.expect.as_str(), result) {
        ("accept", Ok(_)) => true,
        ("reject", Err(error)) => {
            if let Some(expected_code) = &entry.error_code {
                error.spec_code() == Some(expected_code.as_str())
            } else if let Some(expected_error) = &entry.error {
                let debug = format!("{:?}", error);
                let display = error.to_string();
                debug.contains(expected_error) || display.contains(expected_error)
            } else {
                true
            }
        }
        _ => false,
    }
}
