//! Deterministic fuzz and mutation campaigns for the COVE v2 reference code.
//!
//! This is intentionally not a libFuzzer wrapper. It is a small, repeatable
//! release-gate harness that stresses parser and encoding invariants with
//! bounded mutation campaigns.

use std::{
    collections::BTreeSet,
    fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
};

use cove_cache::CoverageCacheV2;
use cove_codec::{CodecExtensionDescriptorV2, RegisteredEncodingEnvelopeV2};
use cove_core::{
    artifact::{covemap::CovemapFile, covm::CovmFile, covx::CovxFile},
    checksum,
    collation::CollationRegistry,
    compression::{column_page_payload, encode_page_payload},
    constants::CompressionCodec,
    dictionary::FileDictionary,
    digest::DigestManifest,
    domain::ColumnDomain,
    encoding::{
        assert_parity,
        bit_packed::{BitPacked, BitPackedPayload},
        constant::{Constant, ConstantPayload},
        delta::{Delta, DeltaPayload},
        frame_of_reference::{ForPayload, FrameOfReference},
        local_codebook::{
            LocalCodebook, LocalCodebookPayload, LocalCodebookValues, LocalIndexPayload,
        },
        patched_base::{PatchedBase, PatchedBasePayload},
        plain::{PlainFixed, PlainFixedPayload, PlainVarint, PlainVarintPayload},
        rle::{Rle, RlePayload},
        run_end::{RunEnd, RunEndPayload},
        sparse::{Sparse, SparsePayload},
    },
    extensions::{
        ExtensionIndexDescriptorV1, ExtensionLogicalTypeV1, ExtensionRegistry,
        ExtensionValidationContext,
    },
    feature_binding::SectionFeatureBindingSectionV2,
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    interop::lakehouse::LakehouseHints,
    io_hints::IoHints,
    kernel::KernelCapabilities,
    metadata::MetadataJson,
    page::{ColumnPageIndexEntryV1, PageIndex},
    profile::{
        cove_e::{
            CodeSpaceDescriptorV1, EngineMountPolicyV1, EngineProfileRegistry,
            ExecutionCodeDescriptorV1, ExecutionScopeDescriptorV1,
        },
        cove_h::HarborMountHintsV1,
        cove_o::{ObjectTypeCatalog, TemporalBloomIndex, TemporalSegmentIndex},
    },
    reader::{self, ValidationOptions},
    redaction::RedactionManifest,
    row_ref::RowRef,
    segment::{RowMorselDirectory, TableSegmentHeaderV1, TableSegmentIndex},
    sort::{ClusteringKeyEntryV1, SortKeyEntryV1},
    table::TableCatalog,
    writer::MinimalCoveWriter,
    CoveError,
};
use cove_coverage::{
    CoveragePlanCandidateV2, CoverageProofRecordV2, CoverageProviderDescriptorV2, CoverageSetV2,
    IntervalPredicateV2, PredicateNormalFormV2,
};
use cove_index::{CoviArtifactV2, IndexCapabilityV2, IndexOnlyCapabilityV2};
use cove_layout::{
    FastMetadataIndexV2, LayoutPlanV2, PageClusterDirectoryV2, ScanSplitIndexV2,
    ZeroCopyBufferMapV2,
};
use cove_runtime::RuntimeCompatibilityHintV2;
use serde_json::Value;

const DEFAULT_SEED: u64 = 0xC0FE_F00D_D15E_A5E5;
const SMOKE_MUTATIONS: usize = 4;
const CORPUS_MUTATIONS: usize = 4;
const PARSER_MUTATIONS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Smoke {
        seed: u64,
        mutations: usize,
    },
    Corpus {
        manifest: PathBuf,
        seed: u64,
        mutations: usize,
    },
    Parsers {
        seed: u64,
        mutations: usize,
    },
    Encodings {
        seed: u64,
        mutations: usize,
    },
    Help,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CampaignStats {
    pub campaign: &'static str,
    pub seed: u64,
    pub cases_run: usize,
    pub rejects_observed: usize,
    pub accepted_mutations: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzFailure {
    pub campaign: &'static str,
    pub case: String,
    pub seed: u64,
    pub mutation_index: usize,
    pub operation: String,
    pub detail: String,
}

impl std::fmt::Display for FuzzFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} failed: case={} seed={} mutation={} op={} detail={}",
            self.campaign, self.case, self.seed, self.mutation_index, self.operation, self.detail
        )
    }
}

impl std::error::Error for FuzzFailure {}

pub fn run_cli(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    match parse_args(args)? {
        Command::Help => {
            print_usage();
            Ok(())
        }
        Command::Smoke { seed, mutations } => {
            let stats = run_smoke(seed, mutations).map_err(|err| err.to_string())?;
            print_stats(&stats);
            Ok(())
        }
        Command::Corpus {
            manifest,
            seed,
            mutations,
        } => {
            let stats = run_corpus(&manifest, seed, mutations).map_err(|err| err.to_string())?;
            print_stats(&stats);
            Ok(())
        }
        Command::Parsers { seed, mutations } => {
            let stats = run_parsers(seed, mutations).map_err(|err| err.to_string())?;
            print_stats(&stats);
            Ok(())
        }
        Command::Encodings { seed, mutations } => {
            let stats = run_encodings(seed, mutations).map_err(|err| err.to_string())?;
            print_stats(&stats);
            Ok(())
        }
    }
}

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut command = None;
    let mut manifest = None;
    let mut seed = DEFAULT_SEED;
    let mut mutations = None;
    let mut args = args.into_iter().peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--seed" => {
                let raw = args
                    .next()
                    .ok_or_else(|| "--seed requires a value".to_string())?;
                seed = parse_u64(&raw).ok_or_else(|| format!("invalid seed '{raw}'"))?;
            }
            "--mutations" => {
                let raw = args
                    .next()
                    .ok_or_else(|| "--mutations requires a value".to_string())?;
                mutations = Some(
                    raw.parse::<usize>()
                        .map_err(|_| format!("invalid mutation count '{raw}'"))?,
                );
            }
            "smoke" | "corpus" | "parsers" | "encodings" => {
                if command.replace(arg).is_some() {
                    return Err("only one subcommand may be provided".into());
                }
            }
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            path => {
                if command.as_deref() != Some("corpus") {
                    return Err(format!("unknown subcommand {path}"));
                }
                if manifest.replace(PathBuf::from(path)).is_some() {
                    return Err("only one manifest path may be provided".into());
                }
            }
        }
    }

    match command.unwrap_or("smoke".to_string()).as_str() {
        "smoke" => Ok(Command::Smoke {
            seed,
            mutations: mutations.unwrap_or(SMOKE_MUTATIONS),
        }),
        "corpus" => Ok(Command::Corpus {
            manifest: manifest.unwrap_or_else(|| PathBuf::from("conformance/manifest.jsonl")),
            seed,
            mutations: mutations.unwrap_or(CORPUS_MUTATIONS),
        }),
        "parsers" => Ok(Command::Parsers {
            seed,
            mutations: mutations.unwrap_or(PARSER_MUTATIONS),
        }),
        "encodings" => Ok(Command::Encodings {
            seed,
            mutations: mutations.unwrap_or(PARSER_MUTATIONS),
        }),
        other => Err(format!("unknown subcommand {other}")),
    }
}

fn parse_u64(raw: &str) -> Option<u64> {
    raw.strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .map(|hex| u64::from_str_radix(hex, 16).ok())
        .unwrap_or_else(|| raw.parse::<u64>().ok())
}

fn print_usage() {
    println!(
        "Usage: cove-fuzz [smoke|corpus|parsers|encodings] [manifest.jsonl] [--seed N] [--mutations N]"
    );
}

fn print_stats(stats: &CampaignStats) {
    println!(
        "{}: seed={} cases={} rejects={} accepted_mutations={} skipped={} panics=0",
        stats.campaign,
        stats.seed,
        stats.cases_run,
        stats.rejects_observed,
        stats.accepted_mutations,
        stats.skipped
    );
}

pub fn run_smoke(seed: u64, mutations: usize) -> Result<CampaignStats, FuzzFailure> {
    let mut stats = CampaignStats {
        campaign: "smoke",
        seed,
        ..CampaignStats::default()
    };
    bootstrap_truncation_campaign(seed, &mut stats)?;
    parser_seed_campaign(seed ^ 0x51A5_0001, mutations, true, &mut stats)?;
    encoding_campaign(seed ^ 0x51A5_0002, mutations, &mut stats)?;
    Ok(stats)
}

pub fn run_parsers(seed: u64, mutations: usize) -> Result<CampaignStats, FuzzFailure> {
    let mut stats = CampaignStats {
        campaign: "parsers",
        seed,
        ..CampaignStats::default()
    };
    parser_seed_campaign(seed, mutations, false, &mut stats)?;
    Ok(stats)
}

pub fn run_encodings(seed: u64, mutations: usize) -> Result<CampaignStats, FuzzFailure> {
    let mut stats = CampaignStats {
        campaign: "encodings",
        seed,
        ..CampaignStats::default()
    };
    encoding_campaign(seed, mutations, &mut stats)?;
    Ok(stats)
}

pub fn run_corpus(
    manifest_path: &Path,
    seed: u64,
    mutations: usize,
) -> Result<CampaignStats, FuzzFailure> {
    let corpus_root = manifest_root(manifest_path);
    let entries = load_manifest(manifest_path).map_err(|detail| FuzzFailure {
        campaign: "corpus",
        case: manifest_path.display().to_string(),
        seed,
        mutation_index: 0,
        operation: "load_manifest".into(),
        detail,
    })?;
    let mut stats = CampaignStats {
        campaign: "corpus",
        seed,
        ..CampaignStats::default()
    };

    for (index, entry) in entries.iter().enumerate() {
        let path = corpus_root.join(&entry.path);
        let bytes = fs::read(&path).map_err(|error| FuzzFailure {
            campaign: "corpus",
            case: entry.path.display().to_string(),
            seed,
            mutation_index: index,
            operation: "read_fixture".into(),
            detail: error.to_string(),
        })?;
        let Some(parser) = parser_for_kind(entry) else {
            stats.skipped += 1;
            continue;
        };

        let original = run_parser_once(
            "corpus",
            &entry.path.display().to_string(),
            seed,
            index,
            "original",
            &parser,
            &bytes,
            &mut stats,
        )?;
        match (entry.expect, original.accepted) {
            (Expect::Accept, false) => {
                return Err(FuzzFailure {
                    campaign: "corpus",
                    case: entry.path.display().to_string(),
                    seed,
                    mutation_index: index,
                    operation: "original".into(),
                    detail: original.detail.unwrap_or_else(|| "fixture rejected".into()),
                });
            }
            (Expect::Reject, true) => {
                return Err(FuzzFailure {
                    campaign: "corpus",
                    case: entry.path.display().to_string(),
                    seed,
                    mutation_index: index,
                    operation: "original".into(),
                    detail: "reject fixture unexpectedly accepted".into(),
                });
            }
            _ => {}
        }

        if entry.expect == Expect::Accept {
            if parser.requires_truncation_rejects() {
                run_required_truncation_rejects(
                    "corpus",
                    &entry.path.display().to_string(),
                    seed ^ (index as u64),
                    &parser,
                    &bytes,
                    &mut stats,
                )?;
            }
            run_mutations(
                "corpus",
                &entry.path.display().to_string(),
                seed ^ ((index as u64) << 32),
                mutations,
                &parser,
                &bytes,
                &mut stats,
            )?;
        }
    }

    Ok(stats)
}

fn manifest_root(manifest_path: &Path) -> PathBuf {
    manifest_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn bootstrap_truncation_campaign(seed: u64, stats: &mut CampaignStats) -> Result<(), FuzzFailure> {
    let bytes = MinimalCoveWriter::write_empty_file().map_err(|error| FuzzFailure {
        campaign: stats.campaign,
        case: "minimal-empty.cove".into(),
        seed,
        mutation_index: 0,
        operation: "write_empty_file".into(),
        detail: error.to_string(),
    })?;
    let parser = ParserKind::Cove;
    run_parser_once(
        stats.campaign,
        "minimal-empty.cove",
        seed,
        0,
        "original",
        &parser,
        &bytes,
        stats,
    )?;
    run_required_truncation_rejects(
        stats.campaign,
        "minimal-empty.cove",
        seed,
        &parser,
        &bytes,
        stats,
    )
}

fn parser_seed_campaign(
    seed: u64,
    mutations: usize,
    smoke_only: bool,
    stats: &mut CampaignStats,
) -> Result<(), FuzzFailure> {
    let mut fixtures = parser_seed_fixtures();
    if smoke_only {
        fixtures.truncate(8);
    }
    for (index, fixture) in fixtures.into_iter().enumerate() {
        let bytes = match fs::read(fixture.path) {
            Ok(bytes) => bytes,
            Err(_) => {
                stats.skipped += 1;
                continue;
            }
        };
        run_parser_once(
            stats.campaign,
            fixture.path,
            seed,
            index,
            "original",
            &fixture.parser,
            &bytes,
            stats,
        )?;
        if fixture.parser.requires_truncation_rejects() {
            run_required_truncation_rejects(
                stats.campaign,
                fixture.path,
                seed ^ (index as u64),
                &fixture.parser,
                &bytes,
                stats,
            )?;
        }
        run_mutations(
            stats.campaign,
            fixture.path,
            seed ^ ((index as u64) << 32),
            mutations,
            &fixture.parser,
            &bytes,
            stats,
        )?;
    }
    Ok(())
}

fn encoding_campaign(
    seed: u64,
    mutations: usize,
    stats: &mut CampaignStats,
) -> Result<(), FuzzFailure> {
    run_encoding_parity(seed, stats)?;
    run_page_compression(seed ^ 0x5151_7000, mutations, stats)
}

fn run_encoding_parity(seed: u64, stats: &mut CampaignStats) -> Result<(), FuzzFailure> {
    for bits_per_value in [1u8, 2, 3, 5, 8, 13, 21, 31] {
        let mask = (1u64 << bits_per_value) - 1;
        let values = (0..128)
            .map(|row| ((row as u64) * 37 + 11) & mask)
            .collect::<Vec<_>>();
        let payload = BitPackedPayload::pack(&values, bits_per_value).map_err(|error| {
            fuzz_failure(
                stats.campaign,
                "bit-packed",
                seed,
                bits_per_value as usize,
                "pack",
                error,
            )
        })?;
        assert_encoding("bit-packed", seed, bits_per_value as usize, stats, || {
            assert_parity::<BitPacked>(&payload)
        })?;
    }

    for (index, row_count) in [0u64, 1, 8, 4096].into_iter().enumerate() {
        assert_encoding("constant", seed, index, stats, || {
            assert_parity::<Constant>(&ConstantPayload {
                value: -17,
                row_count,
            })
        })?;
    }

    assert_encoding("delta", seed, 0, stats, || {
        assert_parity::<Delta>(&DeltaPayload {
            base: 100,
            deltas: vec![1, -2, 4, -8, 16],
        })
    })?;
    assert_encoding("frame-of-reference", seed, 0, stats, || {
        assert_parity::<FrameOfReference>(&ForPayload {
            reference: 1_000,
            offsets: vec![0, 1, -1, 10, -10],
        })
    })?;
    assert_encoding("patched-base", seed, 0, stats, || {
        assert_parity::<PatchedBase>(&PatchedBasePayload {
            base: vec![0, 0, 0, 0],
            patches: vec![(1, 10), (3, -20)],
        })
    })?;
    assert_encoding("plain-fixed", seed, 0, stats, || {
        assert_parity::<PlainFixed>(&PlainFixedPayload {
            values: vec![i64::MIN, -1, 0, 1, i64::MAX],
        })
    })?;
    assert_encoding("plain-varint", seed, 0, stats, || {
        assert_parity::<PlainVarint>(&PlainVarintPayload {
            values: vec![i64::MIN, -123, 0, 456, i64::MAX],
        })
    })?;
    assert_encoding("rle", seed, 0, stats, || {
        assert_parity::<Rle>(&RlePayload {
            runs: vec![(7, 3), (-2, 5), (0, 1)],
        })
    })?;
    assert_encoding("run-end", seed, 0, stats, || {
        assert_parity::<RunEnd>(&RunEndPayload {
            values: vec![7, -2, 0],
            run_ends: vec![3, 8, 9],
        })
    })?;
    assert_encoding("sparse", seed, 0, stats, || {
        assert_parity::<Sparse>(&SparsePayload {
            row_count: 6,
            fill: 0,
            overrides: vec![(1, 10), (5, -3)],
        })
    })?;
    assert_encoding("local-codebook-bit-packed", seed, 0, stats, || {
        assert_parity::<LocalCodebook>(&LocalCodebookPayload {
            values: LocalCodebookValues::FileCode(vec![100, 200, 300]),
            indexes: LocalIndexPayload::BitPacked(
                BitPackedPayload::pack(&[0, 1, 2, 1, 0], 2).unwrap(),
            ),
        })
    })?;
    assert_encoding("local-codebook-rle", seed, 0, stats, || {
        assert_parity::<LocalCodebook>(&LocalCodebookPayload {
            values: LocalCodebookValues::NumCode(vec![7, 9]),
            indexes: LocalIndexPayload::Rle(RlePayload {
                runs: vec![(0, 3), (1, 1), (0, 2)],
            }),
        })
    })?;

    Ok(())
}

fn assert_encoding(
    case: &str,
    seed: u64,
    mutation_index: usize,
    stats: &mut CampaignStats,
    f: impl FnOnce() -> Result<(), CoveError>,
) -> Result<(), FuzzFailure> {
    stats.cases_run += 1;
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(fuzz_failure(
            stats.campaign,
            case,
            seed,
            mutation_index,
            "encoding_parity",
            error,
        )),
        Err(_) => Err(FuzzFailure {
            campaign: stats.campaign,
            case: case.into(),
            seed,
            mutation_index,
            operation: "encoding_parity".into(),
            detail: "panic".into(),
        }),
    }
}

fn run_page_compression(
    seed: u64,
    mutations: usize,
    stats: &mut CampaignStats,
) -> Result<(), FuzzFailure> {
    let payload = b"cove page payload fuzz seed data".repeat(8);
    for codec in [
        CompressionCodec::None,
        CompressionCodec::Lz4,
        CompressionCodec::Zstd,
    ] {
        let encoded = encode_page_payload(&payload, codec).map_err(|error| {
            fuzz_failure(
                stats.campaign,
                "page-compression",
                seed,
                codec as usize,
                "encode_page_payload",
                error,
            )
        })?;
        let entry = page_entry(codec, encoded.len() as u64, payload.len() as u64, &encoded);
        let case = format!("page-compression-{codec:?}");
        let parser = ParserKind::PagePayload(entry);
        let original = run_parser_once(
            stats.campaign,
            &case,
            seed,
            codec as usize,
            "original",
            &parser,
            &encoded,
            stats,
        )?;
        if !original.accepted {
            return Err(FuzzFailure {
                campaign: stats.campaign,
                case,
                seed,
                mutation_index: codec as usize,
                operation: "page_round_trip".into(),
                detail: original.detail.unwrap_or_else(|| "page rejected".into()),
            });
        }
        run_mutations(
            stats.campaign,
            &case,
            seed ^ (codec as u64),
            mutations,
            &parser,
            &encoded,
            stats,
        )?;
    }
    Ok(())
}

fn page_entry(
    codec: CompressionCodec,
    page_length: u64,
    uncompressed_length: u64,
    encoded: &[u8],
) -> ColumnPageIndexEntryV1 {
    ColumnPageIndexEntryV1 {
        column_id: 1,
        morsel_id: 1,
        row_count: 1,
        non_null_count: 1,
        null_count: 0,
        encoding_root: 0,
        page_offset: 0,
        page_length,
        uncompressed_length,
        stats_ref: u32::MAX,
        flags: codec as u32,
        checksum: checksum::crc32c(encoded),
    }
}

fn run_required_truncation_rejects(
    campaign: &'static str,
    case: &str,
    seed: u64,
    parser: &ParserKind,
    bytes: &[u8],
    stats: &mut CampaignStats,
) -> Result<(), FuzzFailure> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut lengths = BTreeSet::new();
    lengths.insert(0usize);
    lengths.insert(bytes.len() / 2);
    lengths.insert(bytes.len().saturating_sub(1));
    for (index, len) in lengths.into_iter().enumerate() {
        if len >= bytes.len() {
            continue;
        }
        let result = run_parser_once(
            campaign,
            case,
            seed,
            index,
            &format!("truncate:{len}"),
            parser,
            &bytes[..len],
            stats,
        )?;
        if result.accepted {
            return Err(FuzzFailure {
                campaign,
                case: case.into(),
                seed,
                mutation_index: index,
                operation: format!("truncate:{len}"),
                detail: "truncated bytes unexpectedly accepted".into(),
            });
        }
    }
    Ok(())
}

fn run_mutations(
    campaign: &'static str,
    case: &str,
    seed: u64,
    mutations: usize,
    parser: &ParserKind,
    bytes: &[u8],
    stats: &mut CampaignStats,
) -> Result<(), FuzzFailure> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut mutator = Mutator::new(seed);
    for index in 0..mutations {
        let op = mutator.next_op(bytes);
        let mutated = op.apply(bytes);
        run_parser_once(
            campaign,
            case,
            seed,
            index,
            &op.to_string(),
            parser,
            &mutated,
            stats,
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ParserOutcome {
    accepted: bool,
    detail: Option<String>,
}

fn run_parser_once(
    campaign: &'static str,
    case: &str,
    seed: u64,
    mutation_index: usize,
    operation: &str,
    parser: &ParserKind,
    bytes: &[u8],
    stats: &mut CampaignStats,
) -> Result<ParserOutcome, FuzzFailure> {
    stats.cases_run += 1;
    match catch_unwind(AssertUnwindSafe(|| parser.parse(bytes))) {
        Ok(Ok(())) => {
            stats.accepted_mutations += usize::from(operation != "original");
            Ok(ParserOutcome {
                accepted: true,
                detail: None,
            })
        }
        Ok(Err(error)) => {
            stats.rejects_observed += 1;
            Ok(ParserOutcome {
                accepted: false,
                detail: Some(error),
            })
        }
        Err(_) => Err(FuzzFailure {
            campaign,
            case: case.into(),
            seed,
            mutation_index,
            operation: operation.into(),
            detail: "panic".into(),
        }),
    }
}

fn fuzz_failure(
    campaign: &'static str,
    case: impl Into<String>,
    seed: u64,
    mutation_index: usize,
    operation: impl Into<String>,
    error: impl std::fmt::Display,
) -> FuzzFailure {
    FuzzFailure {
        campaign,
        case: case.into(),
        seed,
        mutation_index,
        operation: operation.into(),
        detail: error.to_string(),
    }
}

#[derive(Debug, Clone)]
enum ParserKind {
    Cove,
    Covemap,
    Covx,
    Covm,
    Covi,
    CodecDescriptors,
    CodecEnvelopes,
    SectionFeatureBinding,
    FastMetadataIndex,
    PageClusterDirectory,
    LayoutPlan,
    ScanSplitIndex,
    ZeroCopyMap,
    RuntimeHints,
    CoverageProviders,
    CoverageSet,
    CoverageProofRecords,
    PredicateNormalForm,
    IntervalPredicate,
    CoveragePlanCandidates,
    IndexCapabilities,
    IndexOnlyCapabilities,
    Cache,
    MetadataJson,
    FileDictionary,
    CollationRegistry,
    DigestManifest,
    RedactionManifest,
    IoHints,
    LakehouseHints,
    KernelCapabilities,
    PageIndex,
    ColumnDomain,
    TableCatalog,
    TableSegmentIndex,
    TableSegmentHeader,
    RowMorselDirectory(u32),
    ExactSetIndex,
    BloomIndex,
    InvertedMorselIndex,
    LookupIndex,
    RowRef,
    AggregateSynopsis,
    CompositeIndex,
    TopNSummary,
    SortKey,
    ClusteringKey,
    EngineProfileRegistry,
    ExecutionCodeDescriptor,
    ExecutionScopeDescriptor,
    CodeSpaceDescriptor,
    EngineMountPolicy,
    HarborMountHints,
    ObjectTypeCatalog,
    TemporalSegmentIndex,
    TemporalBloomIndex,
    ExtensionRegistry,
    ExtensionLogicalType { collation_count: Option<usize> },
    ExtensionIndexDescriptor,
    PagePayload(ColumnPageIndexEntryV1),
}

impl ParserKind {
    fn requires_truncation_rejects(&self) -> bool {
        !matches!(
            self,
            Self::CodecDescriptors
                | Self::CodecEnvelopes
                | Self::RuntimeHints
                | Self::CoverageProviders
                | Self::CoverageProofRecords
                | Self::PredicateNormalForm
                | Self::IntervalPredicate
                | Self::CoveragePlanCandidates
                | Self::IndexCapabilities
                | Self::IndexOnlyCapabilities
                | Self::MetadataJson
                | Self::CollationRegistry
                | Self::TableCatalog
                | Self::TableSegmentIndex
        )
    }

    fn parse(&self, bytes: &[u8]) -> Result<(), String> {
        match self {
            Self::Cove => reader::validate_bytes_with_options(
                bytes,
                ValidationOptions {
                    semantic: true,
                    verify_digests: false,
                    allow_unknown_optional_extensions: true,
                    ..ValidationOptions::default()
                },
            )
            .map(|_| ())
            .map_err(to_string),
            Self::Covemap => CovemapFile::parse(bytes)
                .and_then(|file| file.validate_map_sections())
                .map_err(to_string),
            Self::Covx => CovxFile::parse(bytes).map(|_| ()).map_err(to_string),
            Self::Covm => CovmFile::parse(bytes).map(|_| ()).map_err(to_string),
            Self::Covi => CoviArtifactV2::parse(bytes).map(|_| ()).map_err(to_string),
            Self::CodecDescriptors => CodecExtensionDescriptorV2::parse_many(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::CodecEnvelopes => RegisteredEncodingEnvelopeV2::parse_many(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::SectionFeatureBinding => SectionFeatureBindingSectionV2::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::FastMetadataIndex => FastMetadataIndexV2::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::PageClusterDirectory => PageClusterDirectoryV2::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::LayoutPlan => LayoutPlanV2::parse(bytes).map(|_| ()).map_err(to_string),
            Self::ScanSplitIndex => ScanSplitIndexV2::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::ZeroCopyMap => ZeroCopyBufferMapV2::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::RuntimeHints => RuntimeCompatibilityHintV2::parse_many(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::CoverageProviders => CoverageProviderDescriptorV2::parse_many(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::CoverageSet => CoverageSetV2::parse(bytes).map(|_| ()).map_err(to_string),
            Self::CoverageProofRecords => CoverageProofRecordV2::parse_many(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::PredicateNormalForm => PredicateNormalFormV2::parse_many(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::IntervalPredicate => IntervalPredicateV2::parse_many(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::CoveragePlanCandidates => CoveragePlanCandidateV2::parse_many(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::IndexCapabilities => IndexCapabilityV2::parse_many(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::IndexOnlyCapabilities => IndexOnlyCapabilityV2::parse_many(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::Cache => CoverageCacheV2::parse(bytes).map(|_| ()).map_err(to_string),
            Self::MetadataJson => MetadataJson::parse(bytes).map(|_| ()).map_err(to_string),
            Self::FileDictionary => validate_file_dictionary_fixture(bytes).map_err(to_string),
            Self::CollationRegistry => CollationRegistry::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::DigestManifest => DigestManifest::parse(bytes).map(|_| ()).map_err(to_string),
            Self::RedactionManifest => RedactionManifest::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::IoHints => IoHints::parse(bytes).map(|_| ()).map_err(to_string),
            Self::LakehouseHints => LakehouseHints::parse(bytes).map(|_| ()).map_err(to_string),
            Self::KernelCapabilities => KernelCapabilities::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::PageIndex => PageIndex::parse(bytes).map(|_| ()).map_err(to_string),
            Self::ColumnDomain => ColumnDomain::parse(bytes).map(|_| ()).map_err(to_string),
            Self::TableCatalog => TableCatalog::parse(bytes).map(|_| ()).map_err(to_string),
            Self::TableSegmentIndex => TableSegmentIndex::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::TableSegmentHeader => TableSegmentHeaderV1::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::RowMorselDirectory(morsel_count) => {
                RowMorselDirectory::parse(bytes, *morsel_count)
                    .map(|_| ())
                    .map_err(to_string)
            }
            Self::ExactSetIndex => ExactSetIndex::parse(bytes).map(|_| ()).map_err(to_string),
            Self::BloomIndex => BloomFilterIndex::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::InvertedMorselIndex => InvertedMorselIndex::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::LookupIndex => LookupIndex::parse(bytes).map(|_| ()).map_err(to_string),
            Self::RowRef => RowRef::decode(bytes).map(|_| ()).map_err(to_string),
            Self::AggregateSynopsis => AggregateSynopsis::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::CompositeIndex => CompositeIndex::parse(bytes).map(|_| ()).map_err(to_string),
            Self::TopNSummary => TopNSummary::parse(bytes).map(|_| ()).map_err(to_string),
            Self::SortKey => SortKeyEntryV1::parse(bytes).map(|_| ()).map_err(to_string),
            Self::ClusteringKey => ClusteringKeyEntryV1::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::EngineProfileRegistry => EngineProfileRegistry::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::ExecutionCodeDescriptor => ExecutionCodeDescriptorV1::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::ExecutionScopeDescriptor => ExecutionScopeDescriptorV1::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::CodeSpaceDescriptor => CodeSpaceDescriptorV1::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::EngineMountPolicy => EngineMountPolicyV1::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::HarborMountHints => HarborMountHintsV1::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::ObjectTypeCatalog => ObjectTypeCatalog::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::TemporalSegmentIndex => TemporalSegmentIndex::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::TemporalBloomIndex => TemporalBloomIndex::parse(bytes)
                .map(|_| ())
                .map_err(to_string),
            Self::ExtensionRegistry => ExtensionRegistry::parse(bytes)
                .and_then(|registry| registry.validate_known(true))
                .map_err(to_string),
            Self::ExtensionLogicalType { collation_count } => ExtensionLogicalTypeV1::parse(bytes)
                .and_then(|descriptor| {
                    descriptor.validate(ExtensionValidationContext {
                        collation_count: *collation_count,
                    })
                })
                .map_err(to_string),
            Self::ExtensionIndexDescriptor => ExtensionIndexDescriptorV1::parse(bytes)
                .and_then(|descriptor| {
                    descriptor.validate()?;
                    if descriptor.can_skip_data() {
                        Ok(())
                    } else {
                        Ok(())
                    }
                })
                .map_err(to_string),
            Self::PagePayload(entry) => column_page_payload(bytes, &entry)
                .map(|payload| {
                    if payload.len() == entry.uncompressed_length as usize {
                        ()
                    }
                })
                .map_err(to_string),
        }
    }
}

fn to_string(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn validate_file_dictionary_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    if bytes.len() < 4 {
        return Err(CoveError::BufferTooShort);
    }
    let index_len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let split = 4usize
        .checked_add(index_len)
        .ok_or(CoveError::ArithOverflow)?;
    if split > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    FileDictionary::parse(&bytes[4..split], &bytes[split..]).map(|_| ())
}

#[derive(Debug, Clone)]
struct ParserSeedFixture {
    path: &'static str,
    parser: ParserKind,
}

fn parser_seed_fixtures() -> Vec<ParserSeedFixture> {
    vec![
        ParserSeedFixture {
            path: "conformance/accept/min_empty.cove",
            parser: ParserKind::Cove,
        },
        ParserSeedFixture {
            path: "conformance/accept/covemap_valid.covemap",
            parser: ParserKind::Covemap,
        },
        ParserSeedFixture {
            path: "conformance/accept/covx_valid.covx",
            parser: ParserKind::Covx,
        },
        ParserSeedFixture {
            path: "conformance/accept/covm_valid.covm",
            parser: ParserKind::Covm,
        },
        ParserSeedFixture {
            path: "conformance/covi/single_section_valid.covi",
            parser: ParserKind::Covi,
        },
        ParserSeedFixture {
            path: "conformance/accept/file_dictionary_valid.bin",
            parser: ParserKind::FileDictionary,
        },
        ParserSeedFixture {
            path: "conformance/accept/column_domain_valid.bin",
            parser: ParserKind::ColumnDomain,
        },
        ParserSeedFixture {
            path: "conformance/accept/page_index_valid.bin",
            parser: ParserKind::PageIndex,
        },
        ParserSeedFixture {
            path: "conformance/feature-scope/section_feature_binding_valid.bin",
            parser: ParserKind::SectionFeatureBinding,
        },
        ParserSeedFixture {
            path: "conformance/codecs/codec_descriptor_valid.bin",
            parser: ParserKind::CodecDescriptors,
        },
        ParserSeedFixture {
            path: "conformance/codecs/registered_encoding_envelope_valid.bin",
            parser: ParserKind::CodecEnvelopes,
        },
        ParserSeedFixture {
            path: "conformance/coverage/provider_registry_valid.bin",
            parser: ParserKind::CoverageProviders,
        },
        ParserSeedFixture {
            path: "conformance/coverage/coverage_set_valid.bin",
            parser: ParserKind::CoverageSet,
        },
        ParserSeedFixture {
            path: "conformance/coverage/coverage_proof_record_valid.bin",
            parser: ParserKind::CoverageProofRecords,
        },
        ParserSeedFixture {
            path: "conformance/covi/index_capability_valid.bin",
            parser: ParserKind::IndexCapabilities,
        },
        ParserSeedFixture {
            path: "conformance/covi/index_only_capability_valid.bin",
            parser: ParserKind::IndexOnlyCapabilities,
        },
        ParserSeedFixture {
            path: "conformance/cache/cache_valid.bin",
            parser: ParserKind::Cache,
        },
        ParserSeedFixture {
            path: "conformance/runtime/runtime_hint_valid.bin",
            parser: ParserKind::RuntimeHints,
        },
        ParserSeedFixture {
            path: "conformance/layout/layout_plan_valid.bin",
            parser: ParserKind::LayoutPlan,
        },
        ParserSeedFixture {
            path: "conformance/layout/fast_metadata_index_valid.bin",
            parser: ParserKind::FastMetadataIndex,
        },
        ParserSeedFixture {
            path: "conformance/layout/page_cluster_directory_valid.bin",
            parser: ParserKind::PageClusterDirectory,
        },
        ParserSeedFixture {
            path: "conformance/layout/scan_split_index_valid.bin",
            parser: ParserKind::ScanSplitIndex,
        },
        ParserSeedFixture {
            path: "conformance/zerocopy/zero_copy_map_valid.bin",
            parser: ParserKind::ZeroCopyMap,
        },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Accept,
    Reject,
}

#[derive(Debug, Clone)]
struct ManifestEntry {
    path: PathBuf,
    kind: String,
    expect: Expect,
    morsel_count: Option<u32>,
    raw: Value,
}

fn load_manifest(path: &Path) -> Result<Vec<ManifestEntry>, String> {
    let contents =
        fs::read_to_string(path).map_err(|error| format!("cannot read manifest: {error}"))?;
    let mut entries = Vec::new();
    for (line_number, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let raw: Value = serde_json::from_str(line)
            .map_err(|error| format!("invalid manifest line {}: {error}", line_number + 1))?;
        let path = raw
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("manifest line {} missing path", line_number + 1))?;
        let expect = match raw.get("expect").and_then(Value::as_str) {
            Some("accept") => Expect::Accept,
            Some("reject") => Expect::Reject,
            other => {
                return Err(format!(
                    "manifest line {} has invalid expect {:?}",
                    line_number + 1,
                    other
                ));
            }
        };
        let kind = raw
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("cove")
            .to_string();
        let morsel_count = raw
            .get("morsel_count")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        entries.push(ManifestEntry {
            path: PathBuf::from(path),
            kind,
            expect,
            morsel_count,
            raw,
        });
    }
    Ok(entries)
}

fn parser_for_kind(entry: &ManifestEntry) -> Option<ParserKind> {
    let kind = match entry.kind.as_str() {
        "cove" => ParserKind::Cove,
        "covemap" => ParserKind::Covemap,
        "covx" => ParserKind::Covx,
        "covm" => ParserKind::Covm,
        "covi" => ParserKind::Covi,
        "cove_codec_descriptors" => ParserKind::CodecDescriptors,
        "cove_codec_envelopes" => ParserKind::CodecEnvelopes,
        "section_feature_binding" => ParserKind::SectionFeatureBinding,
        "fast_metadata_index" => ParserKind::FastMetadataIndex,
        "page_cluster_directory" => ParserKind::PageClusterDirectory,
        "cove_layout_plan" => ParserKind::LayoutPlan,
        "cove_layout_scan_split" => ParserKind::ScanSplitIndex,
        "zero_copy_map" => ParserKind::ZeroCopyMap,
        "cove_runtime_hints" => ParserKind::RuntimeHints,
        "cove_coverage_providers" => ParserKind::CoverageProviders,
        "cove_coverage_set" => ParserKind::CoverageSet,
        "coverage_proof_records" => ParserKind::CoverageProofRecords,
        "predicate_normal_form" => ParserKind::PredicateNormalForm,
        "interval_predicate" => ParserKind::IntervalPredicate,
        "coverage_plan_candidates" => ParserKind::CoveragePlanCandidates,
        "cove_index_capabilities" => ParserKind::IndexCapabilities,
        "cove_index_only_capabilities" => ParserKind::IndexOnlyCapabilities,
        "cove_cache" => ParserKind::Cache,
        "metadata_json" => ParserKind::MetadataJson,
        "file_dictionary" => ParserKind::FileDictionary,
        "collation_registry" => ParserKind::CollationRegistry,
        "digest_manifest" => ParserKind::DigestManifest,
        "redaction_manifest" => ParserKind::RedactionManifest,
        "io_hints" => ParserKind::IoHints,
        "lakehouse_hints" => ParserKind::LakehouseHints,
        "kernel_capabilities" => ParserKind::KernelCapabilities,
        "page_index" => ParserKind::PageIndex,
        "column_domain" => ParserKind::ColumnDomain,
        "table_catalog" => ParserKind::TableCatalog,
        "table_segment_index" => ParserKind::TableSegmentIndex,
        "table_segment_header" => ParserKind::TableSegmentHeader,
        "row_morsel_directory" => ParserKind::RowMorselDirectory(entry.morsel_count?),
        "exact_set_index" => ParserKind::ExactSetIndex,
        "bloom_index" => ParserKind::BloomIndex,
        "inverted_morsel_index" => ParserKind::InvertedMorselIndex,
        "lookup_index" => ParserKind::LookupIndex,
        "row_ref" => ParserKind::RowRef,
        "aggregate_synopsis" => ParserKind::AggregateSynopsis,
        "composite_zone_index" => ParserKind::CompositeIndex,
        "topn_summary" => ParserKind::TopNSummary,
        "sort_key" => ParserKind::SortKey,
        "clustering_key" => ParserKind::ClusteringKey,
        "cove_e_engine_registry" => ParserKind::EngineProfileRegistry,
        "cove_e_execution_code" => ParserKind::ExecutionCodeDescriptor,
        "cove_e_execution_scope" => ParserKind::ExecutionScopeDescriptor,
        "cove_e_code_space" => ParserKind::CodeSpaceDescriptor,
        "cove_e_mount_policy" => ParserKind::EngineMountPolicy,
        "cove_h_mount_hints" => ParserKind::HarborMountHints,
        "cove_o_object_catalog" => ParserKind::ObjectTypeCatalog,
        "cove_o_temporal_segment_index" => ParserKind::TemporalSegmentIndex,
        "cove_o_temporal_bloom_index" => ParserKind::TemporalBloomIndex,
        "extension_registry" => ParserKind::ExtensionRegistry,
        "extension_logical_type" => ParserKind::ExtensionLogicalType {
            collation_count: entry
                .raw
                .get("collation_count")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
        },
        "extension_index_descriptor" => ParserKind::ExtensionIndexDescriptor,
        _ => return None,
    };
    Some(kind)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mutator {
    state: u64,
}

impl Mutator {
    fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 7;
        x ^= x >> 9;
        x ^= x << 8;
        self.state = x;
        x
    }

    fn next_op(&mut self, bytes: &[u8]) -> MutationOp {
        let len = bytes.len().max(1);
        match self.next_u64() % 3 {
            0 => MutationOp::FlipBit {
                offset: (self.next_u64() as usize) % len,
                bit: 1u8 << (self.next_u64() % 8),
            },
            1 => MutationOp::SetByte {
                offset: (self.next_u64() as usize) % len,
                value: self.next_u64() as u8,
            },
            _ => MutationOp::SpliceByte {
                offset: (self.next_u64() as usize) % len,
                value: self.next_u64() as u8,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MutationOp {
    FlipBit { offset: usize, bit: u8 },
    SetByte { offset: usize, value: u8 },
    SpliceByte { offset: usize, value: u8 },
}

impl MutationOp {
    fn apply(&self, bytes: &[u8]) -> Vec<u8> {
        let mut out = bytes.to_vec();
        match *self {
            Self::FlipBit { offset, bit } => out[offset] ^= bit,
            Self::SetByte { offset, value } => out[offset] = value,
            Self::SpliceByte { offset, value } => out.insert(offset, value),
        }
        out
    }
}

impl std::fmt::Display for MutationOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FlipBit { offset, bit } => write!(f, "flip_bit:{offset}:{bit:#04x}"),
            Self::SetByte { offset, value } => write!(f, "set_byte:{offset}:{value:#04x}"),
            Self::SpliceByte { offset, value } => write!(f, "splice_byte:{offset}:{value:#04x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_smoke_command() {
        assert_eq!(
            parse_args(Vec::<String>::new()).unwrap(),
            Command::Smoke {
                seed: DEFAULT_SEED,
                mutations: SMOKE_MUTATIONS,
            }
        );
    }

    #[test]
    fn parses_corpus_manifest_and_seed() {
        assert_eq!(
            parse_args([
                "corpus".to_string(),
                "conformance/manifest.jsonl".to_string(),
                "--seed".to_string(),
                "0x10".to_string(),
                "--mutations".to_string(),
                "2".to_string(),
            ])
            .unwrap(),
            Command::Corpus {
                manifest: PathBuf::from("conformance/manifest.jsonl"),
                seed: 16,
                mutations: 2,
            }
        );
    }

    #[test]
    fn rejects_unknown_subcommand() {
        assert!(parse_args(["unknown".to_string()]).is_err());
    }

    #[test]
    fn deterministic_seed_replays_mutations() {
        let bytes = b"abcdef";
        let mut a = Mutator::new(123);
        let mut b = Mutator::new(123);
        for _ in 0..16 {
            let op_a = a.next_op(bytes);
            let op_b = b.next_op(bytes);
            assert_eq!(op_a, op_b);
            assert_eq!(op_a.apply(bytes), op_b.apply(bytes));
        }
    }

    #[test]
    fn panic_capture_reports_failure() {
        let mut stats = CampaignStats {
            campaign: "test",
            seed: 1,
            ..CampaignStats::default()
        };
        let result = catch_parser_for_test(
            || -> Result<(), String> {
                panic!("intentional panic");
            },
            &mut stats,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().detail.contains("panic"));
    }

    #[test]
    fn parser_reject_is_not_harness_failure() {
        let mut stats = CampaignStats {
            campaign: "test",
            seed: 1,
            ..CampaignStats::default()
        };
        let result = catch_parser_for_test(|| Err("expected reject".into()), &mut stats).unwrap();
        assert!(!result.accepted);
        assert_eq!(stats.rejects_observed, 1);
    }

    #[test]
    fn manifest_root_uses_parent_directory() {
        assert_eq!(
            manifest_root(Path::new("conformance/manifest.jsonl")),
            PathBuf::from("conformance")
        );
        assert_eq!(
            manifest_root(Path::new("manifest.jsonl")),
            PathBuf::from(".")
        );
    }

    fn catch_parser_for_test(
        f: impl FnOnce() -> Result<(), String>,
        stats: &mut CampaignStats,
    ) -> Result<ParserOutcome, FuzzFailure> {
        stats.cases_run += 1;
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(Ok(())) => Ok(ParserOutcome {
                accepted: true,
                detail: None,
            }),
            Ok(Err(error)) => {
                stats.rejects_observed += 1;
                Ok(ParserOutcome {
                    accepted: false,
                    detail: Some(error),
                })
            }
            Err(_) => Err(FuzzFailure {
                campaign: stats.campaign,
                case: "test".into(),
                seed: stats.seed,
                mutation_index: 0,
                operation: "test".into(),
                detail: "panic".into(),
            }),
        }
    }
}
