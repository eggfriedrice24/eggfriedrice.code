//! What every method handler and background task of a running daemon shares.

use std::sync::{Arc, Mutex};

use efr_config::Settings;
use efr_conversation::ConversationDraft;
use efr_permissions::Engine;
use efr_protocol::{DaemonId, DaemonRoots};
use efr_scope::Git;
use efr_shell::ShellSessions;
use efr_snapshot::Snapshots;
use efr_stdx::paths::Dirs;
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use efr_store::recording::Recordings;
use efr_store::{Readers, WriterHandle};
use jiff::Timestamp;
use tokio::sync::{broadcast, watch};

use crate::connections::Connections;
use crate::conversations::Conversations;
use crate::engine::EngineParts;
use crate::full::FullTurns;
use crate::jobs::JobTable;
use crate::presence::PresenceTable;
use crate::proc_view::ProcView;
use crate::providers::Providers;
use crate::ptys::Ptys;
use crate::reload::Reloads;
use crate::repo_hint::RepoHints;
use crate::sandbox::SandboxService;
use crate::shells::StoreRecording;
use crate::telemetry::LogFilter;

/// The parent of every conversation's `$SCRATCH`, under the data directory.
pub(crate) const SCRATCH_DIR: &str = "scratch";

/// The running daemon's shared state. Handlers get it behind an `Arc`; the actors and
/// tables inside keep their own locks, none held across an await.
#[derive(Debug)]
pub(crate) struct State {
    /// The settings. Readers take the latest value when a unit of work starts: a turn
    /// when it starts, a prompt when it arrives. A reload sends the new value.
    pub(crate) settings: watch::Sender<Arc<Settings>>,
    /// The permission engine that each tool call reads. The conversations hold its
    /// receiver; a reload that changes `[permissions]`, or what the engine protects,
    /// sends a new one.
    pub(crate) engine: watch::Sender<Arc<Engine>>,
    /// What the engine is built from besides the settings.
    pub(crate) engine_parts: EngineParts,
    /// How the daemon runs git, for the root of a project that `admin.project_add`
    /// finds from a directory.
    pub(crate) git: Git,
    /// Held while a client's change of the project registry reads and writes the file,
    /// so two changes never overwrite each other.
    pub(crate) registry_writes: Arc<Mutex<()>>,
    /// The running log filter, which a reload replaces; `None` when the caller set up
    /// tracing itself, as a test does.
    pub(crate) log: Option<LogFilter>,
    /// The last reload's outcome, and the lock that runs reloads one at a time.
    pub(crate) reloads: Reloads,
    /// The four roots and where each came from, for `admin.status`.
    pub(crate) roots: DaemonRoots,
    pub(crate) dirs: Dirs,
    pub(crate) daemon_id: DaemonId,
    pub(crate) pid: u32,
    /// efrd's own cgroup when the systemd user unit runs it (`sandbox/peers.rs`, DV49);
    /// `None` for a hand-started efrd and a test daemon.
    pub(crate) unit_cgroup: Option<String>,
    pub(crate) started_at: Timestamp,
    pub(crate) screen_backend: String,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) rng: Arc<dyn Rng>,
    pub(crate) writer: WriterHandle,
    pub(crate) readers: Readers,
    /// The drafts of every running turn. `conversation.subscribe` subscribes one
    /// receiver for each subscriber that asked for drafts; nothing stores them.
    pub(crate) drafts: broadcast::Sender<ConversationDraft>,
    pub(crate) recordings: Recordings,
    /// The writer side of the recordings: `pty.resize` stores each new size there.
    pub(crate) recording: Arc<StoreRecording>,
    pub(crate) conversations: Conversations,
    pub(crate) connections: Arc<Connections>,
    pub(crate) shells: ShellSessions,
    pub(crate) ptys: Arc<Ptys>,
    pub(crate) providers: Providers,
    /// The `auto` sandbox: the launcher, the probe's status, the call dirs.
    pub(crate) sandbox: SandboxService,
    /// efr's own snapshot store, which `conversation.diff` reads and the collector
    /// keeps small.
    pub(crate) snapshots: Snapshots,
    /// Whether a person is at each conversation: the input marks of `methods.rs`,
    /// `presence.set`, and the conversations' reader.
    pub(crate) presence: Arc<PresenceTable>,
    /// The background jobs, which `jobs.list`, `jobs.stop` and the peer check read.
    pub(crate) jobs: JobTable,
    /// True when efrd runs as root, read once at the start: full mode is refused then.
    pub(crate) runs_as_root: bool,
    /// The full work that runs, for the config hold of a reload.
    pub(crate) full: FullTurns,
    /// Whether a shell runs and which terminal a peer runs on, for routing: `/proc`,
    /// or a test's fake.
    pub(crate) procs: Arc<dyn ProcView>,
    /// When efrd last showed the hint for each git repository that is not a project
    /// (`repo_hint.rs`), read from the state root at start.
    pub(crate) repo_hints: RepoHints,
}
