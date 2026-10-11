//! The `efr-meta:` line of a turn's commits (`pre`, `last` and `post`) and of an undo's
//! commits (`before`, `after` and the markers): one JSON object that says which root,
//! conversation, turn and undo the commit belongs to, and what the snapshot left out,
//! the modes it saw, the paths that the turn's calls (or the undo) changed and the
//! project's refs. A field that an older commit lacks reads as its default.

use std::collections::BTreeMap;

use crate::capture::LeftOut;

/// The most left-out paths, modes and changed paths that one line names; the rest
/// only counts.
pub(crate) const MAX_LEFT_OUT: usize = 1000;
pub(crate) const MAX_MODES: usize = 1000;
pub(crate) const MAX_TOUCHED: usize = 5000;

/// The `efr-meta:` line of one commit.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Meta {
    pub(crate) conversation: String,
    pub(crate) turn: String,
    /// `pre`, `last` or `post`; empty in a commit from before phase B.
    pub(crate) phase: String,
    pub(crate) root: String,
    /// The prefix of the root's paths.
    pub(crate) shown: String,
    /// The project's `HEAD`: the branch it names (`refs/heads/main`) or a commit id.
    pub(crate) head: Option<String>,
    pub(crate) at: String,
    /// The mode of the turn (`auto`, `cautious`, `manual`), as the daemon names it.
    pub(crate) mode: Option<String>,
    /// The `(dev, ino)` of the root.
    pub(crate) dev_ino: Option<(u64, u64)>,
    /// The changed paths that are ignored, whose content the diff leaves out.
    pub(crate) hidden: Vec<String>,
    /// What the snapshot left out, at most [`MAX_LEFT_OUT`].
    pub(crate) left_out: Vec<LeftOut>,
    pub(crate) left_out_more: u32,
    /// Modes other than 0644 and 0755 of the files that the snapshots saw, at most
    /// [`MAX_MODES`].
    pub(crate) modes: BTreeMap<String, u32>,
    /// The paths that the turn's calls changed so far, at most [`MAX_TOUCHED`].
    pub(crate) touched: Vec<String>,
    /// True when the turn's calls changed more paths than [`MAX_TOUCHED`].
    pub(crate) touched_more: bool,
    /// True when a snapshot of the root failed in the turn, so the record of what the
    /// turn changed is not complete.
    pub(crate) failed: bool,
    /// The project's git dirs that the daemon trusts, and their `(dev, ino)`.
    pub(crate) git: Option<GitMeta>,
    /// The project's refs at this snapshot: name to commit id, `HEAD` included.
    pub(crate) refs: BTreeMap<String, String>,
    /// The mirror of the project's refs: `taken`, or why there is none.
    pub(crate) mirror: Option<String>,
    /// The number of files in the tree.
    pub(crate) files: u32,
    /// The undo that wrote this commit (`before`, `after`, `started`, `undone`,
    /// `redone`, `home`); `None` in a commit of a turn.
    pub(crate) undo: Option<String>,
}

/// The project's git dirs as a turn's first commit records them.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct GitMeta {
    pub(crate) git_dir: String,
    pub(crate) common_dir: String,
    pub(crate) git_dir_dev_ino: (u64, u64),
    pub(crate) common_dir_dev_ino: (u64, u64),
}

impl Meta {
    /// Sets the left-out record, cut at [`MAX_LEFT_OUT`].
    pub(crate) fn set_left_out(&mut self, left_out: &[LeftOut]) {
        let mut sorted = left_out.to_vec();
        sorted.sort();
        sorted.dedup();
        self.left_out_more =
            u32::try_from(sorted.len().saturating_sub(MAX_LEFT_OUT)).unwrap_or(u32::MAX);
        sorted.truncate(MAX_LEFT_OUT);
        self.left_out = sorted;
    }

    /// Sets the modes, cut at [`MAX_MODES`].
    pub(crate) fn set_modes(&mut self, modes: &BTreeMap<String, u32>) {
        self.modes =
            modes.iter().take(MAX_MODES).map(|(path, mode)| (path.clone(), *mode)).collect();
    }

    /// The commit message of a turn's commit of `phase`, with this line.
    pub(crate) fn message(&self) -> String {
        let line = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned());
        format!("efr turn {} {}\n\nefr-meta: {line}\n", self.turn, self.phase)
    }

    /// The `efr-meta:` line of a commit's text (`git cat-file commit`), or the default.
    pub(crate) fn of_commit(text: &str) -> Meta {
        text.lines()
            .find_map(|line| line.strip_prefix("efr-meta: "))
            .and_then(|line| serde_json::from_str(line).ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
