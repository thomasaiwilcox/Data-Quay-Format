use std::panic::{catch_unwind, AssertUnwindSafe};

use qf_core::{
    constants::QfLogicalType,
    domain::ColumnDomain,
    encoding::{
        assert_parity,
        bit_packed::{BitPacked, BitPackedPayload},
        constant::{Constant, ConstantPayload},
        delta::{Delta, DeltaPayload},
        frame_of_reference::{ForPayload, FrameOfReference},
        plain::{PlainFixed, PlainFixedPayload, PlainVarint, PlainVarintPayload},
        rle::{Rle, RlePayload},
        run_end::{RunEnd, RunEndPayload},
        sparse::{Sparse, SparsePayload},
    },
    reader::{validate_bytes_with_options, ValidationOptions},
    writer::MinimalQfWriter,
};

#[test]
fn bootstrap_truncation_campaign_never_panics() {
    let bytes = MinimalQfWriter::write_empty_file();
    let opts = ValidationOptions {
        semantic: true,
        verify_digests: false,
        allow_unknown_optional_extensions: true,
    };

    assert!(validate_bytes_with_options(&bytes, opts.clone()).is_ok());
    for len in 0..bytes.len() {
        let result = catch_unwind(AssertUnwindSafe(|| {
            validate_bytes_with_options(&bytes[..len], opts.clone())
        }));
        assert!(
            result.is_ok(),
            "validation panicked for truncation len {len}"
        );
        assert!(
            result.unwrap().is_err(),
            "truncation len {len} unexpectedly validated"
        );
    }
}

#[test]
fn column_domain_single_byte_mutation_campaign_never_panics() {
    let domain = ColumnDomain::from_sorted_present_codes(
        &[1, 3, 4],
        6,
        1,
        2,
        QfLogicalType::Utf8 as u16,
        0,
        0,
    )
    .unwrap();
    let bytes = domain.serialize().unwrap();
    assert!(ColumnDomain::parse(&bytes).unwrap().validate().is_ok());

    for index in 0..bytes.len() {
        for bit in [0x01u8, 0x80] {
            let mut mutated = bytes.clone();
            mutated[index] ^= bit;
            let result = catch_unwind(AssertUnwindSafe(|| ColumnDomain::parse(&mutated)));
            assert!(
                result.is_ok(),
                "ColumnDomain parser panicked for byte {index} xor {bit:#04x}"
            );
        }
    }
}

#[test]
fn encoding_fast_paths_match_canonical_decode_for_generated_payloads() {
    for bits_per_value in [1u8, 2, 3, 5, 8, 13, 21, 31] {
        let mask = (1u64 << bits_per_value) - 1;
        let values = (0..128)
            .map(|row| ((row as u64) * 37 + 11) & mask)
            .collect::<Vec<_>>();
        let payload = BitPackedPayload::pack(&values, bits_per_value).unwrap();
        assert_parity::<BitPacked>(&payload).unwrap();
    }

    for row_count in [0u64, 1, 8, 4096] {
        assert_parity::<Constant>(&ConstantPayload {
            value: -17,
            row_count,
        })
        .unwrap();
    }

    assert_parity::<Delta>(&DeltaPayload {
        base: 100,
        deltas: vec![1, -2, 4, -8, 16],
    })
    .unwrap();
    assert_parity::<FrameOfReference>(&ForPayload {
        reference: 1_000,
        offsets: vec![0, 1, -1, 10, -10],
    })
    .unwrap();
    assert_parity::<PlainFixed>(&PlainFixedPayload {
        values: vec![i64::MIN, -1, 0, 1, i64::MAX],
    })
    .unwrap();
    assert_parity::<PlainVarint>(&PlainVarintPayload {
        values: vec![i64::MIN, -123, 0, 456, i64::MAX],
    })
    .unwrap();
    assert_parity::<Rle>(&RlePayload {
        runs: vec![(7, 3), (-2, 5), (0, 1)],
    })
    .unwrap();
    assert_parity::<RunEnd>(&RunEndPayload {
        values: vec![7, -2, 0],
        run_ends: vec![3, 8, 9],
    })
    .unwrap();
    assert_parity::<Sparse>(&SparsePayload {
        row_count: 6,
        fill: 0,
        overrides: vec![(1, 10), (5, -3)],
    })
    .unwrap();
}
