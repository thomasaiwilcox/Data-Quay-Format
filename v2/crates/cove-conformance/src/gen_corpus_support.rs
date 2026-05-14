use std::{fs, path::PathBuf};

use cove_core::{constants::DigestAlgorithm, digest::compute_digest, reader};
use serde_json::{json, Value};

pub(crate) fn check_mode() -> bool {
    std::env::args().any(|arg| arg == "--check")
}

pub(crate) fn fixture(
    path: &str,
    kind: &str,
    expect: &str,
    error_code: Option<&str>,
    sections: &[&str],
) -> Value {
    let mut sections = sections.to_vec();
    if error_code.is_some() && !sections.contains(&"§76") {
        sections.push("§76");
    }
    let mut value = json!({
        "path": path,
        "kind": kind,
        "expect": expect,
        "sections": sections,
    });
    if let Some(code) = error_code {
        value["error_code"] = json!(code);
    }
    value
}

pub(crate) fn with_morsel_count(mut value: Value, morsel_count: u32) -> Value {
    value["morsel_count"] = json!(morsel_count);
    value
}

pub(crate) fn with_collation_count(mut value: Value, collation_count: usize) -> Value {
    value["collation_count"] = json!(collation_count);
    value
}

pub(crate) fn with_expect_can_skip(mut value: Value, expected: bool) -> Value {
    value["expect_can_skip"] = json!(expected);
    value
}

pub(crate) fn write_fixture(
    root: &PathBuf,
    entries: &mut Vec<Value>,
    mut entry: Value,
    bytes: Vec<u8>,
) {
    let path = entry["path"].as_str().unwrap();
    let full_path = root.join(path);
    if check_mode() {
        let existing = fs::read(&full_path).unwrap_or_else(|err| {
            panic!("cannot read {} during --check: {err}", full_path.display())
        });
        assert_eq!(
            existing,
            bytes,
            "{} is not up to date; run cargo run -p cove-conformance --bin gen-corpus",
            full_path.display()
        );
    } else {
        fs::write(full_path, &bytes).unwrap();
    }
    enrich_manifest_entry(&mut entry, &bytes);
    entries.push(entry);
}

pub(crate) fn write_auxiliary_file(root: &PathBuf, path: &str, bytes: &[u8]) {
    let full_path = root.join(path);
    if check_mode() {
        let existing = fs::read(&full_path).unwrap_or_else(|err| {
            panic!("cannot read {} during --check: {err}", full_path.display())
        });
        assert_eq!(
            existing,
            bytes,
            "{} is not up to date; run cargo run -p cove-conformance --bin gen-corpus",
            full_path.display()
        );
    } else {
        fs::write(full_path, bytes).unwrap();
    }
}

pub(crate) fn json_fixture_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec_pretty(&value).unwrap()
}

fn enrich_manifest_entry(entry: &mut Value, bytes: &[u8]) {
    let expect = entry
        .get("expect")
        .and_then(Value::as_str)
        .unwrap_or("accept")
        .to_string();
    let kind = entry
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("cove")
        .to_string();
    let digest = compute_digest(DigestAlgorithm::Sha256, bytes)
        .map(|bytes| format!("sha256:{}", hex_lower(&bytes)))
        .unwrap_or_else(|_| "sha256:unavailable".to_string());
    let feature_bits = reader::validate_bytes(bytes)
        .map(|validated| {
            json!({
                "required": validated.header.required_features,
                "optional": validated.header.optional_features,
            })
        })
        .unwrap_or_else(|_| json!({"required": 0u64, "optional": 0u64}));
    entry["conformance_level"] = json!(if expect == "accept" {
        "reference-v2-accept"
    } else {
        "reference-v2-reject"
    });
    entry["feature_bits"] = feature_bits;
    entry["producer_version"] = json!(env!("CARGO_PKG_VERSION"));
    entry["vector_version"] = json!(2);
    entry["source_digest"] = json!(digest);
    let summary = compact_summary(entry, bytes);
    entry["expected_inspect"] = summary.clone();
    entry["expected_dump"] = summary;
    if kind == "suite_contract_case" {
        entry["conformance_level"] = json!("reference-v2-suite-contract");
    }
}

fn compact_summary(entry: &Value, bytes: &[u8]) -> Value {
    json!({
        "kind": entry.get("kind").and_then(Value::as_str).unwrap_or("cove"),
        "expect": entry.get("expect").and_then(Value::as_str).unwrap_or("accept"),
        "bytes": bytes.len(),
        "error_code": entry.get("error_code").and_then(Value::as_str),
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
