use std::{env, fs, process};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: cove-build-covi <output.covi>");
        process::exit(2);
    };
    let artifact = cove_index::CoviArtifactV2::new_empty([0u8; 16], [0u8; 16]);
    let bytes = match artifact.serialize_empty() {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("failed to build empty COVE-I artifact: {error}");
            process::exit(1);
        }
    };
    if let Err(error) = fs::write(&path, bytes) {
        eprintln!("{path}: {error}");
        process::exit(1);
    }
    println!("wrote empty COVE-I artifact to {path}");
}
