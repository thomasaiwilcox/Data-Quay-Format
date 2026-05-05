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
