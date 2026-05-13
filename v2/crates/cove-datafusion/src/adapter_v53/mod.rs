//! DataFusion 53.x adapter layer.

use cove_core::CoveError;
use datafusion::common::DataFusionError;

#[cfg(feature = "dynamic-filters")]
pub mod dynamic_filter;
pub mod exec;
pub mod explain;
pub mod file_format;
pub mod file_opener;
pub mod file_source;
pub mod filter;
pub mod metadata;
pub mod metrics;
pub mod optimizer;
pub mod physical_filter;
pub mod statistics;
pub mod stream;
pub mod table_provider;
pub mod writer;

pub const VERSION: &str = "v53";

pub(crate) fn cove_to_datafusion(error: CoveError) -> DataFusionError {
    DataFusionError::External(Box::new(error))
}
