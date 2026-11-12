//! The fold of reads (TM-Calls frames 6 and 7, TM-Legend 13): `read_file` calls that
//! went well and follow each other with nothing between them fold into one growing live
//! row, such as `✓ read 6 files  crates/efr-store/src/lib.rs, session.rs, json.rs
//! (+3)`, that is written once as one row when anything else is written: another block,
//! a failed read, the answer to a question, the end of the turn. A single read is
//! `✓ read <path>`. The row names at most three files, each after the first without the
//! directories that it shares with the first, then how many more. Reads show no time.
//!
//! Only reads fold: efr has no search tool, so the `search` row of the Calls board
//! waits for one. [`Fold`] keeps a place for a second kind of call: it holds the
//! words of its calls (`read`) with their subjects, and a fold of another word would
//! end this one.
//!
//! The fold takes its place in the spacing of the turn when its first read joins it, as
//! a row: what follows it is spaced as after a row, whether the fold is written yet or
//! not. When stdout is not a terminal nothing folds: each read is a row of its own.

use efr_render::RenderOptions;

use super::call;
use crate::format::changes::short_path;

/// The files that a fold names; the rest are counted.
const NAMED: usize = 3;

/// The reads that fold into one row.
#[derive(Debug, Default)]
pub(super) struct Fold {
    /// What the turn's spacing put before the row, when the first read joined.
    separator: &'static str,
    /// The word of the calls that fold, such as `read`; empty while it holds none.
    word: String,
    /// What each call read, safe to print, in order.
    subjects: Vec<String>,
}

impl Fold {
    /// True while it holds no call.
    pub(super) fn is_empty(&self) -> bool {
        self.subjects.is_empty()
    }

    /// True when a call of `word` folds into this fold: it holds none, or calls of the
    /// same word.
    pub(super) fn takes(&self, word: &str) -> bool {
        self.is_empty() || self.word == word
    }

    /// Adds a call of `word` that read `subject`; `separator` is what stands before the
    /// row, taken from the turn's spacing when the fold was empty.
    pub(super) fn push(
        &mut self,
        word: &str,
        subject: String,
        separator: impl FnOnce() -> &'static str,
    ) {
        if self.is_empty() {
            self.separator = separator();
            word.clone_into(&mut self.word);
        }
        self.subjects.push(subject);
    }

    /// The live row of the fold, with its separator and its newline, or nothing while it
    /// holds no read. `live_body` writes it after the live text of a message and before
    /// the row of a running call.
    pub(super) fn live(&self, options: &RenderOptions) -> String {
        if self.is_empty() {
            return String::new();
        }
        format!("{}{}", self.separator, row(&self.word, &self.subjects, options))
    }

    /// The fold as it is written once, and an empty fold in its place.
    pub(super) fn take(&mut self, options: &RenderOptions) -> String {
        let written = self.live(options);
        *self = Fold::default();
        written
    }
}

/// The row of calls of `word` that read `subjects`: `✓ read <path>` for one, `✓ read 6
/// files  <first>, <second>, <third> (+3)` for more.
pub(crate) fn row(word: &str, subjects: &[String], options: &RenderOptions) -> String {
    let text = match subjects {
        [one] => one.clone(),
        _ => {
            let first = subjects.first().map(String::as_str).unwrap_or_default();
            let named: Vec<&str> = subjects
                .iter()
                .take(NAMED)
                .enumerate()
                .map(|(at, path)| if at == 0 { path.as_str() } else { short_path(path, first) })
                .collect();
            let mut text = format!("{} files  {}", subjects.len(), named.join(", "));
            let rest = subjects.len().saturating_sub(NAMED);
            if rest > 0 {
                text.push_str(&format!(" (+{rest})"));
            }
            text
        }
    };
    call::folded(word, &text, options)
}
