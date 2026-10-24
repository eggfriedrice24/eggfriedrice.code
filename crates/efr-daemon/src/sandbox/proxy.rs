//! efrd's side of the network proxy of the `auto` sandbox (efr's auto spec 8.2, phase
//! D2): one [`efr_proxy::Proxy`] for the daemon, the live host list that it reads, the
//! pacman mirror hosts of `@pacman-mirrors`, the socket of each contained call with a
//! proxy plan, and what the proxy refused, for the reviewer's facts.
//!
//! - The proxy is built at start with the deps of [`Seams::proxy_deps`], or with the
//!   system resolver, connector and addresses and the strict guard when it gives none.
//!   No `cfg` line is in this file.
//! - The global list is live: each engine that efrd builds sends its list here
//!   ([`ProxyService::set_hosts`]), and every listener reads the newest one for each
//!   request (DP3).
//! - [`ProxyService::bind`] binds `$CALL/proxy.sock` (DP13) and serves it with the call's
//!   frozen policy (DP24). The [`ProxyCall`] lives in `Prepared`: a foreground call
//!   closes it after the launcher wrote its result, a call past its timeout hands it to
//!   [`ProxyService::keep_timed_out`] until `result.json` appears or its call dir goes
//!   ([`ProxyService::sweep`]), and a job keeps it in its entry of the job table until
//!   the job ends (DP14). Dropping a [`ProxyCall`] aborts every task of its call.
//! - The refusals of a conversation's last call and of its jobs are facts of its next
//!   exits ([`ProxyService::refused_hosts`], DP23): a fact, never a name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use efr_config::Settings;
use efr_permissions::{HostList, MirrorHost};
use efr_protocol::{CallId, ConversationId, NetworkMode, SandboxStatus};
use efr_proxy::{
    CallPolicy, CallProxy, CallReport, Deps, Guard, Limits, Proxy, SystemAddresses,
    SystemConnector, SystemResolver,
};
use efr_stdx::time::Clock;
use jiff::Timestamp;
use tokio::sync::watch;

use crate::DaemonError;
use crate::sandbox::Seams;

mod bind;
pub(crate) mod mirrors;
pub(crate) mod tokens;

pub(crate) use bind::{bind_listener, bind_socket};

/// How long a call's open tunnels may run on after its command ended, before the proxy
/// cuts them (DP14).
pub(crate) const CLOSE_GRACE: Duration = Duration::from_millis(200);

/// The warning of `efr status` and the log when `network.mode = "proxy"` but the probe's
/// proxy self-test failed: contained calls then have no network (DP15).
pub(crate) const PROXY_FALLBACK: &str =
    "efr's network proxy is off: its self-test failed; contained commands have no network";

/// The note of the first contained call after each efrd start whose proxy is off
/// because its self-test failed (DP15).
pub(crate) const FALLBACK_NOTE: &str = "[efr: efr's network proxy is off: its self-test failed; run efr status. This command \
     has no network.]";

/// The most conversations whose refusals efrd keeps; the oldest goes first.
const MAX_CONVERSATIONS: usize = 256;

/// The host of a refusal whose host did not pass the host syntax.
const INVALID_HOST: &str = "<invalid>";

/// The proxy of efrd. Cheap to clone; clones share everything.
#[derive(Clone)]
pub(crate) struct ProxyService {
    inner: Arc<Inner>,
}

struct Inner {
    proxy: Proxy,
    hosts: watch::Sender<Arc<HostList>>,
    mirrors: Arc<Mutex<mirrors::Mirrors>>,
    /// The mirror hosts of the last read, which the engine's list holds.
    mirror_hosts: watch::Sender<Arc<[MirrorHost]>>,
    calls: Mutex<Calls>,
    clock: Arc<dyn Clock>,
    /// True once the fallback of DP15 was logged since efrd started.
    warned: AtomicBool,
    /// True once a call got the fallback note since efrd started.
    noted: AtomicBool,
}

impl std::fmt::Debug for ProxyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let calls = self.calls();
        f.debug_struct("ProxyService")
            .field("timed_out", &calls.timed_out.len())
            .field("jobs", &calls.jobs.len())
            .finish_non_exhaustive()
    }
}

/// The calls whose listeners outlive their tool call, and what the proxy refused.
#[derive(Default)]
struct Calls {
    /// The calls past their timeout, each with its call dir, until `result.json`
    /// appears or the dir goes.
    timed_out: Vec<(PathBuf, ProxyCall)>,
    /// The listeners of running jobs, for their refusals so far.
    jobs: Vec<Weak<Live>>,
    /// The refusals of each conversation.
    refused: HashMap<ConversationId, Refused>,
}

/// What the proxy refused for one conversation.
#[derive(Debug, Default)]
struct Refused {
    /// The hosts of its last call that ended or passed its timeout.
    last_call: Vec<String>,
    /// When that call ended or passed its timeout.
    last_call_at: Option<Timestamp>,
    /// The hosts of each job that ended, with the time of its end.
    jobs: Vec<(Timestamp, Vec<String>)>,
    /// When it changed last, so the oldest conversation goes first.
    touched: Timestamp,
}

/// One call's listener, shared with the service's list of running jobs.
struct Live {
    conversation: ConversationId,
    call: CallId,
    max_upload_bytes: u64,
    proxy: Mutex<Option<CallProxy>>,
}

impl Live {
    fn proxy(&self) -> MutexGuard<'_, Option<CallProxy>> {
        // NOTE: the handle is taken whole, so a poisoned lock still holds a usable one.
        self.proxy.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The socket of one call and the proxy's tasks behind it. Dropping it aborts them.
pub(crate) struct ProxyCall {
    live: Arc<Live>,
    job: bool,
    service: Weak<Inner>,
}

impl std::fmt::Debug for ProxyCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyCall")
            .field("call", &self.live.call)
            .field("job", &self.job)
            .field("proxy", &*self.live.proxy())
            .finish()
    }
}

impl ProxyCall {
    /// The upload limit of the call, which the note of its refusals names.
    pub(crate) fn max_upload_bytes(&self) -> u64 {
        self.live.max_upload_bytes
    }

    /// Stops the accept, waits at most `grace` for open tunnels and cuts the rest, and
    /// keeps the refusals for the conversation's next facts.
    pub(crate) async fn close(self, grace: Duration) -> CallReport {
        let proxy = self.live.proxy().take();
        let report = match proxy {
            Some(proxy) => proxy.close(grace).await,
            None => CallReport::default(),
        };
        if let Some(service) = self.service.upgrade() {
            let service = ProxyService { inner: service };
            let hosts = refused(&report.blocked);
            service.record(self.live.conversation, self.job, hosts);
        }
        report
    }

    /// The hosts refused so far.
    fn refused_so_far(&self) -> Vec<String> {
        self.live.proxy().as_ref().map(|proxy| refused(&proxy.blocked().0)).unwrap_or_default()
    }
}

impl ProxyService {
    /// The proxy with the deps of `seams`, or the system ones, and an empty list until
    /// the first engine sends one; `seams` also name the pacman files.
    pub(crate) fn new(seams: &Seams, clock: Arc<dyn Clock>) -> ProxyService {
        let limits = Limits::default();
        let deps = seams.proxy_deps(limits).unwrap_or_else(|| Deps {
            resolver: Arc::new(SystemResolver),
            connector: Arc::new(SystemConnector),
            own: Arc::new(SystemAddresses::new()),
            guard: Guard::strict(),
            limits,
        });
        let (hosts, receiver) = watch::channel(Arc::new(HostList::default()));
        let (mirror_hosts, _) = watch::channel(Arc::from(Vec::new()));
        let [conf, list] = seams.pacman_files();
        ProxyService {
            inner: Arc::new(Inner {
                proxy: Proxy::new(deps, receiver),
                hosts,
                mirrors: Arc::new(Mutex::new(mirrors::Mirrors::new(conf, list))),
                mirror_hosts,
                calls: Mutex::new(Calls::default()),
                clock,
                warned: AtomicBool::new(false),
                noted: AtomicBool::new(false),
            }),
        }
    }

    /// Sends `hosts` to every listener when it differs from the running list: a reload
    /// that changed the list reaches running calls and jobs at once (DP3).
    pub(crate) fn set_hosts(&self, hosts: &HostList) {
        self.inner.hosts.send_if_modified(|running| {
            if **running == *hosts {
                return false;
            }
            *running = Arc::new(hosts.clone());
            true
        });
    }

    /// The global list that the listeners read now.
    #[cfg(test)]
    pub(crate) fn hosts(&self) -> Arc<HostList> {
        Arc::clone(&self.inner.hosts.borrow())
    }

    /// The mirror hosts of the last read of the pacman files.
    pub(crate) fn mirrors(&self) -> Arc<[MirrorHost]> {
        Arc::clone(&self.inner.mirror_hosts.borrow())
    }

    /// The mirror hosts, which change when a read finds other hosts.
    pub(crate) fn mirror_changes(&self) -> watch::Receiver<Arc<[MirrorHost]>> {
        self.inner.mirror_hosts.subscribe()
    }

    /// Reads the pacman files again: always when `force` (at start and at reload),
    /// else at most once a minute and only when a file that the last read used
    /// changed (at a call's prepare, DP4). True when the hosts changed.
    pub(crate) async fn read_mirrors(&self, force: bool) -> bool {
        let now = self.inner.clock.now();
        let mirrors = Arc::clone(&self.inner.mirrors);
        // NOTE: the lock lives on the blocking thread only, never across an await.
        let read = tokio::task::spawn_blocking(move || {
            mirrors.lock().unwrap_or_else(PoisonError::into_inner).refresh(now, force)
        })
        .await;
        let Ok(Some(hosts)) = read else { return false };
        self.inner.mirror_hosts.send_if_modified(|running| {
            if **running == *hosts {
                return false;
            }
            *running = Arc::from(hosts);
            true
        })
    }

    /// Binds `$CALL/proxy.sock` in `call_dir` and serves it with `policy` (DP13, DP24).
    /// A job's listener counts for the refusal facts while it runs.
    pub(crate) async fn bind(
        &self,
        conversation: ConversationId,
        call_dir: &Path,
        policy: CallPolicy,
        job: bool,
    ) -> Result<ProxyCall, DaemonError> {
        let dir = call_dir.to_path_buf();
        let listener = tokio::task::spawn_blocking(move || bind_listener(&dir))
            .await
            .map_err(|_| DaemonError::TaskPanicked { task: "proxy socket" })?
            .map_err(|source| DaemonError::Io {
                path: call_dir.join(efr_sandbox::PROXY_SOCKET_FILE),
                source,
            })?;
        let listener = tokio::net::UnixListener::from_std(listener).map_err(|source| {
            DaemonError::Io { path: call_dir.join(efr_sandbox::PROXY_SOCKET_FILE), source }
        })?;
        let call = policy.call;
        let max_upload_bytes = policy.max_upload_bytes;
        let proxy = self.inner.proxy.serve(listener, policy);
        let live =
            Arc::new(Live { conversation, call, max_upload_bytes, proxy: Mutex::new(Some(proxy)) });
        if job {
            let mut calls = self.calls();
            calls.jobs.retain(|known| known.strong_count() > 0);
            calls.jobs.push(Arc::downgrade(&live));
        }
        Ok(ProxyCall { live, job, service: Arc::downgrade(&self.inner) })
    }

    /// Keeps the listener of a call that passed its timeout until `result.json` appears
    /// in `call_dir` or the dir goes ([`sweep`](Self::sweep)). Its refusals so far count
    /// as the conversation's last call.
    pub(crate) fn keep_timed_out(&self, call: ProxyCall, call_dir: PathBuf) {
        let hosts = call.refused_so_far();
        self.record(call.live.conversation, false, hosts);
        self.calls().timed_out.push((call_dir, call));
    }

    /// Takes the listener of the call past its timeout in `call_dir`, for the job that
    /// its command moved into (OQS16): the job serves it until it ends.
    pub(crate) fn take_timed_out(&self, call_dir: &Path) -> Option<ProxyCall> {
        let mut calls = self.calls();
        let at = calls.timed_out.iter().position(|(dir, _)| dir == call_dir)?;
        Some(calls.timed_out.remove(at).1)
    }

    /// Closes the listener of each call past its timeout whose `result.json` appeared or
    /// whose call dir went, and returns how many it closed. The gc pass and the next
    /// prepare call it.
    pub(crate) async fn sweep(&self) -> usize {
        let held: Vec<PathBuf> =
            self.calls().timed_out.iter().map(|(dir, _)| dir.clone()).collect();
        if held.is_empty() {
            return 0;
        }
        let ended = tokio::task::spawn_blocking(move || {
            held.into_iter().filter(|dir| super::gc::call_ended(dir)).collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let closing: Vec<ProxyCall> = {
            let mut calls = self.calls();
            let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut calls.timed_out)
                .into_iter()
                .partition(|(dir, _)| ended.contains(dir));
            calls.timed_out = kept;
            gone.into_iter().map(|(_, call)| call).collect()
        };
        let count = closing.len();
        for call in closing {
            call.close(CLOSE_GRACE).await;
        }
        count
    }

    /// The listeners that efrd holds besides the foreground calls: those of calls past
    /// their timeout and of running jobs.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        let mut calls = self.calls();
        calls.jobs.retain(|known| known.strong_count() > 0);
        calls.timed_out.len() + calls.jobs.len()
    }

    /// The hosts that the proxy refused in `conversation`'s last call, in its running
    /// jobs, and in its jobs that ended at or after `since` (the turn's start) or after
    /// its last call (DP23), sorted.
    pub(crate) fn refused_hosts(
        &self,
        conversation: ConversationId,
        since: Timestamp,
    ) -> Vec<String> {
        let (mut hosts, running): (Vec<String>, Vec<Arc<Live>>) = {
            let calls = self.calls();
            let mut hosts = Vec::new();
            if let Some(refused) = calls.refused.get(&conversation) {
                hosts.extend(refused.last_call.iter().cloned());
                // NOTE: a job that ended after the last call counts too, so the turn
                // that its end woke sees what the proxy refused it.
                let after_call =
                    |ended: Timestamp| refused.last_call_at.is_none_or(|at| ended >= at);
                for (ended, ended_hosts) in &refused.jobs {
                    if *ended >= since || after_call(*ended) {
                        hosts.extend(ended_hosts.iter().cloned());
                    }
                }
            }
            let running = calls
                .jobs
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|live| live.conversation == conversation)
                .collect();
            (hosts, running)
        };
        for live in running {
            if let Some(proxy) = live.proxy().as_ref() {
                hosts.extend(refused(&proxy.blocked().0));
            }
        }
        hosts.sort();
        hosts.dedup();
        hosts
    }

    /// Logs the fallback of DP15 once per efrd start: `network.mode = "proxy"` while the
    /// probe's proxy self-test failed.
    pub(crate) fn warn_fallback(&self, settings: &Settings, status: &SandboxStatus) {
        if falls_back(settings, status) && !self.inner.warned.swap(true, Ordering::AcqRel) {
            tracing::warn!(
                "network.mode is proxy, but the proxy self-test failed; contained calls have no network"
            );
        }
    }

    /// The note of DP15 for the first contained call after each efrd start that falls
    /// back; `None` for every other call.
    pub(crate) fn fallback_note(
        &self,
        settings: &Settings,
        status: &SandboxStatus,
    ) -> Option<&'static str> {
        (falls_back(settings, status) && !self.inner.noted.swap(true, Ordering::AcqRel))
            .then_some(FALLBACK_NOTE)
    }

    fn record(&self, conversation: ConversationId, job: bool, hosts: Vec<String>) {
        let now = self.inner.clock.now();
        let mut calls = self.calls();
        let refused = calls.refused.entry(conversation).or_default();
        refused.touched = now;
        if job {
            if !hosts.is_empty() {
                refused.jobs.push((now, hosts));
            }
        } else {
            refused.last_call = hosts;
            refused.last_call_at = Some(now);
        }
        if calls.refused.len() > MAX_CONVERSATIONS
            && let Some(oldest) =
                calls.refused.iter().min_by_key(|(_, refused)| refused.touched).map(|(id, _)| *id)
        {
            calls.refused.remove(&oldest);
        }
    }

    fn calls(&self) -> MutexGuard<'_, Calls> {
        // NOTE: each change is whole, so a poisoned lock still holds usable lists.
        self.inner.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// True when `network.mode = "proxy"` but the probe's proxy self-test failed (DP15).
pub(crate) fn falls_back(settings: &Settings, status: &SandboxStatus) -> bool {
    settings.network.mode == NetworkMode::Proxy && status.available && !status.proxy
}

/// True when contained calls reach the network through the proxy: `network.mode =
/// "proxy"` and a passed self-test (DP15).
pub(crate) fn proxy_on(settings: &Settings, status: &SandboxStatus) -> bool {
    settings.network.mode == NetworkMode::Proxy && status.proxy
}

/// The warnings of `efr status` for each `sandbox.env_keep` entry that looks like a
/// secret while `network.mode = "proxy"`: such a variable now reaches the allowed hosts
/// (DP25, the fallback of check C1-5).
pub(crate) fn env_keep_warnings(settings: &Settings) -> Vec<String> {
    if settings.network.mode != NetworkMode::Proxy {
        return Vec::new();
    }
    settings
        .sandbox
        .env_keep
        .iter()
        .filter(|name| efr_sandbox::secret_like(name))
        .map(|name| {
            format!(
                "sandbox.env_keep keeps {name}, which looks like a secret: with the proxy, a \
                 contained command can send it to an allowed host"
            )
        })
        .collect()
}

/// The hosts of `blocked` that passed the host syntax, sorted. A denied host is left
/// out (v0.0.4, DV20): no grant opens it, so the reviewer must not read it as a host
/// that a command tried.
fn refused(blocked: &[efr_protocol::Blocked]) -> Vec<String> {
    let mut hosts: Vec<String> = blocked
        .iter()
        .filter(|entry| entry.reason != efr_protocol::BlockReason::Denied)
        .map(|entry| entry.host.clone())
        .filter(|host| host != INVALID_HOST && !host.is_empty())
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

#[cfg(test)]
mod tests;
