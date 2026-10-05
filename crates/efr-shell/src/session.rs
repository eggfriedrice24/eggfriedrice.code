//! One hidden shell: the actor that owns its state, its runs and its end.
//!
//! Every byte the shell prints reaches the actor in stream order (`Msg::Chunk`), and
//! the actor scans it for marks itself, so a run sees each mark exactly between the
//! bytes before and after it; nothing depends on two channels agreeing on an order.
//! The logic lives in [`SessionCore`], which does no IO and so is tested with plain
//! byte strings; [`SessionActor`] wraps it with the channels, the startup timer and
//! the shutdown.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use efr_holder::{ChildStatus, PtyHolder, Size};
use efr_protocol::{CallId, ConversationId, PtyId, SecretText, Seq};
use efr_screen::{ScreenHandle, ShellMark, ShellMarkScanner};
use efr_stdx::time::{Clock, Sleep};
use jiff::Timestamp;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::capture::Kept;
use crate::input::{self, Probe};
use crate::modes::Terminal;
use crate::run::{Delimiter, MarkRun, MarkStep, Progress, RunMode, RunOutput, marked_line};
use crate::sentinel::{SentinelRun, sentinel_line};
use crate::{Phase, ShellError, ShellNotice, ShellObserver, ShellState};

/// How many messages may wait for a session's actor.
pub(crate) const INBOX_CAPACITY: usize = 64;

/// The key the integration binds to send-break, which cancels an unfinished line at a
/// continuation prompt.
const CANCEL_LINE: &[u8] = b"\x1b[efr-cancel~";

/// What a session's actor is asked to do.
#[derive(Debug)]
pub(crate) enum Msg {
    /// Bytes the shell printed, starting at stream offset `start`.
    Chunk { start: Seq, bytes: Bytes },
    /// The holder reaped the shell.
    Exited(Option<ChildStatus>),
    /// Type a command and answer when it ends.
    Run(RunOrder),
    /// Stop waiting for a run whose caller timed out.
    Detach { id: u64, reply: oneshot::Sender<Detached> },
    /// Tell the shell's state.
    State { reply: oneshot::Sender<ShellState> },
    /// Tell what a look at run `id` needs; `None` once the run ended or was left.
    Probe { id: u64, reply: oneshot::Sender<Option<Probe>> },
    /// Type `text` for the running command of `call`, if it waits for such input.
    /// `Debug` shows no text: `SecretText` hides it.
    Answer {
        call: CallId,
        text: SecretText,
        hidden: bool,
        reply: oneshot::Sender<Result<(), ShellError>>,
    },
}

/// One run, as the caller hands it to the actor.
#[derive(Debug)]
pub(crate) struct RunOrder {
    pub(crate) id: u64,
    pub(crate) command: String,
    pub(crate) mode: RunMode,
    pub(crate) output_limit: usize,
    /// The tool call that runs the command, whose answers may reach it.
    pub(crate) call: Option<CallId>,
    /// The sentinel token, drawn by the caller from the injected generator.
    pub(crate) token: String,
    pub(crate) reply: oneshot::Sender<Result<RunEnd, ShellError>>,
    pub(crate) progress: watch::Sender<Progress>,
}

/// A run that ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunEnd {
    pub(crate) output: RunOutput,
    pub(crate) cwd: PathBuf,
    pub(crate) delimiter: Delimiter,
}

/// The answer to [`Msg::Detach`].
#[derive(Debug)]
pub(crate) enum Detached {
    /// The run had already ended; its answer is on its reply channel.
    Gone,
    /// The run was still waiting for a prompt and was dropped without being typed.
    Unstarted,
    /// The run was typed and goes on without a caller.
    Running {
        kept: Kept,
        range: Option<Range<Seq>>,
        last_output: Option<Timestamp>,
        cwd: PathBuf,
        delimiter: Delimiter,
    },
}

/// Whether the shell still runs; the actor publishes it, handles watch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Life {
    Running,
    Ended(Option<ChildStatus>),
}

/// Where [`SessionCore::submit`] puts a run.
enum Placement {
    Now(Delimiter),
    Wait,
    Refuse,
}

struct Active {
    id: u64,
    call: Option<CallId>,
    machine: Machine,
    reply: oneshot::Sender<Result<RunEnd, ShellError>>,
    progress: watch::Sender<Progress>,
    /// True once the caller was told that the command runs.
    started: bool,
    /// How many answers were typed for this run.
    answers: u64,
}

impl Active {
    /// Tells the caller once that the command runs, so it starts to look for input.
    fn note_start(&mut self) {
        if !self.started && self.machine.running() {
            self.started = true;
            self.progress.send_modify(|progress| progress.started = true);
        }
    }
}

enum Machine {
    Marks(MarkRun),
    Sentinel(SentinelRun),
}

impl Machine {
    fn delimiter(&self) -> Delimiter {
        match self {
            Machine::Marks(_) => Delimiter::Marks,
            Machine::Sentinel(_) => Delimiter::Sentinel,
        }
    }

    /// True between the command's start and its end.
    fn running(&self) -> bool {
        match self {
            Machine::Marks(run) => run.running(),
            Machine::Sentinel(run) => run.running(),
        }
    }
}

/// The state and runs of one shell, without IO. Each method returns the bytes to
/// write to the shell.
pub(crate) struct SessionCore {
    conversation: ConversationId,
    state: ShellState,
    scanner: ShellMarkScanner,
    /// The stream offset after the last byte read: where the next typed line's echo
    /// and output begin.
    next: Seq,
    last_output: Option<Timestamp>,
    active: Option<Active>,
    queued: Option<RunOrder>,
    /// A typed run whose caller stopped waiting, still read so that the next run knows
    /// when the shell is free again. Without it, a run detached before its `C` mark
    /// would leave the phase at `Ready` and the next line would be typed ahead into
    /// the running one.
    orphan: Option<Machine>,
    observer: Arc<dyn ShellObserver>,
}

impl SessionCore {
    pub(crate) fn new(
        conversation: ConversationId,
        state: ShellState,
        observer: Arc<dyn ShellObserver>,
    ) -> Self {
        SessionCore {
            conversation,
            state,
            scanner: ShellMarkScanner::new(),
            next: Seq::ZERO,
            last_output: None,
            active: None,
            queued: None,
            orphan: None,
            observer,
        }
    }

    pub(crate) fn state(&self) -> &ShellState {
        &self.state
    }

    pub(crate) fn pty_id(&self) -> PtyId {
        self.state.pty_id
    }

    /// Takes a run: types it now, keeps it until the shell is free, or refuses it when
    /// another run is under way or an unfinished line waits for input. A run whose
    /// caller has stopped waiting is dropped, never typed: the turn that asked for it
    /// may have been interrupted, and an approved command must not run after that.
    pub(crate) fn submit(&mut self, order: RunOrder) -> Vec<Bytes> {
        if order.reply.is_closed() {
            return Vec::new();
        }
        if self.active.is_some() || self.queued.is_some() {
            self.refuse(order);
            return Vec::new();
        }
        match self.placement(order.mode) {
            Placement::Now(delimiter) => self.start(order, delimiter),
            Placement::Wait => {
                self.queued = Some(order);
                Vec::new()
            }
            Placement::Refuse => {
                self.refuse(order);
                Vec::new()
            }
        }
    }

    /// Whether a run in `mode` can be typed now.
    fn placement(&self, mode: RunMode) -> Placement {
        // An orphaned sentinel run owns the foreground until its end marker. An
        // orphaned marked run holds the next marked line until its `D`, but a sentinel
        // run is meant for whatever runs in the foreground, such as a nested shell.
        match (&self.orphan, mode) {
            (Some(Machine::Sentinel(_)), _) | (Some(Machine::Marks(_)), RunMode::Auto) => {
                return Placement::Wait;
            }
            _ => {}
        }
        match (mode, self.state.phase) {
            (RunMode::Sentinel, _) | (_, Phase::Unmarked) => Placement::Now(Delimiter::Sentinel),
            (_, Phase::Ready) => Placement::Now(Delimiter::Marks),
            // Only the user can finish that line, at the attached screen.
            (_, Phase::Continuation) => Placement::Refuse,
            (_, Phase::Starting | Phase::Prompting { .. } | Phase::Running | Phase::Finished) => {
                Placement::Wait
            }
        }
    }

    /// Takes bytes the shell printed.
    pub(crate) fn chunk(&mut self, start: Seq, bytes: &[u8], now: Timestamp) -> Vec<Bytes> {
        self.last_output = Some(now);
        let base = start.get();
        let end = base.saturating_add(bytes.len() as u64);
        self.next = Seq::new(end);
        let slice = |from: u64, to: u64| {
            let offset = |at: u64| {
                usize::try_from(at.saturating_sub(base)).unwrap_or(bytes.len()).min(bytes.len())
            };
            &bytes[offset(from)..offset(to)]
        };
        let mut writes = Vec::new();
        let mut cursor = base;
        for mark in self.scanner.scan(bytes, start) {
            // A mark that began in an earlier chunk starts before this one.
            let before = mark.start.get().max(cursor);
            if before > cursor {
                self.bytes(Seq::new(cursor), slice(cursor, before));
            }
            self.mark(&mark, &mut writes);
            cursor = cursor.max(mark.end.get());
        }
        if cursor < end {
            self.bytes(Seq::new(cursor), slice(cursor, end));
        }
        writes.extend(self.start_queued());
        writes
    }

    /// What a look at run `id` needs from the session, without the terminal's modes,
    /// which the actor reads; `None` once the run ended or was left.
    pub(crate) fn probe(&self, id: u64) -> Option<Probe> {
        let active = self.active.as_ref().filter(|active| active.id == id)?;
        Some(Probe {
            running: active.machine.running(),
            last_output: self.last_output,
            modes: None,
            answers: active.answers,
        })
    }

    /// Refuses an answer for `call` unless that call's command runs now.
    pub(crate) fn answerable(&self, call: CallId) -> Result<(), ShellError> {
        let conversation = self.conversation;
        let Some(active) = &self.active else {
            return Err(ShellError::NoCall { conversation });
        };
        if active.call != Some(call) {
            return Err(ShellError::NotWaiting { conversation, reason: "another call runs" });
        }
        if !active.machine.running() {
            return Err(ShellError::NotWaiting {
                conversation,
                reason: "the command does not run now",
            });
        }
        Ok(())
    }

    /// An answer was typed for the active run.
    pub(crate) fn answered(&mut self) {
        if let Some(active) = &mut self.active {
            active.answers = active.answers.saturating_add(1);
        }
    }

    /// The first marked prompt did not come in time.
    pub(crate) fn startup_expired(&mut self) -> Vec<Bytes> {
        self.state.startup_expired();
        self.start_queued()
    }

    /// Lets go of a run whose caller stopped waiting.
    pub(crate) fn detach(&mut self, id: u64) -> Detached {
        if self.queued.as_ref().is_some_and(|order| order.id == id) {
            self.queued = None;
            return Detached::Unstarted;
        }
        let Some(active) = self.active.take_if(|active| active.id == id) else {
            return Detached::Gone;
        };
        let delimiter = active.machine.delimiter();
        let (kept, range) = match &active.machine {
            Machine::Marks(run) => run.partial(),
            Machine::Sentinel(run) => run.partial(),
        };
        self.orphan = Some(active.machine);
        Detached::Running {
            kept,
            range,
            last_output: self.last_output,
            cwd: self.state.cwd.clone(),
            delimiter,
        }
    }

    /// The shell ended: every waiting run fails.
    pub(crate) fn exited(&mut self, status: Option<ChildStatus>) {
        let conversation = self.conversation;
        let error = || ShellError::Exited { conversation, status };
        if let Some(active) = self.active.take() {
            let _ = active.reply.send(Err(error()));
        }
        if let Some(order) = self.queued.take() {
            let _ = order.reply.send(Err(error()));
        }
        self.orphan = None;
        self.observer.notice(ShellNotice::Exited {
            conversation: self.conversation,
            pty_id: self.state.pty_id,
            status,
        });
    }

    fn refuse(&self, order: RunOrder) {
        let _ = order.reply.send(Err(ShellError::Busy { conversation: self.conversation }));
    }

    fn start(&mut self, order: RunOrder, delimiter: Delimiter) -> Vec<Bytes> {
        let (line, machine) = match delimiter {
            Delimiter::Marks => (
                marked_line(&order.command),
                Machine::Marks(MarkRun::new(self.next(), order.output_limit)),
            ),
            Delimiter::Sentinel => (
                sentinel_line(&order.command, &order.token),
                Machine::Sentinel(SentinelRun::new(&order.token, order.output_limit)),
            ),
        };
        match line {
            Ok(line) => {
                self.active = Some(Active {
                    id: order.id,
                    call: order.call,
                    machine,
                    reply: order.reply,
                    progress: order.progress,
                    started: false,
                    answers: 0,
                });
                vec![line]
            }
            Err(error) => {
                let _ = order.reply.send(Err(error));
                Vec::new()
            }
        }
    }

    /// Types the waiting run when the shell is free; [`submit`](Self::submit) drops it
    /// when its caller has gone in the meantime.
    fn start_queued(&mut self) -> Vec<Bytes> {
        if self.active.is_some() {
            return Vec::new();
        }
        let Some(order) = self.queued.take() else {
            return Vec::new();
        };
        self.submit(order)
    }

    fn next(&self) -> Seq {
        self.next
    }

    /// Bytes between marks: output for the active run, or for an orphaned sentinel run.
    fn bytes(&mut self, at: Seq, bytes: &[u8]) {
        if let Some(Machine::Sentinel(orphan)) = &mut self.orphan
            && orphan.on_bytes(at, bytes).0.is_some()
        {
            self.orphan = None;
        }
        let Some(active) = &mut self.active else {
            return;
        };
        let (ended, captured) = match &mut active.machine {
            Machine::Marks(run) => (None, run.on_bytes(at, bytes)),
            Machine::Sentinel(run) => run.on_bytes(at, bytes),
        };
        active.note_start();
        if captured {
            let capture = match &active.machine {
                Machine::Marks(run) => run.capture(),
                Machine::Sentinel(run) => run.capture(),
            };
            active.progress.send_replace(Progress::of(capture, active.started));
        }
        if let Some(output) = ended {
            self.finish(output);
        }
    }

    fn mark(&mut self, mark: &ShellMark, writes: &mut Vec<Bytes>) {
        if self.state.apply(&mark.kind) {
            self.notice_cwd();
        }
        if let Some(Machine::Marks(orphan)) = &mut self.orphan {
            match orphan.on_mark(mark) {
                MarkStep::Continue => {}
                MarkStep::Cancel => writes.push(Bytes::from_static(CANCEL_LINE)),
                MarkStep::Ended(_) => self.orphan = None,
            }
        }
        let Some(active) = &mut self.active else {
            return;
        };
        let Machine::Marks(run) = &mut active.machine else {
            return;
        };
        let step = run.on_mark(mark);
        active.note_start();
        match step {
            MarkStep::Continue => {}
            MarkStep::Cancel => writes.push(Bytes::from_static(CANCEL_LINE)),
            MarkStep::Ended(output) => self.finish(output),
        }
    }

    fn finish(&mut self, output: RunOutput) {
        let Some(active) = self.active.take() else {
            return;
        };
        let delimiter = active.machine.delimiter();
        // Without the integration the sentinel's `$PWD` is the shell's own directory.
        // In a nested shell it is the nested one's, which says nothing about the
        // hidden zsh, so the state keeps what the marks said.
        if self.state.phase == Phase::Unmarked
            && let Some(cwd) = &output.cwd
            && *cwd != self.state.cwd
        {
            self.state.cwd.clone_from(cwd);
            self.notice_cwd();
        }
        let cwd = output.cwd.clone().unwrap_or_else(|| self.state.cwd.clone());
        let _ = active.reply.send(Ok(RunEnd { output, cwd, delimiter }));
    }

    fn notice_cwd(&self) {
        self.observer.notice(ShellNotice::CwdChanged {
            conversation: self.conversation,
            pty_id: self.state.pty_id,
            cwd: self.state.cwd.clone(),
            host: self.state.host.clone(),
        });
    }
}

/// The task that owns one shell's [`SessionCore`].
pub(crate) struct SessionActor {
    pub(crate) core: SessionCore,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) holder: Arc<dyn PtyHolder>,
    pub(crate) writer: mpsc::Sender<Bytes>,
    /// The master and its modes, for answers.
    pub(crate) terminal: Terminal,
    pub(crate) screen: ScreenHandle,
    /// The reader, writer and reply tasks, stopped when the shell ends.
    pub(crate) tasks: Vec<JoinHandle<()>>,
    pub(crate) life: watch::Sender<Life>,
    /// The deadline for the first marked prompt; `None` once it passed or is moot.
    pub(crate) startup: Option<Sleep>,
}

impl SessionActor {
    pub(crate) async fn run(mut self, mut inbox: mpsc::Receiver<Msg>) {
        let status = loop {
            let msg = tokio::select! {
                biased;
                () = until(&mut self.startup) => {
                    self.startup = None;
                    let writes = self.core.startup_expired();
                    write_all(&self.writer, writes).await;
                    continue;
                }
                msg = inbox.recv() => msg,
            };
            // Every sender gone means the reader and the waiter are gone too; the
            // shell's end can no longer be learnt.
            let Some(msg) = msg else {
                break None;
            };
            match msg {
                Msg::Chunk { start, bytes } => {
                    let writes = self.core.chunk(start, &bytes, self.clock.now());
                    write_all(&self.writer, writes).await;
                }
                Msg::Run(order) => {
                    let writes = self.core.submit(order);
                    write_all(&self.writer, writes).await;
                }
                Msg::Detach { id, reply } => {
                    let _ = reply.send(self.core.detach(id));
                }
                Msg::State { reply } => {
                    let _ = reply.send(self.core.state().clone());
                }
                Msg::Probe { id, reply } => {
                    let probe = self
                        .core
                        .probe(id)
                        .map(|probe| Probe { modes: self.terminal.modes().ok(), ..probe });
                    let _ = reply.send(probe);
                }
                Msg::Answer { call, text, hidden, reply } => {
                    let _ = reply.send(answer(&mut self.core, &self.terminal, call, &text, hidden));
                }
                Msg::Exited(status) => break status,
            }
        };
        // Published first, so a caller whose message is never read learns why.
        self.life.send_replace(Life::Ended(status));
        self.core.exited(status);
        for task in &self.tasks {
            task.abort();
        }
        let _ = self.screen.shutdown().await;
        // The holder's copy of the master goes; the shell is gone or gets SIGHUP.
        let _ = self.holder.release(self.core.pty_id()).await;
        drop(inbox);
    }
}

/// Types an answer for `call` when its command runs and the terminal reads a line in
/// the right modes. The modes are read right before the one write, with no await
/// between them, so the terminal cannot change hands in between as far as this task
/// can tell: a getpass-style read that ended, or the shell's line editor back at its
/// prompt, is seen and refused.
fn answer(
    core: &mut SessionCore,
    terminal: &Terminal,
    call: CallId,
    text: &SecretText,
    hidden: bool,
) -> Result<(), ShellError> {
    core.answerable(call)?;
    let conversation = core.conversation;
    let modes = terminal.modes().map_err(|source| ShellError::Terminal { conversation, source })?;
    input::check_modes(modes, hidden)
        .map_err(|reason| ShellError::NotWaiting { conversation, reason })?;
    // NOTE: this write bypasses the writer task, so bytes still queued there (a reply to
    // a terminal query) can arrive after the answer; they cannot split it, because the
    // answer is a single write of its own.
    terminal
        .write_line(text.expose_secret())
        .map_err(|source| ShellError::Terminal { conversation, source })?;
    core.answered();
    Ok(())
}

// NOTE: a free function rather than a method, because `&SessionActor` is not `Send`
// (the startup sleep is not `Sync`) and the actor's future must be.
async fn write_all(writer: &mpsc::Sender<Bytes>, writes: Vec<Bytes>) {
    for bytes in writes {
        // A writer that is gone means the PTY failed; the shell's end follows.
        let _ = writer.send(bytes).await;
    }
}

/// Completes when the sleep does; never when there is none.
pub(crate) async fn until(sleep: &mut Option<Sleep>) {
    match sleep {
        Some(sleep) => sleep.as_mut().await,
        None => std::future::pending().await,
    }
}

/// The manager's side of one session.
#[derive(Debug, Clone)]
pub(crate) struct SessionHandle {
    pub(crate) conversation: ConversationId,
    pub(crate) pty_id: PtyId,
    pub(crate) pid: u32,
    pub(crate) screen: ScreenHandle,
    pub(crate) inbox: mpsc::Sender<Msg>,
    pub(crate) writer: mpsc::Sender<Bytes>,
    pub(crate) life: watch::Receiver<Life>,
    /// The terminal size the shell's programs see, shared by every clone; a finished
    /// command's output is replayed at this width.
    pub(crate) size: Arc<Mutex<Size>>,
}

impl SessionHandle {
    pub(crate) fn is_ended(&self) -> bool {
        self.inbox.is_closed() || matches!(*self.life.borrow(), Life::Ended(_))
    }

    pub(crate) fn size(&self) -> Size {
        // A size is copied in and out whole, so a poisoned lock still holds a valid one.
        *self.size.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set_size(&self, size: Size) {
        *self.size.lock().unwrap_or_else(PoisonError::into_inner) = size;
    }

    pub(crate) async fn send(&self, msg: Msg) -> Result<(), ShellError> {
        self.inbox.send(msg).await.map_err(|_| self.exited())
    }

    pub(crate) async fn state(&self) -> Result<ShellState, ShellError> {
        let (reply, answer) = oneshot::channel();
        self.send(Msg::State { reply }).await?;
        answer.await.map_err(|_| self.exited())
    }

    pub(crate) async fn write(&self, bytes: Bytes) -> Result<(), ShellError> {
        self.writer.send(bytes).await.map_err(|_| self.exited())
    }

    /// Waits until the shell has ended and tells how.
    pub(crate) async fn ended(&self) -> Option<ChildStatus> {
        let mut life = self.life.clone();
        match life.wait_for(|life| matches!(life, Life::Ended(_))).await {
            Ok(life) => match *life {
                Life::Ended(status) => status,
                Life::Running => None,
            },
            Err(_) => None,
        }
    }

    pub(crate) fn exited(&self) -> ShellError {
        let status = match *self.life.borrow() {
            Life::Ended(status) => status,
            Life::Running => None,
        };
        ShellError::Exited { conversation: self.conversation, status }
    }
}

#[cfg(test)]
mod tests;
