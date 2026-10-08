//! The patch engine of efr: pure functions from patch text and file contents to new
//! file contents.
//!
//! - [`parse`] reads the text of an `apply_patch` call into a [`Patch`]: its
//!   [`Operation`]s (add, delete, update with an optional move) and the [`Hunk`]s of
//!   each update, made of [`HunkLine`]s. [`GRAMMAR`] is the Lark grammar of that text,
//!   which a provider sends with a freeform tool.
//! - [`apply`] matches each hunk against the current text that a [`Files`] gives, and
//!   computes every new content before anything is written. It returns one
//!   [`FileChange`] ([`ChangeKind`]) per changed path, or a [`PatchError`] that names
//!   the file and the hunk and shows the [`NearLine`]s, so the model can try again.
//! - [`replace`] is the entry point of an edit tool with an old and a new string:
//!   one exact, unique replacement, or every occurrence ([`Occurrences`]), with the
//!   result as a [`Replacement`].
//!
//! Allowed dependencies: no workspace crate. What does not belong here: any IO (the
//! caller reads the files, refuses links, binary and large files, writes atomically
//! and keeps the journal), paths resolved against a directory, permissions, and the
//! text the model reads about a result (`efr-tools`).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod apply;
mod error;
mod grammar;
mod parse;
mod patch;
mod replace;

pub use apply::{ChangeKind, FileChange, Files, apply};
pub use error::{NearLine, ParseProblem, PatchError};
pub use grammar::GRAMMAR;
pub use parse::parse;
pub use patch::{Hunk, HunkLine, Operation, Patch};
pub use replace::{Occurrences, Replacement, replace};
