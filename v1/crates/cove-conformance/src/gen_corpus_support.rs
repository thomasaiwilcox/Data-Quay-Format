use std::{fs, path::PathBuf};

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
    entry: Value,
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
        fs::write(full_path, bytes).unwrap();
    }
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
