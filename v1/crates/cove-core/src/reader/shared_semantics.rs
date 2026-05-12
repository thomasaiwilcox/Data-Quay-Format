use std::borrow::Cow;

use crate::{
    compression, constants::SectionKind, dictionary::FileDictionaryView, footer::CoveFooter,
    CoveError,
};

pub(super) fn parse_validation_dictionary<'a>(
    data: &'a [u8],
    footer: &CoveFooter,
) -> Result<Option<FileDictionaryView<'a>>, CoveError> {
    let Some(index_entry) = footer
        .sections
        .iter()
        .find(|entry| entry.section_kind == SectionKind::FileDictionaryIndex as u16)
    else {
        return Ok(None);
    };
    let index_bytes = compression::section_payload(data, index_entry)?;
    let payload_bytes = match footer
        .sections
        .iter()
        .find(|entry| entry.section_kind == SectionKind::FileDictionaryPayload as u16)
    {
        Some(payload_entry) => compression::section_payload(data, payload_entry)?,
        None => Cow::Borrowed(&[][..]),
    };
    let dictionary = FileDictionaryView::parse(index_bytes, payload_bytes)?;
    dictionary.validate_all()?;
    Ok(Some(dictionary))
}
