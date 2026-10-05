//! `TestDaemon`: the real efr daemon, in-process, on throwaway directories.
//!
//! [`TestDaemon::builder`] starts `efr_daemon::start` with everything a test must
//! control injected: temporary XDG roots and home ([`TestDirs`]), the socket under the
//! temporary runtime directory, vt100 screens, a [`TestClock`], a seeded [`TestRng`],
//! an in-memory store (or a file store for a restart), isolated git, fixed machine
//! facts, the [`FakePtyHolder`] (or the build's own `LocalPtyHolder` on request), and a
//! [`ReplayProvider`] (or the real OpenAI provider against a [`ResponsesServer`]).
//! The daemon serves in a task of its own; the test talks to it through
//! `efr_client::Client`, moves time with [`TestDaemon::clock`] and reads the event log
//! with [`TestDaemon::events`].
//!
//! Without a provider, the daemon gets a replay provider with an empty transcript, so a
//! test can never reach the network by accident.

mod responses;

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{DirBuilder, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_client::{Client, ConnectOptions, ItemStream};
use efr_daemon::{Config, DaemonError, Deps, HostInfo, Provider, ProviderFactory, ScreenChoice};
use efr_protocol::{
    CommandId, ConversationHistory, ConversationHistoryResult, ConversationId,
    ConversationSubscribe, ConversationSubscribeItem, DaemonId, Event, EventEnvelope, Method,
    Origin, PromptSend, Seq, ShellContext,
};
use efr_test_support::{Redactor, ReplayProvider, TestClock, TestDirs, TestRng, Transcript};
use futures::StreamExt as _;
use jiff::tz::TimeZone;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use self::responses::{ReceivedRequest, ResponsesAnswer, ResponsesServer, TokenRequest};
use crate::{FakePtyHolder, TestDaemonError};

/// The system prompt of a test daemon: short, so provider requests in fixtures stay
/// readable.
pub const SYSTEM_PROMPT: &str = "You are efr, under test.";

/// The host name in the preamble of a test daemon.
pub const HOSTNAME: &str = "testhost";

/// The distribution in the preamble of a test daemon.
pub const OS: &str = "TestOS";

/// The API key the test daemon stores for the `openai-api` provider when it talks to
/// a [`ResponsesServer`].
pub const API_KEY: &str = "sk-efr-test-key";

/// The access token of the subscription login the test daemon stores for the
/// `openai-subscription` provider when it talks to a [`ResponsesServer`].
pub const SUBSCRIPTION_ACCESS_TOKEN: &str = "efr-test-access-1";

/// The refresh token of that login.
pub const SUBSCRIPTION_REFRESH_TOKEN: &str = "efr-test-refresh-1";

/// The ChatGPT account of that login.
pub const SUBSCRIPTION_ACCOUNT: &str = "efr-test-account";

/// When that login's access token expires: far enough that only a 401 refreshes it.
const SUBSCRIPTION_EXPIRES_AT: &str = "2099-01-01T00:00:00Z";

/// The terminal the tests' shell clients say hello from, and the one a replay's
/// prompts name; prompts without a conversation id go to its active conversation.
pub const TTY: &str = "/dev/pts/efr-test";

/// The seed of the daemon's generator unless the builder sets another.
pub const DEFAULT_SEED: u64 = 7;

/// The working directory of a test, below the stand-in home.
const PROJECT_DIR: &str = "home/project";

/// The placeholder of the stand-in home in transcripts.
pub const HOME_PLACEHOLDER: &str = "<HOME>";

/// The zsh the fake holder pretends to run. Only its name matters: a `zsh` gets the
/// efr integration, so commands are delimited by OSC 133 marks.
const FAKE_ZSH: &str = "/usr/bin/zsh";

/// The startup files a real zsh reads, created empty in the stand-in home so a real
/// shell never asks to configure itself.
const ZSH_STARTUP_FILES: &[&str] = &[".zshenv", ".zprofile", ".zshrc", ".zlogin"];

/// The provider credential the `openai-api` provider reads.
const API_CREDENTIAL: &str = "secrets/openai-api.json";

/// The provider credential the `openai-subscription` provider reads and refreshes.
pub const SUBSCRIPTION_CREDENTIAL: &str = "secrets/openai-subscription.json";

/// Which PTY holder the daemon gets.
#[derive(Debug, Clone)]
enum HolderChoice {
    Fake(Arc<FakePtyHolder>),
    /// The build's own: `efr_pty::LocalPtyHolder` when efr-daemon has `local-pty`.
    Local,
}

/// Which provider the daemon's conversations talk to.
#[derive(Debug, Clone)]
enum ProviderChoice {
    Replay(Arc<ReplayProvider>),
    /// A provider the test wrote itself.
    Custom(Arc<dyn Provider>),
    /// The real `openai-api` provider with this base URL.
    Responses(String),
    /// The real `openai-subscription` provider with this base URL, refreshing its
    /// login at this issuer.
    Subscription {
        base_url: String,
        issuer: String,
    },
}

/// What a start needs, kept for a restart.
#[derive(Debug, Clone)]
struct Settings {
    config: Config,
    seed: u64,
    holder: HolderChoice,
    provider: ProviderChoice,
    persistent: bool,
    shell_env: Option<BTreeMap<String, String>>,
}

/// Builds a [`TestDaemon`].
#[derive(Debug)]
pub struct TestDaemonBuilder {
    dirs: Option<Arc<TestDirs>>,
    clock: Option<TestClock>,
    settings: Settings,
}

impl Default for TestDaemonBuilder {
    fn default() -> Self {
        let mut config = Config::default();
        config.screen = ScreenChoice::Vt100;
        config.system_prompt = SYSTEM_PROMPT.to_owned();
        config.shell.login = false;
        // Idle shells stay unless a test turns the collector on.
        config.shell.idle_minutes = 0;
        TestDaemonBuilder {
            dirs: None,
            clock: None,
            settings: Settings {
                config,
                seed: DEFAULT_SEED,
                holder: HolderChoice::Fake(FakePtyHolder::new()),
                provider: ProviderChoice::Replay(Arc::new(empty_replay())),
                persistent: false,
                shell_env: None,
            },
        }
    }
}

impl TestDaemonBuilder {
    /// Runs the daemon on `dirs` instead of a new tree, such as the tree of a daemon
    /// that stopped.
    #[must_use]
    pub fn dirs(mut self, dirs: Arc<TestDirs>) -> Self {
        self.dirs = Some(dirs);
        self
    }

    /// Uses `clock` instead of a new [`TestClock`].
    #[must_use]
    pub fn clock(mut self, clock: TestClock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Seeds the daemon's generator with `seed` instead of [`DEFAULT_SEED`].
    #[must_use]
    pub fn seed(mut self, seed: u64) -> Self {
        self.settings.seed = seed;
        self
    }

    /// Changes the daemon's config. The builder has already chosen vt100 screens,
    /// [`SYSTEM_PROMPT`], a non-login shell and no idle collector.
    #[must_use]
    pub fn config(mut self, change: impl FnOnce(&mut Config)) -> Self {
        change(&mut self.settings.config);
        self
    }

    /// Uses `holder` for the hidden shells (the default is a new [`FakePtyHolder`]).
    #[must_use]
    pub fn fake_pty(mut self, holder: Arc<FakePtyHolder>) -> Self {
        self.settings.holder = HolderChoice::Fake(holder);
        self
    }

    /// Uses the build's own PTY holder, so the hidden shells are real zsh processes.
    /// It is `efr_pty::LocalPtyHolder` only when `efr-daemon` is built with its
    /// `local-pty` feature, which its own tests are; otherwise every shell start fails.
    #[must_use]
    pub fn local_pty(mut self) -> Self {
        self.settings.holder = HolderChoice::Local;
        self
    }

    /// The environment of real hidden shells (the default names the stand-in home,
    /// `PATH=/usr/bin:/bin` and `LANG=C.UTF-8`).
    #[must_use]
    pub fn shell_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.settings.shell_env = Some(env);
        self
    }

    /// Answers every model call from `provider`.
    #[must_use]
    pub fn provider(mut self, provider: Arc<ReplayProvider>) -> Self {
        self.settings.provider = ProviderChoice::Replay(provider);
        self
    }

    /// Answers every model call from `provider`, a provider the test wrote itself, for
    /// a test whose requests cannot be known in advance (a real shell's output).
    #[must_use]
    pub fn custom_provider(mut self, provider: Arc<dyn Provider>) -> Self {
        self.settings.provider = ProviderChoice::Custom(provider);
        self
    }

    /// Runs the real `openai-api` provider against `server`, with [`API_KEY`] stored
    /// as its credential.
    #[must_use]
    pub fn responses(mut self, server: &ResponsesServer) -> Self {
        self.settings.provider = ProviderChoice::Responses(server.base_url());
        self
    }

    /// Runs the real `openai-subscription` provider against `server`, with a login
    /// ([`SUBSCRIPTION_ACCESS_TOKEN`], [`SUBSCRIPTION_REFRESH_TOKEN`],
    /// [`SUBSCRIPTION_ACCOUNT`]) stored as its credential and `server` as the issuer
    /// its token source refreshes at. The login is stored once per tree, so a
    /// restart keeps what a refresh saved.
    #[must_use]
    pub fn subscription(mut self, server: &ResponsesServer) -> Self {
        self.settings.provider =
            ProviderChoice::Subscription { base_url: server.base_url(), issuer: server.issuer() };
        self
    }

    /// Keeps the database in `efr.sqlite` under the temporary data directory, so a
    /// restart on the same tree finds it.
    #[must_use]
    pub fn persistent(mut self) -> Self {
        self.settings.persistent = true;
        self
    }

    /// Starts the daemon and serves it in a task of its own.
    pub async fn start(self) -> Result<TestDaemon, TestDaemonError> {
        let dirs = match self.dirs {
            Some(dirs) => dirs,
            None => Arc::new(TestDirs::new()?),
        };
        let clock = self.clock.unwrap_or_default();
        TestDaemon::launch(dirs, clock, self.settings, 0).await
    }
}

/// The real daemon, serving on a socket in a temporary directory.
///
/// Dropping it cancels the daemon without waiting; [`stop`](Self::stop) waits for the
/// drain and reports how it went.
pub struct TestDaemon {
    dirs: Arc<TestDirs>,
    clock: TestClock,
    cwd: PathBuf,
    redactor: Redactor,
    settings: Settings,
    generation: u64,
    socket: PathBuf,
    daemon_id: DaemonId,
    shutdown: CancellationToken,
    served: Option<JoinHandle<Result<(), DaemonError>>>,
}

impl TestDaemon {
    /// A builder with the defaults described in the module docs.
    pub fn builder() -> TestDaemonBuilder {
        TestDaemonBuilder::default()
    }

    /// A daemon with every default.
    pub async fn start() -> Result<TestDaemon, TestDaemonError> {
        TestDaemon::builder().start().await
    }

    async fn launch(
        dirs: Arc<TestDirs>,
        clock: TestClock,
        settings: Settings,
        generation: u64,
    ) -> Result<TestDaemon, TestDaemonError> {
        let (cwd, redactor) = working_dir(&dirs)?;
        let home = dirs.home().to_path_buf();
        let mut config = settings.config.clone();
        let shell_env = settings.shell_env.clone().unwrap_or_else(|| default_shell_env(&home));
        // NOTE: a later start draws other ids, so a restarted daemon never mints an id
        // that the first one stored.
        let seed = settings.seed.wrapping_add(generation);
        let mut deps =
            Deps::new(dirs.dirs().clone(), &home, clock.shared(), Arc::new(TestRng::new(seed)))
                .with_shell_env(shell_env)
                .with_isolated_git()
                .with_host(HostInfo::new(Some(HOSTNAME.to_owned()), Some(OS.to_owned())))
                .with_time_zone(TimeZone::UTC);
        if !settings.persistent {
            deps = deps.with_in_memory_store();
        }
        match &settings.holder {
            HolderChoice::Fake(holder) => {
                if config.shell.program.is_none() {
                    config.shell.program = Some(PathBuf::from(FAKE_ZSH));
                }
                deps = deps.with_holder(Arc::clone(holder) as Arc<dyn efr_daemon::PtyHolder>);
            }
            HolderChoice::Local => {
                for file in ZSH_STARTUP_FILES {
                    let path = home.join(file);
                    if !path.exists() {
                        std::fs::write(&path, "")
                            .map_err(|source| TestDaemonError::Write { path, source })?;
                    }
                }
            }
        }
        match &settings.provider {
            ProviderChoice::Replay(provider) => {
                let provider: Arc<dyn Provider> = Arc::clone(provider) as Arc<dyn Provider>;
                deps = deps.with_providers(Arc::new(FixedProvider(provider)));
            }
            ProviderChoice::Custom(provider) => {
                deps = deps.with_providers(Arc::new(FixedProvider(Arc::clone(provider))));
            }
            ProviderChoice::Responses(base_url) => {
                config.provider = efr_daemon::API.to_owned();
                config.openai.api_base_url = Some(base_url.clone());
                store_api_key(dirs.dirs().data())?;
            }
            ProviderChoice::Subscription { base_url, issuer } => {
                config.provider = efr_daemon::SUBSCRIPTION.to_owned();
                config.openai.subscription_base_url = Some(base_url.clone());
                deps = deps.with_oauth_issuer(issuer.clone());
                store_login(dirs.dirs().data())?;
            }
        }
        let daemon = efr_daemon::start(config, deps).await?;
        let socket = daemon.socket_path().to_path_buf();
        let daemon_id = daemon.daemon_id();
        let shutdown = CancellationToken::new();
        let served = tokio::spawn(daemon.serve(shutdown.clone()));
        Ok(TestDaemon {
            dirs,
            clock,
            cwd,
            redactor,
            settings,
            generation,
            socket,
            daemon_id,
            shutdown,
            served: Some(served),
        })
    }

    /// The daemon's socket.
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    /// The daemon's id, the same after a restart on the same tree.
    pub fn daemon_id(&self) -> DaemonId {
        self.daemon_id
    }

    /// The temporary tree: the four efr roots and the stand-in home.
    pub fn dirs(&self) -> &Arc<TestDirs> {
        &self.dirs
    }

    /// The working directory of the test's prompts, `<home>/project`.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The clock the daemon runs on. Moving it fires the daemon's timers.
    pub fn clock(&self) -> &TestClock {
        &self.clock
    }

    /// Placeholders for the temporary root (`<TMP>`), the working directory (`<CWD>`)
    /// and the stand-in home (`<HOME>`), and timestamps.
    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    /// The fake PTY holder, unless the daemon runs real shells.
    pub fn holder(&self) -> Option<&Arc<FakePtyHolder>> {
        match &self.settings.holder {
            HolderChoice::Fake(holder) => Some(holder),
            HolderChoice::Local => None,
        }
    }

    /// The replay provider, when the daemon answers from one.
    pub fn provider(&self) -> Option<&Arc<ReplayProvider>> {
        match &self.settings.provider {
            ProviderChoice::Replay(provider) => Some(provider),
            ProviderChoice::Custom(_)
            | ProviderChoice::Responses(_)
            | ProviderChoice::Subscription { .. } => None,
        }
    }

    /// How a client of this daemon connects: as the shell plugin (`Origin::Shell`),
    /// with timeouts on the test clock.
    pub fn connect_options(&self) -> ConnectOptions {
        self.connect_options_from(Origin::Shell)
    }

    /// How a client of kind `origin` connects, such as `Origin::Phone` for a test of
    /// the scopes.
    pub fn connect_options_from(&self, origin: Origin) -> ConnectOptions {
        ConnectOptions::new(origin, self.clock.shared()).with_client("efr-test-daemon")
    }

    /// Connects with [`connect_options`](Self::connect_options) and says hello.
    pub async fn client(&self) -> Result<Client, TestDaemonError> {
        self.connect(self.connect_options()).await
    }

    /// Connects as the shell plugin of the terminal `tty`.
    pub async fn client_for_tty(&self, tty: &str) -> Result<Client, TestDaemonError> {
        self.connect(self.connect_options().with_tty(tty)).await
    }

    /// Connects with `options` and says hello.
    pub async fn connect(&self, options: ConnectOptions) -> Result<Client, TestDaemonError> {
        Ok(Client::connect(&self.socket, options).await?)
    }

    /// A `prompt.send` of `text` with the command id [`command_id(n)`](command_id), from
    /// the shell of `tty` in the test's working directory.
    pub fn prompt(&self, n: u128, text: &str, tty: &str) -> Method {
        let mut context = ShellContext::new(&self.cwd);
        context.tty = Some(tty.to_owned());
        Method::PromptSend(PromptSend {
            command_id: command_id(n),
            conversation_id: None,
            new_conversation: false,
            text: text.to_owned(),
            context: Some(context),
            last_command: None,
        })
    }

    /// Subscribes `client` to `conversation_id` from the start of its log.
    pub async fn follow(
        &self,
        client: &Client,
        conversation_id: ConversationId,
    ) -> Result<ItemStream<ConversationSubscribeItem>, TestDaemonError> {
        let params = ConversationSubscribe {
            conversation_id,
            after_seq: Some(Seq::ZERO),
            answers_input: false,
        };
        Ok(client.stream(Method::ConversationSubscribe(params)).await?)
    }

    /// Every event of `conversation_id` in the log, oldest first, read page by page
    /// with `conversation.history` over `client`.
    pub async fn events(
        &self,
        client: &Client,
        conversation_id: ConversationId,
    ) -> Result<Vec<EventEnvelope>, TestDaemonError> {
        let mut pages = Vec::new();
        let mut cursor = None;
        loop {
            let page: ConversationHistoryResult = client
                .call(Method::ConversationHistory(ConversationHistory {
                    conversation_id,
                    cursor,
                    limit: None,
                }))
                .await?;
            cursor = page.next_cursor;
            pages.push(page.events);
            if cursor.is_none() {
                break;
            }
        }
        Ok(pages.into_iter().rev().flatten().collect())
    }

    /// Stops the daemon and waits for its drain: connections end, actors and shells
    /// stop, the store closes and the lock is released.
    pub async fn stop(mut self) -> Result<(), TestDaemonError> {
        self.drain().await
    }

    /// Stops the daemon and starts a new one in its place on the same tree, clock,
    /// holder and provider, as systemd's restart would. Only a
    /// [`persistent`](TestDaemonBuilder::persistent) daemon keeps its database across
    /// it. Clients of the old daemon are disconnected.
    pub async fn restart(&mut self) -> Result<(), TestDaemonError> {
        self.drain().await?;
        let dirs = Arc::clone(&self.dirs);
        let clock = self.clock.clone();
        let settings = self.settings.clone();
        let generation = self.generation.wrapping_add(1);
        *self = TestDaemon::launch(dirs, clock, settings, generation).await?;
        Ok(())
    }

    async fn drain(&mut self) -> Result<(), TestDaemonError> {
        self.shutdown.cancel();
        match self.served.take() {
            Some(served) => {
                served.await.map_err(|source| TestDaemonError::DaemonTask { source })??;
                Ok(())
            }
            None => Ok(()),
        }
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        // A test that did not stop the daemon still must not leave it serving.
        self.shutdown.cancel();
    }
}

impl fmt::Debug for TestDaemon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestDaemon")
            .field("root", &self.dirs.root())
            .field("socket", &self.socket)
            .field("daemon_id", &self.daemon_id)
            .field("generation", &self.generation)
            .field("running", &self.served.is_some())
            .finish_non_exhaustive()
    }
}

/// The command id `0192f0c1-7a00-7000-8000-<n as 12 hex digits>`, the form the
/// scenario fixtures use.
pub fn command_id(n: u128) -> CommandId {
    let text = format!("0192f0c1-7a00-7000-8000-{:012x}", n & 0xffff_ffff_ffff);
    match text.parse() {
        Ok(id) => id,
        Err(_) => unreachable!("a hyphenated UUID with hex digits always parses"),
    }
}

/// The events of `stream` up to and including the first one that `stop` accepts. The
/// events of a snapshot item count as received in order.
pub async fn events_until(
    stream: &mut ItemStream<ConversationSubscribeItem>,
    mut stop: impl FnMut(&Event) -> bool,
) -> Result<Vec<EventEnvelope>, TestDaemonError> {
    let mut seen = Vec::new();
    while let Some(item) = stream.next().await {
        let events = match item? {
            ConversationSubscribeItem::Event(envelope) => vec![envelope],
            ConversationSubscribeItem::Snapshot(snapshot) => snapshot.events,
            _ => Vec::new(),
        };
        for envelope in events {
            let done = stop(&envelope.event);
            seen.push(envelope);
            if done {
                return Ok(seen);
            }
        }
    }
    Err(TestDaemonError::StreamEnded)
}

/// Hands the daemon one provider for every provider id.
struct FixedProvider(Arc<dyn Provider>);

impl ProviderFactory for FixedProvider {
    fn provider(&self, _id: &str) -> Result<Arc<dyn Provider>, DaemonError> {
        Ok(Arc::clone(&self.0))
    }
}

impl fmt::Debug for FixedProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FixedProvider").field(&self.0.id()).finish()
    }
}

/// The working directory of a test's prompts, created, and the redactor of a daemon on
/// `dirs`: `<TMP>` for the tree, `<CWD>` for the working directory, `<HOME>` for the
/// stand-in home, and timestamps.
pub(crate) fn working_dir(dirs: &TestDirs) -> Result<(PathBuf, Redactor), TestDaemonError> {
    let cwd = dirs.create_dir(PROJECT_DIR)?;
    let home = dirs.home().to_string_lossy().into_owned();
    let redactor = dirs.redactor().cwd(&cwd).replace(home, HOME_PLACEHOLDER);
    Ok((cwd, redactor))
}

/// A replay provider that refuses every request: the default, so a test without a
/// provider never reaches the network.
fn empty_replay() -> ReplayProvider {
    match ReplayProvider::new(&Transcript::default()) {
        Ok(provider) => provider,
        Err(_) => unreachable!("an empty transcript has no record that could be invalid"),
    }
}

/// The environment of a real hidden shell: the stand-in home and a plain `PATH`.
fn default_shell_env(home: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("HOME".to_owned(), home.to_string_lossy().into_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
    ])
}

/// Stores [`API_KEY`] as the `openai-api` credential.
fn store_api_key(data: &Path) -> Result<(), TestDaemonError> {
    let record = serde_json::json!({ "version": 1, "kind": "api_key", "key": API_KEY });
    store_credential(&data.join(API_CREDENTIAL), &record)
}

/// Stores the test login as the `openai-subscription` credential, unless one is there
/// already: a refresh before a restart saved a newer one.
fn store_login(data: &Path) -> Result<(), TestDaemonError> {
    let path = data.join(SUBSCRIPTION_CREDENTIAL);
    if path.exists() {
        return Ok(());
    }
    let record = serde_json::json!({
        "version": 1,
        "kind": "oauth",
        "access_token": SUBSCRIPTION_ACCESS_TOKEN,
        "refresh_token": SUBSCRIPTION_REFRESH_TOKEN,
        "expires_at": SUBSCRIPTION_EXPIRES_AT,
        "account_id": SUBSCRIPTION_ACCOUNT,
    });
    store_credential(&path, &record)
}

/// Writes `record` to `path` the way the daemon's file store keeps a credential: the
/// directory 0700, the file 0600.
fn store_credential(path: &Path, record: &serde_json::Value) -> Result<(), TestDaemonError> {
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|source| TestDaemonError::Write { path: dir.to_path_buf(), source })?;
    let write = |path: &Path| -> std::io::Result<()> {
        let mut file =
            OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        file.write_all(format!("{record}\n").as_bytes())
    };
    write(path).map_err(|source| TestDaemonError::Write { path: path.to_path_buf(), source })
}

#[cfg(test)]
mod tests;
