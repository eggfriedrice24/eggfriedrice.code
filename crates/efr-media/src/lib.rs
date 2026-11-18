//! The images and PDFs of efr's attachments (ADR 0026), as pure functions of their
//! bytes: the size from the header, the decode check, the strip of EXIF, GPS, XMP and
//! comment metadata, the downscale, the thumbnail and the facts of a PDF.
//!
//! Allowed dependencies: `efr-stdx` (the one table of magic bytes, `media::sniff`).
//! Only efrd links this crate. What does not belong here: IO, tokio, the blob dir and
//! the rules of what goes to a model, which efrd and `efr-provider` own.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]
