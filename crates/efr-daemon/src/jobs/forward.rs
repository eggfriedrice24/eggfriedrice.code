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
//! - The reach of a contained call (`forward/reach.rs`, v0.0.4 shell items DS34-DS48):
//!   [`Forwards`] also knows which job serves each forwarded port and since when each
//!   job forwards, and a `watch` changes at each forward and each release. A contained
//!   foreground call of a conversation with such a job gets a reach plan, and efrd
//!   routes each connection of the call's reach helper to the job's forwarder.
//! - The model learns each outcome (v0.0.4 L2): [`Forwards::tell`] keeps the last
//!   outcome of each port of a running job for the `jobs` tool ([`Forwards::ports_text`])
//!   and a note ([`note`]) for the model of the job's conversation, unless the port's
//!   last outcome was the same. The turn takes the notes before its next model call
//!   ([`Forwards::take_notes`], `Toolbox::take_notes`), also in the middle of a turn. The
//!   notes and the outcomes of a job go when the job ends or its forwarder fails. A
//!   reload that turns forwards off keeps them, so the model reads the `stopped` notes
//!   ([`Forwards::release_ports`]).
//! - A port that a job of the conversation listens on and efrd does not forward (in use,
//!   refused, a limit, or stopped) is no host for a call ([`Forwards::unreached`]): a
//!   shell line of an `auto` turn that names it on loopback gets [`refusal`] and does
//!   not run (`reach::fact`). The call does not reach the job there, and the whole
//!   network would reach another program of the machine, not the job.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use efr_config::JobsSettings;
use efr_permissions::Egress;
use efr_protocol::{ConversationId, ForwardOutcome, Grant, JobId, Launch};
use efr_sandbox::{BRIDGE_PORT, FORWARD_SOCKET_FILE, ForwardPlan, MAX_DAEMON_FORWARDS, MIN_PORT};
use jiff::Timestamp;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::JobKey;
use crate::DaemonError;
use crate::sandbox::proxy::bind_socket;
use crate::sandbox::seams::HostPorts;

mod host;
pub(crate) mod reach;
mod serve;

pub(crate) use host::SystemHostPorts;
pub(crate) use serve::serve;

/// How long efrd waits for the forwarder's `Hello` after the job started.
pub(crate) const HELLO_WAIT: Duration = Duration::from_secs(5);

/// How long a host connection waits for the forwarder's `Data` after efrd sent `Open`.
pub(crate) const OPEN_WAIT: Duration = Duration::from_secs(5);

/// The most notes of one conversation that wait for the model; a new note drops the
/// oldest one.
const MAX_NOTES: usize = 64;

/// The reason of a `stopped` event at a reload that turned forwards off.
pub(crate) const OFF_REASON: &str = "jobs.forward_ports is off";

/// The sentences of a job's start result when efr forwards its ports (DL16, DS45, L2).
pub(crate) const START_NOTE: &str = "efr forwards each loopback port from 1024 up that this \
    job listens on to the same port of 127.0.0.1 on the user's machine; give the user the \
    URL, such as http://localhost:8765. A port in use there and well-known service ports \
    are not forwarded. A note from efr tells you whether each port is forwarded, and the \
    jobs tool shows it. Later contained calls of this conversation reach the forwarded \
    ports at http://127.0.0.1:<port> or localhost, with no needs.hosts entry.";

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

/// The forwards of every job of efrd: the host ports that jobs hold, how often a
/// reload turned forwards off, and the routes of the calls' reach (DS39, DS41). Cheap to
/// clone; clones share everything.
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
    /// Counts each change of the forwarded ports or of the jobs that forward; a call's
    /// reach and the fact of a line wait on it (DS39, DS44).
    changes: watch::Sender<u64>,
}

#[derive(Default)]
struct Held {
    /// The job that holds each forwarded host port.
    ports: BTreeMap<u16, JobKey>,
    /// The ports of `ports` that efrd forwards: their host bind worked (DS35).
    bound: BTreeSet<u16>,
    /// Each running job whose forwards efrd serves, with its tasks (DS41).
    jobs: HashMap<JobKey, Route>,
    /// How often a reload turned `jobs.forward_ports` off.
    generation: u64,
    /// The last outcome of each port of each job, until the job's forwards end (L2).
    told: HashMap<JobKey, BTreeMap<u16, Told>>,
    /// The notes of each conversation that the model did not read yet, oldest first,
    /// each with its job.
    notes: HashMap<ConversationId, VecDeque<(JobKey, String)>>,
}

/// The last outcome of a port of a job, with its text for the `jobs` tool.
struct Told {
    outcome: ForwardOutcome,
    text: String,
    /// What efr tells the model when a line names the port and efrd does not forward
    /// it ([`refusal`]); `None` for a forward.
    refusal: Option<String>,
}

/// The forwards of one job as a call's reach finds them.
struct Route {
    ctx: Weak<serve::Ctx>,
    /// When efrd began to serve the job's forwards, on efrd's clock (DS44).
    since: Timestamp,
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
        let (changes, _) = watch::channel(0);
        Forwards { inner: Arc::new(Shared { ports, held: Mutex::new(Held::default()), changes }) }
    }

    fn lock(&self) -> MutexGuard<'_, Held> {
        // NOTE: each change is whole, so a poisoned lock still holds a usable table.
        self.inner.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Tells every waiter that the forwarded ports or the forwarding jobs changed. The
    /// caller holds no lock of the table.
    fn changed(&self) {
        self.inner.changes.send_modify(|count| *count = count.wrapping_add(1));
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
        let unbound = {
            let mut held = self.lock();
            if held.ports.get(&port) == Some(&key) {
                held.ports.remove(&port);
                held.bound.remove(&port)
            } else {
                false
            }
        };
        if unbound {
            self.changed();
        }
    }

    /// Frees every port of the job `key`, ends the routes of a call's reach to it, and
    /// forgets its outcomes and the notes about it that wait.
    pub(crate) fn release_job(&self, key: JobKey) {
        self.free_job(key, true);
    }

    /// Frees every port of the job `key` and ends the routes of a call's reach to it,
    /// but keeps its outcomes and its notes: a reload turned forwards off, and the model
    /// must still read the `stopped` notes. They go when the job ends.
    pub(crate) fn release_ports(&self, key: JobKey) {
        self.free_job(key, false);
    }

    /// Frees every port of the job `key`, and forgets its outcomes and notes when
    /// `forget` is true.
    fn free_job(&self, key: JobKey, forget: bool) {
        let changed = {
            let mut held = self.lock();
            if forget {
                held.told.remove(&key);
                if let Some(notes) = held.notes.get_mut(&key.conversation) {
                    notes.retain(|(job, _)| *job != key);
                    if notes.is_empty() {
                        held.notes.remove(&key.conversation);
                    }
                }
            }
            let Held { ports, bound, jobs, .. } = &mut *held;
            let mut changed = jobs.remove(&key).is_some();
            ports.retain(|port, holder| {
                let keep = *holder != key;
                if !keep {
                    changed |= bound.remove(port);
                }
                keep
            });
            changed
        };
        if changed {
            self.changed();
        }
    }

    /// Marks `port` of the job `key` as forwarded, once its host bind worked: a call's
    /// reach routes it from now on (DS35).
    pub(crate) fn forwarded(&self, key: JobKey, port: u16) {
        let added = {
            let mut held = self.lock();
            held.ports.get(&port) == Some(&key) && held.bound.insert(port)
        };
        if added {
            self.changed();
        }
    }

    /// Keeps `outcome` of `port` of the job `key` (a `job_port_forwarded` event of
    /// efrd) for the `jobs` tool, and a note for the model of the job's conversation,
    /// unless the port's last outcome was the same: the model reads each outcome once
    /// (L2). An outcome that efrd does not know is not kept, and nothing is kept for a
    /// job whose forwards efrd does not serve: its end forgot them already.
    pub(crate) fn tell(
        &self,
        key: JobKey,
        listen: &str,
        port: u16,
        outcome: ForwardOutcome,
        by_job: Option<&JobId>,
        reason: Option<&str>,
    ) {
        let job = JobId::nth(key.number);
        let note = note(&job, listen, port, outcome, by_job, reason);
        let text = port_text(port, outcome, by_job, reason);
        let (Some(note), Some(text)) = (note, text) else { return };
        let refusal = refusal(&job, port, outcome, by_job, reason);
        {
            let mut held = self.lock();
            if !held.jobs.contains_key(&key) {
                return;
            }
            let told = held.told.entry(key).or_default();
            if told.get(&port).is_some_and(|told| told.outcome == outcome) {
                return;
            }
            told.insert(port, Told { outcome, text, refusal });
            let notes = held.notes.entry(key.conversation).or_default();
            if notes.len() >= MAX_NOTES {
                notes.pop_front();
            }
            notes.push_back((key, note));
        }
        // NOTE: the fact of a line waits for the outcome of a port of a new job.
        self.changed();
    }

    /// Takes the notes of `conversation` that the model did not read yet, oldest
    /// first; each note is given once.
    pub(crate) fn take_notes(&self, conversation: ConversationId) -> Vec<String> {
        let notes = self.lock().notes.remove(&conversation).unwrap_or_default();
        notes.into_iter().map(|(_, note)| note).collect()
    }

    /// The outcome of each port of the job `key` for the `jobs` tool, by port: `ports
    /// 8765 -> http://localhost:8765; 8767 not forwarded: 127.0.0.1:8767 is in use on
    /// this machine`; `None` when the job has no port.
    pub(crate) fn ports_text(&self, key: JobKey) -> Option<String> {
        let held = self.lock();
        let told = held.told.get(&key).filter(|told| !told.is_empty())?;
        let ports: Vec<&str> = told.values().map(|told| told.text.as_str()).collect();
        Some(format!("ports {}", ports.join("; ")))
    }

    /// Records that efrd serves the forwards of the job `key` with `ctx` since `since`.
    fn serving(&self, key: JobKey, ctx: Weak<serve::Ctx>, since: Timestamp) {
        self.lock().jobs.insert(key, Route { ctx, since });
        self.changed();
    }

    /// The ports that efrd forwards for the running jobs of `conversation` and that a
    /// call's reach helper may listen on: from [`MIN_PORT`] up, never [`BRIDGE_PORT`],
    /// sorted (DS35, DS43).
    pub(crate) fn ports_of(&self, conversation: ConversationId) -> Vec<u16> {
        let held = self.lock();
        held.bound
            .iter()
            .copied()
            .filter(|port| *port >= MIN_PORT && *port != BRIDGE_PORT)
            .filter(|port| held.ports.get(port).is_some_and(|key| key.conversation == conversation))
            .collect()
    }

    /// True while a running job of `conversation` has forwards that efrd serves (DS34).
    pub(crate) fn has_forward_job(&self, conversation: ConversationId) -> bool {
        self.lock().jobs.keys().any(|key| key.conversation == conversation)
    }

    /// When the newest job of `conversation` with forwards began, on efrd's clock.
    pub(crate) fn newest_start(&self, conversation: ConversationId) -> Option<Timestamp> {
        let held = self.lock();
        held.jobs
            .iter()
            .filter(|(key, _)| key.conversation == conversation)
            .map(|(_, route)| route.since)
            .max()
    }

    /// The forwards of the job of `conversation` that forwards `port`, while it runs;
    /// `None` for a port of another conversation or one that efrd does not forward
    /// (DS41).
    fn route(&self, conversation: ConversationId, port: u16) -> Option<Arc<serve::Ctx>> {
        let held = self.lock();
        if !held.bound.contains(&port) {
            return None;
        }
        let key = held.ports.get(&port).filter(|key| key.conversation == conversation)?;
        held.jobs.get(key)?.ctx.upgrade()
    }

    /// A receiver that sees each change of the forwarded ports and of the forwarding
    /// jobs.
    pub(crate) fn changes(&self) -> watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    /// What efr tells the model when a line of `conversation` names `port` on loopback
    /// and a job of the conversation listens on it, but efrd does not forward it for a
    /// job of the conversation: the [`refusal`] of the port's last outcome, of the
    /// newest such job; `None` for a forwarded port and for a port that no job of the
    /// conversation was told about. It never names a job or a port of another
    /// conversation.
    pub(crate) fn unreached(&self, conversation: ConversationId, port: u16) -> Option<String> {
        let held = self.lock();
        let forwarded = held.bound.contains(&port)
            && held.ports.get(&port).is_some_and(|key| key.conversation == conversation);
        if forwarded {
            return None;
        }
        held.told
            .iter()
            .filter(|(key, _)| key.conversation == conversation)
            .filter_map(|(key, told)| Some((key.number, told.get(&port)?.refusal.as_ref()?)))
            .max_by_key(|(number, _)| *number)
            .map(|(_, refusal)| refusal.clone())
    }

    /// Waits until each port of `ports` has an outcome for a job of `conversation`:
    /// efrd forwards it, or [`unreached`](Self::unreached) tells why not.
    pub(crate) async fn wait_for(&self, conversation: ConversationId, ports: &[u16]) {
        let mut changes = self.changes();
        loop {
            changes.borrow_and_update();
            let forwarded = self.ports_of(conversation);
            let settled = |port: &u16| {
                forwarded.contains(port) || self.unreached(conversation, *port).is_some()
            };
            if ports.iter().all(settled) {
                return;
            }
            if changes.changed().await.is_err() {
                return;
            }
        }
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
    let head = format!("job {job} listens on {}", address(listen, port));
    match outcome {
        ForwardOutcome::Forwarded => format!("{head} -> http://localhost:{port}"),
        ForwardOutcome::Stopped => stopped(job, port, reason),
        _ => match not_forwarded(port, outcome, by_job, reason) {
            Some(why) => format!("{head}; not forwarded: {why}"),
            None => head,
        },
    }
}

/// The note for the model about an outcome of a port (v0.0.4 L2): what the user and the
/// model's calls can reach, and what to do; `None` for an outcome that efrd does not
/// know.
pub(crate) fn note(
    job: &JobId,
    listen: &str,
    port: u16,
    outcome: ForwardOutcome,
    by_job: Option<&JobId>,
    reason: Option<&str>,
) -> Option<String> {
    let head = format!("efr: job {job} listens on {}", address(listen, port));
    let what = match outcome {
        ForwardOutcome::Forwarded => {
            return Some(format!(
                "{head}; forwarded: the user can open http://localhost:{port}, and your \
                 contained calls reach it at 127.0.0.1:{port}."
            ));
        }
        ForwardOutcome::Stopped => {
            return Some(format!(
                "efr: {}. The user cannot open it now, and your contained calls do not reach \
                 it.",
                stopped(job, port, reason)
            ));
        }
        ForwardOutcome::Limit => {
            "a job keeps its forwarded ports until it ends; stop a job that you do not need"
        }
        _ => "choose another port",
    };
    let why = not_forwarded(port, outcome, by_job, reason)?;
    Some(format!(
        "{head}; not forwarded: {why}. The user cannot open it in a browser, and your calls \
         do not reach it; {what}."
    ))
}

/// What efr tells the model when a line names `port` of `job` on loopback and efrd does
/// not forward it: the call does not run. `None` for a forward and for an outcome that
/// efrd does not know.
pub(crate) fn refusal(
    job: &JobId,
    port: u16,
    outcome: ForwardOutcome,
    by_job: Option<&JobId>,
    reason: Option<&str>,
) -> Option<String> {
    let head = match outcome {
        ForwardOutcome::Stopped => match reason {
            Some(reason) => format!("port {port} of job {job} is no longer forwarded ({reason})"),
            None => format!("port {port} of job {job} is no longer forwarded"),
        },
        _ => format!(
            "port {port} of job {job} is not forwarded: {}",
            not_forwarded(port, outcome, by_job, reason)?
        ),
    };
    let host = if outcome == ForwardOutcome::InUse && by_job.is_none() {
        "full network would reach another program on this machine, not the job"
    } else {
        "full network does not reach the job either"
    };
    let what = match outcome {
        ForwardOutcome::Limit => "Stop a job that you do not need".to_owned(),
        ForwardOutcome::Stopped => "Tell the user that efr does not forward ports now".to_owned(),
        _ => format!("Stop job {job} and start the server on another port"),
    };
    Some(format!(
        "efr: {head}. Your contained calls do not reach the job on this port, and {host}. \
         efr did not run this call. {what}."
    ))
}

/// The outcome of `port` for the `jobs` tool: `8765 -> http://localhost:8765`, `8767
/// not forwarded: ...` or `8765 no longer forwarded (...)`; `None` for an outcome that
/// efrd does not know.
fn port_text(
    port: u16,
    outcome: ForwardOutcome,
    by_job: Option<&JobId>,
    reason: Option<&str>,
) -> Option<String> {
    match outcome {
        ForwardOutcome::Forwarded => Some(format!("{port} -> http://localhost:{port}")),
        ForwardOutcome::Stopped => Some(match reason {
            Some(reason) => format!("{port} no longer forwarded ({reason})"),
            None => format!("{port} no longer forwarded"),
        }),
        _ => not_forwarded(port, outcome, by_job, reason)
            .map(|why| format!("{port} not forwarded: {why}")),
    }
}

/// `127.0.0.1:8765`, or `[::1]:8765` for an IPv6 address.
fn address(listen: &str, port: u16) -> String {
    if listen.contains(':') { format!("[{listen}]:{port}") } else { format!("{listen}:{port}") }
}

/// `job j1: port 8765 is no longer forwarded (jobs.forward_ports is off)`.
fn stopped(job: &JobId, port: u16, reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("job {job}: port {port} is no longer forwarded ({reason})"),
        None => format!("job {job}: port {port} is no longer forwarded"),
    }
}

/// Why efr does not forward `port`; `None` for a forward, a stop and an outcome that
/// efrd does not know.
fn not_forwarded(
    port: u16,
    outcome: ForwardOutcome,
    by_job: Option<&JobId>,
    reason: Option<&str>,
) -> Option<String> {
    let why = match outcome {
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
        _ => return None,
    };
    Some(why)
}

#[cfg(test)]
mod tests;
