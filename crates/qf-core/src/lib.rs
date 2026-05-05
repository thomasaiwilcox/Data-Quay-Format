//! # qf-core — Quay Format (QF) v1.0 reference library
//!
//! This crate provides the foundational building blocks for reading and writing
//! Quay Format (QF) v1.0 files as defined in the [QF specification].
//!
//! ## Modules
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`constants`] | Magic bytes, feature bits, section kinds, and all spec enumerations. |
//! | [`error`] | [`QfError`] — the single error type for the entire library. |
//! | [`checksum`] | CRC32C utilities (Castagnoli, per Section 8.6). |
//! | [`header`] | [`QfHeaderV1`] — the 128-byte file header (Section 10). |
//! | [`postscript`] | [`QfPostscriptV1`] and [`QfSectionSpecV1`] (Section 12). |
//! | [`footer`] | [`QfFooter`], [`QfFooterHeaderV1`], [`QfSectionEntryV1`] (Section 13). |
//! | [`metadata`] | Descriptive footer metadata JSON (Section 15). |
//! | [`dictionary`] | File dictionary types (Section 16). |
//! | [`types`] | Logical/physical type compatibility and NumCode interpretation helpers. |
//! | [`validity`] | [`validity::ValidityBitmap`] — null bitmap helpers (bit 1 = null). |
//! | [`writer`] | [`MinimalQfWriter`] — writes minimal valid QF files. |
//! | [`array`]      | [`array::EncodedArray`] — single-row decoder for encoded column arrays. |
//! | [`compression`] | Section decompression layer (None/LZ4/Zstd). |
//! | [`extensions`] | [`extensions::ExtensionRegistry`] — extension registry parsing and validation. |
//! | [`collation`]  | [`collation::CollationRegistry`] — v1 collation registry and comparison rules. |
//! | [`digest`]     | [`digest::DigestManifest`] — digest manifest parsing and verification helpers. |
//! | [`registry`]   | Structured spec registries for features, sections, and error codes. |
//!
//! ## Quick start
//!
//! ### Write a minimal empty QF file
//!
//! ```rust
//! use qf_core::writer::MinimalQfWriter;
//!
//! let file_bytes = MinimalQfWriter::write_empty_file();
//! println!("Written {} bytes", file_bytes.len());
//! ```
//!
//! ### Read and validate
//!
//! ```rust
//! use qf_core::{
//!     checksum,
//!     footer::QfFooter,
//!     header::QfHeaderV1,
//!     postscript::QfPostscriptV1,
//!     writer::MinimalQfWriter,
//! };
//!
//! let bytes = MinimalQfWriter::write_empty_file();
//!
//! // 1. Parse header.
//! let header = QfHeaderV1::parse(&bytes, false).unwrap();
//! assert_eq!(header.version_major, 1);
//!
//! // 2. Parse postscript from the tail.
//! let ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
//! assert_eq!(ps.file_len, bytes.len() as u64);
//!
//! // 3. Locate and validate footer.
//! let footer_start = ps.footer.offset as usize;
//! let footer_end   = footer_start + ps.footer.length as usize;
//! let footer_bytes = &bytes[footer_start..footer_end];
//! assert_eq!(checksum::crc32c(footer_bytes), ps.footer.crc32c);
//!
//! // 4. Parse footer and section directory.
//! let footer = QfFooter::parse(footer_bytes).unwrap();
//! println!("Section count: {}", footer.sections.len());
//! ```

pub mod array;
pub mod artifact;
pub mod canonical;
pub mod checksum;
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
pub mod footer;
pub mod header;
pub mod index;
pub mod interop;
pub mod io_hints;
pub mod kernel;
pub mod metadata;
pub mod page;
pub mod postscript;
pub mod predicate;
pub mod profile;
pub mod pruning;
pub mod reader;
pub mod redaction;
pub mod registry;
pub mod row_ref;
pub mod segment;
pub mod sort;
pub mod table;
pub mod trust_chain;
pub mod types;
pub mod validity;
pub mod wire;
pub mod writer;
pub mod zone_stats;

pub use error::QfError;
