use std::collections::BTreeSet;

use crate::{
    compression,
    constants::SectionKind,
    digest::{DigestManifest, DigestTargetKind},
    footer::CoveFooter,
    CoveError,
};

use super::reports::IgnoredOptionalSection;

pub(super) fn verify_digest_manifests(
    data: &[u8],
    footer: &CoveFooter,
    ignored_optional_sections: &[IgnoredOptionalSection],
) -> Result<u32, CoveError> {
    let mut checked = 0u32;
    let ignored_ids = ignored_optional_sections
        .iter()
        .map(|section| section.section_id)
        .collect::<BTreeSet<_>>();
    for digest_section in footer
        .sections
        .iter()
        .filter(|s| s.section_kind == SectionKind::DigestManifest as u16)
    {
        checked += 1;
        let digest_bytes = compression::section_payload(data, digest_section)?;
        let manifest = DigestManifest::parse(&digest_bytes)?;
        for entry in &manifest.entries {
            let section_range = if entry.target_kind == DigestTargetKind::Section {
                let target_section = footer
                    .sections
                    .binary_search_by_key(&entry.section_id, |s| s.section_id)
                    .ok()
                    .and_then(|idx| footer.sections.get(idx));
                let Some(target_section) = target_section else {
                    if ignored_ids.contains(&entry.section_id) {
                        continue;
                    }
                    return Err(CoveError::BadSection(format!(
                        "digest manifest references missing section_id {}",
                        entry.section_id
                    )));
                };

                let section_start =
                    usize::try_from(target_section.offset).map_err(|_| CoveError::OffsetRange)?;
                let section_end = usize::try_from(target_section.end_offset()?)
                    .map_err(|_| CoveError::OffsetRange)?;
                Some((section_start, section_end))
            } else {
                None
            };

            let range_start = usize::try_from(entry.offset).map_err(|_| CoveError::OffsetRange)?;
            let range_len = usize::try_from(entry.length).map_err(|_| CoveError::OffsetRange)?;
            let range_end = range_start
                .checked_add(range_len)
                .ok_or(CoveError::ArithOverflow)?;
            if range_end > data.len() {
                return Err(CoveError::OffsetRange);
            }

            if let Some((section_start, section_end)) = section_range {
                if range_start != section_start || range_end != section_end {
                    return Err(CoveError::BadSection(format!(
                        "digest manifest section {} range does not match footer section range",
                        entry.section_id
                    )));
                }
            }

            // INVARIANT: verification hashes exactly the byte range declared by
            // the manifest entry after proving that range is inside the host file.
            manifest.verify_bytes(entry, &data[range_start..range_end])?;
        }
    }
    Ok(checked)
}
