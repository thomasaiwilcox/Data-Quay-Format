use std::{fs, path::PathBuf};

use cove_core::artifact::covemap::CovemapFile;
use serde_json::{json, Value};

use crate::{embedded_sections, CandidateMatch, PlannedIdentity};

pub(crate) fn explain(file: &CovemapFile, id: &str) -> Result<Value, String> {
    for section in embedded_sections(file)? {
        if let cove_core::profile::cove_map::EmbeddedMapSection::EvidenceIndex(index) = section {
            for entry in index.entries {
                if entry.assertion_id == id || entry.output_object_id == id {
                    return Ok(json!({
                        "source_id": entry.source_id,
                        "source_row_identity": entry.source_row_identity,
                        "rule_id": entry.rule_id,
                        "assertion_id": entry.assertion_id,
                        "output_object_id": entry.output_object_id,
                        "observed_schema_fingerprint": entry.observed_schema_fingerprint,
                        "observed_snapshot_digest": entry.observed_snapshot_digest,
                    }));
                }
            }
        }
    }
    Err(format!(
        "id {id} was not found in COVE-MAP evidence sections"
    ))
}

pub(crate) fn evidence_entry_for_identity(identity: &PlannedIdentity) -> Value {
    json!({
        "source_id": identity.source_id,
        "source_row_identity": identity.source_row_identity,
        "rule_id": identity.row_rule_id,
        "assertion_id": identity_assertion_id(identity),
        "output_object_id": crate::hex_encode(&identity.goid),
        "observed_schema_fingerprint": identity.schema_fingerprint,
    })
}

pub(crate) fn evidence_entry_for_candidate(candidate: &CandidateMatch) -> Value {
    json!({
        "source_id": candidate.source_id,
        "source_row_identity": candidate.source_row_identity,
        "rule_id": candidate.row_rule_id,
        "assertion_id": candidate_assertion_id(candidate),
        "output_object_id": candidate_match_id(candidate),
        "observed_schema_fingerprint": candidate.schema_fingerprint,
        "candidate": true,
        "identity_rule_id": candidate.identity_rule_id,
        "object_type": candidate.object_type,
        "join_key_sha256": candidate.join_key_sha256,
    })
}

pub(crate) fn identity_assertion_id(identity: &PlannedIdentity) -> String {
    format!(
        "assertion:{}:{}",
        identity.identity_rule_id, identity.row_digest
    )
}

pub(crate) fn candidate_assertion_id(candidate: &CandidateMatch) -> String {
    format!(
        "candidate:{}:{}",
        candidate.identity_rule_id, candidate.row_digest
    )
}

pub(crate) fn candidate_match_id(candidate: &CandidateMatch) -> String {
    format!(
        "candidate-match:{}:{}",
        candidate.identity_rule_id, candidate.join_key_sha256
    )
}

pub(crate) fn print_json(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

pub(crate) fn write_or_print(output: Option<PathBuf>, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|err| format!("cannot serialize JSON output: {err}"))?;
    if let Some(output) = output {
        fs::write(&output, text).map_err(|err| format!("cannot write {}: {err}", output.display()))
    } else {
        println!("{text}");
        Ok(())
    }
}

pub(crate) fn print_usage() {
    println!(
        "Usage: cove-map <subcommand> [options]\n\n\
Subcommands:\n  \
validate <mapping.covemap>\n  \
preview <mapping.covemap>\n  \
plan-keys <mapping.covemap> <source.csv|source.jsonl>...\n  \
convert [--format json|cove-o] [-o output] <mapping.covemap> <source.csv|source.jsonl>...\n  \
explain <mapping.covemap> <goid|assertion-id>\n  \
diff <left.covemap> <right.covemap>\n  \
project [-o output.json] <mapping.covemap> <source.csv|source.jsonl>...\n  \
test <fixture.json>"
    );
}
