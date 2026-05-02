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
//! | [`dictionary`] | File dictionary types (Section 16). |
//! | [`writer`] | [`MinimalQfWriter`] — writes minimal valid QF files. |
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

pub mod checksum;
pub mod constants;
pub mod dictionary;
pub mod error;
pub mod footer;
pub mod header;
pub mod postscript;
pub mod reader;
pub mod writer;

pub use error::QfError;
