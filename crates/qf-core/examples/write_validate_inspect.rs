//! End-to-end showcase: write a minimal QF file, validate it, then inspect
//! it via the `reader` API. This example demonstrates the canonical
//! qf-core authoring + verification loop.
//!
//! Run with:
//! ```text
//! cargo run -p qf-core --example write_validate_inspect
//! ```

use qf_core::{
    durable,
    reader::{self, ValidationOptions},
    writer::MinimalQfWriter,
};

fn main() {
    // 1. Build an empty but structurally valid QF-T file.
    let bytes = MinimalQfWriter::write_empty_file();
    println!("wrote {} bytes", bytes.len());

    // 2. Publish via Spec §74 durable-replace into a temp dir.
    let dir = std::env::temp_dir();
    let path = dir.join("qf-core-example.quay");
    durable::durable_replace(&path, &bytes).expect("durable_replace");
    println!("published to {}", path.display());

    // 3. Re-read and validate semantically.
    let read_back = std::fs::read(&path).expect("read back");
    let report = reader::validate_bytes_with_options(
        &read_back,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
        },
    )
    .expect("validation");

    let info = report.validated;
    println!(
        "validated: QF v{}.{} profile={} sections={}",
        info.header.version_major,
        info.header.version_minor,
        info.header.primary_profile,
        info.footer.sections.len(),
    );

    // 4. Clean up.
    let _ = std::fs::remove_file(&path);
}
