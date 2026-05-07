//! DataFusion 53.x explain-plan surfaces.

use crate::{
    dataset_state::DatasetState,
    execution_code::{support_for_mounted_file, ExecutionCodeSupport},
    planner::ScanPlan,
};

pub(crate) fn format_cove_exec(state: &DatasetState, plan: &ScanPlan) -> String {
    let execution_supported = state
        .files()
        .iter()
        .filter(|file| {
            matches!(
                support_for_mounted_file(file.mounted()),
                ExecutionCodeSupport::Supported { .. }
            )
        })
        .count();
    format!(
        "CoveExec: source={}, rows={}, segments={}, files={}, projection={:?}, pushed_filters={}, predicate_columns={}, scan_program=({}), topn_hint={:?}, execution_code_policy={:?}, execution_code_supported_files={}",
        state.source(),
        state.table().row_count,
        state.segments().len(),
        state.file_count(),
        plan.scan_projection,
        plan.filters.len(),
        plan.predicate_columns.len(),
        plan.scan_program.display_summary(),
        plan.topn_hint,
        state.execution_code_policy(),
        execution_supported
    )
}
