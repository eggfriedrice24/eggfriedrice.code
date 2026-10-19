//! One call's listener: the accept loop, the state that every connection of the call
//! shares (the budget, the counts, the records), and the handle [`CallProxy`].

use std::fmt;
use std::io::Write as _;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use efr_permissions::{HostList, HostVerdict};
use efr_protocol::{BlockReason, Blocked, CallId};
use efr_stdx::time::{Clock as _, SystemClock};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Semaphore, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};

use crate::conn;
use crate::deps::Deps;
use crate::proxy::{CallPolicy, Proxy};
use crate::records::{INVALID, Records};
use crate::reply::{self, Status};

/// The pause after a failed accept, such as one for a full fd table, so the loop does
/// not spin.
const ACCEPT_PAUSE: Duration = Duration::from_millis(50);

/// What every connection of one call shares.
pub(crate) struct Call {
    pub(crate) id: CallId,
    grants: Vec<(String, u16)>,
    deny: Vec<String>,
    max_upload: u64,
    pub(crate) deps: Arc<Deps>,
    hosts: watch::Receiver<Arc<HostList>>,
    /// The lookups of this call at once.
    pub(crate) lookups: Semaphore,
    /// The lookups of the whole proxy at once.
    pub(crate) all_lookups: Arc<Semaphore>,
    connections: Arc<Semaphore>,
    /// The bytes of the budget that the call used (DP9).
    used: AtomicU64,
    bytes_up: AtomicU64,
    bytes_down: AtomicU64,
    tunnels: AtomicU32,
    records: Mutex<Records>,
}

impl Call {
    /// The verdict for `host:port`: the deny list and the call's deny set win, then the
    /// live global list, then the call's grants (DP24).
    pub(crate) fn verdict(&self, host: &str, port: u16) -> HostVerdict {
        let list = Arc::clone(&self.hosts.borrow());
        if self.deny.iter().any(|denied| denied == host) {
            return HostVerdict::Denied;
        }
        match list.decide(host, port) {
            HostVerdict::NotListed
                if self.grants.iter().any(|(granted, own)| granted == host && *own == port) =>
            {
                HostVerdict::Allowed { http: port == 80, private: false }
            }
            verdict => verdict,
        }
    }

    /// True when the call may open a TLS tunnel to `host:port`, which
    /// [`verdict`](Self::verdict) allows: the global list opens it for a tunnel, or a
    /// grant names that port and it is not 80 (DP6).
    pub(crate) fn may_tunnel(&self, host: &str, port: u16) -> bool {
        let list = Arc::clone(&self.hosts.borrow());
        port != 80
            && (list.tunnels(host, port)
                || self.grants.iter().any(|(granted, own)| granted == host && *own == port))
    }

    /// True when the budget is spent: the next `CONNECT` or plain request gets 403.
    pub(crate) fn spent(&self) -> bool {
        self.used.load(Ordering::Acquire) >= self.max_upload
    }

    /// Takes up to `want` bytes of the budget and gives the bytes taken.
    pub(crate) fn reserve(&self, want: usize) -> usize {
        let want = u64::try_from(want).unwrap_or(u64::MAX);
        let mut used = self.used.load(Ordering::Acquire);
        loop {
            let take = want.min(self.max_upload.saturating_sub(used));
            match self.used.compare_exchange_weak(
                used,
                used + take,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return usize::try_from(take).unwrap_or(usize::MAX),
                Err(now) => used = now,
            }
        }
    }

    /// Takes `want` bytes of the budget when all of them are left.
    pub(crate) fn reserve_all(&self, want: usize) -> bool {
        let want = u64::try_from(want).unwrap_or(u64::MAX);
        self.used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(want).filter(|after| *after <= self.max_upload)
            })
            .is_ok()
    }

    /// Counts bytes that left for an upstream.
    pub(crate) fn sent(&self, bytes: usize) {
        self.bytes_up.fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// Counts bytes that came back from an upstream.
    pub(crate) fn received(&self, bytes: usize) {
        self.bytes_down.fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// Counts a tunnel or a plain request that reached its upstream.
    pub(crate) fn opened(&self) {
        self.tunnels.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a refusal. `host` must be a host that passed the syntax of DP2, or
    /// [`INVALID`].
    pub(crate) fn refuse(&self, host: &str, port: u16, reason: BlockReason) {
        tracing::debug!(call = %self.id, host, port, ?reason, "efr-proxy refused a connection");
        self.records().add(host, port, reason);
    }

    fn records(&self) -> MutexGuard<'_, Records> {
        self.records.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn report(&self) -> CallReport {
        let (blocked, blocked_more) = self.records().snapshot();
        CallReport {
            blocked,
            blocked_more,
            bytes_up: self.bytes_up.load(Ordering::Relaxed),
            bytes_down: self.bytes_down.load(Ordering::Relaxed),
            tunnels: self.tunnels.load(Ordering::Relaxed),
        }
    }
}

/// What one call did through the proxy, when its listener closed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallReport {
    /// The refusals: at most `Limits::blocked` records.
    pub blocked: Vec<Blocked>,
    /// The refusals that `blocked` leaves out.
    pub blocked_more: u32,
    /// The client bytes that left for an upstream, the ClientHellos included.
    pub bytes_up: u64,
    /// The bytes that came back from an upstream.
    pub bytes_down: u64,
    /// The tunnels and plain requests that reached their upstream.
    pub tunnels: u32,
}

/// One call's listener and its tunnels. Drop aborts every task of the call.
pub struct CallProxy {
    call: Arc<Call>,
    stop: Option<oneshot::Sender<Duration>>,
    task: Option<JoinHandle<()>>,
}

impl fmt::Debug for CallProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let report = self.call.report();
        f.debug_struct("CallProxy")
            .field("call", &self.call.id)
            .field("tunnels", &report.tunnels)
            .field("bytes_up", &report.bytes_up)
            .field("bytes_down", &report.bytes_down)
            .field("blocked", &self.call.records().total())
            .finish()
    }
}

impl CallProxy {
    /// The refusals so far: at most `Limits::blocked` entries, and the count of the
    /// rest.
    pub fn blocked(&self) -> (Vec<Blocked>, u32) {
        self.call.records().snapshot()
    }

    /// Stops the accept, waits at most `grace` for open tunnels, cuts the rest, and
    /// logs one debug line with the call's counts.
    pub async fn close(mut self, grace: Duration) -> CallReport {
        if let Some(stop) = self.stop.take() {
            // NOTE: the send fails only when the accept task has ended already.
            let _ = stop.send(grace);
        }
        if let Some(task) = self.task.take() {
            // NOTE: the task ends by itself after the grace; an error here is a panic in
            // it, and the report below still holds what the call did.
            let _ = task.await;
        }
        let report = self.call.report();
        tracing::debug!(
            phase = "proxy",
            call = %self.call.id,
            tunnels = report.tunnels,
            bytes_up = report.bytes_up,
            bytes_down = report.bytes_down,
            blocked = self.call.records().total(),
            "efr-proxy closed the call's listener"
        );
        report
    }
}

impl Drop for CallProxy {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Starts the accept loop of one call on the current runtime.
pub(crate) fn serve(proxy: &Proxy, listener: UnixListener, policy: CallPolicy) -> CallProxy {
    let limits = proxy.deps.limits;
    let call = Arc::new(Call {
        id: policy.call,
        grants: policy.grants,
        deny: policy.deny,
        max_upload: policy.max_upload_bytes,
        deps: Arc::clone(&proxy.deps),
        hosts: proxy.hosts.clone(),
        lookups: Semaphore::new(limits.lookups_per_call.max(1)),
        all_lookups: Arc::clone(&proxy.lookups),
        connections: Arc::new(Semaphore::new(limits.connections)),
        used: AtomicU64::new(0),
        bytes_up: AtomicU64::new(0),
        bytes_down: AtomicU64::new(0),
        tunnels: AtomicU32::new(0),
        records: Mutex::new(Records::new(limits.blocked)),
    });
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(accept(Arc::clone(&call), listener, stopped));
    CallProxy { call, stop: Some(stop), task: Some(task) }
}

/// Accepts connections until the stop, then waits for them at most the grace and cuts
/// the rest. Dropping this future drops the set, which aborts every connection task.
async fn accept(call: Arc<Call>, listener: UnixListener, mut stop: oneshot::Receiver<Duration>) {
    let mut tasks = JoinSet::new();
    let grace = loop {
        tokio::select! {
            biased;
            grace = &mut stop => break grace.unwrap_or(Duration::ZERO),
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => admit(&call, &mut tasks, stream),
                Err(error) => {
                    tracing::debug!(call = %call.id, %error, "efr-proxy could not accept");
                    SystemClock.sleep(ACCEPT_PAUSE).await;
                }
            },
        }
    };
    drop(listener);
    let drain = async { while tasks.join_next().await.is_some() {} };
    // NOTE: a timeout here only means that the grace is over; the shutdown cuts the
    // connections that are still open.
    let _ = SystemClock.timeout(grace, drain).await;
    tasks.shutdown().await;
}

/// Starts the task of one accepted connection, or refuses it with 503 when the call has
/// too many open connections (DP10).
fn admit(call: &Arc<Call>, tasks: &mut JoinSet<()>, stream: UnixStream) {
    match Arc::clone(&call.connections).try_acquire_owned() {
        Ok(permit) => {
            tasks.spawn(conn::handle(Arc::clone(call), stream, permit));
        }
        Err(_) => {
            call.refuse(INVALID, 0, BlockReason::Limit);
            // NOTE: a short answer to a new socket fits its empty send buffer, so a write
            // that does not wait is enough; a client that cannot take it gets none.
            // tokio's own try_write would wait for the first readiness event.
            if let Ok(mut stream) = stream.into_std() {
                let _ = stream.write(&reply::response(Status::Unavailable, "too many connections"));
            }
        }
    }
}
