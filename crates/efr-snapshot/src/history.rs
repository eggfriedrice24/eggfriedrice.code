//! What a finished turn changed, read back from the refs that
//! [`Snapshots::finish_turn`] kept, for `conversation.diff`.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use efr_protocol::{ConversationId, FileChanges, TurnId};

use crate::SnapshotError;
use crate::changes::{self, Shown};
use crate::runner::Run;
use crate::snapshots::Snapshots;
use crate::store::Store;

/// What a turn changed in files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnDiff {
    /// The turn.
    pub turn_id: TurnId,
    /// The changed files; empty when the turn changed none.
    pub changes: FileChanges,
    /// The unified diff, when it was asked for.
    pub diff: Option<String>,
}

/// One store that holds snapshots of a conversation.
#[derive(Debug, Clone)]
struct Holder {
    store: Store,
    turns: Vec<String>,
}

impl Snapshots {
    /// What the turn `turn` of `conversation` changed: its first snapshot against its
    /// last in every root it wrote. Without `turn`, the newest turn that has
    /// snapshots. With `with_diff`, also the unified diff, cut after `max_lines` lines
    /// with a line `... N more lines`. `None` when the conversation has no snapshot
    /// at all and no turn was named.
    pub async fn turn_diff(
        &self,
        conversation: ConversationId,
        turn: Option<TurnId>,
        with_diff: bool,
        max_lines: usize,
    ) -> Result<Option<TurnDiff>, SnapshotError> {
        let holders = self.holders(conversation).await?;
        let wanted = match turn {
            Some(turn) => turn,
            None => {
                let newest = holders.iter().flat_map(|holder| holder.turns.iter()).max();
                match newest.and_then(|turn| turn.parse::<TurnId>().ok()) {
                    Some(turn) => turn,
                    None => return Ok(None),
                }
            }
        };
        let name = wanted.to_string();
        let mut shown = Vec::new();
        let mut patches: Vec<(String, Vec<u8>)> = Vec::new();
        for holder in holders.iter().filter(|holder| holder.turns.contains(&name)) {
            let base = format!("refs/efr/{conversation}/{name}");
            let pre = format!("{base}/pre");
            let post = format!("{base}/post");
            let prefix = self.shown_of(&holder.store, &pre).await?;
            let changes = self.diff_trees(&holder.store, &pre, &post).await?;
            if with_diff && !changes.is_empty() {
                let src = format!("--src-prefix=a/{prefix}");
                let dst = format!("--dst-prefix=b/{prefix}");
                let args = [
                    "diff-tree",
                    "-r",
                    "-p",
                    "-M",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    src.as_str(),
                    dst.as_str(),
                    pre.as_str(),
                    post.as_str(),
                ];
                let patch = self
                    .runner()
                    .checked(&holder.store, "diff-tree", &args, Run::default())
                    .await?;
                patches.push((prefix.clone(), patch));
            }
            shown.push(Shown { root: holder.store.root().to_path_buf(), shown: prefix, changes });
        }
        let changes = changes::merge(shown).unwrap_or_default();
        let diff = with_diff.then(|| {
            patches.sort_by(|a, b| a.0.cmp(&b.0));
            let text: String =
                patches.iter().map(|(_, patch)| String::from_utf8_lossy(patch)).collect();
            cut_lines(&text, max_lines)
        });
        Ok(Some(TurnDiff { turn_id: wanted, changes, diff }))
    }

    /// The prefix that the turn's first commit in `store` recorded for its paths.
    async fn shown_of(&self, store: &Store, pre: &str) -> Result<String, SnapshotError> {
        let out = self
            .runner()
            .checked(store, "cat-file", &["cat-file", "commit", pre], Run::default())
            .await?;
        let text = String::from_utf8_lossy(&out);
        let shown = text
            .lines()
            .find_map(|line| line.strip_prefix("efr-meta: "))
            .and_then(|meta| serde_json::from_str::<serde_json::Value>(meta).ok())
            .and_then(|meta| meta.get("shown").and_then(|shown| shown.as_str()).map(str::to_owned));
        Ok(shown.unwrap_or_else(|| format!("{}/", store.root().display())))
    }

    /// The stores with refs of `conversation`, with the turns of each.
    async fn holders(&self, conversation: ConversationId) -> Result<Vec<Holder>, SnapshotError> {
        let dir = self.dir().to_path_buf();
        let name = conversation.to_string();
        let candidates =
            self.runner().blocking(&dir.clone(), move || candidates(&dir, &name)).await?;
        let mut holders = Vec::new();
        let prefix = format!("refs/efr/{conversation}/");
        for store in candidates {
            let out = self
                .runner()
                .checked(
                    &store,
                    "for-each-ref",
                    &["for-each-ref", "--format=%(refname)", prefix.as_str()],
                    Run::default(),
                )
                .await?;
            let mut turns: Vec<String> = String::from_utf8_lossy(&out)
                .lines()
                .filter_map(|line| line.strip_prefix(prefix.as_str()))
                .filter_map(|rest| rest.strip_suffix("/pre"))
                .map(str::to_owned)
                .collect();
            turns.sort();
            turns.dedup();
            if !turns.is_empty() {
                holders.push(Holder { store, turns });
            }
        }
        Ok(holders)
    }
}

/// The stores in `dir` that may hold refs of the conversation `name`: a loose ref
/// directory or a line in `packed-refs`.
fn candidates(dir: &Path, name: &str) -> Vec<Store> {
    let needle = format!("refs/efr/{name}/");
    stores(dir)
        .into_iter()
        .filter(|store| {
            let git_dir = store.git_dir();
            git_dir.join("refs/efr").join(name).is_dir()
                || fs::read_to_string(git_dir.join("packed-refs"))
                    .is_ok_and(|packed| packed.contains(&needle))
        })
        .collect()
}

/// Every store in `dir` with the root its root file names.
pub(crate) fn stores(dir: &Path) -> Vec<Store> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut found = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".git")) else {
            continue;
        };
        let root = fs::read(dir.join(format!("{id}.root")))
            .ok()
            .map(|bytes| PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
            .unwrap_or_default();
        found.push(Store::with_id(dir, id, root));
    }
    found.sort_by_key(Store::git_dir);
    found
}

/// `text` cut after `max_lines` lines with a last line `... N more lines`.
pub(crate) fn cut_lines(text: &str, max_lines: usize) -> String {
    let total = text.lines().count();
    if total <= max_lines {
        return text.to_owned();
    }
    let mut out = String::new();
    for line in text.lines().take(max_lines) {
        out.push_str(line);
        out.push('\n');
    }
    let _ = writeln!(out, "... {} more lines", total - max_lines);
    out
}

#[cfg(test)]
mod tests;
