use std::path::Path;

use cove_core::CoveError;

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
        let result = validate_fixture(entry, corpus, &bytes);
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
