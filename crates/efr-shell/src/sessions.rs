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
use efr_holder::{HolderError, PtyHandle, Signal, SignalTarget, Size, SpawnSpec};
use efr_protocol::{CallId, ConversationId, InputWait, PtyId, SecretText};
use efr_screen::ScreenHandle;
use efr_stdx::StdxError;
use efr_stdx::time::{Clock as _, Sleep};
use tokio::sync::{OnceCell, mpsc, oneshot, watch};

use crate::input::{self, InputWatch, Look, Offer, Quiet};
use crate::modes::Terminal;
use crate::reader::{self, ReaderTargets};
use crate::replay::Replayer;
use crate::run::{Progress, screen_tail, timed_out};
use crate::session::{
    Detached, INBOX_CAPACITY, Life, Msg, RunEnd, RunOrder, SessionActor, SessionCore,
    SessionHandle, until,
};
use crate::writer::{self, WRITE_CAPACITY};
use crate::{
    CommandResult, Completion, RunProgress, RunRequest, ShellConfig, ShellDeps, ShellError,
    ShellNotice, ShellState, env, integration, sentinel,
};

/// The program looked for on the `PATH` when the config names none.
const DEFAULT_PROGRAM: &str = "zsh";

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
}

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
    /// [`ShellError::NotReady`] when the prompt does not come before the timeout, and
    /// with [`ShellError::Busy`] when another run is under way or an unfinished line
    /// waits at a continuation prompt.
    ///
    /// At the timeout the command keeps running; the result is
    /// [`Completion::FullScreen`] for a program on the alternate screen,
    /// [`Completion::Interactive`] when it waits for input at the terminal and
    /// [`Completion::StillRunning`] otherwise, with the screen's last lines.
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
    /// call that [`RunRequest::call`](crate::RunRequest::call) named.
    ///
    /// The answer is written only while that call's command runs (between its `C` and
    /// `D`, or its sentinels), the run has reported a wait, the job that was in the
    /// terminal's foreground when it did still is, by its process group, and the
    /// terminal reads a line (canonical input on); with `hidden`, the terminal must also
    /// have echo off, so the text never reaches the output. The group and the modes are
    /// read right before the one write that types the answer.
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
        input::check_answer(text.expose_secret())?;
        let session = self.existing(conversation)?;
        let (reply, answered) = oneshot::channel();
        session.send(Msg::Answer { call, text: text.clone(), hidden, reply }).await?;
        answered.await.map_err(|_| session.exited())?
    }

    /// Writes raw input to the conversation's shell, as `pty.write` does for an
    /// attached client.
    pub async fn write(
        &self,
        conversation: ConversationId,
        bytes: Bytes,
    ) -> Result<(), ShellError> {
        self.existing(conversation)?.write(bytes).await
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
        let holder = &self.inner.deps.holder;
        match holder.signal(session.pty_id, Signal::Hangup, SignalTarget::Child).await {
            Ok(()) | Err(HolderError::Exited { .. } | HolderError::NotFound { .. }) => {}
            Err(source) => return Err(ShellError::Holder { conversation, source }),
        }
        let grace = self.inner.config.close_grace;
        if self.inner.deps.clock.timeout(grace, session.ended()).await.is_err() {
            // Too late to matter whether the kill or the shell's own end came first.
            let _ = holder.signal(session.pty_id, Signal::Kill, SignalTarget::Child).await;
            session.ended().await;
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
                    self.spawn(conversation, start_dir).await
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

    async fn spawn(
        &self,
        conversation: ConversationId,
        start_dir: &Path,
    ) -> Result<SessionHandle, ShellError> {
        let inner = &self.inner;
        let start = self.start();
        if start.integration {
            inner
                .installed
                .get_or_try_init(|| install(inner.config.integration_dir.clone()))
                .await?;
        }
        let mut config = inner.config.clone();
        config.login = start.login;
        config.trusted_programs = start.trusted_programs.to_vec();
        let pty_id = PtyId::from_uuid(efr_stdx::id::uuid_v7(&*inner.deps.clock, &*inner.deps.rng));
        let spec = SpawnSpec::new(pty_id, &start.program, start_dir, config.size)
            .args(integration::args(start.login))
            .vars(env::shell_env(&config, start_dir, start.integration));
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
            .spawn(&screen_name(conversation), inner.config.size)
            .map_err(|source| ShellError::Screen { conversation, source })?;
        let (writer, writes) = mpsc::channel(WRITE_CAPACITY);
        let (inbox, messages) = mpsc::channel(INBOX_CAPACITY);
        let (life, lives) = watch::channel(Life::Running);
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
        let id = self.inner.next_run.fetch_add(1, Ordering::Relaxed);
        let (reply, mut answer) = oneshot::channel();
        let (publish, mut updates) = watch::channel(Progress::default());
        let offer = Offer::of(request.mode, &request.command);
        let order = RunOrder {
            id,
            command: request.command,
            mode: request.mode,
            output_limit: request.output_limit,
            call: request.call,
            forget_credentials: request.forget_credentials,
            token: sentinel::token(&*deps.rng),
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
        loop {
            tokio::select! {
                biased;
                ended = &mut answer => {
                    guard.disarm();
                    let result = self.finished(session, ended).await;
                    end_watch(&mut watch, progress);
                    return result;
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
                            progress.update(&latest.update());
                        }
                    }
                    Err(_) => updates_open = false,
                },
                () = &mut deadline => break,
                () = until(&mut look) => {
                    let looked = self.look_for_input(session, id, offer, &mut watch, progress).await;
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
        }

        let detached = self.detach(session, id, &mut guard).await;
        end_watch(&mut watch, progress);
        match detached? {
            Detached::Gone => self.finished(session, answer.await).await,
            Detached::Unstarted => Err(ShellError::NotReady { conversation: session.conversation }),
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
                })
            }
        }
    }

    /// Lets the session's actor go of run `id`, which goes on without a caller.
    async fn detach(
        &self,
        session: &SessionHandle,
        id: u64,
        guard: &mut DetachOnDrop,
    ) -> Result<Detached, ShellError> {
        let (reply, detached) = oneshot::channel();
        session.send(Msg::Detach { id, reply }).await?;
        guard.disarm();
        detached.await.map_err(|_| session.exited())
    }

    /// One look at whether run `id` waits for input; each change goes to `progress`.
    /// Returns true when the command waits for hidden input that nobody can answer.
    async fn look_for_input(
        &self,
        session: &SessionHandle,
        id: u64,
        offer: Offer,
        watch: &mut InputWatch,
        progress: &mut dyn RunProgress,
    ) -> Result<bool, ShellError> {
        let (reply, probed) = oneshot::channel();
        session.send(Msg::Probe { id, reply }).await?;
        // A run that ended meanwhile is answered on its reply channel.
        let Ok(Some(probe)) = probed.await else {
            return Ok(false);
        };
        let config = &self.inner.config;
        let quiet = Quiet { hidden: config.quiet_period, visible: config.visible_input_quiet };
        let wait = match input::look(&probe, self.inner.deps.clock.now(), quiet, offer) {
            Look::Settled(wait) => wait,
            // A screen that failed only loses the guess; the command goes on.
            Look::ReadScreen => match session.screen.snapshot(0).await {
                Ok(capture) if input::visible_prompt(&capture.snapshot) => InputWait::Visible,
                _ => InputWait::None,
            },
        };
        if let Some(changed) = watch.settle(wait, probe.job.map(|job| job.group), probe.answers) {
            // The actor learns which job waits before any client hears of the wait, so an
            // answer sent for it is checked against that job. The client hears of the
            // change even when the actor is gone, so the `None` that the caller reports
            // then follows a wait that the client heard.
            let told = session.send(Msg::Waiting { id, waiting: watch.waiting() }).await;
            progress.input_changed(changed);
            told?;
        }
        Ok(watch.current() == InputWait::Hidden && !progress.can_answer_hidden())
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
        let detached = self.detach(session, id, guard).await?;
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
                })
            }
            // The command ended between the look and the detach; it needs no stop.
            Detached::Gone => self.finished(session, answer.await).await,
            Detached::Unstarted => Err(ShellError::NotReady { conversation }),
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
        })
    }

    /// Reads a run's output on capture screens from the session's own factory, at the
    /// shell's current size. This runs in the caller's task, so a long replay never
    /// holds up the session's actor.
    fn replayer(&self, session: &SessionHandle) -> Replayer<'_> {
        Replayer::new(&*self.inner.deps.screens, replay_name(session.conversation), session.size())
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
            let _ = inbox.try_send(Msg::Detach { id: self.id, reply });
        }
    }
}

/// Tells `progress` that a run that waited for input no longer does, because it ended
/// or was left.
fn end_watch(watch: &mut InputWatch, progress: &mut dyn RunProgress) {
    if let Some(changed) = watch.end() {
        progress.input_changed(changed);
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

/// `screen-` and the last eight hex digits of the conversation id: the random end of
/// a UUIDv7, so two conversations started in the same minute get different names, in
/// the 15 bytes Linux shows of a thread name.
fn screen_name(conversation: ConversationId) -> String {
    format!("screen-{}", short_id(conversation))
}

/// `replay-` and the same eight digits: the capture screen that reads a command's
/// output, named apart from the shell's own screen.
fn replay_name(conversation: ConversationId) -> String {
    format!("replay-{}", short_id(conversation))
}

fn short_id(conversation: ConversationId) -> String {
    let id = conversation.to_string();
    id.get(id.len().saturating_sub(8)..).unwrap_or(&id).to_owned()
}

#[cfg(test)]
mod tests;
