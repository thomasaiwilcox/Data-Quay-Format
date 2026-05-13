//! # cove-arrow -- Arrow and Parquet interop for COVE
//!
//! This crate owns Arrow-facing import/export and Parquet conversion helpers
//! layered on top of `cove-core` semantics.

pub use cove_core::array;
pub use cove_core::artifact;
pub use cove_core::canonical;
pub use cove_core::checksum;
pub use cove_core::compression;
pub use cove_core::constants;
pub use cove_core::dictionary;
pub use cove_core::digest;
pub use cove_core::domain;
pub use cove_core::encoding;
pub use cove_core::index;
pub use cove_core::nested_schema;
pub use cove_core::page;
pub use cove_core::page_payload;
pub use cove_core::reader;
pub use cove_core::row_ref;
pub use cove_core::segment;
pub use cove_core::table;
pub use cove_core::types;
pub use cove_core::validity;
pub use cove_core::wire;
pub use cove_core::writer;
pub use cove_core::zone_stats;
pub use cove_core::CoveError;

pub mod arrow;

pub mod convert;

pub mod parquet;
