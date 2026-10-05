//! Everything a command needs from the world, gathered once: the directories, the
//! environment, the terminal, time, randomness, keys, Ctrl+C, `Ctrl+\` and the browser.
//!
//! Commands take a [`Context`] instead of reaching for process state themselves, so a
//! test can run a whole command against a fake daemon with a fixed screen, scripted
//! keys and a Ctrl+C it triggers.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use efr_client::{Client, ConnectOptions, Discovered};
use efr_protocol::{CommandId, Origin};
use efr_stdx::env::{Env, Var};
use efr_stdx::paths::{Dirs, RootSources};
use efr_stdx::rng::{Rng, SystemRng};
use efr_stdx::time::{Clock, SystemClock};

use crate::error::CliError;
use crate::keys::{Keys, TtyKeys};
use crate::quit::{CtrlBackslash, Quit};
use crate::settings::Settings;
use crate::terminal::{self, Screen, StdoutScreen, TermFacts};

/// The tracing filter when `EFR_LOG` is unset: only warnings and errors, because
/// stderr shares the terminal with the reply.
pub(crate) const DEFAULT_LOG_FILTER: &str = "warn";

/// A future that resolves when the user asks to stop.
pub(crate) type Stop = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Where Ctrl+C comes from.
pub(crate) trait Interrupt: Send + Sync + fmt::Debug {
    /// Resolves at the next Ctrl+C.
    fn wait(&self) -> Stop;
}

/// Ctrl+C from the terminal, as SIGINT.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CtrlC;

impl Interrupt for CtrlC {
    fn wait(&self) -> Stop {
        Box::pin(async {
            // NOTE: the handler is installed on the first call, so only the commands
            // that follow a stream stop the default SIGINT exit. When it cannot be
            // installed, the default exit stays, which also ends the command.
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        })
    }
}

/// How a URL reaches a browser.
pub(crate) trait Browser: Send + Sync + fmt::Debug {
    /// Opens `url` without waiting for the browser.
    fn open(&self, url: &str) -> io::Result<()>;
}

/// `xdg-open`, the desktop's choice of browser.
#[derive(Debug, Clone, Copy)]
pub(crate) struct XdgOpen;

impl Browser for XdgOpen {
    fn open(&self, url: &str) -> io::Result<()> {
        let mut command = efr_stdx::process::command("xdg-open", "/");
        command.arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        // The child is not awaited: the login waits for the daemon, not the browser.
        command.spawn().map(drop)
    }
}

/// What every command runs with.
#[derive(Debug)]
pub(crate) struct Context {
    pub(crate) dirs: Dirs,
    /// Where each root in `dirs` came from.
    pub(crate) sources: RootSources,
    pub(crate) env: Env,
    /// The editor of `efr config edit`: `$VISUAL`, else `$EDITOR`; `None` uses `vi`.
    pub(crate) editor: Option<String>,
    pub(crate) term: TermFacts,
    pub(crate) settings: Settings,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) rng: Arc<dyn Rng>,
    pub(crate) screen: Arc<dyn Screen>,
    pub(crate) keys: Arc<dyn Keys>,
    pub(crate) interrupt: Arc<dyn Interrupt>,
    /// `Ctrl+\`, which asks to type an input for a command that prints nothing.
    pub(crate) quit: Arc<dyn Quit>,
    pub(crate) browser: Arc<dyn Browser>,
    /// The working directory, for a prompt sent without the plugin's context.
    pub(crate) cwd: Option<PathBuf>,
    /// The terminal on stdin, for a prompt sent without the plugin's context.
    pub(crate) tty: Option<String>,
}

impl Context {
    /// The context of this process.
    pub(crate) async fn from_process(term: TermFacts) -> Result<Context, CliError> {
        let (dirs, sources) =
            Dirs::resolve_with_sources().map_err(|source| CliError::Dirs { source })?;
        let settings = Settings::load(dirs.config()).await;
        let rng = SystemRng::new().map_err(|source| CliError::Random { source })?;
        let keys = TtyKeys { available: term.stdin_tty };
        // NOTE: VISUAL and EDITOR are POSIX conventions, not efr settings, so they are
        // read here and not through efr_stdx::env::Var.
        let editor = ["VISUAL", "EDITOR"]
            .into_iter()
            .filter_map(std::env::var_os)
            .filter_map(|value| value.into_string().ok())
            .find(|value| !value.trim().is_empty());
        Ok(Context {
            dirs,
            sources,
            env: Env::process(),
            editor,
            settings,
            clock: Arc::new(SystemClock),
            rng: Arc::new(rng),
            screen: Arc::new(StdoutScreen),
            keys: Arc::new(keys),
            interrupt: Arc::new(CtrlC),
            quit: Arc::new(CtrlBackslash::new()),
            browser: Arc::new(XdgOpen),
            cwd: std::env::current_dir().ok(),
            tty: if term.stdin_tty { terminal::stdin_tty_name() } else { None },
            term,
        })
    }

    /// Finds the daemon and connects as `origin`, naming `tty` in hello.
    pub(crate) async fn connect(
        &self,
        origin: Origin,
        tty: Option<&str>,
    ) -> Result<Client, CliError> {
        let Discovered { socket, .. } = efr_client::discover(&self.dirs).await?;
        let mut options = ConnectOptions::new(origin, Arc::clone(&self.clock))
            .with_client(concat!("efr ", env!("CARGO_PKG_VERSION")));
        if let Some(tty) = tty {
            options = options.with_tty(tty);
        }
        Ok(Client::connect(&socket, options).await?)
    }

    /// The socket that [`connect`](Self::connect) would use.
    pub(crate) async fn socket(&self) -> Result<PathBuf, CliError> {
        Ok(efr_client::discover(&self.dirs).await?.socket)
    }

    /// A new command id, which makes a write idempotent.
    pub(crate) fn command_id(&self) -> CommandId {
        CommandId::from_uuid(efr_stdx::id::uuid_v7(&*self.clock, &*self.rng))
    }

    /// True when `EFR_OPEN_BROWSER` asks `efr login` to open the URL.
    pub(crate) fn open_browser(&self) -> Result<bool, CliError> {
        self.env.flag(Var::OpenBrowser).map_err(|source| CliError::Environment { source })
    }
}
