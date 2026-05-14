use std::{fs, path::PathBuf};

use cove_arrow::convert::{
    ParquetAccelerationPolicy, ParquetAggregatePolicy, ParquetClusteringPolicy,
    ParquetConversionOptions, ParquetConversionResult, ParquetDictionaryPolicy, ParquetStatsPolicy,
};
use cove_core::{
    constants::{CompressionCodec, DigestAlgorithm},
    digest::compute_digest,
    durable,
    utility::hex_encode,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionCommand {
    pub input: PathBuf,
    pub output: PathBuf,
    pub options: ParquetConversionOptions,
    pub report: Option<ReportTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportTarget {
    Stdout,
    Path(PathBuf),
}

pub fn parse_conversion_args(
    args: impl IntoIterator<Item = String>,
    input_label: &str,
    default_table_name: &str,
) -> Result<Option<ConversionCommand>, String> {
    let mut options = ParquetConversionOptions {
        table_name: default_table_name.to_string(),
        ..ParquetConversionOptions::default()
    };
    let mut report = None;
    let mut positional = Vec::new();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--table-name" => options.table_name = next_value(&mut iter, "--table-name")?,
            "--namespace" => options.namespace = next_value(&mut iter, "--namespace")?,
            "--morsel-row-count" => {
                let raw = next_value(&mut iter, "--morsel-row-count")?;
                options.morsel_row_count = raw
                    .parse::<u32>()
                    .map_err(|_| "--morsel-row-count must be a u32".to_string())?;
                if options.morsel_row_count == 0 {
                    return Err("--morsel-row-count must be greater than zero".into());
                }
            }
            "--segment-row-count" => {
                let raw = next_value(&mut iter, "--segment-row-count")?;
                options.segment_row_count = raw
                    .parse::<u32>()
                    .map_err(|_| "--segment-row-count must be a u32".to_string())?;
                if options.segment_row_count == 0 {
                    return Err("--segment-row-count must be greater than zero".into());
                }
            }
            "--compression" => {
                options.page_compression =
                    parse_compression(&next_value(&mut iter, "--compression")?)?;
            }
            "--dictionary-policy" => {
                options.dictionary_policy =
                    parse_dictionary_policy(&next_value(&mut iter, "--dictionary-policy")?)?;
            }
            "--stats-policy" => {
                options.stats_policy =
                    parse_stats_policy(&next_value(&mut iter, "--stats-policy")?)?;
            }
            "--acceleration-policy" => {
                options.acceleration_policy =
                    parse_acceleration_policy(&next_value(&mut iter, "--acceleration-policy")?)?;
            }
            "--point-lookup-columns" => {
                options.point_lookup_columns =
                    parse_csv_list(&next_value(&mut iter, "--point-lookup-columns")?);
            }
            "--cluster-columns" => {
                options.cluster_columns =
                    parse_csv_list(&next_value(&mut iter, "--cluster-columns")?);
            }
            "--topn-columns" => {
                options.topn_columns = parse_csv_list(&next_value(&mut iter, "--topn-columns")?);
            }
            "--aggregate-synopsis" => {
                options.aggregate_policy =
                    parse_aggregate_policy(&next_value(&mut iter, "--aggregate-synopsis")?)?;
            }
            "--aggregate-columns" => {
                options.aggregate_columns =
                    parse_csv_list(&next_value(&mut iter, "--aggregate-columns")?);
            }
            "--aggregate-topk-columns" => {
                options.aggregate_topk_columns =
                    parse_csv_list(&next_value(&mut iter, "--aggregate-topk-columns")?);
            }
            "--distinct-sketch-columns" => {
                options.distinct_sketch_columns =
                    parse_csv_list(&next_value(&mut iter, "--distinct-sketch-columns")?);
            }
            "--quantile-sketch-columns" => {
                options.quantile_sketch_columns =
                    parse_csv_list(&next_value(&mut iter, "--quantile-sketch-columns")?);
            }
            "--aggregate-topk-k" => {
                let raw = next_value(&mut iter, "--aggregate-topk-k")?;
                options.aggregate_topk_k = raw
                    .parse::<u32>()
                    .map_err(|_| "--aggregate-topk-k must be a u32".to_string())?;
                if options.aggregate_topk_k == 0 {
                    return Err("--aggregate-topk-k must be greater than zero".into());
                }
            }
            "--hll-precision" => {
                let raw = next_value(&mut iter, "--hll-precision")?;
                options.hll_precision = raw
                    .parse::<u8>()
                    .map_err(|_| "--hll-precision must be a u8".to_string())?;
            }
            "--kll-k" => {
                let raw = next_value(&mut iter, "--kll-k")?;
                options.kll_k = raw
                    .parse::<u32>()
                    .map_err(|_| "--kll-k must be a u32".to_string())?;
                if options.kll_k < 8 {
                    return Err("--kll-k must be at least 8".into());
                }
            }
            "--composite-zone" => {
                options
                    .composite_zone_groups
                    .push(parse_csv_list(&next_value(&mut iter, "--composite-zone")?));
            }
            "--emit-covx" => options.emit_covx = true,
            "--emit-covm" => options.emit_covm = true,
            "--stable-clustering" => {
                options.clustering_policy = ParquetClusteringPolicy::StableClusterDeclaredColumns;
            }
            "--report" => {
                let raw = next_value(&mut iter, "--report")?;
                report = Some(if raw == "-" {
                    ReportTarget::Stdout
                } else {
                    ReportTarget::Path(PathBuf::from(raw))
                });
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
            _ => positional.push(PathBuf::from(arg)),
        }
    }

    if positional.len() != 2 {
        return Err(format!("expected <{input_label}> and <output.cove>"));
    }
    Ok(Some(ConversionCommand {
        input: positional.remove(0),
        output: positional.remove(0),
        options,
        report,
    }))
}

pub fn publish_conversion_result(
    command: ConversionCommand,
    result: ParquetConversionResult,
) -> Result<(), String> {
    durable::durable_replace(&command.output, &result.cove_bytes)
        .map_err(|err| format!("cannot durably publish {}: {err}", command.output.display()))?;
    if let Some(covx_bytes) = &result.covx_bytes {
        let path = command.output.with_extension("covx");
        durable::durable_replace(&path, covx_bytes)
            .map_err(|err| format!("cannot durably publish {}: {err}", path.display()))?;
    }
    if let Some(covm_bytes) = &result.covm_bytes {
        let path = command.output.with_extension("covm");
        durable::durable_replace(&path, covm_bytes)
            .map_err(|err| format!("cannot durably publish {}: {err}", path.display()))?;
    }

    if let Some(target) = command.report {
        let report = serde_json::to_string_pretty(&result.report.to_json_value())
            .map_err(|err| format!("cannot serialize conversion report: {err}"))?;
        match target {
            ReportTarget::Stdout => println!("{report}"),
            ReportTarget::Path(path) => fs::write(&path, report)
                .map_err(|err| format!("cannot write {}: {err}", path.display()))?,
        }
    } else {
        eprintln!(
            "converted {} rows and {} columns to {}",
            result.report.row_count,
            result.report.column_count,
            command.output.display()
        );
    }
    Ok(())
}

pub fn set_source_identity(
    options: &mut ParquetConversionOptions,
    input: &std::path::Path,
    bytes: &[u8],
) -> Result<(), String> {
    options.source_identifier = Some(input.display().to_string());
    options.source_digest = Some(source_digest(bytes)?);
    Ok(())
}

pub fn source_digest(bytes: &[u8]) -> Result<String, String> {
    compute_digest(DigestAlgorithm::Sha256, bytes)
        .map(|digest| format!("sha256:{}", hex_encode(&digest)))
        .map_err(|err| err.to_string())
}

fn next_value(iter: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

pub fn parse_compression(value: &str) -> Result<CompressionCodec, String> {
    match value {
        "none" => Ok(CompressionCodec::None),
        "lz4" => Ok(CompressionCodec::Lz4),
        "zstd" => Ok(CompressionCodec::Zstd),
        _ => Err("--compression must be one of: none, lz4, zstd".into()),
    }
}

fn parse_dictionary_policy(value: &str) -> Result<ParquetDictionaryPolicy, String> {
    match value {
        "auto" => Ok(ParquetDictionaryPolicy::Auto),
        "never" => Ok(ParquetDictionaryPolicy::Never),
        "always" => Ok(ParquetDictionaryPolicy::Always),
        _ => Err("--dictionary-policy must be one of: auto, never, always".into()),
    }
}

fn parse_stats_policy(value: &str) -> Result<ParquetStatsPolicy, String> {
    match value {
        "none" => Ok(ParquetStatsPolicy::None),
        "recompute" => Ok(ParquetStatsPolicy::Recompute),
        _ => Err("--stats-policy must be one of: none, recompute".into()),
    }
}

fn parse_acceleration_policy(value: &str) -> Result<ParquetAccelerationPolicy, String> {
    match value {
        "none" => Ok(ParquetAccelerationPolicy::None),
        "declared-only" => Ok(ParquetAccelerationPolicy::DeclaredOnly),
        "auto" => Ok(ParquetAccelerationPolicy::Auto),
        _ => Err("--acceleration-policy must be one of: none, declared-only, auto".into()),
    }
}

fn parse_aggregate_policy(value: &str) -> Result<ParquetAggregatePolicy, String> {
    match value {
        "none" => Ok(ParquetAggregatePolicy::None),
        "declared-only" => Ok(ParquetAggregatePolicy::DeclaredOnly),
        "auto" => Ok(ParquetAggregatePolicy::Auto),
        _ => Err("--aggregate-synopsis must be one of: none, declared-only, auto".into()),
    }
}

fn parse_csv_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn usage(binary: &str, input_label: &str) -> String {
    format!(
        "Usage: {binary} [options] <{input_label}> <output.cove>\n\n\
Options:\n  \
--table-name <name>         Output COVE table name\n  \
--namespace <name>          Output COVE namespace (default: interop)\n  \
--morsel-row-count <rows>   Rows per COVE morsel/page (default: 4096)\n  \
--segment-row-count <rows>  Rows per COVE segment (default: u32::MAX)\n  \
--compression <codec>       Page compression: none, lz4, zstd (default: none)\n  \
--dictionary-policy <mode>  Dictionary synthesis policy: auto, never, always\n  \
--stats-policy <mode>       Stats policy: none, recompute\n  \
--acceleration-policy <m>   Index policy: none, declared-only, auto\n  \
--point-lookup-columns <c>  Comma-separated columns eligible for lookup indexes\n  \
--cluster-columns <cols>    Comma-separated stable clustering columns\n  \
--topn-columns <cols>       Comma-separated ordered hot columns for Top-N summaries\n  \
--aggregate-synopsis <m>    Aggregate synopsis policy: none, declared-only, auto\n  \
--aggregate-columns <cols>  Comma-separated columns for declared aggregate synopsis\n  \
--aggregate-topk-columns <c> Columns for TopK aggregate synopsis payloads\n  \
--distinct-sketch-columns <c> Columns for HLL distinct sketch payloads\n  \
--quantile-sketch-columns <c> Columns for KLL quantile sketch payloads\n  \
--aggregate-topk-k <n>      TopK payload size (default: 64)\n  \
--hll-precision <p>         HLL precision for distinct sketches (default: 14)\n  \
--kll-k <n>                 KLL compactor k for quantile sketches (default: 200)\n  \
--composite-zone <cols>     Comma-separated composite zone group; may be repeated\n  \
--stable-clustering         Opt in to stable clustering when implemented\n  \
--emit-covx                 Request COVX artifact emission\n  \
--emit-covm                 Request COVM artifact emission\n  \
--report <path|->           Write the machine-readable conversion report\n  \
-h, --help                  Show this help"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_paths_and_options() {
        let command = parse_conversion_args(
            [
                "--table-name".to_string(),
                "orders".to_string(),
                "--namespace".to_string(),
                "sales".to_string(),
                "--morsel-row-count".to_string(),
                "128".to_string(),
                "--segment-row-count".to_string(),
                "256".to_string(),
                "--compression".to_string(),
                "none".to_string(),
                "--dictionary-policy".to_string(),
                "never".to_string(),
                "--stats-policy".to_string(),
                "recompute".to_string(),
                "--acceleration-policy".to_string(),
                "declared-only".to_string(),
                "--point-lookup-columns".to_string(),
                "id,email".to_string(),
                "--cluster-columns".to_string(),
                "tenant_id".to_string(),
                "--topn-columns".to_string(),
                "score".to_string(),
                "--aggregate-synopsis".to_string(),
                "declared-only".to_string(),
                "--aggregate-columns".to_string(),
                "score".to_string(),
                "--aggregate-topk-columns".to_string(),
                "score".to_string(),
                "--distinct-sketch-columns".to_string(),
                "email".to_string(),
                "--quantile-sketch-columns".to_string(),
                "score".to_string(),
                "--aggregate-topk-k".to_string(),
                "32".to_string(),
                "--hll-precision".to_string(),
                "12".to_string(),
                "--kll-k".to_string(),
                "128".to_string(),
                "--composite-zone".to_string(),
                "tenant_id,score".to_string(),
                "--emit-covx".to_string(),
                "--emit-covm".to_string(),
                "--stable-clustering".to_string(),
                "--report".to_string(),
                "-".to_string(),
                "in.parquet".to_string(),
                "out.cove".to_string(),
            ],
            "input.parquet",
            "parquet_import",
        )
        .unwrap()
        .unwrap();

        assert_eq!(command.options.table_name, "orders");
        assert_eq!(command.options.namespace, "sales");
        assert_eq!(command.options.morsel_row_count, 128);
        assert_eq!(command.options.segment_row_count, 256);
        assert_eq!(
            command.options.dictionary_policy,
            ParquetDictionaryPolicy::Never
        );
        assert_eq!(command.options.stats_policy, ParquetStatsPolicy::Recompute);
        assert_eq!(
            command.options.acceleration_policy,
            ParquetAccelerationPolicy::DeclaredOnly
        );
        assert_eq!(command.options.point_lookup_columns, vec!["id", "email"]);
        assert_eq!(command.options.cluster_columns, vec!["tenant_id"]);
        assert_eq!(command.options.topn_columns, vec!["score"]);
        assert_eq!(
            command.options.aggregate_policy,
            ParquetAggregatePolicy::DeclaredOnly
        );
        assert_eq!(command.options.aggregate_columns, vec!["score"]);
        assert_eq!(command.options.aggregate_topk_columns, vec!["score"]);
        assert_eq!(command.options.distinct_sketch_columns, vec!["email"]);
        assert_eq!(command.options.quantile_sketch_columns, vec!["score"]);
        assert_eq!(command.options.aggregate_topk_k, 32);
        assert_eq!(command.options.hll_precision, 12);
        assert_eq!(command.options.kll_k, 128);
        assert_eq!(
            command.options.composite_zone_groups,
            vec![vec!["tenant_id".to_string(), "score".to_string()]]
        );
        assert!(command.options.emit_covx);
        assert!(command.options.emit_covm);
        assert_eq!(
            command.options.clustering_policy,
            ParquetClusteringPolicy::StableClusterDeclaredColumns
        );
        assert_eq!(command.report, Some(ReportTarget::Stdout));
    }

    #[test]
    fn rejects_unknown_compression() {
        assert_eq!(
            parse_compression("snappy"),
            Err("--compression must be one of: none, lz4, zstd".into())
        );
    }
}
