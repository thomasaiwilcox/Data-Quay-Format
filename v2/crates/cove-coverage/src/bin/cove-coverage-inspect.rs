use std::{env, fs, process};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: cove-coverage-inspect <coverage-section.bin>");
        process::exit(2);
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{path}: {error}");
            process::exit(1);
        }
    };
    if let Ok(providers) = cove_coverage::CoverageProviderDescriptorV2::parse_many(&bytes) {
        println!(
            "valid COVE-COVERAGE provider registry: {} providers",
            providers.len()
        );
        return;
    }
    if let Ok(set) = cove_coverage::CoverageSetV2::parse(&bytes) {
        println!(
            "valid COVE-COVERAGE set: id={} provider={} entries={} pruning_safe={}",
            set.header.coverage_set_id,
            set.header.provider_id,
            set.entries.len(),
            cove_coverage::can_use_for_pruning(&set.header)
        );
        return;
    }
    if let Ok(records) = cove_coverage::CoverageProofRecordV2::parse_many(&bytes) {
        println!(
            "valid COVE-COVERAGE proof records: {} pruning_safe={}",
            records.len(),
            records.iter().all(cove_coverage::can_use_proof_for_pruning)
        );
        return;
    }
    if let Ok(candidates) = cove_coverage::CoveragePlanCandidateV2::parse_many(&bytes) {
        println!("valid COVE-COVERAGE plan candidates: {}", candidates.len());
        return;
    }
    if let Ok(forms) = cove_coverage::PredicateNormalFormV2::parse_many(&bytes) {
        println!("valid COVE-COVERAGE predicate forms: {}", forms.len());
        return;
    }
    match cove_coverage::IntervalPredicateV2::parse_many(&bytes) {
        Ok(intervals) => {
            println!(
                "valid COVE-COVERAGE interval predicates: {}",
                intervals.len()
            );
        }
        Err(error) => {
            eprintln!(
                "{path}: not a valid provider registry, coverage set, proof record, predicate form, interval predicate, or plan candidate: {error}"
            );
            process::exit(1);
        }
    }
}
