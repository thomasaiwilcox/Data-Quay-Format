//! Cove Format (COVE) v2.0 — Engine and execution profiles.
//!
//! * [`cove_e`] — Spec §38–§43: generic engine profile.
//! * [`cove_h`] — Spec §44: Harbor profile (registered implementation).
//! * [`cove_map`] — Spec §70/§73.6: semantic mapping profile boundary and
//!   reference embedded-section validation schema.
//! * [`cove_o`] — Spec §55–§63: object-temporal profile.

pub mod cove_e;
pub mod cove_h;
pub mod cove_map;
pub mod cove_o;
