use std::{env, fs, process};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: cove-cache-inspect <coverage-cache.bin>");
        process::exit(2);
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{path}: {error}");
            process::exit(1);
        }
    };
    match cove_cache::CoverageCacheV2::parse(&bytes) {
        Ok(cache) => {
            println!(
                "valid COVE-CACHE diagnostic record: entries={} version={}.{}",
                cache.entries.len(),
                cache.header.cache_format_version_major,
                cache.header.cache_format_version_minor
            );
        }
        Err(error) => {
            eprintln!("{path}: {error}");
            process::exit(1);
        }
    }
}
