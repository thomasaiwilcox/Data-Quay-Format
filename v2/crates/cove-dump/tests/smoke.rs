use std::{path::PathBuf, process::Command};

fn accept_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/accept")
        .join(name)
}

#[test]
fn dump_cli_smoke() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-dump"))
        .arg(accept_fixture("min_empty.cove"))
        .arg("--metadata")
        .output()
        .expect("run cove-dump");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("(metadata is empty)") || stdout.contains("metadata_len="),
        "stdout: {stdout}"
    );
}

#[test]
fn dump_pages_mode_reports_scan_pages() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-dump"))
        .arg(accept_fixture("cove_t_scan_table.cove"))
        .arg("--pages")
        .arg("--max-bytes")
        .arg("8")
        .output()
        .expect("run cove-dump --pages");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("segment section_id="), "stdout: {stdout}");
    assert!(stdout.contains("payload_len="), "stdout: {stdout}");
}

#[test]
fn dump_stats_mode_reports_stat_sections() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-dump"))
        .arg(accept_fixture("cove_t_zone_stats_valid.cove"))
        .arg("--stats")
        .arg("--max-bytes")
        .arg("8")
        .output()
        .expect("run cove-dump --stats");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=ZoneStats"), "stdout: {stdout}");
}

#[test]
fn dump_indexes_mode_reports_empty_index_set() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-dump"))
        .arg(accept_fixture("min_empty.cove"))
        .arg("--indexes")
        .output()
        .expect("run cove-dump --indexes");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("(no indexes sections)"), "stdout: {stdout}");
}

#[test]
fn dump_nested_mode_reports_nested_columns() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-dump"))
        .arg(accept_fixture("cove_t_nested_list_valid.cove"))
        .arg("--nested")
        .output()
        .expect("run cove-dump --nested");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("logical=List"), "stdout: {stdout}");
}
