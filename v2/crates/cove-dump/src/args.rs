pub(crate) const USAGE: &str = "Usage: cove-dump <file.cove> [--metadata | --section <id> | --rows | --columns <a,b> | --morsels | --encoded-array <section[:column[:morsel]]> | --pages | --stats | --indexes | --nested | --dictionary | --dictionary-entry <code>] [--max-bytes <n>]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DumpMode {
    Metadata,
    Section(u32),
    Rows { columns: Option<Vec<String>> },
    Morsels,
    EncodedArray(EncodedArraySelector),
    Pages,
    Stats,
    Indexes,
    Nested,
    Dictionary,
    DictionaryEntry(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EncodedArraySelector {
    pub(crate) section_id: u32,
    pub(crate) column_id: Option<u32>,
    pub(crate) morsel_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliArgs {
    pub(crate) path: String,
    pub(crate) mode: DumpMode,
    pub(crate) max_bytes: usize,
}

pub(crate) fn parse_args(args: impl IntoIterator<Item = String>) -> Result<CliArgs, String> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.is_empty() {
        return Err(USAGE.to_string());
    }

    let path = args[0].clone();
    let mut mode = DumpMode::Metadata;
    let mut max_bytes = 256usize;

    let mut index = 1usize;
    while index < args.len() {
        match args[index].as_str() {
            "--metadata" => mode = DumpMode::Metadata,
            "--section" | "--decode-section" => {
                let Some(raw_id) = args.get(index + 1) else {
                    return Err("--section requires an id".into());
                };
                let id = raw_id
                    .parse::<u32>()
                    .map_err(|_| format!("invalid --section id: {raw_id}"))?;
                mode = DumpMode::Section(id);
                index += 1;
            }
            "--pages" => mode = DumpMode::Pages,
            "--rows" => mode = DumpMode::Rows { columns: None },
            "--columns" => {
                let Some(raw_columns) = args.get(index + 1) else {
                    return Err("--columns requires a comma-separated column list".into());
                };
                let columns = raw_columns
                    .split(',')
                    .map(str::trim)
                    .filter(|column| !column.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>();
                if columns.is_empty() {
                    return Err("--columns cannot be empty".into());
                }
                mode = DumpMode::Rows {
                    columns: Some(columns),
                };
                index += 1;
            }
            "--morsels" => mode = DumpMode::Morsels,
            "--encoded-array" => {
                let Some(raw_selector) = args.get(index + 1) else {
                    return Err("--encoded-array requires section[:column[:morsel]]".into());
                };
                mode = DumpMode::EncodedArray(parse_encoded_array_selector(raw_selector)?);
                index += 1;
            }
            "--stats" => mode = DumpMode::Stats,
            "--indexes" => mode = DumpMode::Indexes,
            "--nested" => mode = DumpMode::Nested,
            "--dictionary" => mode = DumpMode::Dictionary,
            "--dictionary-entry" => {
                let Some(raw_code) = args.get(index + 1) else {
                    return Err("--dictionary-entry requires a filecode".into());
                };
                let code = raw_code
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --dictionary-entry code: {raw_code}"))?;
                mode = DumpMode::DictionaryEntry(code);
                index += 1;
            }
            "--max-bytes" => {
                let Some(raw_max) = args.get(index + 1) else {
                    return Err("--max-bytes requires a numeric value".into());
                };
                max_bytes = raw_max
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --max-bytes value: {raw_max}"))?;
                index += 1;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        index += 1;
    }

    Ok(CliArgs {
        path,
        mode,
        max_bytes,
    })
}

fn parse_encoded_array_selector(raw: &str) -> Result<EncodedArraySelector, String> {
    let parts = raw.split(':').collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > 3 {
        return Err("--encoded-array expects section[:column[:morsel]]".into());
    }
    let section_id = parse_selector_u32(parts[0], "section")?;
    let column_id =
        if let Some(raw_column) = parts.get(1).copied().filter(|value| !value.is_empty()) {
            Some(parse_selector_u32(raw_column, "column")?)
        } else {
            None
        };
    let morsel_id =
        if let Some(raw_morsel) = parts.get(2).copied().filter(|value| !value.is_empty()) {
            Some(parse_selector_u32(raw_morsel, "morsel")?)
        } else {
            None
        };
    Ok(EncodedArraySelector {
        section_id,
        column_id,
        morsel_id,
    })
}

fn parse_selector_u32(raw: &str, label: &str) -> Result<u32, String> {
    raw.parse::<u32>()
        .map_err(|_| format!("invalid --encoded-array {label}: {raw}"))
}

#[cfg(test)]
mod tests {
    use super::{parse_args, DumpMode};

    #[test]
    fn parses_section_mode() {
        let args = parse_args([
            "fixture.cove".to_string(),
            "--section".to_string(),
            "42".to_string(),
        ])
        .unwrap();

        assert_eq!(args.path, "fixture.cove");
        assert_eq!(args.mode, DumpMode::Section(42));
    }

    #[test]
    fn rejects_missing_file() {
        assert!(parse_args(Vec::<String>::new()).is_err());
    }
}
