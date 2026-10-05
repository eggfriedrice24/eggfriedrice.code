//! Starting, serving and stopping the daemon, in the order ARCHITECTURE.md gives.
//!
//! [`start`] takes the lock, opens and migrates the database (with the backup copy),
//! reconciles what the last daemon left in flight, builds the providers, the tools,
//! the shells and the conversations, opens the socket and writes `daemon.json`.
//! [`Daemon::serve`] answers clients until the shutdown token is cancelled, then drains:
//! connections, background tasks, conversation actors, shells, recordings, the store,
//! and last the lock. [`run`] does both and tells systemd when the daemon is ready.
//!
//! Everything from the outside world arrives in [`Deps`], so an in-process daemon (the
//! `TestDaemon` of `efr-test-daemon`) runs the same code on temporary directories with
//! a manual clock, a seeded generator, its own PTY holder, screens and provider.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_conversation::{ConversationConfig, ConversationDeps, GitScopeResolver, HostInfo};
use efr_credentials::{FileStore, SecretStore};
use efr_holder::PtyHolder;
use efr_http::{HttpClient, HttpConfig};
use efr_permissions::{Engine, Locations};
use efr_protocol::{DaemonId, PROTOCOL_VERSION};
use efr_scope::{Git, Home, Registry};
use efr_shell::ScreenFactory;
use efr_stdx::paths::Dirs;
use efr_stdx::rng::{Rng, SystemRng};
use efr_stdx::time::{Clock, SystemClock};
use efr_store::recording::Recordings;
use efr_store::{Store, StoreConfig};
use efr_transport::UnixListener;
use jiff::tz::TimeZone;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, PermissionSettings};
use crate::connections::Connections;
use crate::conversations::{self, Conversations};
use crate::discovery::{self, DaemonInfo};
use crate::lock::DaemonLock;
use crate::methods::Methods;
use crate::providers::{ProviderFactory, Providers};
use crate::ptys::Ptys;
use crate::shells::{self, ShellNotices, ShellParts, StoreRecording};
use crate::state::{SCRATCH_DIR, State};
use crate::tools::{self, DaemonToolbox};
use crate::{DaemonError, gc, notices, reconcile, screens};

/// The recordings directory under the data directory.
const RECORDINGS_DIR: &str = "recordings";

/// How long the drain waits for the database to close.
const CLOSE_GRACE: Duration = Duration::from_secs(10);

/// What the daemon takes from the outside world.
#[non_exhaustive]
pub struct Deps {
    /// The four efr roots.
    pub dirs: Dirs,
    /// The user's home directory.
    pub home: PathBuf,
    /// Time.
    pub clock: Arc<dyn Clock>,
    /// Randomness.
    pub rng: Arc<dyn Rng>,
    /// The environment the hidden shells inherit (efr-shell removes the daemon's own).
    pub shell_env: BTreeMap<String, String>,
    /// The PTY holder; `None` uses the build's own.
    pub holder: Option<Arc<dyn PtyHolder>>,
    /// The screen factory and the backend name `admin.status` reports; `None` follows
    /// the config.
    pub screens: Option<(Arc<dyn ScreenFactory>, String)>,
    /// Builds the conversations' provider; `None` uses the stored credentials.
    pub providers: Option<Arc<dyn ProviderFactory>>,
    /// The authorization server of the subscription login and its token refresh;
    /// `None` is `https://auth.openai.com`. Tests point it at a local server.
    pub oauth_issuer: Option<String>,
    /// The machine facts of the preamble; `None` reads them from the system.
    pub host: Option<HostInfo>,
    /// The time zone of scratch directory names; `None` uses the system's.
    pub time_zone: Option<TimeZone>,
    /// Keep the database in memory instead of `efr.sqlite`.
    pub in_memory_store: bool,
    /// Run git for scope derivation without the system and global git configuration.
    pub isolated_git: bool,
}

impl fmt::Debug for Deps {
    // The shell environment can hold tokens, so only its names are shown.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Deps")
            .field("dirs", &self.dirs)
            .field("home", &self.home)
            .field("shell_env", &self.shell_env.keys().collect::<Vec<_>>())
            .field("holder", &self.holder.is_some())
            .field("screens", &self.screens.as_ref().map(|(_, name)| name))
            .field("providers", &self.providers.is_some())
            .field("oauth_issuer", &self.oauth_issuer)
            .field("host", &self.host)
            .field("in_memory_store", &self.in_memory_store)
            .field("isolated_git", &self.isolated_git)
            .finish_non_exhaustive()
    }
}

impl Deps {
    /// Dependencies on `dirs` and `home` with `clock` and `rng`, an empty shell
    /// environment, and the build's own holder, screens and providers.
    pub fn new(
        dirs: Dirs,
        home: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
    ) -> Self {
        Deps {
            dirs,
            home: home.into(),
            clock,
            rng,
            shell_env: BTreeMap::new(),
            holder: None,
            screens: None,
            providers: None,
            oauth_issuer: None,
            host: None,
            time_zone: None,
            in_memory_store: false,
            isolated_git: false,
        }
    }

    /// The process's own: the XDG and `EFR_*` roots, `HOME`, the system clock, an
    /// OS-seeded generator and the process environment for the shells.
    pub fn from_process() -> Result<Self, DaemonError> {
        let dirs = Dirs::resolve().map_err(|source| DaemonError::Paths { source })?;
        // NOTE: HOME is a POSIX convention, not an efr setting, so it is read here and
        // not through efr_stdx::env::Var.
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .ok_or(DaemonError::HomeUnknown)?;
        let rng = SystemRng::new().map_err(|source| DaemonError::Random { source })?;
        let shell_env = std::env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .collect();
        Ok(Deps::new(dirs, home, Arc::new(SystemClock), Arc::new(rng)).with_shell_env(shell_env))
    }

    /// Sets the hidden shells' environment.
    #[must_use]
    pub fn with_shell_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.shell_env = env;
        self
    }

    /// Uses `holder` for the hidden shells' PTYs.
    #[must_use]
    pub fn with_holder(mut self, holder: Arc<dyn PtyHolder>) -> Self {
        self.holder = Some(holder);
        self
    }

    /// Uses `factory` for the shells' screens, reported as `backend`.
    #[must_use]
    pub fn with_screens(
        mut self,
        factory: Arc<dyn ScreenFactory>,
        backend: impl Into<String>,
    ) -> Self {
        self.screens = Some((factory, backend.into()));
        self
    }

    /// Uses `factory` for the conversations' provider.
    #[must_use]
    pub fn with_providers(mut self, factory: Arc<dyn ProviderFactory>) -> Self {
        self.providers = Some(factory);
        self
    }

    /// Uses `issuer` as the authorization server of the subscription login and its
    /// token refresh.
    #[must_use]
    pub fn with_oauth_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.oauth_issuer = Some(issuer.into());
        self
    }

    /// Uses `host` as the machine facts.
    #[must_use]
    pub fn with_host(mut self, host: HostInfo) -> Self {
        self.host = Some(host);
        self
    }

    /// Uses `time_zone` for scratch directory names.
    #[must_use]
    pub fn with_time_zone(mut self, time_zone: TimeZone) -> Self {
        self.time_zone = Some(time_zone);
        self
    }

    /// Keeps the database in memory.
    #[must_use]
    pub fn with_in_memory_store(mut self) -> Self {
        self.in_memory_store = true;
        self
    }

    /// Runs git without the machine's and the user's git configuration, as a test
    /// daemon must.
    #[must_use]
    pub fn with_isolated_git(mut self) -> Self {
        self.isolated_git = true;
        self
    }
}

/// A started daemon: the socket is open and `daemon.json` written; nothing is served
/// until [`Daemon::serve`].
#[derive(Debug)]
pub struct Daemon {
    state: Arc<State>,
    listener: UnixListener,
    store: Store,
    recording: Arc<StoreRecording>,
    stop: CancellationToken,
    background: Vec<JoinHandle<()>>,
    shell_events: JoinHandle<()>,
    socket: PathBuf,
    // NOTE: kept so the conversations' engine receiver always has a live sender.
    _engine: watch::Sender<Arc<Engine>>,
    lock: DaemonLock,
}

/// Starts the daemon, serves until `shutdown` is cancelled and drains. systemd hears
/// `READY=1` once the socket is open and `STOPPING=1` when the drain begins.
pub async fn run(
    config: Config,
    deps: Deps,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let daemon = start(config, deps).await?;
    notify(&[sd_notify::NotifyState::Ready]);
    tracing::info!(socket = %daemon.socket_path().display(), "ready");
    daemon.serve(shutdown).await
}

/// Runs the startup sequence up to the open socket.
pub async fn start(config: Config, deps: Deps) -> Result<Daemon, DaemonError> {
    let Deps {
        dirs,
        home,
        clock,
        rng,
        shell_env,
        holder,
        screens,
        providers,
        oauth_issuer,
        host,
        time_zone,
        in_memory_store,
        isolated_git,
    } = deps;
    let lock = DaemonLock::acquire(&dirs.lock_path())?;
    let daemon_id = discovery::daemon_id(dirs.data(), &*clock, &*rng)?;
    tracing::info!(%daemon_id, version = env!("CARGO_PKG_VERSION"), data = %dirs.data().display(), "starting");

    let store = if in_memory_store {
        Store::open_in_memory(Arc::clone(&clock)).await?
    } else {
        Store::open(StoreConfig::in_data_dir(dirs.data()), Arc::clone(&clock)).await?
    };
    let migration = store.migration();
    if migration.applied() {
        tracing::info!(from = migration.from, to = migration.to, backup = ?migration.backup, "database migrated");
    }
    let writer = store.writer().clone();
    let readers = store.readers().clone();
    let reconciled = reconcile::reconcile(&readers, &writer).await?;
    tracing::info!(?reconciled, "reconciled");

    let ptys = Arc::new(Ptys::default());
    let recordings = Recordings::new(
        dirs.data().join(RECORDINGS_DIR),
        writer.clone(),
        readers.clone(),
        Arc::clone(&clock),
    );
    let recording = Arc::new(StoreRecording::new(recordings.clone(), Arc::clone(&ptys)));
    let (shell_notices, notice_queue) = ShellNotices::new(Arc::clone(&ptys));
    let (screens, screen_backend) = match screens {
        Some((factory, backend)) => (factory, backend),
        None => {
            let backend = screens::choose(config.screen);
            (backend.factory(), backend.as_str().to_owned())
        }
    };
    let shells = shells::sessions(ShellParts {
        settings: config.shell.clone(),
        integration_dir: shells::integration_dir(dirs.runtime()),
        env: shell_env,
        trusted_programs: shells::trusted_programs(&config.permissions.policy()),
        holder: holder.unwrap_or_else(shells::default_holder),
        screens,
        recording: Arc::clone(&recording),
        notices: shell_notices,
        clock: Arc::clone(&clock),
        rng: Arc::clone(&rng),
    })?;

    let http = HttpClient::new(&HttpConfig::default(), Arc::clone(&clock), Arc::clone(&rng))
        .map_err(|source| DaemonError::Http { source })?;
    let secrets = FileStore::in_data_dir(&dirs);
    let secret_root = secrets.dir().to_path_buf();
    let secrets: Arc<dyn SecretStore> = Arc::new(secrets);
    let providers = Providers::build(
        &config,
        secrets,
        http,
        Arc::clone(&clock),
        Arc::clone(&rng),
        providers,
        oauth_issuer,
    )?;

    let home = Home::new(home).map_err(|source| DaemonError::Home { source })?;
    let registry_path = Registry::path_in(dirs.config());
    let engine = Arc::new(engine(&home, &secret_root, &config.permissions, &registry_path).await?);
    let (engine_sender, engine_receiver) = watch::channel(engine);
    let toolbox = DaemonToolbox::new(
        tools::registry(&shells)?,
        shells.clone(),
        home.clone(),
        Arc::clone(&clock),
    );
    let git = Git::new(Arc::clone(&clock));
    let git = if isolated_git { git.isolated() } else { git };
    let resolver = GitScopeResolver::new(home.clone(), git).with_registry(registry_path);
    let host = match host {
        Some(host) => host,
        None => tokio::task::spawn_blocking(host_info).await.unwrap_or_default(),
    };
    let conversation_config = conversation_config(
        &config,
        providers.model(),
        dirs.data().join(SCRATCH_DIR),
        host,
        time_zone.unwrap_or_else(TimeZone::system),
    );
    let conversation_deps = ConversationDeps {
        provider: providers.active(),
        toolbox: Arc::new(toolbox),
        engine: engine_receiver,
        scope: Arc::new(resolver),
        writer: writer.clone(),
        readers: readers.clone(),
        clock: Arc::clone(&clock),
        rng: Arc::clone(&rng),
        home,
    };
    let ttys = conversations::load_ttys(&readers).await?;
    let state = Arc::new(State {
        config,
        dirs,
        daemon_id,
        pid: std::process::id(),
        started_at: clock.now(),
        screen_backend,
        clock,
        rng,
        writer: writer.clone(),
        readers,
        recordings,
        conversations: Conversations::new(conversation_config, conversation_deps, ttys),
        connections: Arc::new(Connections::default()),
        shells,
        ptys,
        providers,
    });

    // The socket opens only now, after migrations and reconciliation.
    let socket = state.dirs.socket_path();
    let listener =
        UnixListener::bind(&socket).await.map_err(|source| DaemonError::Transport { source })?;
    let info = DaemonInfo {
        pid: state.pid,
        socket: socket.clone(),
        protocol: PROTOCOL_VERSION,
        daemon_id,
        tailnet_endpoint: None,
    };
    discovery::write(&state.dirs.daemon_json_path(), &info)?;

    // NOTE: the tasks start last, so a startup that fails earlier leaves none behind.
    let stop = CancellationToken::new();
    let shell_events =
        tokio::spawn(shells::follow_notices(notice_queue, writer, Arc::clone(&recording)));
    let mut background = vec![tokio::spawn(notices::follow(Arc::clone(&state), stop.clone()))];
    if state.config.shell.idle_minutes > 0 {
        let idle = Duration::from_secs(state.config.shell.idle_minutes.saturating_mul(60));
        background.push(tokio::spawn(gc::collect(Arc::clone(&state), idle, stop.clone())));
    }
    Ok(Daemon {
        state,
        listener,
        store,
        recording,
        stop,
        background,
        shell_events,
        socket,
        _engine: engine_sender,
        lock,
    })
}

impl Daemon {
    /// The Unix socket clients connect to.
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    /// The daemon's identity.
    pub fn daemon_id(&self) -> DaemonId {
        self.state.daemon_id
    }

    /// The connection table, for the tests that check what it keeps after a client
    /// left.
    #[cfg(test)]
    pub(crate) fn connections(&self) -> Arc<Connections> {
        Arc::clone(&self.state.connections)
    }

    /// Answers clients until `shutdown` is cancelled, then drains and stops.
    pub async fn serve(self, shutdown: CancellationToken) -> Result<(), DaemonError> {
        let Daemon {
            state,
            listener,
            store,
            recording,
            stop,
            background,
            shell_events,
            socket: _,
            _engine,
            lock,
        } = self;
        let methods = Arc::new(Methods::new(Arc::clone(&state)));
        listener.serve(methods, Arc::clone(&state.clock), shutdown.cancelled_owned()).await;

        notify(&[sd_notify::NotifyState::Stopping]);
        tracing::info!("draining");
        stop.cancel();
        for task in background {
            if task.await.is_err() {
                tracing::warn!("a background task panicked");
            }
        }
        state.conversations.shutdown_all().await;
        state.shells.close_all().await;
        recording.close_all().await;
        drop(recording);
        discovery::remove(&state.dirs.daemon_json_path(), state.pid);
        let clock = Arc::clone(&state.clock);
        drop(state);
        // The shells' notices end once the last shell handle is gone with the state.
        if clock.timeout(CLOSE_GRACE, shell_events).await.is_err() {
            tracing::warn!("the shell events did not finish in time");
        }
        drop(_engine);
        if clock.timeout(CLOSE_GRACE, store.close()).await.is_err() {
            tracing::warn!("the database did not close in time; something still holds it");
        }
        tracing::info!(lock = %lock.path().display(), "stopped");
        drop(lock);
        Ok(())
    }
}

/// The permission engine: the home directory and its resolved form, the daemon's own
/// secrets (sealed, so no rule opens them), the secret paths of the config (`~/` below
/// the home directory), and the registered projects, deciding by the built-in rules
/// followed by the user's.
///
/// NOTE: the user's rules belong to the engine, the machine policy, and not to
/// `ConversationConfig::policy`: a conversation's rules may never open a secret or a
/// system path, and the user's explicit rules must be able to.
async fn engine(
    home: &Home,
    secrets: &Path,
    permissions: &PermissionSettings,
    registry: &Path,
) -> Result<Engine, DaemonError> {
    let invalid = |source| DaemonError::Locations { source };
    let path = registry.to_path_buf();
    let projects = match tokio::task::spawn_blocking(move || Registry::load(&path)).await {
        Ok(Ok(registry)) => registry,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "the project registry could not be read; no project is registered");
            Registry::empty()
        }
        Err(_) => Registry::empty(),
    };
    let mut locations = Locations::new(home.path()).map_err(invalid)?;
    // NOTE: an alias the engine refuses (one inside or above the home directory) only
    // costs the resolved form; paths under the home directory itself still classify.
    locations = match locations.clone().with_home_alias(home.canonical()) {
        Ok(aliased) => aliased,
        Err(error) => {
            tracing::warn!(error = %error, alias = %home.canonical().display(), "the resolved home directory is not used as an alias");
            locations
        }
    };
    // NOTE: the daemon's own tokens would let the model act as the user at the
    // provider, so no rule of the user's may open them, not even `class = "secrets"`.
    locations = locations.with_sealed_root(secrets).map_err(invalid)?;
    for path in &permissions.secret_paths {
        let root = match path.strip_prefix("~") {
            Ok(below) => home.path().join(below),
            Err(_) => path.clone(),
        };
        locations = locations.with_secret_root(root).map_err(invalid)?;
    }
    for project in projects.projects() {
        locations = locations.with_project(project.id(), project.root()).map_err(invalid)?;
    }
    Ok(Engine::new(locations, permissions.policy()))
}

/// The conversations' settings from the config.
pub(crate) fn conversation_config(
    config: &Config,
    model: &str,
    scratch_root: PathBuf,
    host: HostInfo,
    time_zone: TimeZone,
) -> ConversationConfig {
    let mut settings = ConversationConfig::new(model, scratch_root)
        .with_system_prompt(config.system_prompt.clone())
        .with_time_zone(time_zone)
        .with_host(host);
    settings.max_output_tokens = config.max_output_tokens;
    settings.max_queued = config.conversation.max_queued;
    settings.approval_timeout = config.conversation.approval_timeout_secs.map(Duration::from_secs);
    settings.update_interval = Duration::from_millis(config.conversation.update_interval_ms);
    settings
}

/// The host name and the distribution, read once; a fact that cannot be read is left
/// out of the preamble.
fn host_info() -> HostInfo {
    let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    let os = std::fs::read_to_string("/etc/os-release").ok().and_then(|text| os_name(&text));
    HostInfo::new(hostname, os)
}

/// `PRETTY_NAME`, else `NAME`, from the text of `/etc/os-release`.
pub(crate) fn os_name(text: &str) -> Option<String> {
    let fields: HashMap<&str, &str> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim().trim_matches('"')))
        .collect();
    fields
        .get("PRETTY_NAME")
        .or_else(|| fields.get("NAME"))
        .filter(|name| !name.is_empty())
        .map(|name| (*name).to_owned())
}

/// Tells systemd, when it supervises the daemon. A failure only costs the notice.
fn notify(states: &[sd_notify::NotifyState<'_>]) {
    if let Err(error) = sd_notify::notify(states) {
        tracing::warn!(error = %error, "could not notify systemd");
    }
}

#[cfg(test)]
mod tests;
