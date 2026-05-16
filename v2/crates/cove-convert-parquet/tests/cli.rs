use std::{fs, io::Cursor, process::Command, sync::Arc};

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use cove_core::reader::{validate_bytes_with_options, ValidationOptions};
use orc_rust::ArrowReaderBuilder;
use parquet::arrow::ArrowWriter;
use serde_json::Value;

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
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["source_identifier"], input.display().to_string());
    let source_digest = report["source_digest"].as_str().unwrap();
    assert!(source_digest.starts_with("sha256:"), "{source_digest}");
    assert_ne!(report["source_digest"], report["source_schema_fingerprint"]);

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

#[test]
fn cli_converts_csv_with_explicit_delimiter_and_header_policy() {
    let dir = std::env::temp_dir().join(format!("cove-convert-csv-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.csv");
    let output = dir.join("output.cove");
    fs::write(&input, "10|Ada\n20|Linus\n").unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_cove-convert-csv"))
        .args([
            "--no-csv-header",
            "--csv-delimiter",
            "|",
            "--csv-infer-rows",
            "all",
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

#[test]
fn conversion_report_source_to_cove_uses_file_identifier_and_digest_for_csv() {
    let dir = std::env::temp_dir().join(format!("cove-report-csv-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.csv");
    fs::write(&input, "id,name\n10,Ada\n20,Linus\n").unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_cove-conversion-report"))
        .args(["--source-format", "csv", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["source_identifier"], input.display().to_string());
    assert!(report["source_digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_ne!(report["source_digest"], report["source_schema_fingerprint"]);
}

#[test]
fn conversion_report_exports_cove_t_to_arrow_csv_parquet_and_orc() {
    let dir = std::env::temp_dir().join(format!("cove-reverse-export-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.parquet");
    let cove = dir.join("output.cove");
    fs::write(&input, parquet_bytes()).unwrap();

    let convert = Command::new(env!("CARGO_BIN_EXE_cove-convert-parquet"))
        .args([input.to_str().unwrap(), cove.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        convert.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&convert.stdout),
        String::from_utf8_lossy(&convert.stderr)
    );

    for (target, file_name) in [
        ("arrow", "reverse.arrow"),
        ("csv", "reverse.csv"),
        ("parquet", "reverse.parquet"),
        ("orc", "reverse.orc"),
    ] {
        let output = dir.join(file_name);
        let result = Command::new(env!("CARGO_BIN_EXE_cove-conversion-report"))
            .args([
                "--direction",
                "cove-to-source",
                "--target-format",
                target,
                "--output",
                output.to_str().unwrap(),
                cove.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "target={target}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["direction"], "cove-to-source");
        assert_eq!(report["target_format"], target);
        assert_eq!(report["supported"], true);
        assert_eq!(report["rows"], 2);
        assert_eq!(report["columns"], 2);
        assert!(output.metadata().unwrap().len() > 0);
        if target == "orc" {
            let reader = ArrowReaderBuilder::try_new(fs::File::open(&output).unwrap())
                .unwrap()
                .build();
            let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(
                batches.iter().map(|batch| batch.num_rows()).sum::<usize>(),
                2
            );
            assert_eq!(batches[0].num_columns(), 2);
        }
    }
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
