//! Quay Format (QF) v1.0 — Companion artifacts (Spec §68–§69).
//!
//! Sidecar artifacts that accelerate planning. Both QFX and QFM are
//! *advisory*: a reader MUST be able to fall back to the underlying QF file
//! if the sidecar is missing, stale, or corrupt.
//!
//! * [`qfx`] — Spec §68: per-file index extension.
//! * [`qfm`] — Spec §69: cross-file manifest.

pub mod qfm;
pub mod qfx;
