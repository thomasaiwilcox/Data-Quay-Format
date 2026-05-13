//! # cove-core — Cove Format (COVE) v2.0 reference library
//!
//! This crate provides the foundational building blocks for reading and writing
//! Cove Format (COVE) v2.0 files as defined in the [COVE specification].
//!
//! ## Modules
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`constants`] | Magic bytes, feature bits, section kinds, and all spec enumerations. |
//! | [`error`] | [`CoveError`] — the single error type for the entire library. |
//! | [`checksum`] | CRC32C utilities (Castagnoli, per Section 8.6). |
//! | [`header`] | [`CoveHeaderV1`] — the 160-byte v2 file header (Section 10). |
//! | [`postscript`] | [`CovePostscriptV1`] and [`CoveSectionSpecV1`] (Section 12). |
//! | [`footer`] | [`CoveFooter`], [`CoveFooterHeaderV1`], [`CoveSectionEntryV1`] (Section 13). |
//! | [`metadata`] | Descriptive footer metadata JSON (Section 15). |
//! | [`dictionary`] | File dictionary types (Section 16). |
//! | [`types`] | Logical/physical type compatibility and NumCode interpretation helpers. |
//! | [`validity`] | [`validity::ValidityBitmap`] — null bitmap helpers (bit 1 = null). |
//! | [`writer`] | [`MinimalCoveWriter`] — writes minimal valid COVE files. |
//! | [`array`]      | [`array::EncodedArray`] — single-row decoder for encoded column arrays. |
//! | [`compression`] | Section decompression layer (None/LZ4/Zstd). |
//! | [`extensions`] | [`extensions::ExtensionRegistry`] — extension registry parsing and validation. |
//! | [`collation`]  | [`collation::CollationRegistry`] — v1 collation registry and comparison rules. |
//! | [`digest`]     | [`digest::DigestManifest`] — digest manifest parsing and verification helpers. |
//! | [`registry`]   | Structured spec registries for features, sections, and error codes. |
//!
//! ## Quick start
//!
//! ### Write a minimal empty COVE file
//!
//! ```rust
//! use cove_core::writer::MinimalCoveWriter;
//!
//! let file_bytes = MinimalCoveWriter::write_empty_file().unwrap();
//! println!("Written {} bytes", file_bytes.len());
//! ```
//!
//! ### Read and validate
//!
//! ```rust
//! use cove_core::{
//!     checksum,
//!     footer::CoveFooter,
//!     header::CoveHeaderV1,
//!     postscript::CovePostscriptV1,
//!     writer::MinimalCoveWriter,
//! };
//!
//! let bytes = MinimalCoveWriter::write_empty_file().unwrap();
//!
//! // 1. Parse header.
//! let header = CoveHeaderV1::parse(&bytes).unwrap();
//! assert_eq!(header.version_major, 2);
//!
//! // 2. Parse postscript from the tail.
//! let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
//! assert_eq!(ps.file_len, bytes.len() as u64);
//!
//! // 3. Locate and validate footer.
//! let footer_start = ps.footer.offset as usize;
//! let footer_end   = footer_start + ps.footer.length as usize;
//! let footer_bytes = &bytes[footer_start..footer_end];
//! assert_eq!(checksum::crc32c(footer_bytes), ps.footer.crc32c);
//!
//! // 4. Parse footer and section directory.
//! let footer = CoveFooter::parse(footer_bytes).unwrap();
//! println!("Section count: {}", footer.sections.len());
//! ```

pub mod array;
pub mod artifact;
pub mod canonical;
pub mod checksum;
pub mod codec;
pub mod collation;
pub mod compression;
pub mod constants;
pub mod dictionary;
pub mod digest;
pub mod domain;
pub mod durable;
pub mod encoding;
pub mod error;
pub mod extensions;
pub mod feature_binding;
pub mod feature_scope;
pub mod footer;
pub mod header;
pub mod index;
pub mod interop;
pub mod io_hints;
pub mod kernel;
pub mod metadata;
pub mod mount;
pub mod nested_schema;
pub mod page;
pub mod page_payload;
mod page_validation;
pub mod postscript;
pub mod predicate;
pub mod profile;
pub mod pruning;
pub mod reader;
pub mod redaction;
pub mod registry;
pub mod retained_bytes;
pub mod row_ref;
pub mod segment;
pub mod sort;
pub mod table;
pub mod trust_chain;
pub mod types;
pub mod utility;
pub mod validity;
pub mod wire;
pub mod writer;
pub mod zone_stats;

pub use error::CoveError;
