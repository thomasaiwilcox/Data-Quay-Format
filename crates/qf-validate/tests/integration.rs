use qf_core::writer::MinimalQfWriter;
use std::io::Write;

#[test]
fn validate_empty_file() {
    let bytes = MinimalQfWriter::write_empty_file();
    let path = std::env::temp_dir().join("qf_validate_test_empty.quay");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&bytes).unwrap();
    }

    // Run qf-validate on the file.
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_qf-validate"))
        .arg(&path)
        .status()
        .expect("qf-validate binary should be runnable");

    assert!(
        status.success(),
        "qf-validate should return exit code 0 for a valid file"
    );
    // Cleanup is best-effort; if removal fails the test OS will clean up temp files.
    let _ = std::fs::remove_file(&path);
}

#[test]
fn validate_corrupted_file() {
    let mut bytes = MinimalQfWriter::write_empty_file();
    // Corrupt the trailing magic.
    let len = bytes.len();
    bytes[len - 1] = 0xFF;

    let path = std::env::temp_dir().join("qf_validate_test_corrupt.quay");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&bytes).unwrap();
    }

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_qf-validate"))
        .arg(&path)
        .status()
        .expect("qf-validate binary should be runnable");

    assert!(
        !status.success(),
        "qf-validate should return non-zero for a corrupt file"
    );
    // Cleanup is best-effort; if removal fails the test OS will clean up temp files.
    let _ = std::fs::remove_file(&path);
}
