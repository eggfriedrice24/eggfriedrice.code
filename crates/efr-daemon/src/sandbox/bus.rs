//! efrd's side of the filtered bus of the `auto` sandbox (efr's auto spec 11.1, phase
//! E): one `xdg-dbus-proxy` for each filtered bus of a call, started through the
//! launcher's `efr-sbx dbus-proxy` wrapper, which confines it (DE1, DE7, DE8).
//!
//! - The program is the trusted `xdg-dbus-proxy` that efrd found at start
//!   ([`TrustedPrograms::checked_path`], the same inode check as every run of a trusted
//!   program), or a test's fake ([`Seams::bus_proxy_program`](super::Seams)).
//! - [`BusService::start`] makes `$CALL/bus/` (0700), removes a stale socket, and runs
//!   `efr-sbx dbus-proxy --dir $CALL/bus --upstream <bus> -- <program> --fd=3 <address>
//!   <bus>.sock --filter --log --call=...`, with a clean environment, `/dev/null` as
//!   stdin, the write end of a sync pipe as stdout (the wrapper moves it to fd 3) and a
//!   pipe as stderr. The socket name is relative: the wrapper changes to `$CALL/bus/`,
//!   so a deep call dir never meets the 107 bytes of a socket address. One byte `x` on
//!   the sync pipe within [`READY_WAIT`] on the injected clock means ready; the end of
//!   the pipe or no byte is a start failure, and the call fails with a setup error,
//!   never with the real bus.
//! - The rules are `[[dbus.allow]]` on top of the defaults ([`BusFilter::compile`]),
//!   as [`BusService::set_rules`] last set them: a running call or job keeps the rules
//!   of its start.
//! - The upstream is only a bus that [`BusUpstream::check`] accepts: resolved, a
//!   socket, and never in efr's dirs or a write root.
//! - A [`BusCall`] lives in `Prepared`: a foreground call finishes it after the
//!   launcher wrote its result, a call past its timeout hands it to
//!   [`BusService::keep_timed_out`] until `result.json` appears or its call dir goes
//!   ([`BusService::sweep`]), and a job keeps it in its entry of the job table until
//!   the job ends. [`BusCall::finish`] closes the sync pipe, after which the proxy ends
//!   at once; dropping a [`BusCall`] kills the proxies.
//! - Each proxy's stderr, which holds its `--log` lines and its own errors, is kept
//!   raw, at most [`MAX_LOG_BYTES`], for the refusals. A proxy that ended before its
//!   call did (a crash) is reported as stopped.
//!
//! No `cfg` line is in this file.

use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt as _, FileTypeExt as _};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_permissions::{BusFilter, BusRule};
use efr_protocol::{BusKind, BusRefusal};
use efr_sandbox::{BUS_FILTER_DIR, bus_filter_file, is_within};
use efr_stdx::time::{Clock, Stopwatch};
use tokio::io::AsyncReadExt as _;
use tokio::net::unix::pipe;
use tokio::task::JoinHandle;

use crate::DaemonError;
use crate::error::BusProxyProblem;
use crate::trusted_programs::{Program, TrustedPrograms};

/// How long efrd waits for a proxy's ready byte (probe 4: a median of 1.4 ms here).
pub(crate) const READY_WAIT: Duration = Duration::from_secs(2);

/// How long a proxy may take to end after its sync pipe closed (probe 4: 1 ms), before
/// efrd kills it.
pub(crate) const CLOSE_GRACE: Duration = Duration::from_secs(1);

/// The most bytes of a proxy's stderr that efrd keeps; the rest is counted.
pub(crate) const MAX_LOG_BYTES: usize = 256 * 1024;

/// The byte that a proxy writes on its sync pipe when it is ready.
const READY: u8 = b'x';

/// The note of a call whose system bus view efrd dropped (DE8).
pub(crate) const NO_SYSTEM_BUS: &str =
    "[efr: no system bus on this machine; the bus filter did not run.]";

/// The note of a call whose session bus view efrd dropped (DE8, DE13).
pub(crate) const NO_SESSION_BUS: &str =
    "[efr: no session bus on this machine; the bus filter did not run.]";

/// The two upstream buses that efrd accepts (DE7, DE14): each resolved, a socket, and
/// never below efr's dirs or a write root. In a test build, only from the seams.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BusUpstream {
    pub(crate) system: Option<PathBuf>,
    pub(crate) session: Option<PathBuf>,
}

impl BusUpstream {
    /// The buses `system` and `session` that pass the check, each by its resolved
    /// path: it exists, it is a socket, and neither its path nor its resolved path lies
    /// at or below one of `refused` (efr's dirs and the write roots) or that root's
    /// resolved form. It blocks for a few stats.
    pub(crate) fn check(
        system: Option<&Path>,
        session: Option<&Path>,
        refused: &[PathBuf],
    ) -> BusUpstream {
        let refused: Vec<PathBuf> = refused
            .iter()
            .flat_map(|root| {
                let real = std::fs::canonicalize(root).ok();
                std::iter::once(root.clone()).chain(real)
            })
            .collect();
        BusUpstream {
            system: system.and_then(|path| accepted(path, &refused)),
            session: session.and_then(|path| accepted(path, &refused)),
        }
    }

    /// The accepted socket of `bus`.
    pub(crate) fn path(&self, bus: BusKind) -> Option<&Path> {
        if bus == BusKind::System { self.system.as_deref() } else { self.session.as_deref() }
    }
}

/// The resolved `path` when it is a socket outside `refused`.
fn accepted(path: &Path, refused: &[PathBuf]) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let real = std::fs::canonicalize(path).ok()?;
    let socket = std::fs::metadata(&real).ok()?.file_type().is_socket();
    let inside = refused.iter().any(|root| is_within(path, root) || is_within(&real, root));
    (socket && !inside).then_some(real)
}

/// What [`BusCall::finish`] found: what the filter refused, and whether a proxy ended
/// during the call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BusReport {
    /// The refused calls, at most `SandboxSummary::MAX_BUS_REFUSED`.
    pub(crate) refused: Vec<BusRefusal>,
    /// The refused calls that `refused` leaves out.
    pub(crate) more: u32,
    /// True when a proxy ended before the call did: a crash, an OOM kill.
    pub(crate) stopped: bool,
}

/// Starts and ends the filtered bus proxies (DE1, DE7, DE8). Cheap to clone; clones
/// share everything.
#[derive(Clone)]
pub(crate) struct BusService {
    inner: Arc<Inner>,
}

struct Inner {
    clock: Arc<dyn Clock>,
    programs: Arc<TrustedPrograms>,
    /// The launcher's copy, `$R/bin/efr-sbx`, whose `dbus-proxy` runs each proxy.
    launcher: PathBuf,
    /// `[[dbus.allow]]` of the settings of the last [`BusService::set_rules`].
    rules: Mutex<Arc<[BusRule]>>,
    /// The proxies of calls past their timeout, each with its call dir.
    timed_out: Mutex<Vec<(PathBuf, BusCall)>>,
}

impl std::fmt::Debug for BusService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BusService")
            .field("launcher", &self.inner.launcher)
            .field("program", &self.inner.programs.has(Program::XdgDbusProxy))
            .field("timed_out", &self.timed_out().len())
            .finish_non_exhaustive()
    }
}

impl BusService {
    /// The service. `clock` is the injected clock of the ready wait; `programs` holds
    /// the trusted `xdg-dbus-proxy`; `launcher` is the launcher's copy.
    pub(crate) fn new(
        clock: Arc<dyn Clock>,
        programs: Arc<TrustedPrograms>,
        launcher: PathBuf,
    ) -> Self {
        BusService {
            inner: Arc::new(Inner {
                clock,
                programs,
                launcher,
                rules: Mutex::new(Arc::from(Vec::new())),
                timed_out: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The rules of the next calls: `extra` (`[[dbus.allow]]`) on top of the defaults.
    /// A running call or job keeps its own.
    pub(crate) fn set_rules(&self, extra: Arc<[BusRule]>) {
        // NOTE: the list is replaced whole, so a poisoned lock still holds a usable one.
        *self.inner.rules.lock().unwrap_or_else(PoisonError::into_inner) = extra;
    }

    /// The rules that the next call gets.
    pub(crate) fn rules(&self) -> Arc<[BusRule]> {
        Arc::clone(&self.inner.rules.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Starts one proxy for each of `buses` in `call_dir/bus/` and waits for each ready
    /// byte. A failure ends the proxies that started.
    pub(crate) async fn start(
        &self,
        call_dir: &Path,
        buses: &[BusKind],
        upstream: &BusUpstream,
    ) -> Result<BusCall, DaemonError> {
        let failed = |problem| DaemonError::BusProxy { problem };
        let programs = Arc::clone(&self.inner.programs);
        let program =
            tokio::task::spawn_blocking(move || programs.checked_path(Program::XdgDbusProxy))
                .await
                .ok()
                .flatten()
                .ok_or(failed(BusProxyProblem::NoProgram))?;
        let rules = self.rules();
        let dir = call_dir.join(BUS_FILTER_DIR);
        let call = call_dir.file_name().map(|name| name.to_string_lossy().into_owned());
        let mut started = BusCall {
            call: call.unwrap_or_default(),
            clock: Arc::clone(&self.inner.clock),
            proxies: Vec::new(),
        };
        for &bus in buses {
            let Some(socket) = upstream.path(bus) else {
                started.finish().await;
                return Err(failed(BusProxyProblem::BadUpstream { bus }));
            };
            let filter = match BusFilter::compile(bus, &rules) {
                Ok(filter) => filter,
                Err(error) => {
                    tracing::warn!(error = %error, "the bus rules do not compile");
                    started.finish().await;
                    return Err(failed(BusProxyProblem::Rules));
                }
            };
            match self.spawn(&program, &dir, socket, &filter).await {
                Ok(proxy) => started.proxies.push(proxy),
                Err(problem) => {
                    started.finish().await;
                    return Err(failed(problem));
                }
            }
        }
        Ok(started)
    }

    /// One proxy for `filter`'s bus at `upstream`, serving `dir/<bus>.sock`.
    async fn spawn(
        &self,
        program: &Path,
        dir: &Path,
        upstream: &Path,
        filter: &BusFilter,
    ) -> Result<Proxy, BusProxyProblem> {
        let bus = filter.bus();
        let file = bus_filter_file(bus);
        let made = {
            let (dir, socket) = (dir.to_path_buf(), dir.join(file));
            tokio::task::spawn_blocking(move || make_dir_and_clear(&dir, &socket)).await
        };
        if !matches!(made, Ok(Ok(()))) {
            tracing::warn!(dir = %dir.display(), "the bus dir of a call could not be made");
            return Err(BusProxyProblem::StartFailed);
        }
        let (read, write) = std::io::pipe().map_err(|_| BusProxyProblem::StartFailed)?;
        let ready = pipe::Receiver::from_owned_fd(OwnedFd::from(read))
            .map_err(|_| BusProxyProblem::StartFailed)?;
        let watch = Stopwatch::start();
        let spawned = {
            // NOTE: the command owns efrd's copy of the write end; it goes with the
            // command right after the spawn, so the pipe ends when the proxy goes.
            let mut command = efr_stdx::process::command(&self.inner.launcher, dir);
            command
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("LC_ALL", "C")
                .arg("dbus-proxy")
                .arg("--dir")
                .arg(dir)
                .arg("--upstream")
                .arg(upstream)
                .arg("--")
                .arg(program)
                .arg("--fd=3")
                .arg(format!("unix:path={}", upstream.display()))
                .arg(file)
                .args(filter.proxy_args())
                .stdin(Stdio::null())
                .stdout(Stdio::from(OwnedFd::from(write)))
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            command.spawn()
        };
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!(error = %error, launcher = %self.inner.launcher.display(), "the bus filter wrapper could not be started");
                return Err(BusProxyProblem::StartFailed);
            }
        };
        let log = child.stderr.take().map(capture);
        let mut proxy = Proxy { bus, child, ready: Some(ready), log, ready_ms: 0 };
        let mut byte = [0_u8; 1];
        let read = match proxy.ready.as_mut() {
            Some(ready) => self.inner.clock.timeout(READY_WAIT, ready.read(&mut byte)).await,
            None => Ok(Ok(0)),
        };
        if !matches!(read, Ok(Ok(1))) || byte[0] != READY {
            // NOTE: a proxy that never became ready may never read its pipe; it goes at
            // once.
            let _ = proxy.child.start_kill();
            let (_, log) = proxy.close(&*self.inner.clock).await;
            let text = String::from_utf8_lossy(&log.bytes);
            let first = text.lines().next().unwrap_or_default();
            tracing::warn!(bus = bus_name(bus), stderr = %first, "the bus filter did not become ready");
            return Err(BusProxyProblem::NoReadyByte);
        }
        proxy.ready_ms = u64::try_from(watch.elapsed().as_millis()).unwrap_or(u64::MAX);
        Ok(proxy)
    }

    /// Keeps the proxies of a call that passed its timeout until `result.json` appears
    /// in `call_dir` or the dir goes ([`sweep`](Self::sweep)).
    pub(crate) fn keep_timed_out(&self, call: BusCall, call_dir: PathBuf) {
        self.timed_out().push((call_dir, call));
    }

    /// Finishes the proxies of each call past its timeout whose `result.json` appeared
    /// or whose call dir went, and returns how many calls it finished. The gc pass and
    /// the next prepare of a call with a bus view call it, as the proxy's sweep.
    pub(crate) async fn sweep(&self) -> usize {
        let held: Vec<PathBuf> = self.timed_out().iter().map(|(dir, _)| dir.clone()).collect();
        if held.is_empty() {
            return 0;
        }
        let ended = tokio::task::spawn_blocking(move || {
            held.into_iter().filter(|dir| super::gc::call_ended(dir)).collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let finishing: Vec<BusCall> = {
            let mut timed_out = self.timed_out();
            let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut *timed_out)
                .into_iter()
                .partition(|(dir, _)| ended.contains(dir));
            *timed_out = kept;
            gone.into_iter().map(|(_, call)| call).collect()
        };
        let count = finishing.len();
        for call in finishing {
            call.finish().await;
        }
        if count > 0 {
            tracing::debug!(
                count,
                "the bus filters of calls that ended after their timeout closed"
            );
        }
        count
    }

    /// The calls past their timeout whose proxies efrd holds.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.timed_out().len()
    }

    fn timed_out(&self) -> MutexGuard<'_, Vec<(PathBuf, BusCall)>> {
        // NOTE: each change is whole, so a poisoned lock still holds a usable list.
        self.inner.timed_out.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Gives `bus` the `[[dbus.allow]]` of each settings that `settings` sends, until
/// `stop`: a reload reaches the next call that starts a bus filter.
pub(crate) async fn follow_settings(
    bus: BusService,
    mut settings: tokio::sync::watch::Receiver<Arc<efr_config::Settings>>,
    stop: tokio_util::sync::CancellationToken,
) {
    loop {
        let allow = settings.borrow_and_update().dbus.allow.clone();
        if *bus.rules() != *allow {
            bus.set_rules(Arc::from(allow));
        }
        tokio::select! {
            changed = settings.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = stop.cancelled() => return,
        }
    }
}

/// Makes `dir` (0700) when it is missing, refuses a dir that is a link or not a dir,
/// and removes a stale socket file `socket` that an earlier proxy left. It blocks.
fn make_dir_and_clear(dir: &Path, socket: &Path) -> std::io::Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !std::fs::symlink_metadata(dir)?.is_dir() {
                return Err(error);
            }
        }
        Err(error) => return Err(error),
    }
    match std::fs::remove_file(socket) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The bus's name in the log.
fn bus_name(bus: BusKind) -> &'static str {
    if bus == BusKind::System { "system" } else { "session" }
}

/// What a proxy wrote on its stderr: at most [`MAX_LOG_BYTES`], and how many more.
#[derive(Debug, Default)]
struct Captured {
    bytes: Vec<u8>,
    dropped: u64,
}

/// Reads `stderr` to its end in a task of its own, keeping at most [`MAX_LOG_BYTES`].
fn capture(mut stderr: tokio::process::ChildStderr) -> JoinHandle<Captured> {
    tokio::spawn(async move {
        let mut captured = Captured::default();
        let mut buf = vec![0_u8; 8192];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let room = MAX_LOG_BYTES.saturating_sub(captured.bytes.len());
                    let kept = read.min(room);
                    captured.bytes.extend_from_slice(&buf[..kept]);
                    captured.dropped += u64::try_from(read - kept).unwrap_or(u64::MAX);
                }
            }
        }
        captured
    })
}

/// One running proxy.
#[derive(Debug)]
struct Proxy {
    bus: BusKind,
    /// The wrapper's process, which became the proxy at its exec.
    child: tokio::process::Child,
    /// The read end of the sync pipe; closing it ends the proxy.
    ready: Option<pipe::Receiver>,
    /// The task that reads its stderr.
    log: Option<JoinHandle<Captured>>,
    /// How long the ready byte took.
    ready_ms: u64,
}

impl Proxy {
    /// Closes the sync pipe and waits at most [`CLOSE_GRACE`] for the proxy to end,
    /// then kills it. Returns its exit status when it ended by itself, and its stderr.
    async fn close(&mut self, clock: &dyn Clock) -> (Option<ExitStatus>, Captured) {
        drop(self.ready.take());
        let status = match Clock::timeout(&clock, CLOSE_GRACE, self.child.wait()).await {
            Ok(Ok(status)) => Some(status),
            _ => {
                let _ = self.child.start_kill();
                let _ = self.child.wait().await;
                None
            }
        };
        let log = match self.log.take() {
            Some(task) => match Clock::timeout(&clock, CLOSE_GRACE, task).await {
                Ok(Ok(captured)) => captured,
                _ => Captured::default(),
            },
            None => Captured::default(),
        };
        (status, log)
    }
}

/// The proxies of one call or job. [`finish`](Self::finish) ends them; drop kills them.
#[derive(Debug)]
pub(crate) struct BusCall {
    /// The call's id, for the log.
    call: String,
    clock: Arc<dyn Clock>,
    proxies: Vec<Proxy>,
}

impl BusCall {
    /// Ends the proxies and returns what the filter refused and whether a proxy died
    /// during the call.
    pub(crate) async fn finish(self) -> BusReport {
        let call = self.call.clone();
        let mut report = BusReport::default();
        for closed in self.close().await {
            let bus = bus_name(closed.bus);
            report.stopped |= closed.early;
            if closed.early {
                tracing::warn!(call = %call, bus, status = ?closed.status, "the bus filter stopped during the command");
            }
            tracing::debug!(
                phase = "bus_proxy",
                call = %call,
                bus,
                ready_ms = closed.ready_ms,
                log_bytes = closed.log.bytes.len(),
                log_dropped = closed.log.dropped,
                "phase=bus_proxy call={} bus={} ready_ms={}",
                call,
                bus,
                closed.ready_ms,
            );
        }
        report
    }

    /// The pid of each proxy, in the order they started.
    #[cfg(test)]
    pub(crate) fn pids(&self) -> Vec<u32> {
        self.proxies.iter().filter_map(|proxy| proxy.child.id()).collect()
    }

    /// Closes each proxy, as [`Proxy::close`] does, in the order they started.
    async fn close(self) -> Vec<Closed> {
        let BusCall { clock, proxies, .. } = self;
        let mut closed = Vec::with_capacity(proxies.len());
        for mut proxy in proxies {
            // NOTE: a proxy that ended before its sync pipe closed died during the call.
            let before = proxy.child.try_wait().ok().flatten();
            let (status, log) = proxy.close(&*clock).await;
            closed.push(Closed {
                bus: proxy.bus,
                status: before.or(status),
                early: before.is_some(),
                ready_ms: proxy.ready_ms,
                log,
            });
        }
        closed
    }
}

/// One proxy after its close.
#[derive(Debug)]
struct Closed {
    bus: BusKind,
    /// Its exit status; `None` when efrd had to kill it.
    status: Option<ExitStatus>,
    /// True when it ended before the close.
    early: bool,
    ready_ms: u64,
    log: Captured,
}

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;
