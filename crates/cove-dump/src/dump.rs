use std::{fs, path::Path};

use cove_core::{
    compression,
    constants::{
        CoveLogicalType, CovePhysicalKind, SectionKind, StorageClass, FEATURE_FILE_DICTIONARY,
    },
    dictionary::FileDictionary,
    page::ColumnPageIndex,
    reader::{self, ValidationOptions},
    segment::TableSegmentPayloadV1,
    table::TableCatalog,
};

use crate::args::DumpMode;

pub(crate) fn dump_file(path: &Path, mode: DumpMode, max_bytes: usize) -> Result<(), String> {
    let data = fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;

    match mode {
        DumpMode::Metadata | DumpMode::Section(_) => {
            let validated = structurally_validated(&data)?;
            match mode {
                DumpMode::Metadata => {
                    if validated.footer.metadata_json.is_empty() {
                        println!("(metadata is empty)");
                        return Ok(());
                    }
                    let shown = validated.footer.metadata_json.len().min(max_bytes);
                    println!(
                        "metadata_len={} showing={} bytes",
                        validated.footer.metadata_json.len(),
                        shown
                    );
                    print_hex(&validated.footer.metadata_json[..shown]);
                }
                DumpMode::Section(section_id) => {
                    let entry = validated
                        .footer
                        .sections
                        .iter()
                        .find(|section| section.section_id == section_id)
                        .ok_or_else(|| format!("section id {} not found", section_id))?;
                    let end = entry
                        .end_offset()
                        .map_err(|_| "section offset overflow".to_string())?
                        as usize;
                    if end > data.len() {
                        return Err(format!("section {} outside file bounds", section_id));
                    }
                    let section = &data[entry.offset as usize..end];
                    let shown = section.len().min(max_bytes);
                    println!(
                        "section_id={} len={} showing={} bytes",
                        section_id,
                        section.len(),
                        shown
                    );
                    print_hex(&section[..shown]);
                }
                _ => unreachable!(),
            }
        }
        DumpMode::Pages => {
            let report = semantically_validated(&data)?;
            dump_pages(&data, &report.validated, max_bytes)?;
        }
        DumpMode::Stats => {
            let validated = structurally_validated(&data)?;
            dump_section_group(
                &data,
                &validated,
                "stats",
                &[
                    SectionKind::ZoneStats,
                    SectionKind::AggregateSynopsis,
                    SectionKind::TopNZoneSummary,
                ],
                max_bytes,
            )?;
        }
        DumpMode::Indexes => {
            let validated = structurally_validated(&data)?;
            dump_section_group(
                &data,
                &validated,
                "indexes",
                &[
                    SectionKind::ColumnDomain,
                    SectionKind::ExactSetIndex,
                    SectionKind::BloomIndex,
                    SectionKind::InvertedMorselIndex,
                    SectionKind::LookupIndex,
                    SectionKind::CompositeZoneIndex,
                    SectionKind::TopNZoneSummary,
                    SectionKind::TemporalBloomIndex,
                    SectionKind::MapIdentityEquivalenceIndex,
                    SectionKind::MapEvidenceIndex,
                ],
                max_bytes,
            )?;
        }
        DumpMode::Nested => {
            let report = semantically_validated(&data)?;
            dump_nested_layout(&data, &report.validated)?;
        }
        DumpMode::Dictionary => {
            let report = semantically_validated(&data)?;
            let validated = &report.validated;

            if validated.header.required_features & FEATURE_FILE_DICTIONARY == 0 {
                println!("(no file dictionary in this file)");
                return Ok(());
            }

            let dict =
                parse_dictionary(&data, validated).map_err(|e| format!("dictionary parse: {e}"))?;

            println!("dictionary_entries={}", dict.len());
            let max_entries = (dict.len() as usize).min(max_bytes.max(4));
            for filecode in 0..max_entries as u32 {
                let entry = dict.get_entry(filecode).map_err(|e| e.to_string())?;
                let storage = match StorageClass::from_u8(entry.storage_class) {
                    Some(StorageClass::Inline) => "inline",
                    Some(StorageClass::Payload) => "payload",
                    Some(StorageClass::Redacted) => "redacted",
                    Some(_) => "future",
                    None => "unknown",
                };
                let tag_str = format!("{}", entry.value_tag);
                println!("  filecode={filecode} tag={tag_str} storage={storage}");
            }
        }
        DumpMode::DictionaryEntry(code) => {
            let report = semantically_validated(&data)?;
            let validated = &report.validated;

            if validated.header.required_features & FEATURE_FILE_DICTIONARY == 0 {
                return Err("no file dictionary in this file".into());
            }

            let dict =
                parse_dictionary(&data, validated).map_err(|e| format!("dictionary parse: {e}"))?;

            let code32 =
                u32::try_from(code).map_err(|_| "filecode out of u32 range".to_string())?;
            let value = dict.decode_value(code32).map_err(|e| e.to_string())?;

            match value {
                cove_core::dictionary::DictionaryValue::RedactedPresent => {
                    println!("filecode={code} value=REDACTED");
                }
                cove_core::dictionary::DictionaryValue::RawBytes(bytes) => {
                    println!("filecode={code} value_len={}", bytes.len());
                    let shown = bytes.len().min(max_bytes);
                    print_hex(&bytes[..shown]);
                }
                _ => return Err("unsupported future dictionary value kind".into()),
            }
        }
    }

    Ok(())
}

fn structurally_validated(data: &[u8]) -> Result<reader::ValidatedCoveFile, String> {
    reader::validate_bytes(data).map_err(|e| format!("validation: {e}"))
}

fn semantically_validated(data: &[u8]) -> Result<reader::ValidationReport, String> {
    let options = ValidationOptions {
        semantic: true,
        verify_digests: false,
        allow_unknown_optional_extensions: true,
        ..ValidationOptions::default()
    };
    reader::validate_bytes_with_options(data, options).map_err(|e| format!("validation: {e}"))
}

fn dump_pages(
    data: &[u8],
    validated: &cove_core::reader::ValidatedCoveFile,
    max_bytes: usize,
) -> Result<(), String> {
    let segments = validated
        .footer
        .sections
        .iter()
        .filter(|section| section.section_kind == SectionKind::TableSegmentData as u16)
        .collect::<Vec<_>>();
    if segments.is_empty() {
        println!("(no table segment data sections)");
        return Ok(());
    }

    for section in segments {
        let segment_bytes = compression::section_payload(data, section)
            .map_err(|e| format!("section payload: {e}"))?;
        let segment = TableSegmentPayloadV1::parse(segment_bytes.as_ref())
            .map_err(|e| format!("segment parse: {e}"))?;
        println!(
            "segment section_id={} table={} segment={} rows={} columns={}",
            section.section_id,
            segment.header.table_id,
            segment.header.segment_id,
            segment.header.row_count,
            segment.columns.len()
        );
        for column in &segment.columns {
            let start = column.page_index_offset as usize;
            let end = column
                .page_index_offset
                .checked_add(column.page_index_length)
                .ok_or_else(|| "page index offset overflow".to_string())?
                as usize;
            let page_index = ColumnPageIndex::parse(&segment_bytes[start..end])
                .map_err(|e| format!("page index parse: {e}"))?;
            for (page_idx, page) in page_index.entries.iter().enumerate() {
                println!(
                    "  column={} logical={:?} physical={:?} page={} morsel={} rows={} non_null={} nulls={} encoding={} offset={} len={} raw_len={} flags=0x{:08x}",
                    column.column_id,
                    column.logical_type,
                    column.physical_kind,
                    page_idx,
                    page.morsel_id,
                    page.row_count,
                    page.non_null_count,
                    page.null_count,
                    page.encoding_root,
                    page.page_offset,
                    page.page_length,
                    page.uncompressed_length,
                    page.flags,
                );
                if page.page_length != 0 && max_bytes != 0 {
                    let start = page.page_offset as usize;
                    let end = page
                        .page_offset
                        .checked_add(page.page_length)
                        .ok_or_else(|| "page payload offset overflow".to_string())?
                        as usize;
                    let payload =
                        compression::column_page_payload(&segment_bytes[start..end], page)
                            .map_err(|e| format!("page payload: {e}"))?;
                    let shown = payload.len().min(max_bytes);
                    println!("    payload_len={} showing={} bytes", payload.len(), shown);
                    print_hex(&payload[..shown]);
                }
            }
        }
    }
    Ok(())
}

fn dump_section_group(
    data: &[u8],
    validated: &cove_core::reader::ValidatedCoveFile,
    label: &str,
    kinds: &[SectionKind],
    max_bytes: usize,
) -> Result<(), String> {
    let sections = validated
        .footer
        .sections
        .iter()
        .filter(|section| {
            kinds
                .iter()
                .any(|kind| section.section_kind == *kind as u16)
        })
        .collect::<Vec<_>>();
    if sections.is_empty() {
        println!("(no {label} sections)");
        return Ok(());
    }
    for section in sections {
        let payload = compression::section_payload(data, section)
            .map_err(|e| format!("section payload: {e}"))?;
        let shown = payload.len().min(max_bytes);
        println!(
            "{} section_id={} kind={} len={} raw_len={} rows={} items={} showing={} bytes",
            label,
            section.section_id,
            section_kind_name(section.section_kind),
            payload.len(),
            section.length,
            section.row_count,
            section.item_count,
            shown
        );
        print_hex(&payload[..shown]);
    }
    Ok(())
}

fn dump_nested_layout(
    data: &[u8],
    validated: &cove_core::reader::ValidatedCoveFile,
) -> Result<(), String> {
    let catalog_entry = validated
        .footer
        .sections
        .iter()
        .find(|section| section.section_kind == SectionKind::TableCatalog as u16);
    let Some(catalog_entry) = catalog_entry else {
        println!("(no table catalog section)");
        return Ok(());
    };
    let catalog_bytes = compression::section_payload(data, catalog_entry)
        .map_err(|e| format!("table catalog payload: {e}"))?;
    let catalog = TableCatalog::parse(catalog_bytes.as_ref())
        .map_err(|e| format!("table catalog parse: {e}"))?;
    let mut found = false;
    for table in &catalog.tables {
        for column in &table.columns {
            if is_nested(column.logical, column.physical) {
                found = true;
                println!(
                    "table={} column={} name={} logical={:?} physical={:?} nullable={} flags=0x{:08x}",
                    table.table_id,
                    column.column_id,
                    column.name,
                    column.logical,
                    column.physical,
                    column.nullable,
                    column.flags,
                );
            }
        }
    }
    if !found {
        println!("(no nested columns)");
    }
    Ok(())
}

fn is_nested(logical: CoveLogicalType, physical: CovePhysicalKind) -> bool {
    matches!(
        (logical, physical),
        (CoveLogicalType::List, _)
            | (CoveLogicalType::Struct, _)
            | (CoveLogicalType::Map, _)
            | (_, CovePhysicalKind::List)
            | (_, CovePhysicalKind::Struct)
            | (_, CovePhysicalKind::Map)
    )
}

fn parse_dictionary(
    data: &[u8],
    validated: &cove_core::reader::ValidatedCoveFile,
) -> Result<FileDictionary, cove_core::CoveError> {
    let index_entry = validated
        .footer
        .sections
        .iter()
        .find(|section| section.section_kind == SectionKind::FileDictionaryIndex as u16)
        .ok_or_else(|| {
            cove_core::CoveError::BadSection("FILE_DICTIONARY_INDEX section missing".into())
        })?;
    let payload_entry = validated
        .footer
        .sections
        .iter()
        .find(|section| section.section_kind == SectionKind::FileDictionaryPayload as u16);

    let index_bytes = compression::section_payload(data, index_entry)?;
    let payload_bytes = match payload_entry {
        Some(entry) => compression::section_payload(data, entry)?,
        None => std::borrow::Cow::Borrowed(&[][..]),
    };

    FileDictionary::parse(&index_bytes, &payload_bytes)
}

fn print_hex(bytes: &[u8]) {
    for (line_idx, chunk) in bytes.chunks(16).enumerate() {
        print!("{:08x}: ", line_idx * 16);
        for byte in chunk {
            print!("{:02x} ", byte);
        }
        println!();
    }
}

fn section_kind_name(kind: u16) -> String {
    SectionKind::from_u16(kind)
        .map(|kind| format!("{kind:?}"))
        .unwrap_or_else(|| format!("Unknown({kind})"))
}
