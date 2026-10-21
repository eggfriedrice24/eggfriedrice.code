//! The port forwards of a contained background job (DL1-DL16 of the v0.0.4 live items
//! plan): efrd's side of the forwarder that runs in the job (`efr-sbx forward`).
//!
//! - Which job: a job gets forwards when it runs contained in `auto` with a network
//!   namespace of its own (`network.mode = "none"` or the proxy, no grant of the whole
//!   network, no network view) and `jobs.forward_ports` is true when it starts
//!   ([`plan_of`]). Then the spec's job plan holds a [`ForwardPlan`], and efrd binds the
//!   job's forward socket `$CALL/forward.sock` (mode 0600) before the launcher starts
//!   ([`bind`]); the launcher binds it read-only into the job for the forwarder.
//! - The control connection ([`serve`], `forward/serve.rs`): the first connection to the
//!   socket must send `Hello`. When none came within [`HELLO_WAIT`], efrd logs one
//!   warning and waits on until the job's forwards end. A first frame that is no
//!   `Hello` leaves the job with no forwards. Only that one connection reports `Listen` and `Gone`. efrd
//!   decodes each frame with `efr_sandbox::Frame::decode`, which trusts no byte; a bad
//!   frame or another kind closes the forwarder's connection, and the job runs on with no
//!   forwards.
//! - Each `Listen` passes efrd's own checks, because the forwarder runs where the job may
//!   control it (DL5, DL8): a port below 1024, the bridge's port of a proxy job and the
//!   service ports are refused; a job forwards at most 8 ports and efrd at most 32
//!   ([`Forwards`]); a port that another job holds, that a listener on `::1` or `::` of
//!   the host holds, or whose bind fails is in use. Each outcome is one
//!   `job_port_forwarded` event, in the conversation and not in a turn; a refusal of a
//!   port comes at most once for each job.
//! - A forwarded port: efrd listens on `127.0.0.1:<port>` through [`HostPorts`]. Each
//!   connection first passes the peer check (DL9: the uid of the peer's row in the
//!   host's tcp table is efrd's own), then efrd sends `Open` on the control connection
//!   and waits [`OPEN_WAIT`] for the forwarder's data connection with `Data` of the same
//!   id; with status 0 it copies bytes both ways. At most 64 connections of a job run at
//!   one time. A `Gone` keeps the port for the job (DL11); a connection then closes at
//!   once.
//! - The end: the listeners and connections close when the job ends, when efrd stops,
//!   and at a reload that turns `jobs.forward_ports` off, which records `stopped` for
//!   each port ([`JobForward::turn_off`]). A later `true` does not open them again:
//!   [`Forwards`] counts each turn off, and a job that read the old settings forwards
//!   nothing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_config::JobsSettings;
use efr_permissions::Egress;
use efr_protocol::{ForwardOutcome, Grant, JobId, Launch, PortForward};
use efr_sandbox::{FORWARD_SOCKET_FILE, ForwardPlan, MAX_DAEMON_FORWARDS};
use tokio_util::sync::CancellationToken;

use super::JobKey;
use crate::DaemonError;
use crate::sandbox::proxy::bind_socket;
use crate::sandbox::seams::HostPorts;

mod host;
mod serve;

pub(crate) use host::SystemHostPorts;
pub(crate) use serve::serve;

/// How long efrd waits for the forwarder's `Hello` after the job started.
pub(crate) const HELLO_WAIT: Duration = Duration::from_secs(5);

/// How long a host connection waits for the forwarder's `Data` after efrd sent `Open`.
pub(crate) const OPEN_WAIT: Duration = Duration::from_secs(5);

/// The reason of a `stopped` event at a reload that turned forwards off.
pub(crate) const OFF_REASON: &str = "jobs.forward_ports is off";

/// The sentence of a job's start result when efr forwards its ports (DL16).
pub(crate) const START_NOTE: &str = "efr forwards each loopback port from 1024 up that this \
    job listens on to the same port of 127.0.0.1 on the user's machine; give the user the \
    URL, such as http://localhost:8765. A port in use there and well-known service ports \
    are not forwarded.";

/// The forward plan of a job that starts with `launch` in an efrd whose engine has
/// `egress`, with its forward socket at `socket` (`$CALL/forward.sock`); `None` when
/// the job gets no forwards (DL1, DL12).
///
/// Only a contained job with a network namespace of its own gets forwards: not a job in
/// the exit child, not one with a grant of the whole network, which listens on the
/// host's loopback already, and not a network view job, which cannot listen. The plan
/// says `proxy` exactly when the plan of the sandbox gives the job the network proxy
/// (`sandbox::plan::network`), so the spec check meets the same network.
pub(crate) fn plan_of(
    settings: &JobsSettings,
    launch: &Launch,
    egress: Egress,
    socket: PathBuf,
) -> Option<ForwardPlan> {
    if !settings.forward_ports {
        return None;
    }
    let Launch::Contained { grants } = launch else { return None };
    if grants.iter().any(|grant| matches!(grant, Grant::OpenNetwork | Grant::NetView)) {
        return None;
    }
    Some(ForwardPlan::new(socket, egress == Egress::Proxy))
}

/// The forwards of every job of efrd: the host ports that jobs hold, and how often a
/// reload turned forwards off. Cheap to clone; clones share everything.
#[derive(Clone)]
pub(crate) struct Forwards {
    inner: Arc<Shared>,
}

impl std::fmt::Debug for Forwards {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forwards").field("held", &self.lock().ports.len()).finish()
    }
}

struct Shared {
    ports: Arc<dyn HostPorts>,
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    /// The job that holds each forwarded host port.
    ports: std::collections::BTreeMap<u16, JobKey>,
    /// How often a reload turned `jobs.forward_ports` off.
    generation: u64,
}

/// Why a job cannot hold a host port in efrd's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Taken {
    /// Another job holds it.
    By(JobKey),
    /// efrd forwards [`MAX_DAEMON_FORWARDS`] ports already.
    Full,
}

impl Forwards {
    /// The forwards of an efrd that binds host ports through `ports`.
    pub(crate) fn new(ports: Arc<dyn HostPorts>) -> Forwards {
        Forwards { inner: Arc::new(Shared { ports, held: Mutex::new(Held::default()) }) }
    }

    fn lock(&self) -> MutexGuard<'_, Held> {
        // NOTE: each change is whole, so a poisoned lock still holds a usable table.
        self.inner.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The host ports.
    pub(crate) fn ports(&self) -> Arc<dyn HostPorts> {
        Arc::clone(&self.inner.ports)
    }

    /// How often a reload turned forwards off so far. A job takes it before it reads
    /// the settings, and forwards nothing once it changed.
    pub(crate) fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Counts a reload that turned forwards off.
    pub(crate) fn turn_off(&self) {
        let mut held = self.lock();
        held.generation = held.generation.saturating_add(1);
    }

    /// Holds `port` for the job `key`, unless another job holds it or efrd forwards as
    /// many ports as it may.
    pub(crate) fn reserve(&self, key: JobKey, port: u16) -> Result<(), Taken> {
        let mut held = self.lock();
        match held.ports.get(&port) {
            Some(other) if *other != key => return Err(Taken::By(*other)),
            Some(_) => return Ok(()),
            None => {}
        }
        if held.ports.len() >= MAX_DAEMON_FORWARDS {
            return Err(Taken::Full);
        }
        held.ports.insert(port, key);
        Ok(())
    }

    /// Frees `port` when the job `key` holds it.
    pub(crate) fn release(&self, key: JobKey, port: u16) {
        let mut held = self.lock();
        if held.ports.get(&port) == Some(&key) {
            held.ports.remove(&port);
        }
    }

    /// Frees every port of the job `key`.
    pub(crate) fn release_job(&self, key: JobKey) {
        self.lock().ports.retain(|_, holder| *holder != key);
    }
}

/// A job's forward socket, bound before its launcher starts. Dropping it closes the
/// socket.
#[derive(Debug)]
pub(crate) struct ForwardSocket {
    listener: tokio::net::UnixListener,
    plan: ForwardPlan,
    /// [`Forwards::generation`] before the job read its settings.
    generation: u64,
}

/// Binds `$CALL/forward.sock` in the job's `call_dir` for the job's `plan`, for a job
/// that read its settings at the `generation` of [`Forwards`].
pub(crate) async fn bind(
    call_dir: &Path,
    plan: ForwardPlan,
    generation: u64,
) -> Result<ForwardSocket, DaemonError> {
    let dir = call_dir.to_path_buf();
    let path = call_dir.join(FORWARD_SOCKET_FILE);
    let listener = tokio::task::spawn_blocking(move || bind_socket(&dir, FORWARD_SOCKET_FILE))
        .await
        .map_err(|_| DaemonError::TaskPanicked { task: "forward socket" })?
        .map_err(|source| DaemonError::Io { path: path.clone(), source })?;
    let listener = tokio::net::UnixListener::from_std(listener)
        .map_err(|source| DaemonError::Io { path, source })?;
    Ok(ForwardSocket { listener, plan, generation })
}

/// The forwards of one running job, in its entry of the job table. Dropping it closes
/// the forward socket, every listener and every connection of the job, and frees its
/// ports.
pub(crate) struct JobForward {
    key: JobKey,
    forwards: Forwards,
    token: CancellationToken,
    off: CancellationToken,
}

impl std::fmt::Debug for JobForward {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobForward").field("key", &self.key).finish_non_exhaustive()
    }
}

impl JobForward {
    /// Stops every forward of the job with a `stopped` event for each port, after a
    /// reload turned `jobs.forward_ports` off; the job forwards nothing more.
    pub(crate) fn turn_off(&self) {
        self.off.cancel();
    }
}

impl Drop for JobForward {
    fn drop(&mut self) {
        self.token.cancel();
        self.forwards.release_job(self.key);
    }
}

/// The line of a `job_port_forwarded` event (DL15), without a prefix: `job j1 listens
/// on 127.0.0.1:8765 -> http://localhost:8765`, `...; not forwarded: ...`, or `job j1:
/// port 8765 is no longer forwarded (jobs.forward_ports is off)`.
pub(crate) fn line(
    job: &JobId,
    listen: &str,
    port: u16,
    outcome: ForwardOutcome,
    by_job: Option<&JobId>,
    reason: Option<&str>,
) -> String {
    let address = if listen.contains(':') {
        format!("[{listen}]:{port}")
    } else {
        format!("{listen}:{port}")
    };
    let head = format!("job {job} listens on {address}");
    let why = match outcome {
        ForwardOutcome::Forwarded => return format!("{head} -> http://localhost:{port}"),
        ForwardOutcome::InUse => match by_job {
            Some(other) => format!("127.0.0.1:{port} is in use by job {other}"),
            None => format!("127.0.0.1:{port} is in use on this machine"),
        },
        ForwardOutcome::Refused => match reason {
            Some(reason) => format!("efr does not forward port {port} ({reason})"),
            None => format!("efr does not forward port {port}"),
        },
        ForwardOutcome::Limit => {
            reason.unwrap_or("efr forwards as many ports as it may").to_owned()
        }
        ForwardOutcome::Stopped => {
            return match reason {
                Some(reason) => format!("job {job}: port {port} is no longer forwarded ({reason})"),
                None => format!("job {job}: port {port} is no longer forwarded"),
            };
        }
        _ => return head,
    };
    format!("{head}; not forwarded: {why}")
}

/// `forwarded ports 8765, 5173` for the `jobs` tool; `None` with no forward.
pub(crate) fn ports_text(forwards: &[PortForward]) -> Option<String> {
    if forwards.is_empty() {
        return None;
    }
    let ports: Vec<String> = forwards.iter().map(|forward| forward.port.to_string()).collect();
    Some(format!("forwarded ports {}", ports.join(", ")))
}

#[cfg(test)]
mod tests;
