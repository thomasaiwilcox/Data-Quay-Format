use cove_core::{
    artifact::covemap::CovemapFile,
    constants::SectionKind,
    profile::cove_map::{parse_embedded_section, EmbeddedMapSection},
};

pub(crate) fn mapping_identity(file: &CovemapFile) -> Result<(String, String), String> {
    for section in embedded_sections(file)? {
        match section {
            EmbeddedMapSection::SourceCatalog(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::FunctionRegistry(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::IdentityRuleCatalog(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::RowSemanticsCatalog(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::AssertionLog(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::IdentityEquivalenceIndex(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::EvidenceIndex(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::ConversionReport(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::ProjectionCatalog(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            _ => {}
        }
    }
    Err("mapping contains no embedded sections".into())
}

pub(crate) fn embedded_sections(file: &CovemapFile) -> Result<Vec<EmbeddedMapSection>, String> {
    let mut out = Vec::new();
    for section in &file.sections {
        let kind = u16::try_from(section.entry.section_id)
            .ok()
            .and_then(SectionKind::from_u16)
            .ok_or_else(|| "invalid COVE-MAP section id".to_string())?;
        out.push(
            parse_embedded_section(kind, &section.payload)
                .map_err(|err| format!("invalid embedded map section: {err}"))?,
        );
    }
    Ok(out)
}

pub(crate) fn section_kind(section_id: u32) -> String {
    u16::try_from(section_id)
        .ok()
        .and_then(SectionKind::from_u16)
        .map(|kind| format!("{kind:?}"))
        .unwrap_or_else(|| format!("Unknown({section_id})"))
}
