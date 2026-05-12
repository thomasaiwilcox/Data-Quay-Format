use std::{path::PathBuf, process::Command};

fn accept_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/accept")
        .join(name)
}

#[test]
fn inspect_cli_smoke() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-inspect"))
        .arg(accept_fixture("min_empty.cove"))
        .output()
        .expect("run cove-inspect");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("File:"), "stdout: {stdout}");
    assert!(stdout.contains("Primary Profile"), "stdout: {stdout}");
}

#[test]
fn inspect_reports_semantic_mapping_profile_and_feature() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-inspect"))
        .arg(accept_fixture("cove_map_valid.cove"))
        .output()
        .expect("run cove-inspect");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Primary Profile : COVE-MAP (Semantic Mapping)"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("SEMANTIC_MAP"), "stdout: {stdout}");
}

#[test]
fn inspect_reports_page_payload_elision_feature() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-inspect"))
        .arg(accept_fixture(
            "cove_t_payload_elision_stats_only_all_null_valid.cove",
        ))
        .output()
        .expect("run cove-inspect");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PAGE_PAYLOAD_ELISION"), "stdout: {stdout}");
}

#[test]
fn inspect_reports_table_and_segment_summary() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-inspect"))
        .arg(accept_fixture("cove_t_scan_table.cove"))
        .output()
        .expect("run cove-inspect");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Tables          : 1"), "stdout: {stdout}");
    assert!(stdout.contains("Segments        : 1"), "stdout: {stdout}");
    assert!(stdout.contains("logical=Bool"), "stdout: {stdout}");
}

#[test]
fn inspect_reports_standalone_covemap_artifact() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-inspect"))
        .arg(accept_fixture("covemap_valid.covemap"))
        .output()
        .expect("run cove-inspect");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Artifact        : COVEMAP"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("Mapping Version : example/v1"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("MapSourceCatalog"), "stdout: {stdout}");
}
