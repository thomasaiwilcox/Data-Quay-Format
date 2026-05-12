//! # cove-datafusion -- DataFusion integration for COVE
//!
//! M0 provides the crate boundary, pinned dependency surface, and module tree.
//! Query planning and execution behavior land in later milestones.

pub mod adapter_v53;
pub mod bootstrap;
pub mod dataset_state;
pub mod decode;
pub mod execution_code;
pub mod expr_lowering;
pub mod metadata_aggregate;
pub mod options;
pub mod overlay;
pub mod planner;
pub mod prune;
pub mod range_reader;
pub mod register;
pub mod scan_program;
pub mod task_graph;

#[cfg(test)]
mod tests {
    #[test]
    fn scaffold_loads() {
        assert_eq!(crate::adapter_v53::VERSION, "v53");
    }
}
