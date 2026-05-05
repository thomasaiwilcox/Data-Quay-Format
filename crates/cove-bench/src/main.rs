//! `cove-bench` — Cove Format reference benchmark harness (Spec §78).
//!
//! Smoke-level timing for the operations Spec §78 calls out as benchmark
//! categories. Real numbers belong in dedicated benchmark crates (criterion);
//! this binary just exercises the codepaths in a loop and reports wall time
//! so CI can detect regressions.

use std::time::Instant;

use cove_core::{canonical::CanonicalValue, checksum, interop::arrow};

fn main() {
    bench_crc32c();
    bench_canonical_int();
    bench_arrow_inversion();
}

fn bench_crc32c() {
    let payload = vec![0xABu8; 1 << 20]; // 1 MiB
    let iters = 64;
    let start = Instant::now();
    let mut sink = 0u32;
    for _ in 0..iters {
        sink ^= checksum::crc32c(&payload);
    }
    let elapsed = start.elapsed();
    let mb = (iters as f64) * (payload.len() as f64) / (1024.0 * 1024.0);
    println!(
        "crc32c: {:.1} MiB in {:.3?} -> {:.1} MiB/s (sink={sink:08x})",
        mb,
        elapsed,
        mb / elapsed.as_secs_f64()
    );
}

fn bench_canonical_int() {
    let iters = 100_000;
    let start = Instant::now();
    let mut total = 0usize;
    for i in 0..iters {
        let v = CanonicalValue::Int {
            width: 8,
            value: i as i128,
        };
        let bytes = v.encode().expect("canonical int encoding");
        total += bytes.len();
    }
    let elapsed = start.elapsed();
    println!(
        "canonical_int: {iters} encodes in {:.3?} ({} bytes total)",
        elapsed, total
    );
}

fn bench_arrow_inversion() {
    let row_count = 1 << 20; // 1 Mi rows
    let cove_null = vec![0xA5u8; (row_count + 7) / 8];
    let iters = 16;
    let start = Instant::now();
    let mut sink = 0u8;
    for _ in 0..iters {
        let v = arrow::cove_null_to_arrow_validity(&cove_null, row_count)
            .expect("arrow validity conversion");
        sink ^= v[0];
    }
    let elapsed = start.elapsed();
    let mrows = (iters as f64) * (row_count as f64) / 1.0e6;
    println!(
        "arrow_inversion: {:.1} Mrows in {:.3?} -> {:.1} Mrows/s (sink={sink:02x})",
        mrows,
        elapsed,
        mrows / elapsed.as_secs_f64()
    );
}
