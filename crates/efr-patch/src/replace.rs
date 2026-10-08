//! One exact replacement of text, for an edit tool that takes an old and a new string.

use crate::{PatchError, seek};

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
    if old.is_empty() {
        return Err(PatchError::EmptyOld);
    }
    let count = text.matches(old).count();
    match (count, occurrences) {
        (0, _) => {
            let lines = lines(text);
            Err(PatchError::NotFound { nearest: seek::nearest(&lines, &lines_of(old)) })
        }
        (1, _) | (_, Occurrences::All) => Ok(Replacement { text: text.replace(old, new), count }),
        (count, Occurrences::One) => Err(PatchError::NotUnique { count }),
    }
}

/// The lines of a text, without their endings.
fn lines(text: &str) -> Vec<&str> {
    crate::text::split(text).lines.iter().map(|line| line.text).collect()
}

/// The lines of an old text. Its blank lines at both ends say nothing about where it
/// nearly occurs.
fn lines_of(old: &str) -> Vec<&str> {
    let lines = lines(old);
    let first = lines.iter().position(|line| !line.trim().is_empty()).unwrap_or(lines.len());
    let last = lines.iter().rposition(|line| !line.trim().is_empty()).map_or(first, |at| at + 1);
    lines[first..last.max(first)].to_vec()
}

#[cfg(test)]
mod tests;
