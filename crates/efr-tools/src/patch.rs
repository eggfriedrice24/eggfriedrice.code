//! The patch engine of `apply_patch` and the `edit` tool: pure functions from patch
//! text and file contents to new file contents.
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
//! The engine is pure: it reads no file, writes no file, has no runtime and uses
//! nothing of this crate outside this module. `cargo xtask tidy` checks it (no
//! `tokio`, no `async`, no `std::fs`, `std::io`, `std::net`, `std::process` or
//! `std::env`, no workspace crate and no `crate::` path outside `crate::patch`). It
//! keeps its own error type, [`PatchError`], because the tools turn it into the text
//! that the model reads.
//!
//! What does not belong here: any IO (the tools read the files, refuse links, binary
//! and large files, write atomically and keep the journal), paths resolved against a
//! directory, permissions, and the text the model reads about a result (the
//! `apply_patch` and `edit` tools of this crate).

// NOTE: the engine was a tier 0 crate, so its public items keep their docs, as the
// docs rule of the low tiers asks.
#![warn(missing_docs)]

mod apply;
mod error;
mod grammar;
mod model;
mod parse;
mod replace;
mod seek;
mod text;

pub use apply::{ChangeKind, FileChange, Files, apply};
pub use error::{NearLine, ParseProblem, PatchError};
pub use grammar::GRAMMAR;
pub use model::{Hunk, HunkLine, Operation, Patch};
pub use parse::parse;
pub use replace::{Occurrences, Replacement, replace};
