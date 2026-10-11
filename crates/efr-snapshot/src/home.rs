//! The home store (efr's auto spec, 10.8): copies of the paths outside the roots that a
//! call can change, in one more bare repository, `$D/snapshots/home.git`.
//!
//! The daemon reads each path itself, without following a link, and hands the entries
//! over through a [`HomeReader`]; the bytes stream into `git fast-import`, at most one
//! file in memory, so git never reads a home path and no filter runs. Each copy is one
//! ref `refs/efr/<conversation>/<turn>/home/<n>`: a "before" commit before the call
//! ([`Snapshots::home_before`]), then an "after" commit, its child, after the call
//! ([`Snapshots::home_after`]). Each tree holds `files/<path without its leading />`,
//! `state/<name>` for `@crontab` and `@linger`, and the manifest `efr-home.json`, which
//! names the state of each path: a file with its whole mode, a link (stored as a link;
//! the real file behind it is an entry of its own), a missing path, a state, or why no
//! copy covers it. A path that the "after" lists and the "before" does not, such as a
//! file that a call made in a directory that it could write, was missing before.

use std::io::Read;
use std::path::PathBuf;

use efr_protocol::{ConversationId, TurnId, UndoReason};
use tokio::sync::mpsc;

use crate::SnapshotError;
use crate::meta::Meta;
use crate::object::ObjectId;
use crate::runner::Run;
use crate::snapshots::Snapshots;
use crate::store::Store;
use crate::turn::turn_ref;
use crate::undo::{Expect, undo_ref, valid_undo_id};

mod copy;
mod plan;

pub use copy::MAX_HOME_FILE_BYTES;
pub(crate) use copy::{Item, Kind, MANIFEST, Manifest};

/// The id of the home store in the store directory: `home.git`.
pub(crate) const HOME_ID: &str = "home";

/// The state of a home path in a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HomeState {
    /// A regular file with this mode (`& 0o7777`); its bytes come from the reader.
    File {
        /// The mode.
        mode: u32,
    },
    /// A symbolic link to `target`, stored as a link, never followed.
    Link {
        /// The link's target.
        target: Vec<u8>,
    },
    /// The path does not exist.
    Missing,
    /// The text of a state that a command reads and sets (`@crontab`, `@linger`); its
    /// bytes come from the reader.
    State,
    /// No copy holds the path, for this reason (masked, over a limit).
    NotCovered(UndoReason),
}

/// One path that the daemon read for a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeEntry {
    /// The absolute path, or `@crontab` or `@linger`.
    pub path: PathBuf,
    /// What it is.
    pub state: HomeState,
    /// True for a floor or persistence file.
    pub floor: bool,
    /// The links from a floor path to its real file, for the report.
    pub chain: Vec<PathBuf>,
}

/// The entries of one copy, in the order that the daemon reads them. The bytes of a
/// [`HomeState::File`] or a [`HomeState::State`] come with the entry, and the store
/// reads them before it asks for the next one, so at most one file
/// ([`MAX_HOME_FILE_BYTES`]) is in memory. An error stops the copy: no ref moves.
pub trait HomeReader: Send {
    /// The next entry, with the reader of its bytes; `None` at the end.
    #[expect(clippy::type_complexity, reason = "the entry and its streamed bytes")]
    fn next(&mut self) -> Option<Result<(HomeEntry, Option<Box<dyn Read + Send>>), SnapshotError>>;
}

/// A list of entries, each with its bytes in memory, as a [`HomeReader`].
#[derive(Debug, Default)]
pub struct HomeEntries {
    entries: std::collections::VecDeque<(HomeEntry, Option<Vec<u8>>)>,
}

impl HomeEntries {
    /// The entries of `entries`.
    pub fn new(entries: Vec<(HomeEntry, Option<Vec<u8>>)>) -> Self {
        HomeEntries { entries: entries.into() }
    }
}

impl HomeReader for HomeEntries {
    fn next(&mut self) -> Option<Result<(HomeEntry, Option<Box<dyn Read + Send>>), SnapshotError>> {
        let (entry, bytes) = self.entries.pop_front()?;
        let bytes =
            bytes.map(|bytes| Box::new(std::io::Cursor::new(bytes)) as Box<dyn Read + Send>);
        Some(Ok((entry, bytes)))
    }
}

/// One copy of a turn: its ref `refs/efr/<conversation>/<turn>/home/<n>` and its
/// "before" commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeCopy {
    /// The conversation.
    pub conversation: ConversationId,
    /// The turn.
    pub turn: TurnId,
    /// The copy's number in the turn, from 1.
    pub n: u32,
    /// The "before" commit.
    pub before: ObjectId,
}

/// What a call did to one path of a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeDelta {
    /// The path.
    pub path: PathBuf,
    /// True for a floor or persistence file.
    pub floor: bool,
    /// True when the path changed, or when a copy does not cover it, so nobody can
    /// tell.
    pub changed: bool,
    /// True when both copies hold the path, so undo can bring it back.
    pub covered: bool,
}

/// What undo does with one home path: it writes `write` when the path is `expect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeStep {
    /// The absolute path, or `@crontab` or `@linger`.
    pub path: PathBuf,
    /// The state that undo gives the path; [`HomeState::NotCovered`] leaves it as it
    /// is, for that reason.
    pub write: HomeState,
    /// The blob of a file or a state that undo writes ([`Snapshots::home_blob`]).
    pub blob: Option<ObjectId>,
    /// What the path must be just before undo writes it.
    pub expect: Expect,
    /// The blob and mode of what is there then (`0o100000 | mode & 0o777` for a file,
    /// `0o120000` for a link), which go to the undo trash first.
    pub trash: Option<(ObjectId, u32)>,
    /// True for a floor or persistence file.
    pub floor: bool,
}

impl HomeStep {
    /// True when undo writes or removes the path.
    pub fn acts(&self) -> bool {
        !matches!(self.write, HomeState::NotCovered(_)) && self.expect != Expect::Unknown
    }
}

/// The home paths of a plan, one step per path: the state before the turn's first copy
/// of the path, and the state after its last copy. A path below a root of the plan
/// belongs to the root's plan, not here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HomePlan {
    /// The steps, sorted by path.
    pub steps: Vec<HomeStep>,
}

impl Snapshots {
    /// Copies the home paths of `entries` before a call of the turn `turn`: a new copy
    /// `home/<n>` of the turn with its "before" commit.
    pub async fn home_before(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        entries: impl HomeReader + 'static,
    ) -> Result<HomeCopy, SnapshotError> {
        let store = self.open_home().await?;
        let slot = self.slot(store.root());
        let _turn = slot.enter().await;
        let n = self.next_copy(&store, conversation, turn).await?;
        let name = format!("{}/{n}", turn_ref(conversation, turn, "home"));
        let meta = self.home_meta(conversation, Some(turn), None, "before");
        let (commit, _) = self.import(&store, &name, None, meta, "before", entries).await?;
        Ok(HomeCopy { conversation, turn, n, before: commit })
    }

    /// Copies the home paths of `entries` after the call of `copy`: the copy's "after"
    /// commit, a child of its "before", and what changed in each path of either.
    pub async fn home_after(
        &self,
        copy: &HomeCopy,
        entries: impl HomeReader + 'static,
    ) -> Result<Vec<HomeDelta>, SnapshotError> {
        let store = self.open_home().await?;
        let slot = self.slot(store.root());
        let _turn = slot.enter().await;
        let before = self.manifest_of(&store, copy.before.as_str()).await?;
        let name = format!("{}/{}", turn_ref(copy.conversation, copy.turn, "home"), copy.n);
        let meta = self.home_meta(copy.conversation, Some(copy.turn), None, "after");
        let from = Some(copy.before.as_str());
        let (_, after) = self.import(&store, &name, from, meta, "after", entries).await?;
        Ok(deltas(&before.entries, &after))
    }

    /// Copies the home paths that the undo `undo_id` changes, as they are before it
    /// writes: `refs/efr-undo/<conversation>/<undo>/home`, for a later redo. A copy
    /// that the undo has already keeps its first one.
    pub async fn home_undo_before(
        &self,
        conversation: ConversationId,
        undo_id: &str,
        entries: impl HomeReader + 'static,
    ) -> Result<ObjectId, SnapshotError> {
        if !valid_undo_id(undo_id) {
            return Err(SnapshotError::InvalidUndoId { id: undo_id.to_owned() });
        }
        let store = self.open_home().await?;
        let slot = self.slot(store.root());
        let _turn = slot.enter().await;
        let name = undo_ref(conversation, undo_id, "home");
        if let Some(kept) = self.ref_value(&store, &name).await {
            return Ok(kept);
        }
        // NOTE: the journal ref of the undo names its turn, which the collector needs to
        // keep this copy as long as the turn.
        let started = undo_ref(conversation, undo_id, "started");
        let turn = match self.ref_value(&store, &started).await {
            Some(commit) => self.turn_of(&store, commit.as_str()).await,
            None => None,
        };
        let meta = self.home_meta(conversation, turn, Some(undo_id), "undo-before");
        let (commit, _) = self.import(&store, &name, None, meta, "undo-before", entries).await?;
        Ok(commit)
    }

    /// The bytes of the blob `id` in the home store.
    pub async fn home_blob(&self, id: &ObjectId) -> Result<Vec<u8>, SnapshotError> {
        self.blob_in(&self.home_store(), id).await
    }

    /// The home store, `$D/snapshots/home.git`. Its "root" is its own git dir, so its
    /// gate is no root's; its root file `home.root` is the stamp of its last copy.
    pub(crate) fn home_store(&self) -> Store {
        Store::with_id(self.dir(), HOME_ID, self.dir().join(format!("{HOME_ID}.git")))
    }

    /// The home store, made when it is not there yet, and stamped as used now.
    async fn open_home(&self) -> Result<Store, SnapshotError> {
        let store = self.home_store();
        if !store.exists() {
            let made = store.clone();
            self.runner().blocking(store.dir(), move || made.make_dir()).await??;
            self.runner().init(&store).await?;
            let made = store.clone();
            self.runner().blocking(store.dir(), move || made.finish_init()).await??;
        }
        let stamped = store.clone();
        let now = std::time::SystemTime::from(self.runner().clock().now());
        self.runner().blocking(store.dir(), move || stamped.stamp(now)).await?;
        Ok(store)
    }

    /// The next copy number of the turn.
    async fn next_copy(
        &self,
        store: &Store,
        conversation: ConversationId,
        turn: TurnId,
    ) -> Result<u32, SnapshotError> {
        let prefix = format!("{}/", turn_ref(conversation, turn, "home"));
        let out = self
            .runner()
            .checked(
                store,
                "for-each-ref",
                &["for-each-ref", "--format=%(refname)", prefix.as_str()],
                Run::default(),
            )
            .await?;
        let most = String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|name| name.strip_prefix(prefix.as_str()))
            .filter_map(|n| n.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        Ok(most + 1)
    }

    /// The commit that the ref `name` of `store` names.
    async fn ref_value(&self, store: &Store, name: &str) -> Option<ObjectId> {
        let out = self
            .runner()
            .run(store, &["rev-parse", "--verify", "--quiet", name], Run::default())
            .await
            .ok()?;
        out.success.then(|| ObjectId::parse(&String::from_utf8_lossy(&out.stdout))).flatten()
    }

    fn home_meta(
        &self,
        conversation: ConversationId,
        turn: Option<TurnId>,
        undo: Option<&str>,
        phase: &str,
    ) -> Meta {
        Meta {
            conversation: conversation.to_string(),
            turn: turn.map(|turn| turn.to_string()).unwrap_or_default(),
            phase: phase.to_owned(),
            root: "~".to_owned(),
            at: self.runner().clock().now().to_string(),
            undo: undo.map(str::to_owned),
            ..Meta::default()
        }
    }

    /// Writes one copy of `entries` with `git fast-import`: the commit `name` (a child
    /// of `from`) with the manifest of `phase`. Returns the commit and the manifest's
    /// items. The caller holds the gate of the home store.
    async fn import(
        &self,
        store: &Store,
        name: &str,
        from: Option<&str>,
        meta: Meta,
        phase: &str,
        entries: impl HomeReader + 'static,
    ) -> Result<(ObjectId, Vec<Item>), SnapshotError> {
        let plan = copy::CommitPlan {
            name: name.to_owned(),
            from: from.map(str::to_owned),
            message: meta.message(),
            time: self.commit_time(),
            phase: phase.to_owned(),
        };
        let (sender, feed) = mpsc::channel(2);
        let mut entries = entries;
        let producer =
            tokio::task::spawn_blocking(move || copy::produce(&mut entries, &plan, sender));
        let args = ["fast-import", "--quiet", "--done", "--date-format=raw"];
        let run = Run { feed: Some(feed), stderr: true, ..Run::default() };
        let (produced, imported) = tokio::join!(producer, self.runner().run(store, &args, run));
        let items = produced.map_err(|_| SnapshotError::TaskFailed)??;
        let output = imported?;
        if !output.success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let error = stderr.lines().find(|line| line.starts_with("fatal:")).unwrap_or_default();
            tracing::warn!(error, "a copy of home files could not be written");
            return Err(SnapshotError::GitFailed {
                command: "fast-import",
                store: store.git_dir(),
            });
        }
        let commit = self
            .ref_value(store, name)
            .await
            .ok_or(SnapshotError::BadOutput { command: "fast-import" })?;
        Ok((commit, items))
    }

    /// The manifest of the copy `commit` of `store`.
    pub(crate) async fn manifest_of(
        &self,
        store: &Store,
        commit: &str,
    ) -> Result<Manifest, SnapshotError> {
        let spec = format!("{commit}:{MANIFEST}");
        let out = self
            .runner()
            .checked(store, "cat-file", &["cat-file", "blob", spec.as_str()], Run::default())
            .await?;
        serde_json::from_slice(&out).map_err(|_| SnapshotError::BadOutput { command: "cat-file" })
    }
}

/// What changed in each path of the copies `before` and `after`. A path that only
/// `after` lists was missing before.
fn deltas(before: &[Item], after: &[Item]) -> Vec<HomeDelta> {
    let mut out = Vec::new();
    for item in after {
        let missing = Item { path: item.path.clone(), ..Item::default() };
        let was = before.iter().find(|known| known.path == item.path).unwrap_or(&missing);
        let covered = was.kind != Kind::NotCovered && item.kind != Kind::NotCovered;
        out.push(HomeDelta {
            path: PathBuf::from(&item.path),
            floor: item.floor || was.floor,
            changed: !covered || !was.same_state(item),
            covered,
        });
    }
    for was in before.iter().filter(|was| !after.iter().any(|item| item.path == was.path)) {
        out.push(HomeDelta {
            path: PathBuf::from(&was.path),
            floor: was.floor,
            changed: true,
            covered: false,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}
