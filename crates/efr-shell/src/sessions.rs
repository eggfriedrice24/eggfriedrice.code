//! The manager: one hidden shell per conversation, spawned on first use.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use efr_holder::{PtyHandle, Signal, SignalTarget, Size, SpawnSpec};
use efr_protocol::{
    ApprovedBy, CallId, ConversationId, InputWait, PtyId, SecretText, ShellCommandEnd,
};
use efr_screen::ScreenHandle;
use efr_stdx::StdxError;
use efr_stdx::time::{Clock as _, Sleep};
use tokio::sync::{OnceCell, mpsc, oneshot, watch};

use crate::adopt::{self, AdoptedRun, AdoptedWatch, Adoption, Parts, TeeReader};
use crate::input::{self, InputWatch, Offer, Quiet};
use crate::live_tail::{LiveTail, Step};
use crate::modes::Terminal;
use crate::orphan::{LeftRun, OrphanWatch};
use crate::reader::{self, ReaderTargets};
use crate::replay::Replayer;
use crate::run::{Progress, RunMode, screen_tail, timed_out};
use crate::session::{
    Activity, Detached, ENDED, INBOX_CAPACITY, Life, Looked, Msg, NOT_STARTED, RunEnd, RunOrder,
    SessionActor, SessionCore, SessionHandle, until,
};
use crate::writer::{self, WRITE_CAPACITY};
use crate::{
    CommandResult, Completion, Delimiter, LeftRunning, NoProgress, OutputUpdate, RunProgress,
    RunRequest, SandboxRun, ShellConfig, ShellDeps, ShellError, ShellNotice, ShellState, env,
    integration, sandbox, sentinel,
};

/// The program looked for on the `PATH` when the config names none.
const DEFAULT_PROGRAM: &str = "zsh";

/// How long [`ShellSessions::move_to`] waits for the shell to take its `cd`.
pub const MOVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Ctrl+Z, the terminal's suspend character.
const SUSPEND: u8 = 0x1a;

/// One long-lived hidden zsh per conversation.
///
/// The first call that needs a conversation's shell spawns it through the injected
/// `PtyHolder`, with a screen from the injected `ScreenFactory`, and it lives until it
/// exits or [`close`](Self::close) ends it; the next call after that spawns a new one.
/// Cheap to clone; clones share the shells.
#[derive(Clone)]
pub struct ShellSessions {
    inner: Arc<Inner>,
}

/// The slot of one conversation: empty until its shell is spawned. Concurrent first
/// calls wait on the same spawn instead of starting two shells.
type Slot = Arc<OnceCell<SessionHandle>>;

struct Inner {
    /// The config as given; [`Start`] holds the parts that change while shells run.
    config: ShellConfig,
    deps: ShellDeps,
    start: Mutex<Arc<Start>>,
    installed: OnceCell<()>,
    sessions: Mutex<HashMap<ConversationId, Slot>>,
    next_run: AtomicU64,
    /// The runs of tool calls that a background job may adopt now, by call.
    moves: Mutex<HashMap<CallId, MoveSlot>>,
    /// Where the next shell of a conversation starts: the directory of its shell that a
    /// job adopted.
    restarts: Mutex<HashMap<ConversationId, PathBuf>>,
}

/// A run that takes a move request: it registers when it starts, and its registration
/// goes when it stops waiting for its command.
struct MoveSlot {
    conversation: ConversationId,
    run: u64,
    requests: mpsc::Sender<MoveRequest>,
}

/// A request to hand a run to a background job.
pub(crate) struct MoveRequest {
    to: oneshot::Sender<AdoptedRun>,
    reply: oneshot::Sender<Result<(), ShellError>>,
}

/// A run's end of its move requests; dropping it takes the run out of the map, so a
/// later request fails at once.
struct Moves {
    requests: mpsc::Receiver<MoveRequest>,
    inner: Arc<Inner>,
    call: CallId,
    run: u64,
}

impl Drop for Moves {
    fn drop(&mut self) {
        let mut moves = self.inner.moves.lock().unwrap_or_else(PoisonError::into_inner);
        if moves.get(&self.call).is_some_and(|slot| slot.run == self.run) {
            moves.remove(&self.call);
        }
    }
}

/// The next move request of a run, never when it takes none.
async fn next_move(moves: &mut Option<Moves>) -> MoveRequest {
    match moves {
        Some(moves) => match moves.requests.recv().await {
            Some(request) => request,
            None => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}

/// A move that the session's actor took.
struct Moved {
    request: MoveRequest,
    adoption: Adoption,
    output: TeeReader,
}

/// Why a run stopped waiting for its command before its end.
enum Leave {
    /// Its timeout passed.
    Timeout,
    /// A background job adopted it.
    Moved(Box<Moved>),
}

/// Why a move fails once the run answered its caller.
const ANSWERED: &str = "the call answered already";

/// How the next shell starts. The daemon changes it when its config reloads; a running
/// shell keeps what it started with.
#[derive(Debug)]
struct Start {
    program: PathBuf,
    /// True when the program is a zsh, which gets the integration.
    integration: bool,
    login: bool,
    trusted_programs: Arc<[String]>,
}

/// What [`ShellSessions::open`] found or started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShellInfo {
    /// The shell's PTY.
    pub pty_id: PtyId,
    /// The shell's process id.
    pub pid: u32,
    /// True when this call started the shell.
    pub spawned: bool,
}

impl ShellSessions {
    /// A manager with no shells yet.
    ///
    /// Fails when the config names no program and `zsh` is not on the `PATH` of
    /// `config.base_env`. Looking it up touches the file system, which is fine at
    /// startup.
    pub fn new(config: ShellConfig, deps: ShellDeps) -> Result<Self, ShellError> {
        let program = find_program(&config, config.program.as_deref())?;
        let start = Start {
            integration: integration::supports(&program),
            program,
            login: config.login,
            trusted_programs: config.trusted_programs.clone().into(),
        };
        Ok(ShellSessions {
            inner: Arc::new(Inner {
                config,
                deps,
                start: Mutex::new(Arc::new(start)),
                installed: OnceCell::new(),
                sessions: Mutex::new(HashMap::new()),
                next_run: AtomicU64::new(0),
                moves: Mutex::new(HashMap::new()),
                restarts: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Starts the shells spawned from now on with `program` (`None`: `zsh` on the
    /// `PATH`) and `login`, as [`ShellConfig::program`] and [`ShellConfig::login`] say.
    /// A running shell keeps what it started with. Fails, and changes nothing, when the
    /// program cannot be found.
    pub fn set_start(&self, program: Option<&Path>, login: bool) -> Result<(), ShellError> {
        let program = find_program(&self.inner.config, program)?;
        let mut start = self.start_lock();
        *start = Arc::new(Start {
            integration: integration::supports(&program),
            program,
            login,
            trusted_programs: Arc::clone(&start.trusted_programs),
        });
        Ok(())
    }

    /// Sets the programs whose aliases and functions a zsh with the integration removes,
    /// as [`ShellConfig::trusted_programs`] says.
    ///
    /// A zsh reads the set once, when it starts, so a running one with another set
    /// would still run an alias of a newly trusted name. Its next
    /// [`run_command`](Self::run_command) therefore restarts it first, in its current
    /// directory; while a command still runs in it, the run fails with
    /// [`ShellError::Busy`] instead, so no command ever runs with the old set.
    pub fn set_trusted_programs(&self, programs: Vec<String>) {
        let mut start = self.start_lock();
        if *start.trusted_programs == *programs {
            return;
        }
        *start = Arc::new(Start {
            program: start.program.clone(),
            integration: start.integration,
            login: start.login,
            trusted_programs: programs.into(),
        });
    }

    fn start(&self) -> Arc<Start> {
        Arc::clone(&self.start_lock())
    }

    fn start_lock(&self) -> MutexGuard<'_, Arc<Start>> {
        // The value is replaced whole, so a poisoned lock still holds a usable one.
        self.inner.start.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The conversation's shell, spawned in `start_dir` when it has none.
    pub async fn open(
        &self,
        conversation: ConversationId,
        start_dir: &Path,
    ) -> Result<ShellInfo, ShellError> {
        let (session, spawned) = self.session(conversation, start_dir).await?;
        Ok(ShellInfo { pty_id: session.pty_id, pid: session.pid, spawned })
    }

    /// Runs one command line in the conversation's shell, spawning the shell in
    /// `request.start_dir` when there is none, and returns when the command ends or
    /// the request's timeout passes.
    ///
    /// The command is typed as the user would type it and shows on the shell's
    /// screen. With the integration, the output is the recording between the `C` and
    /// `D` marks; without it (or with [`RunMode::Sentinel`](crate::RunMode::Sentinel)),
    /// between two random-token sentinels. `progress` hears the output as it grows.
    ///
    /// A run waits for the shell's prompt: while a new shell starts, while an earlier
    /// command still runs, and until the user answers what it asks. It fails with
    /// [`ShellError::NotReady`] when the prompt does not come before the timeout, at once
    /// with [`ShellError::HeldBy`] while a command left running at an earlier run's
    /// timeout waits for input (only the user frees the shell then), and
    /// with [`ShellError::Busy`] when another run is under way or an unfinished line
    /// waits at a continuation prompt. The timeout starts once a new shell has shown its
    /// first prompt; that wait has its own limit, the config's `startup_timeout`.
    ///
    /// At the timeout the command keeps running; the result is
    /// [`Completion::FullScreen`] for a program on the alternate screen,
    /// [`Completion::Interactive`] when it waits for input at the terminal and
    /// [`Completion::StillRunning`] otherwise, with the screen's last lines. A run with
    /// an [`interactive_limit`](RunRequest::interactive_limit) whose command runs, or
    /// with an [`input_limit`](RunRequest::input_limit) whose command waits for input,
    /// keeps waiting past the timeout while
    /// [`RunProgress::can_answer`](crate::RunProgress::can_answer) says yes, asked once
    /// per quiet period, up to that limit in all; then the same results apply.
    ///
    /// While the command runs, the run looks once per `quiet_period` whether it waits
    /// for input and tells `progress` of each change (see
    /// [`RunProgress::input_changed`](crate::RunProgress::input_changed)). When it waits
    /// for hidden input and
    /// [`RunProgress::can_answer_hidden`](crate::RunProgress::can_answer_hidden) says
    /// that nobody can answer, the run interrupts the command and returns
    /// [`Completion::Unanswered`] at once.
    pub async fn run_command(
        &self,
        conversation: ConversationId,
        request: RunRequest,
        progress: &mut dyn RunProgress,
    ) -> Result<CommandResult, ShellError> {
        let (session, _) = self.session(conversation, &request.start_dir).await?;
        let session = self.current(session).await?;
        self.run_on(&session, request, progress).await
    }

    /// `session`, or a new shell in its directory when it started with trusted
    /// programs other than the current ones (see
    /// [`set_trusted_programs`](Self::set_trusted_programs)).
    async fn current(&self, session: SessionHandle) -> Result<SessionHandle, ShellError> {
        let conversation = session.conversation;
        let trusted = Arc::clone(&self.start().trusted_programs);
        if session.trusted.as_ref().is_none_or(|started| *started == trusted) {
            return Ok(session);
        }
        let state = session.state().await?;
        if state.is_busy() {
            return Err(ShellError::Busy { conversation });
        }
        tracing::info!(%conversation, pty_id = %state.pty_id, "restarting an idle hidden shell for new trusted programs");
        self.close(conversation).await?;
        let (fresh, _) = self.session(conversation, &state.cwd).await?;
        Ok(fresh)
    }

    /// The state of the conversation's shell.
    pub async fn state(&self, conversation: ConversationId) -> Result<ShellState, ShellError> {
        self.existing(conversation)?.state().await
    }

    /// Types `text` and a carriage return for the running command of `call`, the tool
    /// call that [`RunRequest::call`](crate::RunRequest::call) named, also when its run
    /// was left at its timeout and its command goes on.
    ///
    /// The answer is written only while that call's command runs (between its `C` and
    /// `D`, or its sentinels), the run has reported a wait of the answer's kind (hidden
    /// with `hidden`, visible without), and the job that was in the terminal's
    /// foreground when it did still is, by its process group. With `hidden`, the
    /// terminal must also read a line (canonical input on) with echo off, so the text
    /// never reaches the output. A visible answer takes any modes: behind a relay such
    /// as `sudo`'s own terminal they are the relay's, and the program on the inner
    /// terminal decides whether the answer is shown. The group and the modes are read
    /// right before the one write that types the answer.
    /// It fails with [`ShellError::InvalidAnswer`] for text that is not one line of at
    /// most [`InputRespond::MAX_TEXT_BYTES`](efr_protocol::InputRespond::MAX_TEXT_BYTES)
    /// bytes without control characters, [`ShellError::NoShell`] or
    /// [`ShellError::NoCall`] when nothing runs, and [`ShellError::NotWaiting`] when the
    /// command does not wait for that input; then nothing is written. The text never
    /// reaches an error, a log or the recording (unless the terminal echoes it).
    pub async fn answer(
        &self,
        conversation: ConversationId,
        call: CallId,
        text: &SecretText,
        hidden: bool,
    ) -> Result<(), ShellError> {
        self.type_answer(conversation, call, text, hidden, false).await
    }

    /// Types `text` and a carriage return for the running command of `call` without a
    /// reported wait: the user chose to type an input, such as for a command that has
    /// printed nothing for a while. The answer is written only while that call's
    /// command runs, the run reports visible waits (a sentinel run, or one whose command
    /// starts a shell or a REPL, types into a shell that reads command lines, and takes
    /// none), and the job that the last look saw while the command ran, not the shell
    /// itself, holds the terminal; with `hidden` only while the terminal reads a line
    /// with echo off. The check against a reported wait and its kind does not apply.
    /// The errors are those of [`answer`](Self::answer).
    pub async fn answer_manual(
        &self,
        conversation: ConversationId,
        call: CallId,
        text: &SecretText,
        hidden: bool,
    ) -> Result<(), ShellError> {
        self.type_answer(conversation, call, text, hidden, true).await
    }

    async fn type_answer(
        &self,
        conversation: ConversationId,
        call: CallId,
        text: &SecretText,
        hidden: bool,
        manual: bool,
    ) -> Result<(), ShellError> {
        input::check_answer(text.expose_secret())?;
        let session = self.existing(conversation)?;
        let (reply, answered) = oneshot::channel();
        session.send(Msg::Answer { call, text: text.clone(), hidden, manual, reply }).await?;
        answered.await.map_err(|_| session.exited())?
    }

    /// Stops the command left running of `call` in the conversation's shell, as the user
    /// asks. Without `close`, it sends `SIGINT` to the job whose wait was reported, or
    /// that the last look saw, only while that job holds the terminal; with no such
    /// job known, to the job in the terminal's foreground, only when that is not the
    /// shell itself. With `close`, it closes the shell (`SIGHUP`, then `SIGKILL` after
    /// the close grace), for a command that ignores `SIGINT`; the next run starts a new
    /// shell. The command's end is then `stopped` or `closed`.
    ///
    /// It fails with [`ShellError::NoShell`] or [`ShellError::NoCall`] when no command
    /// of that call goes on, and with [`ShellError::NotWaiting`] when its job does not
    /// hold the terminal or the shell itself does; then nothing is signalled.
    pub async fn interrupt_left(
        &self,
        conversation: ConversationId,
        call: CallId,
        close: bool,
    ) -> Result<(), ShellError> {
        let session = self.existing(conversation)?;
        let deps = &self.inner.deps;
        let how = if close { ShellCommandEnd::Closed } else { ShellCommandEnd::Stopped };
        let (reply, target) = oneshot::channel();
        session.send(Msg::Stop { call, how, reply }).await?;
        let target = target.await.map_err(|_| session.exited())??;
        if close {
            return session.close(&*deps.holder, &*deps.clock, self.inner.config.close_grace).await;
        }
        let refused = |reason| ShellError::NotWaiting { conversation, reason };
        let terminal = |source| ShellError::Terminal { conversation, source };
        // NOTE: the group is read right before the signal, as for the stop of an
        // unanswered hidden wait: the job may have ended, and zsh in its precmd hooks,
        // or a command that a later hook started, holds the terminal then.
        let allowed = match target.group {
            Some(group) => match session.terminal.holds(group) {
                Ok(true) => Ok(()),
                Ok(false) => Err(refused("the command's job does not hold the terminal")),
                Err(source) => Err(terminal(source)),
            },
            None => match session.terminal.job() {
                Ok(Some(_)) => Ok(()),
                Ok(None) => Err(refused("the shell itself holds the terminal")),
                Err(source) => Err(terminal(source)),
            },
        };
        let signalled = match allowed {
            Ok(()) => deps
                .holder
                .signal(session.pty_id, Signal::Interrupt, SignalTarget::ForegroundGroup)
                .await
                .map_err(|source| ShellError::Holder { conversation, source }),
            Err(error) => Err(error),
        };
        if signalled.is_err() {
            // The command goes on, and its end is no stop.
            let _ = session.send(Msg::Stopping { id: target.id, how: None }).await;
        }
        signalled
    }

    /// Hands the running command of `call` to a background job, as the user asks with
    /// Ctrl+B: the command keeps its PTY, its process tree and its sandbox, and the job
    /// gets them as the [`AdoptedRun`] that goes on `to`. The call's run then answers its
    /// caller with [`Completion::Moved`] and the output so far, and the conversation's
    /// slot lets go of the shell: [`state`](Self::state), [`write`](Self::write),
    /// [`answer`](Self::answer), [`interrupt`](Self::interrupt), [`close`](Self::close)
    /// and [`close_all`](Self::close_all) no longer reach it, and the conversation's next
    /// run starts a new shell in the old one's directory. The new shell has none of the
    /// old one's variables, functions or aliases.
    ///
    /// It fails with [`ShellError::NotMovable`], and `to` is dropped, when the call's run
    /// has not typed its command or the command has not started, the run answered
    /// already (it ended, or it was left at its timeout), it is a sentinel run or its
    /// line starts a shell or a REPL, the command waits for input, or a full-screen
    /// program holds the screen. Nothing moves then, and the call goes on.
    pub async fn request_move(
        &self,
        conversation: ConversationId,
        call: CallId,
        to: oneshot::Sender<AdoptedRun>,
    ) -> Result<(), ShellError> {
        let refused = |reason| ShellError::NotMovable { conversation, reason };
        let requests = self
            .moves_lock()
            .get(&call)
            .filter(|slot| slot.conversation == conversation)
            .map(|slot| slot.requests.clone())
            .ok_or(refused("the call does not run"))?;
        let (reply, answer) = oneshot::channel();
        requests.send(MoveRequest { to, reply }).await.map_err(|_| refused(ANSWERED))?;
        answer.await.map_err(|_| refused(ANSWERED))?
    }

    fn moves_lock(&self) -> MutexGuard<'_, HashMap<CallId, MoveSlot>> {
        // Every update leaves the map whole, so a poisoned lock still guards a usable map.
        self.inner.moves.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lets run `run` of `call` take move requests until the returned end is dropped.
    fn register_move(&self, conversation: ConversationId, call: CallId, run: u64) -> Moves {
        let (requests, receiver) = mpsc::channel(1);
        self.moves_lock().insert(call, MoveSlot { conversation, run, requests });
        Moves { requests: receiver, inner: Arc::clone(&self.inner), call, run }
    }

    /// Writes raw input to the conversation's shell, as `pty.write` does for an
    /// attached client. While a call through the sandbox's launcher runs, Ctrl+Z is
    /// left out: it would stop bwrap and the command but not the launcher, which waits
    /// for them, and the shell would never get its terminal back (efr's auto spec,
    /// 3.13).
    pub async fn write(
        &self,
        conversation: ConversationId,
        bytes: Bytes,
    ) -> Result<(), ShellError> {
        let session = self.existing(conversation)?;
        let bytes = if session.activity.borrow().sandboxed && bytes.contains(&SUSPEND) {
            Bytes::from(bytes.iter().copied().filter(|byte| *byte != SUSPEND).collect::<Vec<u8>>())
        } else {
            bytes
        };
        if bytes.is_empty() {
            return Ok(());
        }
        session.write(bytes).await
    }

    /// Changes the terminal size of the conversation's shell and its screen.
    pub async fn resize(&self, conversation: ConversationId, size: Size) -> Result<(), ShellError> {
        let session = self.existing(conversation)?;
        self.inner
            .deps
            .holder
            .resize(session.pty_id, size)
            .await
            .map_err(|source| ShellError::Holder { conversation, source })?;
        // The programs see the new size from here on, whatever the screen does.
        session.set_size(size);
        session
            .screen
            .resize(size)
            .await
            .map_err(|source| ShellError::Screen { conversation, source })
    }

    /// Sends `SIGINT` to whatever runs in the foreground of the conversation's shell,
    /// as Ctrl+C at the terminal would.
    pub async fn interrupt(&self, conversation: ConversationId) -> Result<(), ShellError> {
        let session = self.existing(conversation)?;
        self.inner
            .deps
            .holder
            .signal(session.pty_id, Signal::Interrupt, SignalTarget::ForegroundGroup)
            .await
            .map_err(|source| ShellError::Holder { conversation, source })
    }

    /// Waits until the conversation's shell has no run: none active, none waiting to be
    /// typed and none left running past its timeout. A conversation without a shell is
    /// free. Fails with [`ShellError::NotReady`] when `timeout` passes first, and at once
    /// with [`ShellError::HeldBy`] while a command left running waits for input: only
    /// the user can free the shell then.
    ///
    /// The auto mode plans a sandboxed call under a lock of its projects, which it holds
    /// until the launcher starts; it waits here first, so a call queued behind a
    /// command that still runs (a dev server) holds no lock while it waits.
    pub async fn until_free(
        &self,
        conversation: ConversationId,
        timeout: Duration,
    ) -> Result<(), ShellError> {
        let Ok(session) = self.existing(conversation) else {
            return Ok(());
        };
        let mut activity = session.activity.clone();
        // A shell that ends while this waits is free: the next run starts a new one.
        let wait = async move {
            activity
                .wait_for(|activity| activity.free || activity.held_waiting)
                .await
                .map(|activity| activity.held_waiting && !activity.free)
                .unwrap_or(false)
        };
        let held = self.inner.deps.clock.timeout(timeout, wait).await;
        match held {
            Err(_) => Err(session.not_ready()),
            Ok(false) => Ok(()),
            Ok(true) => Err(session.held_by()),
        }
    }

    /// The command left running in the conversation's shell, if any: a command whose
    /// run reached its timeout and goes on there. The shell takes no other marked line
    /// until it ends. `None` when the conversation has no shell or the shell is free.
    pub fn busy(&self, conversation: ConversationId) -> Option<LeftRunning> {
        let session = self.existing(conversation).ok()?;
        session.left.borrow().clone()
    }

    /// The screen of the conversation's shell, for attach snapshots.
    pub fn screen(&self, conversation: ConversationId) -> Option<ScreenHandle> {
        self.existing(conversation).ok().map(|session| session.screen)
    }

    /// Ends the conversation's shell: `SIGHUP`, then `SIGKILL` when it has not ended
    /// within the config's grace period. A conversation without a shell is fine.
    pub async fn close(&self, conversation: ConversationId) -> Result<(), ShellError> {
        let Ok(session) = self.existing(conversation) else {
            return Ok(());
        };
        let deps = &self.inner.deps;
        session.close(&*deps.holder, &*deps.clock, self.inner.config.close_grace).await
    }

    /// Moves the conversation's shell to `dir`, as a `cd` that the user types, because
    /// the user went there between two prompts. The shell keeps its variables and
    /// functions. Nothing happens when the conversation has no shell (the next call
    /// starts one where it asks) or when the shell is already there.
    ///
    /// It fails with [`ShellError::Busy`] while a command runs, with
    /// [`ShellError::InvalidCommand`] for a path that is not UTF-8, and with
    /// [`ShellError::NotReady`] when the shell does not take the `cd` within
    /// [`MOVE_TIMEOUT`]. A `cd` that the shell refuses (the directory is gone or not
    /// readable) leaves the shell where it was. In each case the shell stays where it
    /// is, and the caller reads [`state`](Self::state) to learn where that is.
    pub async fn move_to(
        &self,
        conversation: ConversationId,
        dir: &Path,
    ) -> Result<(), ShellError> {
        let Ok(session) = self.existing(conversation) else {
            return Ok(());
        };
        let state = session.state().await?;
        // NOTE: a shell whose real directory is `dir` but whose last sandboxed call
        // ended in the sandbox's private tmp keeps that tmp: a `cd` to the directory it
        // is in changes nothing that the marks report.
        if state.cwd == dir {
            return Ok(());
        }
        if state.is_busy() {
            return Err(ShellError::Busy { conversation });
        }
        let path =
            dir.to_str().ok_or(ShellError::InvalidCommand { reason: "the path is not UTF-8" })?;
        let mut request =
            RunRequest::new(format!("builtin cd -q -- {}", sentinel::quote(path)), dir);
        request.timeout = MOVE_TIMEOUT;
        let session = self.current(session).await?;
        let result = self.run_on(&session, request, &mut NoProgress).await?;
        if result.exit_code != Some(0) {
            tracing::info!(%conversation, exit_code = ?result.exit_code, "the hidden shell did not move to the user's directory");
        }
        Ok(())
    }

    /// Ends every shell, for a daemon that shuts down.
    pub async fn close_all(&self) {
        let conversations: Vec<ConversationId> = self.lock().keys().copied().collect();
        for conversation in conversations {
            // A shell that cannot be signalled is released by its own actor anyway.
            let _ = self.close(conversation).await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<ConversationId, Slot>> {
        // Nothing panics while it holds the lock, and every update leaves the map
        // whole, so a poisoned lock still guards a usable map.
        self.inner.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn existing(&self, conversation: ConversationId) -> Result<SessionHandle, ShellError> {
        self.lock()
            .get(&conversation)
            .and_then(|slot| slot.get())
            .filter(|session| !session.is_ended())
            .cloned()
            .ok_or(ShellError::NoShell { conversation })
    }

    async fn session(
        &self,
        conversation: ConversationId,
        start_dir: &Path,
    ) -> Result<(SessionHandle, bool), ShellError> {
        loop {
            let slot = Arc::clone(self.lock().entry(conversation).or_default());
            let spawned = AtomicBool::new(false);
            let session = slot
                .get_or_try_init(|| async {
                    spawned.store(true, Ordering::Relaxed);
                    let dir = self.restart_dir(conversation).await;
                    self.spawn(conversation, dir.as_deref().unwrap_or(start_dir)).await
                })
                .await?;
            let spawned = spawned.load(Ordering::Relaxed);
            if !session.is_ended() {
                return Ok((session.clone(), spawned));
            }
            // The shell ended; a fresh slot gets a fresh shell.
            let exited = session.exited();
            {
                let mut sessions = self.lock();
                if sessions.get(&conversation).is_some_and(|current| Arc::ptr_eq(current, &slot)) {
                    sessions.remove(&conversation);
                }
            }
            // A shell that ended before this call could even use it (an `exit` in the
            // user's startup files, say) would end again; respawning it in a loop would
            // only burn processes.
            if spawned {
                return Err(exited);
            }
        }
    }

    /// The directory of the conversation's shell that a background job adopted, once,
    /// while it is still a directory: the next shell starts there, where the old one
    /// was.
    async fn restart_dir(&self, conversation: ConversationId) -> Option<PathBuf> {
        let dir = self
            .inner
            .restarts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&conversation)?;
        let is_dir = tokio::fs::metadata(&dir).await.is_ok_and(|meta| meta.is_dir());
        is_dir.then_some(dir)
    }

    async fn spawn(
        &self,
        conversation: ConversationId,
        start_dir: &Path,
    ) -> Result<SessionHandle, ShellError> {
        let inner = &self.inner;
        let start = self.start();
        // Every shell gets the files, a zsh or not: the editor variables of each one
        // name the stub among them.
        inner.installed.get_or_try_init(|| install(inner.config.integration_dir.clone())).await?;
        let mut config = inner.config.clone();
        config.login = start.login;
        config.trusted_programs = start.trusted_programs.to_vec();
        make_shell_dir(&config, conversation, start.integration).await;
        let pty_id = PtyId::from_uuid(efr_stdx::id::uuid_v7(&*inner.deps.clock, &*inner.deps.rng));
        // NOTE: the shell is a child subreaper, so a process that a command leaves
        // behind after a double fork or `setsid` stays in the shell's process tree,
        // where the daemon's peer check finds it (efr's auto spec, 2.2 and 13.5).
        let spec = SpawnSpec::new(pty_id, &start.program, start_dir, config.size)
            .args(integration::args(start.login))
            .vars(env::shell_env(&config, start_dir, start.integration))
            .vars(env::sandbox_env(&config, conversation, start.integration))
            .child_subreaper(true);
        let PtyHandle { master, child_pid, .. } = inner
            .deps
            .holder
            .spawn(spec)
            .await
            .map_err(|source| ShellError::Spawn { conversation, source })?;
        let trusted = start.integration.then(|| Arc::clone(&start.trusted_programs));
        match self.launch(conversation, pty_id, child_pid, master, start_dir, trusted) {
            Ok(session) => {
                tracing::info!(%conversation, %pty_id, pid = child_pid, "hidden shell started");
                inner.deps.observer.notice(ShellNotice::Started {
                    conversation,
                    pty_id,
                    pid: child_pid,
                    cwd: start_dir.to_path_buf(),
                });
                Ok(session)
            }
            Err(error) => {
                // Nobody reads this shell; it must not outlive the failed spawn.
                let _ = inner.deps.holder.signal(pty_id, Signal::Kill, SignalTarget::Child).await;
                let _ = inner.deps.holder.release(pty_id).await;
                Err(error)
            }
        }
    }

    /// Starts the tasks of a spawned shell: the writer, the reader, the screen's reply
    /// forwarder, the exit waiter and the actor. `trusted` is the set of trusted programs
    /// a shell with the integration started with, `None` for a shell without it.
    fn launch(
        &self,
        conversation: ConversationId,
        pty_id: PtyId,
        pid: u32,
        master: OwnedFd,
        cwd: &Path,
        trusted: Option<Arc<[String]>>,
    ) -> Result<SessionHandle, ShellError> {
        let inner = &self.inner;
        let integration = trusted.is_some();
        let deps = &inner.deps;
        let master =
            reader::master(master).map_err(|source| ShellError::Master { conversation, source })?;
        let (screen, events) = deps
            .screens
            .spawn(&screen_name(conversation, pty_id), inner.config.size)
            .map_err(|source| ShellError::Screen { conversation, source })?;
        let (writer, writes) = mpsc::channel(WRITE_CAPACITY);
        let (inbox, messages) = mpsc::channel(INBOX_CAPACITY);
        let (life, lives) = watch::channel(Life::Running);
        // A shell with the integration starts in `Phase::Starting`.
        let (activity, activities) =
            watch::channel(Activity { starting: integration, ..Activity::default() });
        let (left, lefts) = watch::channel(None);
        let targets = ReaderTargets {
            pty_id,
            recording: Arc::clone(&deps.recording),
            session: inbox.clone(),
            screen: screen.clone(),
        };
        let terminal = Terminal::new(Arc::clone(&master), Arc::clone(&deps.modes), pid);
        let tasks = vec![
            tokio::spawn(writer::write_loop(Arc::clone(&master), writes)),
            tokio::spawn(reader::read_loop(master, targets)),
            tokio::spawn(writer::forward_replies(events, writer.clone())),
        ];
        let holder = Arc::clone(&deps.holder);
        let waiter = inbox.clone();
        tokio::spawn(async move {
            let status = holder.wait(pty_id).await.ok();
            let _ = waiter.send(Msg::Exited(status)).await;
        });
        let state = ShellState::new(pty_id, pid, cwd.to_path_buf(), integration);
        let actor = SessionActor {
            core: SessionCore::new(conversation, state, Arc::clone(&deps.observer)),
            clock: Arc::clone(&deps.clock),
            holder: Arc::clone(&deps.holder),
            writer: writer.clone(),
            terminal: terminal.clone(),
            screen: screen.clone(),
            tasks,
            life,
            activity,
            left,
            startup: integration.then(|| deps.clock.sleep(inner.config.startup_timeout)),
        };
        tokio::spawn(actor.run(messages));
        Ok(SessionHandle {
            conversation,
            pty_id,
            pid,
            screen,
            inbox,
            writer,
            terminal,
            life: lives,
            activity: activities,
            left: lefts,
            size: Arc::new(Mutex::new(inner.config.size)),
            trusted,
        })
    }

    async fn run_on(
        &self,
        session: &SessionHandle,
        request: RunRequest,
        progress: &mut dyn RunProgress,
    ) -> Result<CommandResult, ShellError> {
        let deps = &self.inner.deps;
        if let Some(run) = &request.sandbox {
            let root = self.inner.config.sandbox_dir.as_deref();
            sandbox::check(root, session.conversation, run, &request.command)?;
            // The command never reaches the shell's line editor: the launcher reads it
            // from this file.
            sandbox::write_line(&run.dir, &request.command).await?;
        }
        let contained = request.sandbox.as_ref().is_some_and(SandboxRun::contained);
        // NOTE: the wait for a new shell's first prompt is not the command's time: a
        // big rc on a busy machine can take longer than a short timeout, and the model
        // would read that its command never ran. The startup timeout limits this wait.
        // A sentinel run does not wait for the prompt at all.
        if request.mode != RunMode::Sentinel {
            session.until_started().await;
        }
        let id = self.inner.next_run.fetch_add(1, Ordering::Relaxed);
        let mut moves = request.call.map(|call| self.register_move(session.conversation, call, id));
        let (reply, mut answer) = oneshot::channel();
        let (publish, mut updates) = watch::channel(Progress::default());
        let offer = Offer::of(request.mode, &request.command);
        let started = deps.clock.now();
        let limits = Limits { interactive: request.interactive_limit, input: request.input_limit };
        let kept_unfollowed =
            request.may_prompt && request.approved_by.is_some_and(ApprovedBy::by_person);
        let order = RunOrder {
            id,
            command: request.command,
            mode: request.mode,
            output_limit: request.output_limit,
            call: request.call,
            forget_credentials: request.forget_credentials,
            forget_before: request.forget_before,
            token: sentinel::token(&*deps.rng),
            sandbox: request.sandbox,
            since: started,
            approved_by: request.approved_by,
            reply,
            progress: publish,
        };
        session.send(Msg::Run(order)).await?;
        let mut guard = DetachOnDrop { inbox: Some(session.inbox.clone()), id };

        let mut deadline = deps.clock.sleep(request.timeout);
        let mut updates_open = true;
        let mut reported = 0;
        // The looks for input start once the command runs, one per quiet period.
        let mut look: Option<Sleep> = None;
        let mut watch = InputWatch::default();
        let mut live = LiveTail::new(self.inner.config.tail_interval);
        // Set while a change that needs a screen is held for the interval.
        let mut held_tail: Option<Sleep> = None;
        let leave = loop {
            tokio::select! {
                biased;
                ended = &mut answer => {
                    guard.disarm();
                    let result = self.finished(session, ended).await;
                    end_watch(&mut watch, progress);
                    return result;
                }
                request = next_move(&mut moves) => {
                    if let Some(moved) = self.adopt(session, id, look.is_some(), &watch, request).await {
                        break Leave::Moved(Box::new(moved));
                    }
                }
                changed = updates.changed(), if updates_open => match changed {
                    Ok(()) => {
                        let latest = updates.borrow_and_update().clone();
                        if latest.started && look.is_none() {
                            look = Some(deps.clock.sleep(self.inner.config.quiet_period));
                        }
                        // The start alone is not output.
                        if latest.bytes > reported {
                            reported = latest.bytes;
                            match live.offer(latest, deps.clock.now()) {
                                Step::Clean(latest) => progress.update(&latest.update()),
                                Step::Screen(window) => {
                                    let tail = self.tail_reader(session).tail(&window).await;
                                    progress.update(&OutputUpdate::new(reported, tail));
                                }
                                Step::Hold(left) => {
                                    if held_tail.is_none() {
                                        held_tail = Some(deps.clock.sleep(left));
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => updates_open = false,
                },
                () = until(&mut held_tail) => {
                    held_tail = None;
                    // A change that needed no screen may have replaced the held one.
                    if let Some(window) = live.due(deps.clock.now()) {
                        let tail = self.tail_reader(session).tail(&window).await;
                        progress.update(&OutputUpdate::new(reported, tail));
                    }
                }
                () = &mut deadline => {
                    // NOTE: the looks start with the command, so a run still waiting for
                    // the prompt is never kept past its timeout.
                    let waits = watch.current() != InputWait::None;
                    match self.extension(limits, waits, started, look.is_some(), progress) {
                        Some(wait) => deadline = deps.clock.sleep(wait),
                        None => break Leave::Timeout,
                    }
                }
                () = until(&mut look) => {
                    let looked = self
                        .look_for_input(session, id, offer, contained, &mut watch, progress)
                        .await;
                    let stop = match looked {
                        Ok(stop) => stop,
                        // The actor went away while the run's reply was still open; a wait
                        // reported before must still end with `None`.
                        Err(error) => {
                            end_watch(&mut watch, progress);
                            return Err(error);
                        }
                    };
                    if stop {
                        let waiting = watch.waiting().map(|waiting| waiting.group);
                        let result = self
                            .stop_unanswered(session, id, waiting, &mut guard, &mut answer)
                            .await;
                        end_watch(&mut watch, progress);
                        return result;
                    }
                    look = Some(deps.clock.sleep(self.inner.config.quiet_period));
                }
            }
        };
        // A request from now on finds no run to move.
        drop(moves);
        if let Leave::Moved(moved) = leave {
            guard.disarm();
            end_watch(&mut watch, progress);
            return Ok(self.hand_over(session, id, offer, *moved, answer).await);
        }

        let detached = self.detach(session, id, true, &mut guard).await;
        // NOTE: the wait that the run reported last goes on with its command, which a
        // watch of its own follows from here.
        let reported = watch;
        end_watch(&mut watch, progress);
        if let Ok(Detached::Running { .. }) = &detached {
            let left = LeftRun {
                id,
                offer,
                contained,
                kept_unfollowed,
                started,
                limit: limits.input.or(limits.interactive),
                watch: reported,
            };
            self.watch_left(session, left);
        }
        match detached? {
            Detached::Gone => self.finished(session, answer.await).await,
            Detached::Unstarted => Err(session.not_ready()),
            Detached::Running { kept, range, last_output, cwd, delimiter } => {
                // This task does not read the screen's events, so it may wait for a
                // snapshot.
                let capture = session.screen.snapshot(0).await.map_err(|source| {
                    ShellError::Screen { conversation: session.conversation, source }
                })?;
                let now = deps.clock.now();
                let quiet = last_output.is_none_or(|at| {
                    Duration::try_from(now.duration_since(at))
                        .is_ok_and(|silence| silence >= self.inner.config.quiet_period)
                });
                let completion = timed_out(&capture.snapshot, quiet);
                let captured = self.replayer(session).render(&kept).await;
                Ok(CommandResult {
                    completion,
                    exit_code: None,
                    output: captured.text,
                    truncated: captured.truncated,
                    output_bytes: captured.bytes,
                    output_range: range,
                    cwd_after: cwd,
                    screen_tail: Some(screen_tail(&capture.snapshot)),
                    delimiter,
                    sandbox: None,
                })
            }
        }
    }

    /// Asks the session's actor to hand run `id` to the background job of `request`, when
    /// its command `started`, the run's last look saw no wait in `watch`, and no
    /// full-screen program holds the screen. A refusal is the request's answer, and the
    /// run goes on.
    async fn adopt(
        &self,
        session: &SessionHandle,
        id: u64,
        started: bool,
        watch: &InputWatch,
        request: MoveRequest,
    ) -> Option<Moved> {
        let conversation = session.conversation;
        let refuse = |request: MoveRequest, reason| {
            let _ = request.reply.send(Err(ShellError::NotMovable { conversation, reason }));
            None
        };
        if !started {
            return refuse(request, NOT_STARTED);
        }
        if watch.current() != InputWait::None {
            return refuse(request, "the command waits for input; answer it first");
        }
        // NOTE: a job reads its command's output as lines; a full-screen program has
        // none to read, and nobody could reach its keys there.
        match session.screen.snapshot(0).await {
            Ok(capture) if capture.snapshot.alternate_screen => {
                return refuse(request, "a full-screen program cannot move");
            }
            Ok(_) => {}
            Err(_) => return refuse(request, "the screen of the hidden shell cannot be read"),
        }
        let (tee, output) = adopt::tee();
        let (reply, adopted) = oneshot::channel();
        if session.send(Msg::Adopt { id, tee, reply }).await.is_err() {
            return refuse(request, ENDED);
        }
        match adopted.await {
            Ok(Ok(adoption)) => Some(Moved { request, adoption, output }),
            Ok(Err(reason)) => refuse(request, reason),
            Err(_) => refuse(request, ENDED),
        }
    }

    /// Hands the adopted run `id` to its job: the conversation's slot lets go of the
    /// shell, a task looks for input of the command, the [`AdoptedRun`] goes to the
    /// job, and the call's result holds the output up to the move. `answer` is the
    /// run's reply channel, whose end the job awaits now.
    async fn hand_over(
        &self,
        session: &SessionHandle,
        id: u64,
        offer: Offer,
        moved: Moved,
        answer: oneshot::Receiver<Result<RunEnd, ShellError>>,
    ) -> CommandResult {
        let Moved { request, adoption, output } = moved;
        let conversation = session.conversation;
        let deps = &self.inner.deps;
        let config = &self.inner.config;
        deps.observer.notice(ShellNotice::Adopted { conversation, pty_id: session.pty_id });
        self.let_go(session, &adoption.cwd);
        let captured = self.replayer(session).render(&adoption.kept).await;
        let (looks, looked) = watch::channel((InputWait::None, false));
        let watch = AdoptedWatch {
            session: session.clone(),
            id,
            offer,
            clock: Arc::clone(&deps.clock),
            holder: Arc::clone(&deps.holder),
            quiet: self.quiet(),
            quiet_period: config.quiet_period,
            looks,
        };
        tokio::spawn(watch.run());
        let adopted = AdoptedRun::new(Parts {
            session: session.clone(),
            call_dir: adoption.call_dir,
            head: captured.text.clone(),
            output,
            end: answer,
            looks: looked,
            holder: Arc::clone(&deps.holder),
            clock: Arc::clone(&deps.clock),
            close_grace: config.close_grace,
        });
        tracing::info!(%conversation, pty_id = %session.pty_id, "a background job adopted a running call and its hidden shell");
        match request.to.send(adopted) {
            Ok(()) => {
                let _ = request.reply.send(Ok(()));
            }
            Err(adopted) => {
                // NOTE: nobody owns the command now, and nothing could ever stop it or
                // close its shell, so it ends here.
                tracing::warn!(%conversation, "nobody took the call that moved to the background; closing its shell");
                if let Err(error) = adopted.close().await {
                    tracing::warn!(%conversation, %error, "could not close an adopted shell");
                }
                let reason = "nobody took the command; efr closed its shell";
                let _ = request.reply.send(Err(ShellError::NotMovable { conversation, reason }));
            }
        }
        CommandResult {
            completion: Completion::Moved,
            exit_code: None,
            output: captured.text,
            truncated: captured.truncated,
            output_bytes: captured.bytes,
            output_range: adoption.range,
            cwd_after: adoption.cwd,
            screen_tail: None,
            delimiter: Delimiter::Marks,
            sandbox: None,
        }
    }

    /// Takes `session` out of its conversation's slot, so nothing of the conversation
    /// reaches it any more, and makes the conversation's next shell start in `cwd`.
    fn let_go(&self, session: &SessionHandle, cwd: &Path) {
        let conversation = session.conversation;
        let mut sessions = self.lock();
        let ours = sessions
            .get(&conversation)
            .and_then(|slot| slot.get())
            .is_some_and(|current| current.pty_id == session.pty_id);
        if ours {
            sessions.remove(&conversation);
            drop(sessions);
            self.inner
                .restarts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(conversation, cwd.to_path_buf());
        }
    }

    /// Lets the session's actor go of run `id`, which goes on without a caller; `left`
    /// when its command is left running at its timeout.
    async fn detach(
        &self,
        session: &SessionHandle,
        id: u64,
        left: bool,
        guard: &mut DetachOnDrop,
    ) -> Result<Detached, ShellError> {
        let (reply, detached) = oneshot::channel();
        session.send(Msg::Detach { id, left, reply }).await?;
        guard.disarm();
        detached.await.map_err(|_| session.exited())
    }

    /// One look at whether run `id` waits for input; each change goes to `progress`.
    /// Returns true when the command waits for hidden input that nobody can answer,
    /// which is always so for a `contained` sandboxed command, also for a visible wait
    /// that looks secret: efr never types a secret into one (efr's auto spec, 3.12).
    async fn look_for_input(
        &self,
        session: &SessionHandle,
        id: u64,
        offer: Offer,
        contained: bool,
        watch: &mut InputWatch,
        progress: &mut dyn RunProgress,
    ) -> Result<bool, ShellError> {
        let clock = &*self.inner.deps.clock;
        match session.look(id, offer, self.quiet(), clock, watch).await? {
            Looked::Gone => return Ok(false),
            Looked::Same => {}
            Looked::Changed { wait, told } => {
                progress.input_changed(wait, watch.looks_secret());
                told?;
            }
        }
        // NOTE: a visible wait that looks secret is a hidden one for a command in the
        // sandbox: sandboxed code can put the terminal in raw mode and print a password
        // prompt, and no answer that may be a secret reaches it (efr's auto spec, 3.12).
        let secret = watch.current() == InputWait::Visible && watch.looks_secret();
        Ok((watch.current() == InputWait::Hidden || (contained && secret))
            && (contained || !progress.can_answer_hidden()))
    }

    /// The quiet times of a look, from the config.
    fn quiet(&self) -> Quiet {
        let config = &self.inner.config;
        Quiet {
            hidden: config.quiet_period,
            visible: config.visible_input_quiet,
            question: config.question_input_quiet,
        }
    }

    /// Watches the command of run `id`, left running at its timeout, until it ends:
    /// its waits, the stop of a wait that may not stay, and the limit of a wait.
    fn watch_left(&self, session: &SessionHandle, left: LeftRun) {
        let deps = &self.inner.deps;
        let config = &self.inner.config;
        let watch = OrphanWatch {
            session: session.clone(),
            run: left,
            clock: Arc::clone(&deps.clock),
            holder: Arc::clone(&deps.holder),
            followers: Arc::clone(&deps.followers),
            quiet: self.quiet(),
            quiet_period: config.quiet_period,
            close_grace: config.close_grace,
        };
        tokio::spawn(watch.run());
    }

    /// How much longer a run that reached its deadline waits for its command, when its
    /// command runs, it has an interactive limit or an input limit and its command
    /// `waits` for input, that limit since `started` has not passed, and a person who
    /// can answer follows it: at most one quiet period, so a person who stops following,
    /// or a wait that ends, ends the run soon. `None` ends the run as at its timeout.
    ///
    /// NOTE: a command in the sandbox never gets here with a wait for an answer that may
    /// be a secret: the look that saw it stopped the command.
    fn extension(
        &self,
        limits: Limits,
        waits: bool,
        started: jiff::Timestamp,
        running: bool,
        progress: &mut dyn RunProgress,
    ) -> Option<Duration> {
        let input = limits.input.filter(|_| waits);
        let limit = limits.interactive.max(input).filter(|_| running)?;
        // A clock that went back counts as no time passed.
        let elapsed = Duration::try_from(self.inner.deps.clock.now().duration_since(started))
            .unwrap_or_default();
        let left = limit.checked_sub(elapsed).filter(|left| !left.is_zero())?;
        progress.can_answer().then(|| left.min(self.inner.config.quiet_period))
    }

    /// Interrupts run `id`, whose command waits for hidden input that nobody can
    /// answer, and returns its output so far as [`Completion::Unanswered`]. `waiting` is
    /// the process group of the job whose wait the run reported; only it is signalled.
    async fn stop_unanswered(
        &self,
        session: &SessionHandle,
        id: u64,
        waiting: Option<u32>,
        guard: &mut DetachOnDrop,
        answer: &mut oneshot::Receiver<Result<RunEnd, ShellError>>,
    ) -> Result<CommandResult, ShellError> {
        let conversation = session.conversation;
        // Detached first, so the end that SIGINT brings belongs to the orphan, and this
        // result says why the command stopped.
        let detached = self.detach(session, id, false, guard).await?;
        // NOTE: the command may have ended since the look. zsh then holds the terminal
        // again and runs its precmd hooks, and a SIGINT to the foreground group would
        // reach zsh and could cut short the hook that prints `D`, which the orphan waits
        // for; a hook after the integration's may also have started a command of its
        // own, which never waited. So the group is read right before the signal, and it
        // goes out only while the job whose wait was reported holds the terminal. What
        // is left is the moment between this read and the holder's own read of the
        // group as it signals.
        let still_waiting = match waiting {
            Some(group) => session.terminal.holds(group),
            None => Ok(false),
        };
        match still_waiting {
            Ok(true) => {
                tracing::info!(%conversation, "a command waits for hidden input that no client can answer; interrupting it");
                if let Err(error) = self.interrupt(conversation).await {
                    tracing::warn!(%conversation, error = %error, "could not interrupt a command that waits for hidden input");
                }
            }
            Ok(false) => {
                tracing::debug!(%conversation, "the job that waited for hidden input no longer holds the terminal; nothing is interrupted");
            }
            Err(error) => {
                tracing::warn!(%conversation, error = %error, "could not read who holds the terminal; a command that waits for hidden input is not interrupted");
            }
        }
        match detached {
            Detached::Running { kept, range, cwd, delimiter, .. } => {
                let captured = self.replayer(session).render(&kept).await;
                Ok(CommandResult {
                    completion: Completion::Unanswered,
                    exit_code: None,
                    output: captured.text,
                    truncated: captured.truncated,
                    output_bytes: captured.bytes,
                    output_range: range,
                    cwd_after: cwd,
                    screen_tail: None,
                    delimiter,
                    sandbox: None,
                })
            }
            // The command ended between the look and the detach; it needs no stop.
            Detached::Gone => self.finished(session, answer.await).await,
            Detached::Unstarted => Err(session.not_ready()),
        }
    }

    /// The result of a run that ended.
    async fn finished(
        &self,
        session: &SessionHandle,
        ended: Result<Result<RunEnd, ShellError>, oneshot::error::RecvError>,
    ) -> Result<CommandResult, ShellError> {
        let RunEnd { output, cwd, delimiter } = ended.map_err(|_| session.exited())??;
        let captured = match output.completion {
            // A line that ran nothing has no program output to replay: its text is the
            // shell's complaint around the echo of the line, which the line editor
            // drew relative to a prompt that a capture screen does not have.
            Completion::NotStarted => output.kept.clean(),
            _ => self.replayer(session).render(&output.kept).await,
        };
        Ok(CommandResult {
            completion: output.completion,
            exit_code: output.exit_code,
            output: captured.text,
            truncated: captured.truncated,
            output_bytes: captured.bytes,
            output_range: output.range,
            cwd_after: cwd,
            screen_tail: None,
            delimiter,
            sandbox: output.sandbox.map(|result| *result),
        })
    }

    /// Reads a run's output on capture screens from the session's own factory, at the
    /// shell's current size. This runs in the caller's task, so a long replay never
    /// holds up the session's actor.
    fn replayer(&self, session: &SessionHandle) -> Replayer<'_> {
        Replayer::new(&*self.inner.deps.screens, replay_name(session.conversation), session.size())
    }

    /// Reads a running command's live tail on a screen of the shell's current size, from
    /// the session's own factory, in the caller's task as [`replayer`](Self::replayer)
    /// does. Its screen lives for one read.
    fn tail_reader(&self, session: &SessionHandle) -> Replayer<'_> {
        Replayer::new(&*self.inner.deps.screens, tail_name(session.conversation), session.size())
    }
}

impl fmt::Debug for ShellSessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let start = self.start();
        f.debug_struct("ShellSessions")
            .field("program", &start.program)
            .field("integration", &start.integration)
            .field("login", &start.login)
            .field("trusted_programs", &start.trusted_programs)
            .field("shells", &self.lock().len())
            .finish_non_exhaustive()
    }
}

/// `program`, or `zsh` on the `PATH` of the config's environment when it is `None`.
fn find_program(config: &ShellConfig, program: Option<&Path>) -> Result<PathBuf, ShellError> {
    match program {
        Some(program) => Ok(program.to_path_buf()),
        None => {
            let path = config.base_env.get("PATH").cloned().unwrap_or_default();
            which::which_in(DEFAULT_PROGRAM, Some(path), "/").map_err(|source| {
                ShellError::ProgramNotFound { program: DEFAULT_PROGRAM.to_owned(), source }
            })
        }
    }
}

/// Runs command lines in a conversation's hidden shell. [`ShellSessions`] is the real
/// one; `efr-tools` drives its `ShellTool` through this trait, so its tests use a fake.
#[async_trait]
pub trait CommandRunner: Send + Sync + fmt::Debug {
    /// See [`ShellSessions::run_command`].
    async fn run_command(
        &self,
        conversation: ConversationId,
        request: RunRequest,
        progress: &mut dyn RunProgress,
    ) -> Result<CommandResult, ShellError>;
}

#[async_trait]
impl CommandRunner for ShellSessions {
    async fn run_command(
        &self,
        conversation: ConversationId,
        request: RunRequest,
        progress: &mut dyn RunProgress,
    ) -> Result<CommandResult, ShellError> {
        ShellSessions::run_command(self, conversation, request, progress).await
    }
}

/// The limits past a run's timeout ([`RunRequest::interactive_limit`] and
/// [`RunRequest::input_limit`]).
#[derive(Debug, Clone, Copy)]
struct Limits {
    interactive: Option<Duration>,
    input: Option<Duration>,
}

/// Lets the session's actor go of a run whose caller dropped the future before an
/// answer came, as a turn does when the user interrupts it: a run still waiting for
/// the prompt is dropped instead of typed later, and a typed one goes on without a
/// caller, so the next run waits for the prompt instead of being refused as busy.
struct DetachOnDrop {
    inbox: Option<mpsc::Sender<Msg>>,
    id: u64,
}

impl DetachOnDrop {
    /// The run was answered or detached on purpose; nothing is left to do.
    fn disarm(&mut self) {
        self.inbox = None;
    }
}

impl Drop for DetachOnDrop {
    fn drop(&mut self) {
        if let Some(inbox) = self.inbox.take() {
            let (reply, _) = oneshot::channel();
            // NOTE: a full inbox loses the detach; the actor then drops a waiting run
            // anyway, because its reply channel is closed.
            let _ = inbox.try_send(Msg::Detach { id: self.id, left: false, reply });
        }
    }
}

/// Tells `progress` that a run that waited for input no longer does, because it ended
/// or was left.
fn end_watch(watch: &mut InputWatch, progress: &mut dyn RunProgress) {
    if let Some(changed) = watch.end() {
        progress.input_changed(changed, false);
    }
}

/// Writes the integration files on the blocking pool.
async fn install(dir: PathBuf) -> Result<(), ShellError> {
    let target = dir.clone();
    match tokio::task::spawn_blocking(move || integration::install(&target)).await {
        Ok(result) => result.map_err(|source| ShellError::Integration { dir, source }),
        Err(join) => Err(ShellError::Integration {
            source: StdxError::WriteFile { path: dir.clone(), source: io::Error::other(join) },
            dir,
        }),
    }
}

/// Makes the conversation's sandbox dir (`$R/sbx/<conversation>`, 0700) before its zsh
/// starts, for a zsh with the integration and a sandbox in the config. The integration
/// writes the shell's `env` file and `snapshot.zsh` at a prompt only into a dir that
/// exists, and efrd gives a background job both (efr's auto spec, section 3.18). efrd
/// makes the same dirs with the same mode before each sandboxed call. A failure is
/// only logged: the shell starts, and efrd then starts no job until the dir exists.
async fn make_shell_dir(config: &ShellConfig, conversation: ConversationId, integration: bool) {
    let (Some(root), Some(_)) = (&config.sandbox_dir, &config.sandbox_launcher) else { return };
    if !integration {
        return;
    }
    let dir = sandbox::conversation_dir(root, conversation);
    let made = dir.clone();
    let done = tokio::task::spawn_blocking(move || {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&made)
    })
    .await;
    match done {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%conversation, dir = %dir.display(), %error, "the hidden shell's sandbox dir could not be made");
        }
        Err(error) => {
            tracing::warn!(%conversation, %error, "the hidden shell's sandbox dir could not be made");
        }
    }
}

/// `screen-`, the last four hex digits of the conversation id and the last four of the
/// PTY id: the random ends of two UUIDv7s, so two conversations started in the same
/// minute get different names, and so do two shells of one conversation (one that a
/// background job adopted, and the new one), in the 15 bytes Linux shows of a thread
/// name.
fn screen_name(conversation: ConversationId, pty_id: PtyId) -> String {
    let pty = pty_id.to_string();
    let pty = pty.get(pty.len().saturating_sub(4)..).unwrap_or(&pty);
    let conversation = short_id(conversation);
    let conversation =
        conversation.get(conversation.len().saturating_sub(4)..).unwrap_or(&conversation);
    format!("screen-{conversation}{pty}")
}

/// `replay-` and the same eight digits: the capture screen that reads a command's
/// output, named apart from the shell's own screen.
fn replay_name(conversation: ConversationId) -> String {
    format!("replay-{}", short_id(conversation))
}

/// `tail-` and the same eight digits: the screen that reads a running command's live
/// tail.
fn tail_name(conversation: ConversationId) -> String {
    format!("tail-{}", short_id(conversation))
}

fn short_id(conversation: ConversationId) -> String {
    let id = conversation.to_string();
    id.get(id.len().saturating_sub(8)..).unwrap_or(&id).to_owned()
}

#[cfg(test)]
mod tests;
