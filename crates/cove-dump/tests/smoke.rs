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
