//! What every method handler and background task of a running daemon shares.

use std::sync::{Arc, Mutex};

use efr_config::Settings;
use efr_permissions::Engine;
use efr_protocol::{DaemonId, DaemonRoots};
use efr_scope::Git;
use efr_shell::ShellSessions;
use efr_stdx::paths::Dirs;
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use efr_store::recording::Recordings;
use efr_store::{Readers, WriterHandle};
use jiff::Timestamp;
use tokio::sync::watch;

use crate::connections::Connections;
use crate::conversations::Conversations;
use crate::engine::EngineParts;
use crate::providers::Providers;
use crate::ptys::Ptys;
use crate::reload::Reloads;
use crate::sandbox::SandboxService;
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
    pub(crate) started_at: Timestamp,
    pub(crate) screen_backend: String,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) rng: Arc<dyn Rng>,
    pub(crate) writer: WriterHandle,
    pub(crate) readers: Readers,
    pub(crate) recordings: Recordings,
    pub(crate) conversations: Conversations,
    pub(crate) connections: Arc<Connections>,
    pub(crate) shells: ShellSessions,
    pub(crate) ptys: Arc<Ptys>,
    pub(crate) providers: Providers,
    /// The `auto` sandbox: the launcher, the probe's status, the call dirs.
    pub(crate) sandbox: SandboxService,
}
