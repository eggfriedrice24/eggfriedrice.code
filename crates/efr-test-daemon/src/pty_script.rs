//! The fake PTY holder: every "PTY" is one end of a socketpair, and the test plays the
//! shell on the other end with scripted bytes.
//!
//! The daemon's shell sessions read and write the master exactly as they would a real
//! PTY (`AsyncFd` works on a socket), so the whole path from the shell tool to the
//! recording, the screen and `pty.attach` runs, without a zsh and without real time.
//! The test reads what the session types (a command is a bracketed paste and Enter) and
//! prints what a zsh with the efr integration would: OSC 133 marks around prompts and
//! commands.
//!
//! [`PtyScript`] is a list of [`PtyStep`]s, prints and expected input, that a
//! [`FakeTerminal`] plays; the scenario replay turns `pty_bytes` records into the same
//! steps.

use std::collections::HashMap;
use std::fmt;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use efr_daemon::{
    ChildStatus, HolderError, PtyHandle, PtyHolder, PtyInfo, Signal, SignalTarget, SpawnSpec,
};
use efr_protocol::{PtyId, Size};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::TestDaemonError;

/// A marked primary prompt, as the efr zsh integration prints it: `A`, the prompt
/// text, `B`. A session becomes ready to type when it sees one.
pub const PROMPT: &[u8] = b"\x1b]133;A;cl=line\x07% \x1b]133;B\x07";

/// The first pid the fake holder hands out; each spawn gets the next one.
const FIRST_PID: u32 = 4000;

/// The bytes a shell session types for `command` at a marked prompt: one bracketed
/// paste and Enter.
pub fn typed_command(command: &str) -> Vec<u8> {
    let mut bytes = b"\x1b[200~".to_vec();
    bytes.extend_from_slice(command.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~\r");
    bytes
}

/// What a zsh with the integration prints for a command that ran: the newline after
/// the typed line, `C`, the output, `D` with the status, and the next prompt.
pub fn command_output(output: &[u8], status: i32) -> Vec<u8> {
    let mut bytes = b"\r\n\x1b]133;C\x07".to_vec();
    bytes.extend_from_slice(output);
    bytes.extend_from_slice(format!("\x1b]133;D;{status}\x07").as_bytes());
    bytes.extend_from_slice(PROMPT);
    bytes
}

/// One step of a script.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PtyStep {
    /// The shell prints these bytes.
    Print(Vec<u8>),
    /// The session must type exactly these bytes next.
    Expect(Vec<u8>),
}

/// What a fake shell does, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PtyScript {
    steps: Vec<PtyStep>,
}

impl PtyScript {
    /// An empty script.
    pub fn new() -> Self {
        PtyScript::default()
    }

    /// The shell prints `bytes`.
    #[must_use]
    pub fn print(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.steps.push(PtyStep::Print(bytes.into()));
        self
    }

    /// The shell prints a marked prompt.
    #[must_use]
    pub fn prompt(self) -> Self {
        self.print(PROMPT)
    }

    /// The session must type exactly `bytes` next.
    #[must_use]
    pub fn expect(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.steps.push(PtyStep::Expect(bytes.into()));
        self
    }

    /// The session types `command`, and the shell runs it: `output`, then `status`,
    /// then the next prompt.
    #[must_use]
    pub fn command(self, command: &str, output: &[u8], status: i32) -> Self {
        self.expect(typed_command(command)).print(command_output(output, status))
    }

    /// The steps, in order.
    pub fn steps(&self) -> &[PtyStep] {
        &self.steps
    }
}

/// A [`PtyHolder`] whose masters are socketpairs.
///
/// Each spawn hands the daemon one end and keeps the other for the test, which takes
/// it with [`terminal`](Self::terminal) in the order of spawns. A child runs until the
/// test ends it with [`end`](Self::end) or the daemon sends it `SIGHUP` or `SIGKILL`;
/// [`PtyHolder::wait`] returns then, as it does for a reaped child.
pub struct FakePtyHolder {
    state: Mutex<State>,
    spawns: watch::Sender<usize>,
}

#[derive(Default)]
struct State {
    specs: Vec<SpawnSpec>,
    ptys: HashMap<PtyId, FakePty>,
    terminals: Vec<Option<(PtyId, UnixStream)>>,
    signals: Vec<(PtyId, Signal, SignalTarget)>,
    resizes: Vec<(PtyId, Size)>,
    released: Vec<PtyId>,
}

struct FakePty {
    pid: u32,
    size: Size,
    status: watch::Sender<ChildStatus>,
}

impl FakePtyHolder {
    /// A holder that has spawned nothing.
    pub fn new() -> Arc<Self> {
        Arc::new(FakePtyHolder { state: Mutex::default(), spawns: watch::Sender::new(0) })
    }

    /// The test's end of the `index`-th spawned PTY (from 0), once the daemon has
    /// spawned it. Each end can be taken once; a second call for the same index waits
    /// until the holder is dropped and then fails.
    pub async fn terminal(&self, index: usize) -> Result<FakeTerminal, TestDaemonError> {
        let mut spawns = self.spawns.subscribe();
        spawns
            .wait_for(|count| *count > index)
            .await
            .map_err(|_| TestDaemonError::PtyNeverSpawned { index })?;
        let taken = self.lock().terminals.get_mut(index).and_then(Option::take);
        let Some((pty_id, stream)) = taken else {
            return Err(TestDaemonError::PtyNeverSpawned { index });
        };
        stream
            .set_nonblocking(true)
            .map_err(|source| TestDaemonError::Io { what: "preparing the fake PTY", source })?;
        let stream = tokio::net::UnixStream::from_std(stream)
            .map_err(|source| TestDaemonError::Io { what: "preparing the fake PTY", source })?;
        Ok(FakeTerminal { pty_id, stream, unread: Vec::new() })
    }

    /// How many PTYs the daemon has spawned.
    pub fn spawned(&self) -> usize {
        self.lock().specs.len()
    }

    /// The spec of every spawn, in order.
    pub fn specs(&self) -> Vec<SpawnSpec> {
        self.lock().specs.clone()
    }

    /// Every signal the daemon sent, in order.
    pub fn signals(&self) -> Vec<(PtyId, Signal, SignalTarget)> {
        self.lock().signals.clone()
    }

    /// Every resize the daemon asked for, in order.
    pub fn resizes(&self) -> Vec<(PtyId, Size)> {
        self.lock().resizes.clone()
    }

    /// Every PTY the daemon released, in order.
    pub fn released(&self) -> Vec<PtyId> {
        self.lock().released.clone()
    }

    /// Ends the child of `pty_id` with `status`, as if the shell exited. Returns false
    /// when the holder does not hold that PTY.
    pub fn end(&self, pty_id: PtyId, status: ChildStatus) -> bool {
        match self.lock().ptys.get(&pty_id) {
            Some(pty) => {
                pty.status.send_replace(status);
                true
            }
            None => false,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Every critical section leaves the state whole, so a poisoned lock is usable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for FakePtyHolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock();
        f.debug_struct("FakePtyHolder")
            .field("spawned", &state.specs.len())
            .field("held", &state.ptys.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl PtyHolder for FakePtyHolder {
    async fn spawn(&self, spec: SpawnSpec) -> Result<PtyHandle, HolderError> {
        spec.validate()?;
        let (ours, theirs) = UnixStream::pair()
            .map_err(|source| HolderError::Spawn { program: spec.program.clone(), source })?;
        let mut state = self.lock();
        if state.ptys.contains_key(&spec.pty_id) {
            return Err(HolderError::AlreadyExists { pty_id: spec.pty_id });
        }
        let pid = FIRST_PID.saturating_add(u32::try_from(state.specs.len()).unwrap_or(u32::MAX));
        let pty_id = spec.pty_id;
        state.ptys.insert(
            pty_id,
            FakePty { pid, size: spec.size, status: watch::Sender::new(ChildStatus::Running) },
        );
        state.terminals.push(Some((pty_id, theirs)));
        state.specs.push(spec);
        let count = state.specs.len();
        drop(state);
        self.spawns.send_replace(count);
        Ok(PtyHandle { master: OwnedFd::from(ours), child_pid: pid, pty_id })
    }

    async fn resize(&self, pty_id: PtyId, size: Size) -> Result<(), HolderError> {
        let mut state = self.lock();
        state.ptys.get_mut(&pty_id).ok_or(HolderError::NotFound { pty_id })?.size = size;
        state.resizes.push((pty_id, size));
        Ok(())
    }

    async fn signal(
        &self,
        pty_id: PtyId,
        signal: Signal,
        target: SignalTarget,
    ) -> Result<(), HolderError> {
        let mut state = self.lock();
        let pty = state.ptys.get(&pty_id).ok_or(HolderError::NotFound { pty_id })?;
        if !pty.status.borrow().is_running() {
            return Err(HolderError::Exited { pty_id });
        }
        // A shell dies of these when they reach it; SIGINT only reaches its command.
        if target == SignalTarget::Child {
            match signal {
                Signal::Hangup => {
                    pty.status.send_replace(ChildStatus::Signaled { signal: 1 });
                }
                Signal::Kill => {
                    pty.status.send_replace(ChildStatus::Signaled { signal: 9 });
                }
                _ => {}
            }
        }
        state.signals.push((pty_id, signal, target));
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PtyInfo>, HolderError> {
        let state = self.lock();
        Ok(state
            .ptys
            .iter()
            .map(|(&pty_id, pty)| PtyInfo {
                pty_id,
                child_pid: pty.pid,
                size: pty.size,
                status: *pty.status.borrow(),
            })
            .collect())
    }

    async fn wait(&self, pty_id: PtyId) -> Result<ChildStatus, HolderError> {
        let mut status = self
            .lock()
            .ptys
            .get(&pty_id)
            .ok_or(HolderError::NotFound { pty_id })?
            .status
            .subscribe();
        match status.wait_for(|status| !status.is_running()).await {
            Ok(status) => Ok(*status),
            // The sender went with a release: the PTY is no longer held.
            Err(_) => Err(HolderError::NotFound { pty_id }),
        }
    }

    async fn release(&self, pty_id: PtyId) -> Result<(), HolderError> {
        let mut state = self.lock();
        state.ptys.remove(&pty_id).ok_or(HolderError::NotFound { pty_id })?;
        state.released.push(pty_id);
        Ok(())
    }
}

/// The test's end of a fake PTY: it reads what the session types and writes what the
/// shell prints.
#[derive(Debug)]
pub struct FakeTerminal {
    pty_id: PtyId,
    stream: tokio::net::UnixStream,
    /// Bytes read from the session that no read has returned yet.
    unread: Vec<u8>,
}

impl FakeTerminal {
    /// The PTY's id.
    pub fn pty_id(&self) -> PtyId {
        self.pty_id
    }

    /// Prints `bytes` as the shell.
    pub async fn print(&mut self, bytes: &[u8]) -> Result<(), TestDaemonError> {
        self.stream
            .write_all(bytes)
            .await
            .map_err(|source| TestDaemonError::Io { what: "writing to the fake PTY", source })
    }

    /// Prints a marked prompt, which makes the session ready.
    pub async fn prompt(&mut self) -> Result<(), TestDaemonError> {
        self.print(PROMPT).await
    }

    /// Prints what a zsh prints for a command that ran with `output` and `status`, up
    /// to the next prompt.
    pub async fn run(&mut self, output: &[u8], status: i32) -> Result<(), TestDaemonError> {
        self.print(&command_output(output, status)).await
    }

    /// Reads exactly `len` typed bytes.
    pub async fn typed_exact(&mut self, len: usize) -> Result<Vec<u8>, TestDaemonError> {
        while self.unread.len() < len {
            self.fill().await?;
        }
        Ok(self.unread.drain(..len).collect())
    }

    /// Reads what the session types against `expected`: exactly `expected.len()` bytes
    /// while they agree with it, and stops at the first byte that differs, returning
    /// what was read up to the end of that typed line. The caller compares the result
    /// with `expected`; a session that types something else never makes this wait for
    /// bytes that will not come.
    pub async fn typed_against(&mut self, expected: &[u8]) -> Result<Vec<u8>, TestDaemonError> {
        loop {
            let common = self.unread.len().min(expected.len());
            if let Some(differs) = (0..common).find(|&at| self.unread[at] != expected[at]) {
                let end = find(&self.unread[differs..], b"\r")
                    .map_or(self.unread.len(), |at| differs + at + 1);
                return Ok(self.unread.drain(..end).collect());
            }
            if common == expected.len() {
                return Ok(self.unread.drain(..common).collect());
            }
            self.fill().await?;
        }
    }

    /// Reads typed bytes up to and including the first `needle`.
    pub async fn typed_until(&mut self, needle: &[u8]) -> Result<Vec<u8>, TestDaemonError> {
        loop {
            if let Some(at) = find(&self.unread, needle) {
                return Ok(self.unread.drain(..at + needle.len()).collect());
            }
            self.fill().await?;
        }
    }

    /// Reads one typed line, up to and including the Enter (`\r`) that ends it.
    pub async fn typed_line(&mut self) -> Result<Vec<u8>, TestDaemonError> {
        self.typed_until(b"\r").await
    }

    /// Plays `script`: prints its prints and checks that the session types what it
    /// expects, in order. A difference fails with [`TestDaemonError::Mismatch`].
    pub async fn play(&mut self, script: &PtyScript) -> Result<(), TestDaemonError> {
        for (index, step) in script.steps().iter().enumerate() {
            match step {
                PtyStep::Print(bytes) => self.print(bytes).await?,
                PtyStep::Expect(expected) => {
                    let typed = self.typed_against(expected).await?;
                    if &typed != expected {
                        return Err(TestDaemonError::Mismatch {
                            line: index + 1,
                            kind: "pty_bytes",
                            expected: bytes_json(expected),
                            actual: bytes_json(&typed),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    async fn fill(&mut self) -> Result<(), TestDaemonError> {
        let mut buf = [0_u8; 4096];
        let read = self
            .stream
            .read(&mut buf)
            .await
            .map_err(|source| TestDaemonError::Io { what: "reading from the fake PTY", source })?;
        if read == 0 {
            return Err(TestDaemonError::PtyClosed { typed: std::mem::take(&mut self.unread) });
        }
        self.unread.extend_from_slice(&buf[..read]);
        Ok(())
    }
}

/// Bytes as JSON for a mismatch report: the text when it is UTF-8, else the numbers.
pub(crate) fn bytes_json(bytes: &[u8]) -> serde_json::Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => serde_json::Value::from(text),
        Err(_) => serde_json::Value::from(bytes.to_vec()),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

#[cfg(test)]
mod tests;
