//! Conversion facade for stable COVE v2 import APIs.

pub use cove_arrow::convert;
pub use cove_convert_parquet::cli;
pub use cove_convert_parquet::source::{
    convert_bytes_to_cove, convert_file_to_cove, detect_source_format, read_arrow_batches,
    read_csv_batches, read_orc_batches, schema_fingerprint, source_digest, ConversionOptions,
    CsvReadOptions, SourceFormat,
};

pub use cove_arrow::convert::ParquetConversionResult as ConversionResult;
