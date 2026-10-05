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

use efr_config::Settings;
use efr_conversation::{ConversationDeps, GitScopeResolver, HostInfo};
use efr_credentials::{FileStore, SecretStore};
use efr_holder::PtyHolder;
use efr_http::{HttpClient, HttpConfig};
use efr_protocol::{DaemonId, DaemonRoots, Mode, PROTOCOL_VERSION, RootDir, RootSource};
use efr_scope::{Git, Home, Registry};
use efr_shell::ScreenFactory;
use efr_stdx::env::Var;
use efr_stdx::paths::{Dirs, RootSource as StdxRootSource, RootSources};
use efr_stdx::rng::{Rng, SystemRng};
use efr_stdx::time::{Clock, SystemClock};
use efr_store::recording::Recordings;
use efr_store::{Store, StoreConfig};
use efr_transport::UnixListener;
use jiff::tz::TimeZone;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::connections::Connections;
use crate::conversations::{self, Conversations};
use crate::discovery::{self, DaemonInfo};
use crate::engine::EngineParts;
use crate::lock::DaemonLock;
use crate::methods::Methods;
use crate::providers::{ProviderFactory, Providers};
use crate::ptys::Ptys;
use crate::reload::{self, Reloads};
use crate::settings::LiveSettings;
use crate::shells::{self, ShellNotices, ShellParts, StoreRecording};
use crate::state::{SCRATCH_DIR, State};
use crate::telemetry::LogFilter;
use crate::tools::{self, DaemonToolbox};
use crate::{DaemonError, gc, notices, reconcile, screens, signals};

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
    /// Where each root in `dirs` came from, for `admin.status`.
    pub root_sources: RootSources,
    /// The running log filter, which a config reload replaces; `None` leaves the logs
    /// to whoever set up tracing.
    pub log: Option<LogFilter>,
    /// Reload when `config.toml` changes on disk.
    pub watch_config: bool,
    /// Reload at each SIGHUP, which `systemctl --user reload efrd` sends.
    pub reload_on_hangup: bool,
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
            .field("root_sources", &self.root_sources)
            .field("log", &self.log.is_some())
            .field("watch_config", &self.watch_config)
            .field("reload_on_hangup", &self.reload_on_hangup)
            .finish_non_exhaustive()
    }
}

impl Deps {
    /// Dependencies on `dirs` and `home` with `clock` and `rng`, an empty shell
    /// environment, and the build's own holder, screens and providers. The roots count
    /// as named by their own variables, and nothing watches the config file or SIGHUP.
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
            root_sources: RootSources::all(StdxRootSource::Variable(Var::DataDir)),
            log: None,
            watch_config: false,
            reload_on_hangup: false,
        }
    }

    /// The process's own: the XDG and `EFR_*` roots, `HOME`, the system clock, an
    /// OS-seeded generator and the process environment for the shells. The config
    /// reloads when the file changes and at each SIGHUP.
    pub fn from_process() -> Result<Self, DaemonError> {
        let (dirs, root_sources) =
            Dirs::resolve_with_sources().map_err(|source| DaemonError::Paths { source })?;
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
        Ok(Deps::new(dirs, home, Arc::new(SystemClock), Arc::new(rng))
            .with_shell_env(shell_env)
            .with_root_sources(root_sources)
            .with_config_watch()
            .with_reload_on_hangup())
    }

    /// Says where each root came from.
    #[must_use]
    pub fn with_root_sources(mut self, sources: RootSources) -> Self {
        self.root_sources = sources;
        self
    }

    /// Replaces `filter` at each config reload that changes `log`.
    #[must_use]
    pub fn with_log_filter(mut self, filter: LogFilter) -> Self {
        self.log = Some(filter);
        self
    }

    /// Reloads the config when `config.toml` changes on disk.
    #[must_use]
    pub fn with_config_watch(mut self) -> Self {
        self.watch_config = true;
        self
    }

    /// Reloads the config at each SIGHUP.
    #[must_use]
    pub fn with_reload_on_hangup(mut self) -> Self {
        self.reload_on_hangup = true;
        self
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
    lock: DaemonLock,
}

/// Starts the daemon, serves until `shutdown` is cancelled and drains. systemd hears
/// `READY=1` once the socket is open and `STOPPING=1` when the drain begins.
pub async fn run(
    config: Settings,
    deps: Deps,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let daemon = start(config, deps).await?;
    notify(&[sd_notify::NotifyState::Ready]);
    tracing::info!(socket = %daemon.socket_path().display(), "ready");
    daemon.serve(shutdown).await
}

/// Runs the startup sequence up to the open socket.
pub async fn start(config: Settings, deps: Deps) -> Result<Daemon, DaemonError> {
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
        root_sources,
        log,
        watch_config,
        reload_on_hangup,
    } = deps;
    // NOTE: checked first, so a runtime root deep below EFR_HOME fails with the fix
    // before anything else happens.
    let socket = dirs.checked_socket_path().map_err(|source| DaemonError::SocketPath { source })?;
    let lock = DaemonLock::acquire(&dirs.lock_path())?;
    let settings = Arc::new(config);
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
            let backend = screens::choose(settings.screen);
            (backend.factory(), backend.as_str().to_owned())
        }
    };
    let shells = shells::sessions(ShellParts {
        settings: settings.shell.clone(),
        integration_dir: shells::integration_dir(dirs.runtime()),
        env: shell_env,
        // NOTE: the auto policy names the programs of every mode, because a turn in any
        // mode may run in this shell.
        trusted_programs: shells::trusted_programs(&settings.permissions.policy(Mode::Auto)),
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
        &settings,
        secrets,
        http,
        Arc::clone(&clock),
        Arc::clone(&rng),
        providers,
        oauth_issuer,
    )?;

    let home = Home::new(home).map_err(|source| DaemonError::Home { source })?;
    let registry_path = Registry::path_in(dirs.config());
    let engine_parts = EngineParts {
        home: home.clone(),
        secrets: secret_root,
        registry: registry_path.clone(),
        config: dirs.config().to_path_buf(),
    };
    let engine = Arc::new(engine_parts.engine(&settings).await?);
    let (engine_sender, engine_receiver) = watch::channel(engine);
    // NOTE: a reload (`reload.rs`) sends new settings here and, when `[permissions]`
    // changed, a new engine on the channel above. Turns read the settings when they
    // start; tool calls read the engine. A change of the mode alone needs no new
    // engine: a turn passes its own.
    let (settings_sender, settings_receiver) = watch::channel(Arc::clone(&settings));
    let connections = Arc::new(Connections::default());
    let toolbox = DaemonToolbox::new(
        tools::registry(&shells)?,
        shells.clone(),
        home.clone(),
        Arc::clone(&clock),
        Arc::clone(&connections),
        settings_receiver.clone(),
    );
    let git = Git::new(Arc::clone(&clock));
    let git = if isolated_git { git.isolated() } else { git };
    let resolver = GitScopeResolver::new(home.clone(), git).with_registry(registry_path);
    let host = match host {
        Some(host) => host,
        None => tokio::task::spawn_blocking(host_info).await.unwrap_or_default(),
    };
    let live_settings = LiveSettings::new(
        settings_receiver,
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
    let roots = roots(&dirs, root_sources);
    let (reloads, reload_requests) = Reloads::new();
    let state = Arc::new(State {
        settings: settings_sender,
        engine: engine_sender,
        engine_parts,
        log,
        reloads,
        roots,
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
        conversations: Conversations::new(Arc::new(live_settings), conversation_deps, ttys),
        connections,
        shells,
        ptys,
        providers,
    });

    // The socket opens only now, after migrations and reconciliation.
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
    let mut background = vec![
        tokio::spawn(notices::follow(Arc::clone(&state), stop.clone())),
        tokio::spawn(gc::collect(Arc::clone(&state), stop.clone())),
        tokio::spawn(reload::serve(Arc::clone(&state), reload_requests, stop.clone())),
    ];
    if watch_config && let Some(watching) = reload::watcher::watch(&state).await {
        let follow = reload::watcher::follow(Arc::clone(&state), watching, stop.clone());
        background.push(tokio::spawn(follow));
    }
    if reload_on_hangup {
        let hangups = signals::hangups()?;
        background.push(tokio::spawn(reload::on_hangup(Arc::clone(&state), hangups, stop.clone())));
    }
    Ok(Daemon { state, listener, store, recording, stop, background, shell_events, socket, lock })
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

    /// The settings watch, for the tests that send new settings as a reload does.
    #[cfg(test)]
    pub(crate) fn settings(&self) -> watch::Sender<Arc<Settings>> {
        self.state.settings.clone()
    }

    /// The hidden shells, for the tests that check what a reload changes in them.
    #[cfg(test)]
    pub(crate) fn shells(&self) -> efr_shell::ShellSessions {
        self.state.shells.clone()
    }

    /// The engine watch, for the tests that check what a reload sends on it.
    #[cfg(test)]
    pub(crate) fn engine(&self) -> watch::Sender<Arc<efr_permissions::Engine>> {
        self.state.engine.clone()
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
        if clock.timeout(CLOSE_GRACE, store.close()).await.is_err() {
            tracing::warn!("the database did not close in time; something still holds it");
        }
        tracing::info!(lock = %lock.path().display(), "stopped");
        drop(lock);
        Ok(())
    }
}

/// The roots in `dirs` with their `sources`, as `admin.status` reports them.
fn roots(dirs: &Dirs, sources: RootSources) -> DaemonRoots {
    let root = |path: &Path, source: StdxRootSource| RootDir {
        path: path.to_path_buf(),
        source: match source {
            StdxRootSource::EfrHome => RootSource::EfrHome,
            StdxRootSource::Xdg => RootSource::Xdg,
            StdxRootSource::RunUser => RootSource::RunUser,
            // NOTE: a source added to efr-stdx later reads as the root's own variable,
            // the one that names a path outright.
            _ => RootSource::DirVariable,
        },
    };
    DaemonRoots {
        config: root(dirs.config(), sources.config),
        data: root(dirs.data(), sources.data),
        state: root(dirs.state(), sources.state),
        runtime: root(dirs.runtime(), sources.runtime),
    }
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
