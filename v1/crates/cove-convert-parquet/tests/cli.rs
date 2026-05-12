use std::{fs, io::Cursor, process::Command, sync::Arc};

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use cove_core::reader::{validate_bytes_with_options, ValidationOptions};
use parquet::arrow::ArrowWriter;

#[test]
fn cli_converts_parquet_to_valid_cove_and_prints_report() {
    let dir =
        std::env::temp_dir().join(format!("cove-convert-parquet-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.parquet");
    let output = dir.join("output.cove");
    fs::write(&input, parquet_bytes()).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_cove-convert-parquet"))
        .args([
            "--table-name",
            "cli_demo",
            "--namespace",
            "interop_test",
            "--report",
            "-",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(stdout.contains("\"table_name\": \"cli_demo\""), "{stdout}");

    let cove = fs::read(output).unwrap();
    validate_bytes_with_options(
        &cove,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        },
    )
    .unwrap();
}

fn parquet_bytes() -> Vec<u8> {
    let batch = RecordBatch::try_from_iter(vec![
        ("id", Arc::new(Int64Array::from(vec![10, 20])) as ArrayRef),
        (
            "name",
            Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef,
        ),
    ])
    .unwrap();
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = ArrowWriter::try_new(&mut cursor, batch.schema(), None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
    }
    cursor.into_inner()
}
