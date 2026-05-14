//! Writer facade for stable COVE v2 writer APIs.

use std::{path::Path, path::PathBuf};

pub use cove_core::{
    array, constants, dictionary, encoding, page, page_payload, segment, table, validity, writer,
    CoveError,
};

pub fn write_table(writer: &writer::ScanProfileCoveWriter) -> Result<Vec<u8>, CoveError> {
    writer.write()
}

pub fn publish_table(
    writer: &writer::ScanProfileCoveWriter,
    path: impl AsRef<Path>,
) -> Result<PathBuf, CoveError> {
    writer.publish_durable(path.as_ref())
}

pub fn write_minimal(writer: &writer::MinimalCoveWriter) -> Result<Vec<u8>, CoveError> {
    writer.write()
}

pub fn publish_minimal(
    writer: &writer::MinimalCoveWriter,
    path: impl AsRef<Path>,
) -> Result<PathBuf, CoveError> {
    writer.publish_durable(path.as_ref())
}
