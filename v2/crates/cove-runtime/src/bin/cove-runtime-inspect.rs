use std::{env, fs, process};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: cove-runtime-inspect <runtime-hints-section.bin>");
        process::exit(2);
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{path}: {error}");
            process::exit(1);
        }
    };
    match cove_runtime::RuntimeCompatibilityHintV2::parse_many(&bytes) {
        Ok(hints) => {
            println!("valid COVE-R runtime hints: {} hints", hints.len());
            for hint in hints {
                println!(
                    "hint_id={} kind={:?} required={} {}::{} v{}.{}",
                    hint.hint_id,
                    hint.hint_kind,
                    hint.required,
                    hint.namespace,
                    hint.name,
                    hint.version_major,
                    hint.version_minor
                );
            }
        }
        Err(error) => {
            eprintln!("{path}: {error}");
            process::exit(1);
        }
    }
}
