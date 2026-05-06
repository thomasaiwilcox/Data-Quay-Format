//! End-to-end showcase: write a minimal COVE file, validate it, then inspect
//! it via the `reader` API. This example demonstrates the canonical
//! cove-core authoring + verification loop.
//!
//! Run with:
//! ```text
//! cargo run -p cove-core --example write_validate_inspect
//! ```

use cove_core::{
    reader::{self, ValidationOptions},
    writer::MinimalCoveWriter,
};

fn main() {
    // 1. Build an empty but structurally valid COVE-T file.
    let writer = MinimalCoveWriter::new();
    let bytes = writer.write();
    println!("wrote {} bytes", bytes.len());

    // 2. Publish via Spec §75 durable-replace into a temp dir.
    let dir = std::env::temp_dir();
    let path = dir.join("cove-core-example.cove");
    writer.publish_durable(&path).expect("publish_durable");
    println!("published to {}", path.display());

    // 3. Re-read and validate semantically.
    let read_back = std::fs::read(&path).expect("read back");
    let report = reader::validate_bytes_with_options(
        &read_back,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        },
    )
    .expect("validation");

    let info = report.validated;
    println!(
        "validated: COVE v{}.{} profile={} sections={}",
        info.header.version_major,
        info.header.version_minor,
        info.header.primary_profile,
        info.footer.sections.len(),
    );

    // 4. Clean up.
    let _ = std::fs::remove_file(&path);
}
