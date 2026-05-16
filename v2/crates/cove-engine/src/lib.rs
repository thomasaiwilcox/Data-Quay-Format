//! Engine facade for stable COVE v2 runtime and DataFusion APIs.

use std::{fs, path::Path};

pub use cove_core::mount;
pub use cove_core::CoveError;
pub use cove_datafusion::register::register_cove_file_format as register_datafusion;
pub use cove_datafusion::{
    coverage_plan, dataset_state, execution_code, options, planner, prune, register,
};
pub use cove_runtime as runtime;

pub fn validate_execution_profile(
    path: impl AsRef<Path>,
) -> Result<mount::EngineMetadata, CoveError> {
    inspect_execution_metadata(path)
}

pub fn inspect_execution_metadata(
    path: impl AsRef<Path>,
) -> Result<mount::EngineMetadata, CoveError> {
    let data = fs::read(path)?;
    Ok(mount::mount_cove_file(&data, mount::MountOptions::default(), None)?.engine_metadata)
}
