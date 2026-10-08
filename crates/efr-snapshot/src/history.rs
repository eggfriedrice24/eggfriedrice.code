//! What a finished turn changed, read back from the refs that
//! [`Snapshots::finish_turn`] kept, for `conversation.diff`.

use std::ffi::OsString;
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

/// What a turn's commits record in their `efr-meta:` line.
#[derive(Debug, Clone, Default)]
struct Meta {
    /// The prefix of the root's paths.
    shown: String,
    /// The changed paths that are ignored, whose content the diff leaves out.
    hidden: Vec<String>,
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
        let holders: Vec<&Holder> =
            holders.iter().filter(|holder| holder.turns.contains(&name)).collect();
        let roots: Vec<PathBuf> =
            holders.iter().map(|holder| holder.store.root().to_path_buf()).collect();
        for holder in holders {
            let base = format!("refs/efr/{conversation}/{name}");
            let pre = format!("{base}/pre");
            let post = format!("{base}/post");
            let Meta { shown: prefix, hidden } = self.meta_of(&holder.store, &pre).await?;
            let changes = self.diff_trees(&holder.store, &pre, &post).await?;
            if with_diff && !changes.is_empty() {
                let src = format!("--src-prefix=a/{prefix}");
                let dst = format!("--dst-prefix=b/{prefix}");
                let mut args: Vec<OsString> = [
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
                ]
                .into_iter()
                .map(OsString::from)
                .collect();
                // NOTE: the files below a deeper root come from that root's patch, as
                // `changes::merge` lists them. An ignored file, such as `.env`, may
                // hold secrets that nobody read in this turn, so its content stays
                // out of a diff that any read-scope client and the scrollback get.
                let deeper = changes::deeper_roots(holder.store.root(), &roots);
                let left_out: Vec<&Path> = deeper
                    .iter()
                    .map(PathBuf::as_path)
                    .chain(hidden.iter().map(Path::new))
                    .collect();
                if !left_out.is_empty() {
                    args.push("--".into());
                    for path in &left_out {
                        let mut spec = OsString::from(":(exclude,literal)");
                        spec.push(path.as_os_str());
                        args.push(spec);
                    }
                }
                let mut patch = self
                    .runner()
                    .checked(&holder.store, "diff-tree", &args, Run::default())
                    .await?;
                for change in &changes {
                    let named = |path: &str| hidden.iter().any(|hidden| hidden == path);
                    let below =
                        deeper.iter().any(|below| Path::new(&change.path).starts_with(below));
                    if !below && (named(&change.path) || change.from.as_deref().is_some_and(named))
                    {
                        let note =
                            format!("{prefix}{}: ignored file, content not shown\n", change.path);
                        patch.extend_from_slice(note.as_bytes());
                    }
                }
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

    /// What the turn's first commit in `store` recorded: the prefix of its paths and
    /// its ignored changed paths.
    async fn meta_of(&self, store: &Store, pre: &str) -> Result<Meta, SnapshotError> {
        let out = self
            .runner()
            .checked(store, "cat-file", &["cat-file", "commit", pre], Run::default())
            .await?;
        let text = String::from_utf8_lossy(&out);
        let meta = text
            .lines()
            .find_map(|line| line.strip_prefix("efr-meta: "))
            .and_then(|meta| serde_json::from_str::<serde_json::Value>(meta).ok())
            .unwrap_or_default();
        let shown = meta.get("shown").and_then(|shown| shown.as_str()).map(str::to_owned);
        let hidden = meta
            .get("hidden")
            .and_then(|hidden| hidden.as_array())
            .map(|paths| paths.iter().filter_map(|path| path.as_str()).map(str::to_owned).collect())
            .unwrap_or_default();
        Ok(Meta { shown: shown.unwrap_or_else(|| format!("{}/", store.root().display())), hidden })
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
