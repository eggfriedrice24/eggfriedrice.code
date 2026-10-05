//! Live reload: reading `config.toml` again while the daemon runs, and applying it.
//!
//! Four triggers call [`reload`]: `admin.config_reload` (`efr config reload`), `SIGHUP`
//! (`systemctl --user reload efrd`), the file watcher ([`watcher`]) and the settings
//! tool after it wrote the file (`tools/settings_tool.rs`). A reload parses
//! and checks the whole file with `efr-config`. A file with an error changes nothing:
//! the old settings stay, the error is kept for `admin.status`, and each terminal with a
//! recent conversation gets a notice. A valid file is laid over the running settings
//! ([`Settings::reloaded`]): a key that needs a restart keeps its running value and is
//! listed, and a value from a variable or a flag still wins. Then every applier takes
//! the new value:
//!
//! - the settings watch, which a turn reads when it starts and a prompt when it
//!   arrives, and the idle shell collector at each look;
//! - the permission engine, sent again when the `[permissions]` table changed or what
//!   it protects did (the links in the config directory, the registered projects),
//!   and the trusted programs of the hidden shells when the table changed;
//! - how new hidden shells start (`shell.program`, `shell.login`);
//! - the log filter.
//!
//! Reloads run one at a time in one task ([`serve`]), so two triggers at once never
//! interleave their sends; a trigger asks it through [`reload`] and waits for the
//! outcome.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_config::{CONFIG_FILE, ConfigError, FileState, Reloaded, Settings};
use efr_protocol::{AdminConfigReloadResult, ConfigFileError, ConfigStatus, Mode};
use jiff::SignedDuration;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::notices::{self, NOTICES_DIR};
use crate::shells;
use crate::state::State;

pub(crate) mod watcher;

/// How many conversations one read for the reload notices asks for at a time.
const PAGE: u32 = 256;

/// Reload requests that may wait for the reload task.
const REQUESTS: usize = 16;

/// One request for a reload: what asked for it, and where the outcome goes.
#[derive(Debug)]
pub(crate) struct Request {
    trigger: &'static str,
    reply: oneshot::Sender<AdminConfigReloadResult>,
}

#[cfg(test)]
impl Request {
    /// Answers the request with `result`, as the reload task would.
    pub(crate) fn answer(self, result: AdminConfigReloadResult) {
        // A trigger that stopped waiting needs no answer.
        let _ = self.reply.send(result);
    }
}

/// The way to the reload task, and what the last reload left, for `admin.status`.
/// Cheap to clone; clones share both, so the settings tool asks the same task.
#[derive(Debug, Clone)]
pub(crate) struct Reloads {
    requests: mpsc::Sender<Request>,
    last: Arc<Mutex<Outcome>>,
}

/// The state of the last reload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// What was wrong with the file, until a reload succeeds.
    pub(crate) error: Option<ConfigFileError>,
    /// The keys whose new values wait for a restart.
    pub(crate) restart_needed: Vec<String>,
}

impl Reloads {
    /// The reloads and the requests that [`serve`] answers.
    pub(crate) fn new() -> (Self, mpsc::Receiver<Request>) {
        let (requests, received) = mpsc::channel(REQUESTS);
        (Reloads { requests, last: Arc::default() }, received)
    }

    pub(crate) fn last(&self) -> Outcome {
        self.lock().clone()
    }

    /// Asks the reload task to read the config file again and apply it, and returns
    /// how that went. Fails only when the task has stopped, as it does when the daemon
    /// drains.
    pub(crate) async fn request(
        &self,
        trigger: &'static str,
    ) -> Result<AdminConfigReloadResult, DaemonError> {
        let (reply, answer) = oneshot::channel();
        let stopped = || DaemonError::ReloadStopped;
        self.requests.send(Request { trigger, reply }).await.map_err(|_| stopped())?;
        answer.await.map_err(|_| stopped())
    }

    fn lock(&self) -> MutexGuard<'_, Outcome> {
        // The outcome is replaced whole, so a poisoned lock still holds a usable one.
        self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The config file and the last reload, as `admin.status` reports them.
pub(crate) async fn status(state: &State) -> ConfigStatus {
    let path = state.dirs.config().join(CONFIG_FILE);
    let file = file_state(&path).await;
    let Outcome { error, restart_needed } = state.reloads.last();
    ConfigStatus {
        path,
        exists: file.exists,
        symlink_target: file.symlink_target,
        reload_error: error,
        restart_needed,
    }
}

/// What is at `path`, looked at off the async workers.
pub(crate) async fn file_state(path: &Path) -> FileState {
    let owned = path.to_path_buf();
    match tokio::task::spawn_blocking(move || FileState::of(&owned)).await {
        Ok(file) => file,
        // NOTE: only a panic gets here; looking again on this worker costs one stat.
        Err(_) => FileState::of(path),
    }
}

/// Asks the reload task to read the config file again and apply it, and returns how
/// that went. Fails only when the task has stopped, as it does when the daemon drains.
pub(crate) async fn reload(
    state: &State,
    trigger: &'static str,
) -> Result<AdminConfigReloadResult, DaemonError> {
    state.reloads.request(trigger).await
}

/// Answers reload requests one at a time, until `stop`.
pub(crate) async fn serve(
    state: Arc<State>,
    mut requests: mpsc::Receiver<Request>,
    stop: CancellationToken,
) {
    loop {
        let request = tokio::select! {
            () = stop.cancelled() => return,
            request = requests.recv() => request,
        };
        let Some(Request { trigger, reply }) = request else {
            return;
        };
        let result = reload_now(&state, trigger).await;
        // A trigger that stopped waiting needs no answer.
        let _ = reply.send(result);
    }
}

/// Reads the config file again and applies it, then says how that went.
#[tracing::instrument(skip_all, fields(trigger = trigger))]
async fn reload_now(state: &State, trigger: &'static str) -> AdminConfigReloadResult {
    let outcome = match read(state).await {
        Ok(next) => apply(state, next).await,
        Err(error) => Err(error.file_error()),
    };
    let previous = state.reloads.last();
    let (now, result) = match outcome {
        Ok(restart_needed) => {
            tracing::info!(restart_needed = ?restart_needed, "config reloaded");
            let now = Outcome { error: None, restart_needed: restart_needed.clone() };
            (now, AdminConfigReloadResult { applied: true, error: None, restart_needed })
        }
        Err(error) => {
            tracing::warn!(error = %error.message, "config.toml has an error; the old settings stay");
            let restart_needed = previous.restart_needed.clone();
            let now =
                Outcome { error: Some(error.clone()), restart_needed: restart_needed.clone() };
            (now, AdminConfigReloadResult { applied: false, error: Some(error), restart_needed })
        }
    };
    *state.reloads.lock() = now.clone();
    if let Some(line) = notice(&previous, &now) {
        tell_terminals(state, line).await;
    }
    result
}

/// Reloads at each SIGHUP in `hangups`, until `stop`.
pub(crate) async fn on_hangup(
    state: Arc<State>,
    mut hangups: tokio::signal::unix::Signal,
    stop: CancellationToken,
) {
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            got = hangups.recv() => match got {
                // The outcome is logged and kept for admin.status; a stopped task means
                // the daemon drains.
                Some(()) => {
                    if reload(&state, "SIGHUP").await.is_err() {
                        return;
                    }
                }
                None => return,
            },
        }
    }
}

/// The settings of the file as it is now, with the variables and flags of the start.
async fn read(state: &State) -> Result<Settings, ConfigError> {
    let dir = state.dirs.config().to_path_buf();
    match tokio::task::spawn_blocking(move || Settings::load(&dir)).await {
        Ok(loaded) => loaded,
        Err(join) => Err(ConfigError::Read {
            path: state.dirs.config().join(CONFIG_FILE),
            source: std::io::Error::other(join),
        }),
    }
}

/// Lays `next` over the running settings and hands each applier its new value.
/// Returns the keys that wait for a restart, or why nothing was applied.
async fn apply(state: &State, next: Settings) -> Result<Vec<String>, ConfigFileError> {
    let running = Arc::clone(&state.settings.borrow());
    let Reloaded { settings, restart_needed, .. } =
        running.reloaded(next).map_err(|error| error.file_error())?;
    if settings.log != running.log {
        crate::telemetry::parse(&settings.log).map_err(|error| refused(&error, Some("log")))?;
    }
    // NOTE: everything that can fail runs before the first send, so a refused file
    // changes nothing.
    // NOTE: the engine is built again on every reload, because what it protects also
    // follows the links in the config directory and the project registry, which a
    // reload reads again: a `config.toml` link that now points elsewhere must be
    // write-sealed from the next tool call on. It is sent only when it differs.
    let rules_changed = settings.permissions != running.permissions;
    let current = Arc::clone(&state.engine.borrow());
    let engine = match state.engine_parts.engine(&settings).await {
        Ok(engine) if rules_changed || engine.locations() != current.locations() => Some(engine),
        Ok(_) => None,
        Err(error) if rules_changed => return Err(refused(&error, None)),
        Err(error) => {
            tracing::warn!(error = %error, "the permission engine stays as it was");
            None
        }
    };
    if let Some(engine) = engine {
        state.engine.send_replace(Arc::new(engine));
    }
    if rules_changed {
        // NOTE: the auto policy names the programs of every mode, as at the start.
        let auto = settings.permissions.policy(Mode::Auto);
        state.shells.set_trusted_programs(shells::trusted_programs(&auto));
    }
    let shell = &settings.shell;
    if (shell.program.as_ref(), shell.login)
        != (running.shell.program.as_ref(), running.shell.login)
        && let Err(error) = state.shells.set_start(shell.program.as_deref(), shell.login)
    {
        tracing::warn!(error = %error, "new hidden shells keep starting as before");
    }
    if settings.log != running.log
        && let Some(log) = &state.log
        && let Err(error) = log.set(&settings.log)
    {
        tracing::warn!(error = %error, "the log filter stays as it was");
    }
    if settings != *running {
        state.settings.send_replace(Arc::new(settings));
    }
    Ok(restart_needed)
}

/// `error` as the wire error of a refused file, belonging to `key` when one is known.
fn refused(error: &DaemonError, key: Option<&str>) -> ConfigFileError {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    ConfigFileError {
        message: message.split_whitespace().collect::<Vec<_>>().join(" "),
        line: None,
        column: None,
        key: key.map(str::to_owned),
    }
}

/// The line each terminal hears when the outcome changed from `previous` to `now`: a
/// new error, or a new set of keys that wait for a restart.
pub(crate) fn notice(previous: &Outcome, now: &Outcome) -> Option<String> {
    if let Some(error) = &now.error {
        if previous.error.as_ref() == Some(error) {
            return None;
        }
        let place = match (&error.key, error.line) {
            (Some(key), Some(line)) => format!(" ({key}, line {line})"),
            (Some(key), None) => format!(" ({key})"),
            (None, Some(line)) => format!(" (line {line})"),
            (None, None) => String::new(),
        };
        return Some(format!(
            "efr: config.toml has an error; the old settings stay: {}{place}",
            error.message
        ));
    }
    if now.restart_needed.is_empty() || now.restart_needed == previous.restart_needed {
        return None;
    }
    Some(format!("efr: restart efrd to apply: {}", now.restart_needed.join(", ")))
}

/// Writes `line` to the notice file of every terminal whose conversation was active
/// within `conversation.tty_idle_hours` (every terminal when it is 0).
async fn tell_terminals(state: &State, line: String) {
    let hours = state.settings.borrow().conversation.tty_idle_hours;
    let since = match i64::try_from(hours) {
        Ok(hours) if hours > 0 => {
            state.clock.now().checked_sub(SignedDuration::from_hours(hours)).ok()
        }
        _ => None,
    };
    let ttys = match recent_ttys(state, since).await {
        Ok(ttys) => ttys,
        Err(error) => {
            tracing::warn!(error = %error, "the terminals to tell about the config could not be read");
            return;
        }
    };
    let dir = state.dirs.runtime().join(NOTICES_DIR);
    let written = tokio::task::spawn_blocking(move || {
        for tty in ttys {
            if let Err(error) = notices::append(&dir, &tty, &line) {
                tracing::warn!(error = %error, "a config notice could not be written");
            }
        }
    })
    .await;
    if written.is_err() {
        tracing::warn!("writing the config notices panicked");
    }
}

/// The terminals of conversations updated since `since` (all when `None`).
async fn recent_ttys(
    state: &State,
    since: Option<jiff::Timestamp>,
) -> Result<Vec<String>, DaemonError> {
    let mut ttys = Vec::new();
    let mut before = None;
    loop {
        let page = state
            .readers
            .with(move |conn| efr_store::conversations::list(conn, before, PAGE))
            .await?;
        let Some(last) = page.last() else {
            break;
        };
        before = Some(last.last_seq);
        let mut older = false;
        for summary in page {
            if since.is_some_and(|since| summary.updated_at < since) {
                older = true;
                continue;
            }
            if let Some(tty) = summary.tty
                && !ttys.contains(&tty)
            {
                ttys.push(tty);
            }
        }
        // The list is newest first, so a page that reached older ones is the last.
        if older {
            break;
        }
    }
    Ok(ttys)
}

#[cfg(test)]
mod tests;
