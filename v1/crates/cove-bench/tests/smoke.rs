use std::process::Command;

#[test]
fn bench_cli_smoke() {
    let output = Command::new(env!("CARGO_BIN_EXE_cove-bench"))
        .output()
        .expect("run cove-bench");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("crc32c:"), "stdout: {stdout}");
    assert!(stdout.contains("canonical_int:"), "stdout: {stdout}");
    assert!(stdout.contains("arrow_inversion:"), "stdout: {stdout}");
}
