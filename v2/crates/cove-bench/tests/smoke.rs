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
    assert!(stdout.contains("cove-bench gen"), "stdout: {stdout}");
    assert!(stdout.contains("cove-bench run"), "stdout: {stdout}");
    assert!(stdout.contains("cove-bench check"), "stdout: {stdout}");
}
