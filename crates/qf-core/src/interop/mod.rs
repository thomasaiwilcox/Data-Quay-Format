//! Quay Format (QF) v1.0 — Ecosystem interop.
//!
//! * [`arrow`] — Spec §49 Arrow null-bitmap inversion.
//! * [`lakehouse`] — Spec §50 lakehouse hints.
//! * [`parquet`] — Spec §51 conversion profile.

pub mod arrow;
pub mod lakehouse;
pub mod parquet;
