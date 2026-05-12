use cove_core::reader::{OptionalPushdownPolicy, ValidationOptions};

pub(crate) const USAGE: &str = "Usage: cove-validate [--semantic] [--verify-digests] [--fail-open-optional-pushdown] [--json] [--explain] <file.cove|file.covemap> [<file2> ...]";

#[derive(Clone)]
pub(crate) struct CliArgs {
    pub(crate) validation: ValidationOptions,
    pub(crate) json_out: bool,
    pub(crate) explain: bool,
    pub(crate) file_paths: Vec<String>,
}

pub(crate) fn parse_args(args: impl IntoIterator<Item = String>) -> Result<CliArgs, String> {
    let mut semantic = false;
    let mut verify_digests = false;
    let mut fail_open_optional_pushdown = false;
    let mut json_out = false;
    let mut explain = false;
    let mut file_paths = Vec::new();

    let mut parsing_flags = true;
    for arg in args {
        if parsing_flags && arg.starts_with("--") {
            match arg.as_str() {
                "--semantic" => semantic = true,
                "--verify-digests" => verify_digests = true,
                "--fail-open-optional-pushdown" => fail_open_optional_pushdown = true,
                "--json" => json_out = true,
                "--explain" => explain = true,
                other => return Err(format!("Unknown flag: {other}")),
            }
        } else {
            parsing_flags = false;
            file_paths.push(arg);
        }
    }

    if file_paths.is_empty() {
        return Err(USAGE.to_string());
    }

    Ok(CliArgs {
        validation: ValidationOptions {
            semantic,
            verify_digests,
            allow_unknown_optional_extensions: true,
            optional_pushdown_policy: if fail_open_optional_pushdown {
                OptionalPushdownPolicy::FailOpen
            } else {
                OptionalPushdownPolicy::Strict
            },
        },
        json_out,
        explain,
        file_paths,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    #[test]
    fn parses_validation_flags() {
        let args = parse_args([
            "--semantic".to_string(),
            "--verify-digests".to_string(),
            "fixture.cove".to_string(),
        ])
        .unwrap();

        assert!(args.validation.semantic);
        assert!(args.validation.verify_digests);
        assert_eq!(args.file_paths, vec!["fixture.cove"]);
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse_args(Vec::<String>::new()).is_err());
    }
}
