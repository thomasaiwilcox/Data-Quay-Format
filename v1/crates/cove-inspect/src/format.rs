use cove_core::constants::{
    PrimaryProfile, FEATURE_AGGREGATE_SYNOPSES, FEATURE_ARCHIVE_PROFILE,
    FEATURE_ARROW_INTEROP_HINTS, FEATURE_BLOOM_FILTERS, FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD,
    FEATURE_COLUMN_DOMAINS, FEATURE_COMPOSITE_ZONES, FEATURE_DIGEST_MANIFEST,
    FEATURE_ENGINE_PROFILE, FEATURE_EXACT_SETS, FEATURE_EXTENSION_REGISTRY,
    FEATURE_FILE_DICTIONARY, FEATURE_HARBOR_PROFILE, FEATURE_INVERTED_INDEXES,
    FEATURE_LAKEHOUSE_HINTS, FEATURE_LOOKUP_INDEXES, FEATURE_NESTED_COLUMNS, FEATURE_NUMCODES,
    FEATURE_OBJECT_PROFILE, FEATURE_PAGE_PAYLOAD_ELISION, FEATURE_REDACTIONS, FEATURE_SEMANTIC_MAP,
    FEATURE_TABLE_PROFILE, FEATURE_TOPN_SUMMARIES, FEATURE_TRUST_CHAIN,
};

pub(crate) fn section_kind_name(section_id: u32) -> String {
    u16::try_from(section_id)
        .ok()
        .and_then(cove_core::constants::SectionKind::from_u16)
        .map(|kind| format!("{kind:?}"))
        .unwrap_or_else(|| format!("Unknown({section_id})"))
}

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

pub(crate) fn comp_name(codec: u8) -> String {
    match codec {
        0 => "None".into(),
        1 => "LZ4".into(),
        2 => "Zstd".into(),
        other => format!("Unknown({other})"),
    }
}

pub(crate) fn feature_names(bits: u64) -> Vec<&'static str> {
    let all: &[(u64, &str)] = &[
        (FEATURE_OBJECT_PROFILE, "OBJECT_PROFILE"),
        (FEATURE_TABLE_PROFILE, "TABLE_PROFILE"),
        (FEATURE_ARCHIVE_PROFILE, "ARCHIVE_PROFILE"),
        (FEATURE_ENGINE_PROFILE, "ENGINE_PROFILE"),
        (FEATURE_HARBOR_PROFILE, "HARBOR_PROFILE"),
        (FEATURE_FILE_DICTIONARY, "FILE_DICTIONARY"),
        (FEATURE_NUMCODES, "NUMCODES"),
        (FEATURE_COLUMN_DOMAINS, "COLUMN_DOMAINS"),
        (FEATURE_EXACT_SETS, "EXACT_SETS"),
        (FEATURE_BLOOM_FILTERS, "BLOOM_FILTERS"),
        (FEATURE_INVERTED_INDEXES, "INVERTED_INDEXES"),
        (FEATURE_LOOKUP_INDEXES, "LOOKUP_INDEXES"),
        (FEATURE_AGGREGATE_SYNOPSES, "AGGREGATE_SYNOPSES"),
        (FEATURE_COMPOSITE_ZONES, "COMPOSITE_ZONES"),
        (FEATURE_TOPN_SUMMARIES, "TOPN_SUMMARIES"),
        (FEATURE_TRUST_CHAIN, "TRUST_CHAIN"),
        (FEATURE_REDACTIONS, "REDACTIONS"),
        (FEATURE_NESTED_COLUMNS, "NESTED_COLUMNS"),
        (FEATURE_DIGEST_MANIFEST, "DIGEST_MANIFEST"),
        (FEATURE_ARROW_INTEROP_HINTS, "ARROW_INTEROP_HINTS"),
        (FEATURE_LAKEHOUSE_HINTS, "LAKEHOUSE_HINTS"),
        (FEATURE_EXTENSION_REGISTRY, "EXTENSION_REGISTRY"),
        (FEATURE_CODEC_LZ4, "CODEC_LZ4"),
        (FEATURE_CODEC_ZSTD, "CODEC_ZSTD"),
        (FEATURE_SEMANTIC_MAP, "SEMANTIC_MAP"),
        (FEATURE_PAGE_PAYLOAD_ELISION, "PAGE_PAYLOAD_ELISION"),
    ];
    all.iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| *name)
        .collect()
}
