use super::*;

#[cfg(test)]
pub(crate) fn build_cove_o(file: &CovemapFile, rows: &[SourceRow]) -> Result<Vec<u8>, String> {
    build_cove_o_with_source_states(file, rows, &[])
}

pub(crate) fn build_cove_o_with_source_states(
    file: &CovemapFile,
    rows: &[SourceRow],
    source_states: &[ObservedSourceState],
) -> Result<Vec<u8>, String> {
    let materialized = materialize_with_source_states(file, rows, source_states)?;
    let catalog = ObjectTypeCatalog {
        flags: 0,
        types: materialized.object_types.clone(),
    };
    let segments = build_temporal_segments(&materialized)?;
    let segment_index = temporal_segment_index(&segments)?;
    let trust_manifest = trust_manifest(&segments)?;

    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::ObjectTemporal as u8;
    writer.required_features = FEATURE_OBJECT_PROFILE | FEATURE_TRUST_CHAIN;
    writer.optional_features = FEATURE_SEMANTIC_MAP;
    for section in map_passthrough_sections(file) {
        writer.sections.push(section);
    }
    writer.sections.push(object_section(
        SectionKind::ObjectTypeCatalog,
        catalog.types.len() as u64,
        0,
        catalog.serialize().map_err(|err| err.to_string())?,
    ));
    writer.sections.push(object_section(
        SectionKind::TemporalSegmentIndex,
        segments.len() as u64,
        materialized.rows.len() as u64,
        segment_index.serialize().map_err(|err| err.to_string())?,
    ));
    for segment in &segments {
        writer.sections.push(object_section(
            SectionKind::TemporalSegmentData,
            1,
            segment.rows.len() as u64,
            segment.payload.clone(),
        ));
    }
    writer.sections.push(object_section(
        SectionKind::TrustManifest,
        trust_manifest.entries.len() as u64,
        0,
        trust_manifest.serialize().map_err(|err| err.to_string())?,
    ));
    writer.sections.push(map_section(
        SectionKind::MapAssertionLog,
        materialized.assertions.len() as u64,
        serde_json::to_vec_pretty(&materialized.assertion_log).map_err(|err| err.to_string())?,
    ));
    writer.sections.push(map_section(
        SectionKind::MapIdentityEquivalenceIndex,
        materialized
            .identity_equivalence_index
            .get("equivalences")
            .and_then(Value::as_array)
            .map(|values| values.len() as u64)
            .unwrap_or(0),
        serde_json::to_vec_pretty(&materialized.identity_equivalence_index)
            .map_err(|err| err.to_string())?,
    ));
    writer.sections.push(map_section(
        SectionKind::MapEvidenceIndex,
        materialized.evidence_entries.len() as u64,
        serde_json::to_vec_pretty(&materialized.evidence_index).map_err(|err| err.to_string())?,
    ));
    writer.sections.push(map_section(
        SectionKind::MapConversionReport,
        1,
        serde_json::to_vec_pretty(&materialized.conversion_report)
            .map_err(|err| err.to_string())?,
    ));
    let bytes = writer.write().map_err(|err| err.to_string())?;
    validate_bytes_with_options(
        &bytes,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        },
    )
    .map_err(|err| err.to_string())?;
    Ok(bytes)
}
