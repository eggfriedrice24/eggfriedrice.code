//! Patch text to a [`Patch`].

use crate::{Patch, PatchError};

/// Reads the text of an `apply_patch` call.
///
/// The text follows [`GRAMMAR`](crate::GRAMMAR). The reader is lenient where models
/// are known to slip, as Codex's is: whitespace around the marker lines, and a
/// missing final newline. Anything else that is not a patch is
/// [`PatchError::Parse`] with the line and the problem.
pub fn parse(text: &str) -> Result<Patch, PatchError> {
    // NOTE: a stub until the engine is built; the contract is in the README.
    let _ = text;
    Err(PatchError::NotBuilt)
}
