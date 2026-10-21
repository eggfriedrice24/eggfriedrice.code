//! Live reload: reading `config.toml` again while the daemon runs, and applying it.
//!
//! These triggers call [`reload`]: `admin.config_reload` (`efr config reload`), `SIGHUP`
//! (`systemctl --user reload efrd`), the file watcher ([`watcher`]), the settings tool
//! after it wrote the file (`tools/settings_tool.rs`), and `admin.project_add` and
//! `admin.project_remove` after they wrote the project registry. A reload parses
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
//!   it protects did (the links in the config directory, the registered projects), or
//!   the always rules of `sandbox.allow` with their disabled marks (phase E), and the
//!   trusted programs of the hidden shells when the table changed;
//! - how new hidden shells start (`shell.program`, `shell.login`);
//! - the log filter.
//!
//! A reloaded `[model] name` of another company than the running `[model] provider`
//! gets a warning in the log, as at the start, but only when the reload changed it: a
//! reload of the same file warns no more (C3.1).
//!
//! The reply to a reload carries its warnings (DV44), so `efr config reload` shows what
//! went only to the log before: the foreign `[model] name` above, each always rule that
//! a floor now disables, and a bus filter that the reload turned off. Each comes once,
//! from the reload that changed it. A change that the config hold keeps back is the
//! reply's `error`, as before, and is no warning.
//!
//! The config hold (full mode, DF17): a command of a full turn runs as the user, so it
//! can write `config.toml`. While full work runs ([`crate::full::FullTurns`]), a reload
//! whose new settings loosen permissions ([`crate::loosens::loosening`]) is held: the
//! old settings stay, one WARN goes to the log, terminals hear it once, and
//! `admin.status` names the keys that loosen (`ConfigStatus::held`). The hold is sticky:
//! after the full work ended, each reload holds again while the file still loosens
//! against the running settings. Only `admin.config_reload`, which the user's own
//! `efr config reload` sends, applies a held change, and only when no full work runs;
//! while it runs, that reload is refused with a text that says so. While a change is
//! held, the running settings are kept in the state dir ([`kept`]), so a restart of
//! efrd starts with them and the change stays held. A reload whose
//! settings loosen nothing applies as before and ends the hold. A held reload still
//! applies the keys that turn full mode off or down (the gate off, a lower default mode,
//! undo or the snapshots off), so the user can stop a full turn while a change is held.
//!
//! When the settings that a reload installed refuse full mode, efrd stops every job that
//! a call of full mode started, and only those (`JobStopBy::FullOff`, DV37). The stop
//! comes after `install` and reads the installed settings from the watch, so it can
//! never come first: a full call after it meets the new gate, and a full job that
//! starts while the stop runs stops at once (`jobs.rs`, `JobTable::begin`). Such a stop
//! starts no job notice turn; its notice waits for the next prompt.
//!
//! When a reload turns `jobs.forward_ports` from true to false, `install` stops the port
//! forwards of every running job after it sent the settings, with a `stopped` event for
//! each port (`JobTable::forwards_off`, DL11). A job that read the old settings and
//! starts after the stop forwards nothing, and a later `true` does not open the
//! forwards of a running job again.
//!
//! Reloads run one at a time in one task ([`serve`]), so two triggers at once never
//! interleave their sends; a trigger asks it through [`reload`] and waits for the
//! outcome.
//!
//! The same task rebuilds the engine when a probe changed what the sandbox supports
//! ([`Reloads::rebuild`]): [`follow_probes`] watches the probe's status and asks for a
//! rebuild when the network view or the proxy of its last result differs from the
//! running engine's, and [`follow_mirrors`] asks for one when a read of the pacman files
//! found other mirror hosts. Each engine that the task sends gives its host list to the
//! proxy, which every running listener reads (phase D2).
//! A rebuild uses the running settings and the last probe; it never reads the file
//! again and never starts a probe, so a probe and a rebuild cannot start each other in
//! a loop. The task is the only one that sends on the engine watch after the start.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_config::{CONFIG_FILE, ConfigError, FileState, Reloaded, Settings};
use efr_permissions::{Egress, Engine};
use efr_protocol::{
    AdminConfigReloadResult, BusProxyOff, ConfigFileError, ConfigStatus, HeldConfig, Mode,
    NetworkMode, SandboxStatus,
};
use jiff::SignedDuration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::engine::Probed;
use crate::notices::{self, NOTICES_DIR};
use crate::shells;
use crate::state::State;

pub(crate) mod kept;
pub(crate) mod watcher;

/// How many conversations one read for the reload notices asks for at a time.
const PAGE: u32 = 256;

/// Reload requests that may wait for the reload task.
const REQUESTS: usize = 16;

/// One request for the reload task.
#[derive(Debug)]
pub(crate) enum Request {
    /// Read the config file again and apply it: what asked for it, and where the
    /// outcome goes.
    Reload { trigger: &'static str, reply: oneshot::Sender<AdminConfigReloadResult> },
    /// Build the engine again from the running settings and the last probe, and send it
    /// when what the sandbox supports changed; the answer says whether it was sent.
    Rebuild { reply: oneshot::Sender<bool> },
}

#[cfg(test)]
impl Request {
    /// Answers the request as the reload task would: a reload with `result`, a rebuild
    /// with "nothing sent".
    pub(crate) fn answer(self, result: AdminConfigReloadResult) {
        // A trigger that stopped waiting needs no answer.
        match self {
            Request::Reload { reply, .. } => {
                let _ = reply.send(result);
            }
            Request::Rebuild { reply } => {
                let _ = reply.send(false);
            }
        }
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
    /// The change of the file that loosens permissions and that the config hold keeps
    /// back, until a reload applies the file or the file no longer loosens.
    pub(crate) held: Option<HeldConfig>,
}

/// The trigger of the user's own `efr config reload`: the only one that applies a held
/// change, and only while no full work runs.
const USER_RELOAD: &str = "admin.config_reload";

/// What `efr config reload` reads while full work runs and the file loosens
/// permissions.
pub(crate) const HELD_WHILE_FULL: &str = "efr does not load a change that loosens \
     permissions while a full turn or its job runs; run efr config reload again after it ends";

/// What the config hold does with a reload of `trigger` whose new settings loosen the
/// keys `loosening`, while full work runs when `full_runs` is true and with a held
/// change when `held` is true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hold {
    /// The reload applies, as before full mode.
    Apply,
    /// The old settings stay and the change is held, with the keys that loosen.
    Keep(Vec<&'static str>),
    /// The user's reload while full work runs: refused, and the change stays held.
    Refuse(Vec<&'static str>),
}

/// See [`Hold`]. Every trigger but the user's own reload holds: the file watcher,
/// SIGHUP and the project methods, and also the settings tool and the writer of an
/// always rule, whose reload reads the whole file and so would apply a held change
/// that the user never saw.
pub(crate) fn hold(
    trigger: &str,
    loosening: Vec<&'static str>,
    full_runs: bool,
    held: bool,
) -> Hold {
    if loosening.is_empty() {
        return Hold::Apply;
    }
    if trigger == USER_RELOAD {
        return if full_runs { Hold::Refuse(loosening) } else { Hold::Apply };
    }
    if full_runs || held { Hold::Keep(loosening) } else { Hold::Apply }
}

/// The text of a held change with the keys of `held` that loosen, for the log, the
/// terminals and the result of the reload.
pub(crate) fn held_text(held: &HeldConfig) -> String {
    format!(
        "config.toml changed while a full turn ran, and the change loosens {}. If you did \
         not make this change, take it out of the file. Then run efr config reload.",
        held.loosens.join(", ")
    )
}

/// What [`apply`] did.
enum Applied {
    /// The new settings apply; the keys that wait for a restart, and the warnings of
    /// the reload.
    Done { restart_needed: Vec<String>, warnings: Vec<String> },
    /// The config hold kept the old settings: the keys that loosen, whether it refused
    /// the user's own reload ([`Hold::Refuse`]), and the warnings of what still applied.
    Held { loosens: Vec<&'static str>, refused: bool, warnings: Vec<String> },
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

    /// Marks `held` as the change that the hold keeps back since before this start
    /// ([`kept::restore`]).
    pub(crate) fn held_at_start(&self, held: Option<HeldConfig>) {
        self.lock().held = held;
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
        self.requests.send(Request::Reload { trigger, reply }).await.map_err(|_| stopped())?;
        answer.await.map_err(|_| stopped())
    }

    /// Asks the reload task to build the engine again from the running settings and the
    /// last probe, and returns whether it sent a new one. It reads no file and starts no
    /// probe. Fails only when the task has stopped.
    pub(crate) async fn rebuild(&self) -> Result<bool, DaemonError> {
        let (reply, answer) = oneshot::channel();
        let stopped = || DaemonError::ReloadStopped;
        self.requests.send(Request::Rebuild { reply }).await.map_err(|_| stopped())?;
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
    let Outcome { error, restart_needed, held } = state.reloads.last();
    ConfigStatus {
        path,
        exists: file.exists,
        symlink_target: file.symlink_target,
        reload_error: error,
        restart_needed,
        held,
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
        // A trigger that stopped waiting needs no answer.
        match request {
            Some(Request::Reload { trigger, reply }) => {
                let result = reload_now(&state, trigger).await;
                let _ = reply.send(result);
            }
            Some(Request::Rebuild { reply }) => {
                let sent = rebuild_now(&state).await;
                let _ = reply.send(sent);
            }
            None => return,
        }
    }
}

/// Builds the engine again from the running settings and what the last probe found,
/// and sends it when what the sandbox supports changed. Returns whether it sent one.
///
/// NOTE: it reads no file and starts no probe: a probe asks for it, so a probe here
/// could ask for the next rebuild without end.
#[tracing::instrument(skip_all)]
async fn rebuild_now(state: &State) -> bool {
    let settings = Arc::clone(&state.settings.borrow());
    let current = Arc::clone(&state.engine.borrow());
    state.sandbox.proxy().warn_fallback(&settings, &state.sandbox.current());
    match state.engine_parts.engine(&settings, Probed::of(&state.sandbox)).await {
        Ok(engine)
            if engine.support() != current.support() || engine.hosts() != current.hosts() =>
        {
            tracing::info!(
                netview = engine.support().netview,
                proxy = engine.support().egress == Egress::Proxy,
                "the engine follows the probe"
            );
            state.sandbox.proxy().set_hosts(engine.hosts());
            state.engine.send_replace(Arc::new(engine));
            true
        }
        Ok(_) => false,
        Err(error) => {
            tracing::warn!(error = %error, "the permission engine does not follow the probe");
            false
        }
    }
}

/// Asks `reloads` for a rebuild of the engine each time the probe's status in `status`
/// says other than the running engine in `engine` about the network view, until
/// `stop`. The view needs both the user's `network.netview` of `settings` and a passed
/// self-test, as the engine's build reads them.
///
/// NOTE: a probe never waits for the rebuild that it causes, and a rebuild never
/// probes, so the two cannot start each other in a loop.
pub(crate) async fn follow_probes(
    reloads: Reloads,
    mut status: watch::Receiver<SandboxStatus>,
    settings: watch::Receiver<Arc<Settings>>,
    engine: watch::Receiver<Arc<Engine>>,
    stop: CancellationToken,
) {
    loop {
        let (netview, proxy) = {
            let settings = settings.borrow();
            let status = status.borrow_and_update();
            (
                settings.network.netview && status.netview,
                settings.network.mode == NetworkMode::Proxy && status.proxy,
            )
        };
        let support = *engine.borrow().support();
        let differs = netview != support.netview || proxy != (support.egress == Egress::Proxy);
        if differs && reloads.rebuild().await.is_err() {
            return;
        }
        // NOTE: biased, so a status that came before the stop is still followed.
        tokio::select! {
            biased;
            changed = status.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = stop.cancelled() => return,
        }
    }
}

/// Asks `reloads` for a rebuild of the engine each time a read of the pacman files in
/// `mirrors` found other mirror hosts, until `stop`: the rebuild sends the new list to
/// the engine and the proxy (DP4).
pub(crate) async fn follow_mirrors(
    reloads: Reloads,
    mut mirrors: watch::Receiver<Arc<[efr_permissions::MirrorHost]>>,
    stop: CancellationToken,
) {
    loop {
        tokio::select! {
            changed = mirrors.changed() => {
                if changed.is_err() || reloads.rebuild().await.is_err() {
                    return;
                }
            }
            () = stop.cancelled() => return,
        }
    }
}

/// Reads the config file again and applies it, then says how that went.
#[tracing::instrument(skip_all, fields(trigger = trigger))]
async fn reload_now(state: &State, trigger: &'static str) -> AdminConfigReloadResult {
    let outcome = match read(state).await {
        Ok(next) => apply(state, next, trigger).await,
        Err(error) => Err(error.file_error()),
    };
    let previous = state.reloads.last();
    let (now, result) = match outcome {
        Ok(Applied::Done { restart_needed, warnings }) => {
            tracing::info!(restart_needed = ?restart_needed, "config reloaded");
            let now = Outcome { error: None, restart_needed: restart_needed.clone(), held: None };
            (now, AdminConfigReloadResult { applied: true, error: None, restart_needed, warnings })
        }
        Ok(Applied::Held { loosens, refused, warnings }) => {
            let held =
                HeldConfig { loosens: loosens.iter().map(|key| (*key).to_owned()).collect() };
            if previous.held.as_ref() != Some(&held) {
                tracing::warn!(loosens = ?held.loosens, "{}", held_text(&held));
            }
            let message = if refused { HELD_WHILE_FULL.to_owned() } else { held_text(&held) };
            let error = ConfigFileError { message, line: None, column: None, key: None };
            let restart_needed = previous.restart_needed.clone();
            // NOTE: the file loads, so no error stays; the change waits as held.
            let now =
                Outcome { error: None, restart_needed: restart_needed.clone(), held: Some(held) };
            let result = AdminConfigReloadResult {
                applied: false,
                error: Some(error),
                restart_needed,
                warnings,
            };
            (now, result)
        }
        Err(error) => {
            tracing::warn!(error = %error.message, "config.toml has an error; the old settings stay");
            let restart_needed = previous.restart_needed.clone();
            let now = Outcome {
                error: Some(error.clone()),
                restart_needed: restart_needed.clone(),
                held: previous.held.clone(),
            };
            let result = AdminConfigReloadResult {
                applied: false,
                error: Some(error),
                restart_needed,
                warnings: Vec::new(),
            };
            (now, result)
        }
    };
    *state.reloads.lock() = now.clone();
    // NOTE: the settings that run while a change is held are kept on disk, so a restart
    // of efrd does not apply the change (`kept`).
    match (&previous.held, &now.held) {
        (_, Some(_)) => {
            let running = Arc::clone(&state.settings.borrow());
            kept::write(state.dirs.state(), &running).await;
        }
        (Some(_), None) => kept::remove(state.dirs.state()).await,
        (None, None) => {}
    }
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

/// Lays `next` over the running settings and hands each applier its new value, unless
/// the config hold keeps it back for a reload of `trigger`; a held reload still applies
/// what turns full mode off or down ([`full_lowered`]). Returns the keys that wait for a
/// restart and the warnings, or what the hold kept, or why nothing was applied.
async fn apply(
    state: &State,
    next: Settings,
    trigger: &'static str,
) -> Result<Applied, ConfigFileError> {
    let running = Arc::clone(&state.settings.borrow());
    let Reloaded { settings, restart_needed, .. } =
        running.reloaded(next).map_err(|error| error.file_error())?;
    // NOTE: before anything is built or sent, so a held change changes nothing but what
    // turns full mode down.
    let held = state.reloads.last().held.is_some();
    let loosening = crate::loosens::loosening(&running, &settings);
    let (loosens, refused) = match hold(trigger, loosening, state.full.running(), held) {
        Hold::Apply => {
            let warnings = install(state, &running, settings).await?;
            follow_full_gate(state);
            return Ok(Applied::Done { restart_needed, warnings });
        }
        Hold::Keep(loosens) => (loosens, false),
        Hold::Refuse(loosens) => (loosens, true),
    };
    // NOTE: the hold keeps back what loosens, never a change that turns full mode off or
    // down, so the user can always stop a full turn from the file (DF16).
    let lowered = full_lowered(&running, &settings);
    let mut warnings = Vec::new();
    if lowered != *running {
        warnings = install(state, &running, lowered).await?;
        follow_full_gate(state);
    }
    Ok(Applied::Held { loosens, refused, warnings })
}

/// The step after [`install`]: marks full mode off in the count of full work when the
/// settings that run now refuse it, or allowed again, and when it is off, stops every
/// job that a full call started (DV37).
///
/// NOTE: it reads the settings that `install` sent, never the ones that the reload
/// read, so it cannot act before them. It marks full mode off before it looks at the
/// jobs: a full job that enters the table after the look sees the mark and stops.
pub(crate) fn follow_full_gate(state: &State) {
    let settings = Arc::clone(&state.settings.borrow());
    let off = full_refused(&settings, state.runs_as_root);
    state.full.set_off(off);
    if !off {
        return;
    }
    let stopped = state.jobs.stop_full();
    if !stopped.is_empty() {
        tracing::info!(jobs = stopped.len(), "full mode is off; efrd stops the jobs of full calls");
    }
}

/// True when `settings` refuse full mode in an efrd that runs as root when `root`:
/// `permissions.allow_full` is false, or a refusal of efrd holds.
pub(crate) fn full_refused(settings: &Settings, root: bool) -> bool {
    let status = crate::full::status(settings, root);
    !status.allowed || status.refused.is_some()
}

/// The running settings with each key of `next` that turns full mode off or down: the
/// gate off, a lower default mode, undo or the snapshots off. These keys never loosen,
/// so a held reload still applies them.
fn full_lowered(running: &Settings, next: &Settings) -> Settings {
    let mut lowered = running.clone();
    if !next.permissions.allow_full {
        lowered.permissions.allow_full = false;
    }
    if next.permissions.mode < running.permissions.mode {
        lowered.permissions.mode = next.permissions.mode;
    }
    if !next.undo.enabled {
        lowered.undo.enabled = false;
    }
    if !next.snapshot.enabled {
        lowered.snapshot.enabled = false;
    }
    lowered
}

/// Hands each applier its value of `settings`, which replace `running`, and returns the
/// warnings of the change for the reply (DV44). Everything that can fail runs before the
/// first send, so a refused file changes nothing.
async fn install(
    state: &State,
    running: &Settings,
    settings: Settings,
) -> Result<Vec<String>, ConfigFileError> {
    if settings.log != running.log {
        crate::telemetry::parse(&settings.log).map_err(|error| refused(&error, Some("log")))?;
    }
    // NOTE: everything that can fail runs before the first send, so a refused file
    // changes nothing.
    // NOTE: the engine is built again on every reload, because what it protects also
    // follows the links in the config directory and the project registry, which a
    // reload reads again: a `config.toml` link that now points elsewhere must be
    // write-sealed from the next tool call on. It is sent only when it differs, also in
    // what the sandbox supports (`snapshot.enabled`, `undo.enabled` and
    // `sandbox.ask_destructive`, plan PB17).
    let rules_changed = settings.permissions != running.permissions;
    let current = Arc::clone(&state.engine.borrow());
    // NOTE: a reload reads the pacman files again (DP4); the engine's list holds their
    // mirror hosts.
    state.sandbox.proxy().read_mirrors(true).await;
    let engine = match state.engine_parts.engine(&settings, Probed::of(&state.sandbox)).await {
        // NOTE: an `a` answer changes only `sandbox.allow`, and the next call must see
        // its rule; a rule that a floor disables now, or no longer, counts too (DE28).
        Ok(engine)
            if rules_changed
                || engine.locations() != current.locations()
                || engine.support() != current.support()
                || engine.hosts() != current.hosts()
                || engine.allow_rules() != current.allow_rules() =>
        {
            Some(engine)
        }
        Ok(_) => None,
        Err(error) if rules_changed => return Err(refused(&error, None)),
        Err(error) => {
            tracing::warn!(error = %error, "the permission engine stays as it was");
            None
        }
    };
    // NOTE: the shells get the new trusted programs before the engine that trusts them,
    // so no call that only the new engine allows reaches a shell that still keeps an
    // alias of such a program. A shell that restarts early under the old engine is
    // harmless.
    if rules_changed {
        // NOTE: the cautious policy names the programs of every mode, as at the start:
        // auto contains every line and names no program of its own.
        let cautious = settings.permissions.policy(Mode::Cautious);
        state.shells.set_trusted_programs(shells::trusted_programs(&cautious));
    }
    let probe_again = probes_again(running, &settings, &current, engine.as_ref());
    let mut warnings = Vec::new();
    if let Some(engine) = engine {
        warnings.extend(crate::always::warn_disabled(&engine, Some(&current)));
        // NOTE: the list reaches every running listener at once (DP3); a running call
        // keeps its own grants.
        state.sandbox.proxy().set_hosts(engine.hosts());
        state.engine.send_replace(Arc::new(engine));
    }
    state.sandbox.proxy().warn_fallback(&settings, &state.sandbox.current());
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
    // NOTE: `[model] provider` needs a restart, so the check is against the running one.
    // A reload that left the foreign name as it was warns no more: a start that runs a
    // reload, or one save that the watcher sees twice, logged it a second time (C3.1).
    if settings.foreign_model() != running.foreign_model()
        && let Some(warning) = crate::providers::warn_foreign_model(&settings)
    {
        warnings.push(warning);
    }
    warnings.extend(bus_warning(state, running, &settings));
    if running.permissions.mode != Mode::Full {
        crate::full::warn_default(&settings, state.runs_as_root);
    }
    let forwards_off = running.jobs.forward_ports && !settings.jobs.forward_ports;
    if settings != *running {
        state.settings.send_replace(Arc::new(settings));
    }
    // NOTE: after the settings went out: a job that read the old ones finds the count
    // of turn offs changed and forwards nothing (DL11).
    if forwards_off {
        state.jobs.forwards_off();
        tracing::info!("jobs.forward_ports is off; efrd stops the port forwards of every job");
    }
    if probe_again {
        state.sandbox.reprobe(Arc::clone(&state.settings.borrow()));
    }
    Ok(warnings)
}

/// The warning of a bus filter that a reload from `running` to `next` turned off, as
/// `efr sandbox check` words it: `dbus.proxy` went to false, or it went to true while
/// the last bus check of this efrd found the filter off for a reason that warns
/// (`bus::warns`). The check that a reload to true starts runs after the reply, so a
/// filter that this efrd never checked has no warning here; its result goes to the log
/// and to `efr status`.
fn bus_warning(state: &State, running: &Settings, next: &Settings) -> Option<String> {
    use crate::sandbox::bus;
    match (running.dbus.proxy, next.dbus.proxy) {
        (true, false) => Some(bus::off_warning(BusProxyOff::Config)),
        (false, true) => state
            .sandbox
            .reported(next)
            .bus_proxy_off
            .filter(|off| *off != BusProxyOff::Config && bus::warns(next, *off))
            .map(bus::off_warning),
        _ => None,
    }
}

/// True when a reload from `running` to `next` must run the sandbox probe again: the
/// probe checks that no program of the sandbox lies in a write root, and which bwrap
/// and cache mode it uses, so a change of `[sandbox]` runs it, and so does a `sent`
/// engine whose locations (the projects) or support differ from `current`.
///
/// NOTE: `sandbox.allow` is left out, and so is whether an engine was sent at all: an
/// `a` answer changes only the always rules, and its reload must start no probe
/// (DE28).
pub(crate) fn probes_again(
    running: &Settings,
    next: &Settings,
    current: &Engine,
    sent: Option<&Engine>,
) -> bool {
    let without_rules = |settings: &Settings| {
        let mut sandbox = settings.sandbox.clone();
        sandbox.allow.clear();
        sandbox
    };
    without_rules(running) != without_rules(next)
        || sent.is_some_and(|engine| {
            engine.locations() != current.locations() || engine.support() != current.support()
        })
}

/// `error` as the wire error of a refused file, belonging to `key` when one is known.
fn refused(error: &DaemonError, key: Option<&str>) -> ConfigFileError {
    let message = efr_stdx::with_causes(error);
    ConfigFileError {
        message: message.split_whitespace().collect::<Vec<_>>().join(" "),
        line: None,
        column: None,
        key: key.map(str::to_owned),
    }
}

/// The line each terminal hears when the outcome changed from `previous` to `now`: a
/// new error, a new held change, or a new set of keys that wait for a restart.
pub(crate) fn notice(previous: &Outcome, now: &Outcome) -> Option<String> {
    if let Some(held) = &now.held
        && previous.held.as_ref() != Some(held)
    {
        return Some(format!("efr: {}", held_text(held)));
    }
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
