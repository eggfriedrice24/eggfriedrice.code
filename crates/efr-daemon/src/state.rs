//! What every method handler and background task of a running daemon shares.

use std::sync::Arc;

use efr_config::Settings;
use efr_permissions::Engine;
use efr_protocol::DaemonId;
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

/// The parent of every conversation's `$SCRATCH`, under the data directory.
pub(crate) const SCRATCH_DIR: &str = "scratch";

/// The running daemon's shared state. Handlers get it behind an `Arc`; the actors and
/// tables inside keep their own locks, none held across an await.
#[derive(Debug)]
pub(crate) struct State {
    /// The settings. Readers take the latest value when a unit of work starts: a turn
    /// when it starts, a prompt when it arrives.
    pub(crate) settings: watch::Sender<Arc<Settings>>,
    /// The permission engine that each tool call reads. The conversations hold its
    /// receiver.
    #[expect(dead_code, reason = "the live reload sends a new engine on it")]
    pub(crate) engine: watch::Sender<Arc<Engine>>,
    /// What the engine is built from besides the settings.
    #[expect(dead_code, reason = "the live reload builds the new engine from it")]
    pub(crate) engine_parts: EngineParts,
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
}
