use cove_core::constants::PrimaryProfile;

pub(crate) fn profile_name(code: u8) -> String {
    match PrimaryProfile::from_u8(code) {
        Some(PrimaryProfile::Mixed) => "Mixed/Unknown".into(),
        Some(PrimaryProfile::ObjectTemporal) => "COVE-O (Object Temporal)".into(),
        Some(PrimaryProfile::TableScan) => "COVE-T (Table Scan)".into(),
        Some(PrimaryProfile::ArchiveAcceleration) => "COVE-A (Archive Acceleration)".into(),
        Some(PrimaryProfile::EngineExecution) => "COVE-E (Engine Execution)".into(),
        Some(PrimaryProfile::HarborExecution) => "COVE-H (Harbor Execution)".into(),
        Some(PrimaryProfile::SemanticMapping) => "COVE-MAP (Semantic Mapping)".into(),
        Some(other) => format!("{other:?}"),
        None => format!("Unknown({code})"),
    }
}
