use arrow_schema::Schema;
use cove_arrow::arrow::{
    arrow_data_type_for_column_export_options, ArrowExportOptions, ArrowFidelitySeverity,
};
use cove_core::{table::TableEntry, CoveError};

use super::FileMetadata;

pub(super) fn validate_schema_compatible_files(files: &[FileMetadata]) -> Result<(), CoveError> {
    let first = &files[0].table;
    for (ordinal, file) in files.iter().enumerate().skip(1) {
        let candidate = &file.table;
        if candidate.columns.len() != first.columns.len() {
            return Err(CoveError::BadSchema(format!(
                "COVE manifest schema mismatch for file {ordinal}: expected {} columns, found {}",
                first.columns.len(),
                candidate.columns.len()
            )));
        }
        for (column_index, (expected, actual)) in first
            .columns
            .iter()
            .zip(candidate.columns.iter())
            .enumerate()
        {
            if expected.name != actual.name
                || expected.logical != actual.logical
                || expected.physical != actual.physical
                || expected.nullable != actual.nullable
            {
                return Err(CoveError::BadSchema(format!(
                    "COVE manifest schema mismatch for file {ordinal}, column {column_index}: expected {} {:?}/{:?} nullable={}, found {} {:?}/{:?} nullable={}",
                    expected.name,
                    expected.logical,
                    expected.physical,
                    expected.nullable,
                    actual.name,
                    actual.logical,
                    actual.physical,
                    actual.nullable
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn schema_for_table(
    table: &TableEntry,
    has_file_dictionary: bool,
    arrow_export_options: ArrowExportOptions,
) -> Result<Schema, CoveError> {
    let fields = table
        .columns
        .iter()
        .map(|column| {
            let result = arrow_data_type_for_column_export_options(
                column.logical,
                column.physical,
                has_file_dictionary,
                arrow_export_options,
            )?;
            if result
                .report
                .issues
                .iter()
                .any(|issue| issue.severity == ArrowFidelitySeverity::Unsupported)
            {
                return Err(CoveError::UnsupportedEncoding(format!(
                    "Arrow schema export for {:?} is unsupported",
                    column.logical
                )));
            }
            if result.report.has_lossy_or_unsupported() {
                return Err(CoveError::UnsupportedEncoding(format!(
                    "Arrow schema export for {:?} requires explicit fidelity reporting",
                    column.logical
                )));
            }
            Ok(arrow_schema::Field::new(
                column.name.clone(),
                result.value,
                column.nullable,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Schema::new(fields))
}
