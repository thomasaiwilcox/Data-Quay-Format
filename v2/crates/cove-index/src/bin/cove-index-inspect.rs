use std::{env, fs, process};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: cove-index-inspect <index.covi>");
        process::exit(2);
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{path}: {error}");
            process::exit(1);
        }
    };
    if bytes.len() >= 4 && bytes[bytes.len() - 4..] == *b"CVI2" {
        inspect_artifact(&path, &bytes);
        return;
    }
    if let Ok(capabilities) = cove_index::IndexCapabilityV2::parse_many(&bytes) {
        println!(
            "valid COVE-I index capability section: {} capabilities",
            capabilities.len()
        );
        return;
    }
    if let Ok(capabilities) = cove_index::IndexOnlyCapabilityV2::parse_many(&bytes) {
        println!(
            "valid COVE-I index-only capability section: {} capabilities",
            capabilities.len()
        );
        return;
    }
    inspect_artifact(&path, &bytes);
}

fn inspect_artifact(path: &str, bytes: &[u8]) {
    match cove_index::CoviArtifactV2::parse(bytes) {
        Ok(artifact) => {
            println!(
                "valid COVE-I artifact: sections={} roots={} files={} capabilities={} key_blocks={} entry_blocks={} postings_blocks={}",
                artifact.sections.len(),
                artifact.header.index_root_count,
                artifact.header.referenced_file_count,
                artifact.header.capability_count,
                artifact.key_blocks.len(),
                artifact.entry_blocks.len(),
                artifact.postings_blocks.len()
            );
        }
        Err(error) => {
            eprintln!("{path}: {error}");
            process::exit(1);
        }
    }
}
