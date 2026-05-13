//! # cove-datafusion -- DataFusion integration for COVE
//!
//! Reference DataFusion SQL, FileFormat, and execution integration for COVE v2.

pub mod adapter_v53;
pub mod bootstrap;
pub mod coverage_plan;
pub mod dataset_state;
pub mod decode;
pub mod execution_code;
pub mod explain;
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
