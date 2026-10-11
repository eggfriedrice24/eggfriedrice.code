//! [`Held`]: the refs that one store holds of one conversation, by turn and by undo.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use efr_protocol::{ConversationId, TurnId};

use crate::SnapshotError;
use crate::history::stores;
use crate::meta::Meta;
use crate::runner::Run;
use crate::snapshots::Snapshots;
use crate::store::Store;

/// The refs of one turn in one store.
#[derive(Debug, Clone, Default)]
pub(crate) struct TurnHas {
    pub(crate) pre: Option<String>,
    pub(crate) last: Option<String>,
    pub(crate) post: Option<String>,
    /// The marker of the undo that undid the turn.
    pub(crate) undone: Option<String>,
    /// The home copies: `home/<n>` by `n`.
    pub(crate) home: BTreeMap<u32, String>,
}

/// The refs of one undo in one store, below `refs/efr-undo/<conversation>/<undo>/`.
#[derive(Debug, Clone, Default)]
pub(crate) struct UndoHas {
    pub(crate) before: Option<String>,
    pub(crate) started: Option<String>,
    pub(crate) after: Option<String>,
    pub(crate) redone: Option<String>,
    /// The copy of the home paths that the undo changes.
    pub(crate) home: Option<String>,
    /// The refs that the undo moves, as the project had them before it: the name below
    /// `mirror/` and the object id.
    pub(crate) mirror: Vec<(String, String)>,
}

/// The refs that one store holds of one conversation.
#[derive(Debug, Clone, Default)]
pub(crate) struct Held {
    pub(crate) turns: BTreeMap<TurnId, TurnHas>,
    pub(crate) undos: BTreeMap<String, UndoHas>,
}

/// `refs/efr-undo/<conversation>/<undo>/<name>`.
pub(crate) fn undo_ref(conversation: ConversationId, undo: &str, name: &str) -> String {
    format!("refs/efr-undo/{conversation}/{undo}/{name}")
}

/// Reads the `for-each-ref` lines `<id> <name>` of one conversation.
pub(crate) fn parse(conversation: ConversationId, out: &str) -> Held {
    let turns_prefix = format!("refs/efr/{conversation}/");
    let undos_prefix = format!("refs/efr-undo/{conversation}/");
    let mut held = Held::default();
    for line in out.lines() {
        let Some((id, name)) = line.split_once(' ') else { continue };
        let id = id.to_owned();
        if let Some(rest) = name.strip_prefix(&turns_prefix) {
            let Some((turn, kind)) = rest.split_once('/') else { continue };
            let Ok(turn) = turn.parse::<TurnId>() else { continue };
            let has = held.turns.entry(turn).or_default();
            match kind {
                "pre" => has.pre = Some(id),
                "last" => has.last = Some(id),
                "post" => has.post = Some(id),
                "undone" => has.undone = Some(id),
                other => {
                    if let Some(n) = other.strip_prefix("home/").and_then(|n| n.parse().ok()) {
                        has.home.insert(n, id);
                    }
                }
            }
        } else if let Some(rest) = name.strip_prefix(&undos_prefix) {
            let Some((undo, kind)) = rest.split_once('/') else { continue };
            let has = held.undos.entry(undo.to_owned()).or_default();
            match kind {
                "before" => has.before = Some(id),
                "started" => has.started = Some(id),
                "after" => has.after = Some(id),
                "redone" => has.redone = Some(id),
                "home" => has.home = Some(id),
                other => {
                    if let Some(mirrored) = other.strip_prefix("mirror/") {
                        has.mirror.push((mirrored.to_owned(), id));
                    }
                }
            }
        }
    }
    held
}

/// True when `id` can name an undo's refs and its trash directory: letters, digits,
/// `-`, `_` and `.`, at most 80 characters, not starting with `.` or `-`.
pub(crate) fn valid_undo_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && !id.starts_with(['.', '-'])
        && !id.contains("..")
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

impl Snapshots {
    /// Every root store that holds refs of `conversation`, with them.
    pub(crate) async fn held_roots(
        &self,
        conversation: ConversationId,
    ) -> Result<Vec<(Store, Held)>, SnapshotError> {
        let dir = self.dir().to_path_buf();
        let name = conversation.to_string();
        let found = self.runner().blocking(&dir.clone(), move || candidates(&dir, &name)).await?;
        let mut all = Vec::new();
        for store in found {
            let held = self.held_in(&store, conversation).await?;
            if !held.turns.is_empty() || !held.undos.is_empty() {
                all.push((store, held));
            }
        }
        Ok(all)
    }

    /// The refs of `conversation` in `store`.
    pub(crate) async fn held_in(
        &self,
        store: &Store,
        conversation: ConversationId,
    ) -> Result<Held, SnapshotError> {
        if !store.exists() {
            return Ok(Held::default());
        }
        let turns = format!("refs/efr/{conversation}/");
        let undos = format!("refs/efr-undo/{conversation}/");
        let out = self
            .runner()
            .checked(
                store,
                "for-each-ref",
                &["for-each-ref", "--format=%(objectname) %(refname)", &turns, &undos],
                Run::default(),
            )
            .await?;
        Ok(parse(conversation, &String::from_utf8_lossy(&out)))
    }

    /// The `efr-meta:` line of `commit` in `store`, its tree and its first parent.
    pub(crate) async fn commit_info(
        &self,
        store: &Store,
        commit: &str,
    ) -> Result<CommitInfo, SnapshotError> {
        let out = self
            .runner()
            .checked(store, "cat-file", &["cat-file", "commit", commit], Run::default())
            .await?;
        let text = String::from_utf8_lossy(&out);
        let header = |key: &str| {
            text.lines()
                .take_while(|line| !line.is_empty())
                .find_map(|line| line.strip_prefix(key))
                .map(str::to_owned)
        };
        let tree = header("tree ").ok_or(SnapshotError::BadOutput { command: "cat-file" })?;
        Ok(CommitInfo { tree, parent: header("parent "), meta: Meta::of_commit(&text) })
    }
}

/// One commit of a store, read back.
#[derive(Debug, Clone)]
pub(crate) struct CommitInfo {
    pub(crate) tree: String,
    /// The first parent.
    pub(crate) parent: Option<String>,
    pub(crate) meta: Meta,
}

/// The root stores in `dir` that may hold refs of the conversation `name`: a loose ref
/// directory or a line in `packed-refs`, of its turns or of its undos.
fn candidates(dir: &Path, name: &str) -> Vec<Store> {
    let needles = [format!("refs/efr/{name}/"), format!("refs/efr-undo/{name}/")];
    stores(dir)
        .into_iter()
        .filter(|store| {
            let git_dir = store.git_dir();
            git_dir.join("refs/efr").join(name).is_dir()
                || git_dir.join("refs/efr-undo").join(name).is_dir()
                || fs::read_to_string(git_dir.join("packed-refs"))
                    .is_ok_and(|packed| needles.iter().any(|needle| packed.contains(needle)))
        })
        .collect()
}
