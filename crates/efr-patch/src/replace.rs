//! One exact replacement of text, for an edit tool that takes an old and a new string.

use crate::PatchError;

/// Which occurrences of the old text a [`replace`] changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occurrences {
    /// Exactly one: the old text must occur once, else [`PatchError::NotUnique`].
    One,
    /// Every one: the old text must occur at least once.
    All,
}

/// The result of a [`replace`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    /// The whole new text.
    pub text: String,
    /// How many occurrences were replaced, at least one.
    pub count: usize,
}

/// Replaces `old` with `new` in `text`, exactly: no tolerant pass, because the model
/// copies the old text from what it read.
///
/// An empty `old` is [`PatchError::EmptyOld`]; an `old` that does not occur is
/// [`PatchError::NotFound`] with the nearest lines; with [`Occurrences::One`], an
/// `old` that occurs more than once is [`PatchError::NotUnique`].
pub fn replace(
    text: &str,
    old: &str,
    new: &str,
    occurrences: Occurrences,
) -> Result<Replacement, PatchError> {
    // NOTE: a stub until the engine is built; the contract is in the README.
    let _ = (text, old, new, occurrences);
    Err(PatchError::NotBuilt)
}
