pub(crate) const USAGE: &str = "Usage: cove-dump <file.cove> [--metadata | --section <id> | --pages | --stats | --indexes | --nested | --dictionary | --dictionary-entry <code>] [--max-bytes <n>]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DumpMode {
    Metadata,
    Section(u32),
    Pages,
    Stats,
    Indexes,
    Nested,
    Dictionary,
    DictionaryEntry(u64),
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
