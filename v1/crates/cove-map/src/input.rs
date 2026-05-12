use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceRow {
    pub(crate) source_id: String,
    pub(crate) row_index: usize,
    pub(crate) values: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceInputs {
    pub(crate) rows: Vec<SourceRow>,
    pub(crate) states: Vec<ObservedSourceState>,
}

#[derive(Debug, Clone)]
pub(crate) struct ObservedSourceState {
    pub(crate) source_id: String,
    pub(crate) source_kind: String,
    pub(crate) schema_fingerprint: String,
    pub(crate) snapshot_digest: String,
}

pub(crate) fn read_sources(paths: &[PathBuf]) -> Result<Vec<SourceRow>, String> {
    read_source_inputs(paths).map(|inputs| inputs.rows)
}

pub(crate) fn read_source_inputs(paths: &[PathBuf]) -> Result<SourceInputs, String> {
    let mut rows = Vec::new();
    let mut states = Vec::new();
    for path in paths {
        let source_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("source")
            .to_string();
        let bytes =
            fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        let source_kind = match path.extension().and_then(|ext| ext.to_str()) {
            Some("jsonl") => "jsonl",
            Some("csv") => "csv",
            _ => return Err(format!("{} must be .jsonl or .csv", path.display())),
        };
        let before_len = rows.len();
        match source_kind {
            "jsonl" => rows.extend(read_jsonl(path, &source_id)?),
            "csv" => rows.extend(read_csv(path, &source_id)?),
            _ => unreachable!(),
        }
        let source_rows = &rows[before_len..];
        states.push(ObservedSourceState {
            source_id,
            source_kind: source_kind.to_string(),
            schema_fingerprint: observed_schema_fingerprint(source_kind, source_rows),
            snapshot_digest: format!("sha256:{}", sha256_hex(&bytes)),
        });
    }
    Ok(SourceInputs { rows, states })
}

fn read_jsonl(path: &Path, source_id: &str) -> Result<Vec<SourceRow>, String> {
    let text =
        fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let value: Value = serde_json::from_str(line)
                .map_err(|err| format!("{}:{} invalid JSONL: {err}", path.display(), index + 1))?;
            let object = value.as_object().ok_or_else(|| {
                format!(
                    "{}:{} JSONL row must be an object",
                    path.display(),
                    index + 1
                )
            })?;
            Ok(SourceRow {
                source_id: source_id.to_string(),
                row_index: index,
                values: object_to_btree(object),
            })
        })
        .collect()
}

pub(crate) fn read_csv(path: &Path, source_id: &str) -> Result<Vec<SourceRow>, String> {
    let text =
        fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header = lines
        .next()
        .ok_or_else(|| format!("{} is empty", path.display()))?
        .split(',')
        .map(|field| field.trim().to_string())
        .collect::<Vec<_>>();
    lines
        .enumerate()
        .map(|(index, line)| {
            let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
            if fields.len() != header.len() {
                return Err(format!(
                    "{}:{} field count {} did not match header count {}",
                    path.display(),
                    index + 2,
                    fields.len(),
                    header.len()
                ));
            }
            let values = header
                .iter()
                .cloned()
                .zip(
                    fields
                        .into_iter()
                        .map(|field| Value::String(field.to_string())),
                )
                .collect::<BTreeMap<_, _>>();
            Ok(SourceRow {
                source_id: source_id.to_string(),
                row_index: index,
                values,
            })
        })
        .collect()
}

pub(crate) fn validate_source_inputs(
    file: &CovemapFile,
    states: &[ObservedSourceState],
) -> Result<(), String> {
    let context = mapping_context(file)?;
    let mut observed = BTreeMap::<String, &ObservedSourceState>::new();
    for state in states {
        if observed.insert(state.source_id.clone(), state).is_some() {
            return Err(format!(
                "source '{}' was supplied more than once",
                state.source_id
            ));
        }
        let expected = context.sources.get(&state.source_id).ok_or_else(|| {
            format!(
                "source '{}' is not declared by the mapping",
                state.source_id
            )
        })?;
        if expected.replay_claimed {
            let expected_schema = expected.schema_fingerprint.as_deref().ok_or_else(|| {
                format!(
                    "source '{}' claims replayability but has no schema_fingerprint",
                    state.source_id
                )
            })?;
            let expected_digest = expected.snapshot_digest.as_deref().ok_or_else(|| {
                format!(
                    "source '{}' claims replayability but has no snapshot_digest",
                    state.source_id
                )
            })?;
            if !is_reference_schema_fingerprint(expected_schema) {
                return Err(format!(
                    "source '{}' replay schema_fingerprint must use cove-map-schema-v1:<sha256>",
                    state.source_id
                ));
            }
            if !is_sha256_digest(expected_digest) {
                return Err(format!(
                    "source '{}' replay snapshot_digest must use sha256:<64 hex>",
                    state.source_id
                ));
            }
            if expected_schema != state.schema_fingerprint
                || expected_digest != state.snapshot_digest
            {
                return Err(format!(
                    "source '{}' does not match replay fingerprint",
                    state.source_id
                ));
            }
            continue;
        }
        if expected
            .schema_fingerprint
            .as_deref()
            .is_some_and(is_reference_schema_fingerprint)
            && expected.schema_fingerprint.as_deref() != Some(state.schema_fingerprint.as_str())
        {
            return Err(format!(
                "source '{}' schema_fingerprint mismatch",
                state.source_id
            ));
        }
        if expected
            .snapshot_digest
            .as_deref()
            .is_some_and(is_sha256_digest)
            && expected.snapshot_digest.as_deref() != Some(state.snapshot_digest.as_str())
        {
            return Err(format!(
                "source '{}' snapshot_digest mismatch",
                state.source_id
            ));
        }
    }
    let row_sources = context
        .row_rules
        .iter()
        .map(|rule| rule.source_id.as_str())
        .collect::<BTreeSet<_>>();
    for (source_id, source) in &context.sources {
        if (source.replay_claimed || row_sources.contains(source_id.as_str()))
            && !observed.contains_key(source_id)
        {
            return Err(format!(
                "source '{}' is required by the mapping but was not supplied",
                source_id
            ));
        }
    }
    Ok(())
}

fn observed_schema_fingerprint(source_kind: &str, rows: &[SourceRow]) -> String {
    let mut fields = BTreeMap::<String, BTreeSet<String>>::new();
    for row in rows {
        for (key, value) in &row.values {
            fields
                .entry(key.clone())
                .or_default()
                .insert(json_primitive_kind(value).to_string());
        }
    }
    let schema = fields
        .into_iter()
        .map(|(key, kinds)| format!("{key}:{}", kinds.into_iter().collect::<Vec<_>>().join(",")))
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "cove-map-schema-v1:{}",
        sha256_hex(format!("{source_kind}\n{schema}").as_bytes())
    )
}

fn json_primitive_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_i64() => "int",
        Value::Number(number) if number.is_u64() => "uint",
        Value::Number(_) => "float",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn is_reference_schema_fingerprint(value: &str) -> bool {
    value
        .strip_prefix("cove-map-schema-v1:")
        .is_some_and(is_lower_hex_sha256)
}

fn is_sha256_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(is_lower_hex_sha256)
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
