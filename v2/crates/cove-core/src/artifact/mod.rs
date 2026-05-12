//! Cove Format (COVE) v1.0 — Companion artifacts (Spec §68–§69).
//!
//! Sidecar artifacts that accelerate planning. Both COVX and COVM are
//! *advisory*: a reader MUST be able to fall back to the underlying COVE file
//! if the sidecar is missing, stale, or corrupt.
//!
//! * [`covx`] — Spec §68: per-file index extension.
//! * [`covm`] — Spec §69: cross-file manifest.
//! * [`covemap`] — Spec §70: reusable COVE-MAP artifact.

pub mod covemap;
pub mod covm;
pub mod covx;
