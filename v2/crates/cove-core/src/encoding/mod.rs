//! Cove Format (COVE) v1.0 — Encoding cascades (Spec §20.3).
//!
//! Each encoding implements the same logical contract:
//!
//! * [`canonical_decode`](trait.Encoding.html#tymethod.canonical_decode) —
//!   reconstruct the full logical array as `Vec<i64>`. This is the
//!   reference implementation a conformant reader MUST match.
//! * [`fast_decode`](trait.Encoding.html#method.fast_decode) — an optional
//!   accelerated path. When implemented, Spec §21 requires it to produce
//!   bit-identical results to the canonical decode for every well-formed
//!   input. The trait-level [`assert_parity`] helper exists so each
//!   submodule can prove this with a property-style test.
//!
//! The shared trait operates on `i64` for compact conformance fixtures;
//! encodings that need a wider physical surface, such as LocalCodebook, expose
//! typed decode APIs alongside this compatibility path. The cascade kinds
//! (Constant, LocalCodebook, RLE, RunEnd, BitPacked, Delta, FrameOfReference,
//! PatchedBase, Sparse, PlainFixed, PlainVarint) are listed in
//! [`crate::constants::CoveEncodingKind`].

pub mod bit_packed;
pub mod constant;
pub mod delta;
pub mod frame_of_reference;
pub mod local_codebook;
pub mod nested;
pub mod patched_base;
pub mod plain;
pub mod rle;
pub mod run_end;
pub mod sparse;

use crate::CoveError;

/// Common contract for every COVE v1 cascade (Spec §20.3, §21).
pub trait Encoding {
    /// Encoded payload for a single page.
    type Payload;

    /// Reference (canonical) decode — Spec §17 + §20.3 semantics.
    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, CoveError>;

    /// Optional fast-path decode. Default implementation defers to
    /// [`Self::canonical_decode`] so kernels are always *safe* even when no
    /// specialised path exists.
    fn fast_decode(payload: &Self::Payload) -> Result<Vec<i64>, CoveError> {
        Self::canonical_decode(payload)
    }
}

/// Asserts canonical-vs-fast decode parity for one payload (Spec §21.4).
pub fn assert_parity<E: Encoding>(payload: &E::Payload) -> Result<(), CoveError> {
    let canon = E::canonical_decode(payload)?;
    let fast = E::fast_decode(payload)?;
    if canon != fast {
        return Err(CoveError::BadSection(
            "fast decode disagrees with canonical decode (Spec §21.4)".into(),
        ));
    }
    Ok(())
}
