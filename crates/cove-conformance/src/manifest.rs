use std::path::Path;

use serde_json::Value;

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) expect: String,
    pub(crate) error_code: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) morsel_count: Option<u32>,
    pub(crate) raw: Value,
}

pub(crate) fn load_manifest(corpus: &Path) -> Result<Vec<Entry>, String> {
    let manifest = corpus.join("manifest.jsonl");
    let manifest_bytes = std::fs::read(&manifest)
        .map_err(|err| format!("cannot read manifest {}: {err}", manifest.display()))?;
    let text = String::from_utf8_lossy(&manifest_bytes);
    let mut entries = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(entry) = parse_entry(line) else {
            eprintln!("manifest line {}: malformed", lineno + 1);
            continue;
        };
        entries.push(entry);
    }
    Ok(entries)
}

fn parse_entry(line: &str) -> Option<Entry> {
    let value: Value = serde_json::from_str(line).ok()?;
    let path = value.get("path")?.as_str()?.to_string();
    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("cove")
        .to_string();
    let expect = value.get("expect")?.as_str()?.to_string();
    let error_code = value
        .get("error_code")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);
    let error = value
        .get("error")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);
    let morsel_count = value
        .get("morsel_count")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok());
    Some(Entry {
        path,
        kind,
        expect,
        error_code,
        error,
        morsel_count,
        raw: value,
    })
}
