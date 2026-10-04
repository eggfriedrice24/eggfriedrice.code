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
use efr_protocol::{ConversationId, PtyId};
use efr_screen::ScreenHandle;
use efr_stdx::StdxError;
use efr_stdx::time::Clock as _;
use tokio::sync::{OnceCell, mpsc, oneshot, watch};

use crate::reader::{self, ReaderTargets};
use crate::run::{Progress, screen_tail, waits_for_input};
use crate::session::{
    Detached, INBOX_CAPACITY, Life, Msg, RunEnd, RunOrder, SessionActor, SessionCore, SessionHandle,
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
    config: ShellConfig,
    deps: ShellDeps,
    program: PathBuf,
    /// True when the program is a zsh, which gets the integration.
    integration: bool,
    installed: OnceCell<()>,
    sessions: Mutex<HashMap<ConversationId, Slot>>,
    next_run: AtomicU64,
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
        let program = match &config.program {
            Some(program) => program.clone(),
            None => {
                let path = config.base_env.get("PATH").cloned().unwrap_or_default();
                which::which_in(DEFAULT_PROGRAM, Some(path), "/").map_err(|source| {
                    ShellError::ProgramNotFound { program: DEFAULT_PROGRAM.to_owned(), source }
                })?
            }
        };
        let integration = integration::supports(&program);
        Ok(ShellSessions {
            inner: Arc::new(Inner {
                config,
                deps,
                program,
                integration,
                installed: OnceCell::new(),
                sessions: Mutex::new(HashMap::new()),
                next_run: AtomicU64::new(0),
            }),
        })
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
    /// [`Completion::Interactive`] when it waits for input at the terminal and
    /// [`Completion::StillRunning`] otherwise, with the screen's last lines.
    pub async fn run_command(
        &self,
        conversation: ConversationId,
        request: RunRequest,
        progress: &mut dyn RunProgress,
    ) -> Result<CommandResult, ShellError> {
        let (session, _) = self.session(conversation, &request.start_dir).await?;
        self.run_on(&session, request, progress).await
    }

    /// The state of the conversation's shell.
    pub async fn state(&self, conversation: ConversationId) -> Result<ShellState, ShellError> {
        self.existing(conversation)?.state().await
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
        if inner.integration {
            inner
                .installed
                .get_or_try_init(|| install(inner.config.integration_dir.clone()))
                .await?;
        }
        let pty_id = PtyId::from_uuid(efr_stdx::id::uuid_v7(&*inner.deps.clock, &*inner.deps.rng));
        let spec = SpawnSpec::new(pty_id, &inner.program, start_dir, inner.config.size)
            .args(integration::args(inner.config.login))
            .vars(env::shell_env(&inner.config, start_dir, inner.integration));
        let PtyHandle { master, child_pid, .. } = inner
            .deps
            .holder
            .spawn(spec)
            .await
            .map_err(|source| ShellError::Spawn { conversation, source })?;
        match self.start(conversation, pty_id, child_pid, master, start_dir) {
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
    /// forwarder, the exit waiter and the actor.
    fn start(
        &self,
        conversation: ConversationId,
        pty_id: PtyId,
        pid: u32,
        master: OwnedFd,
        cwd: &Path,
    ) -> Result<SessionHandle, ShellError> {
        let inner = &self.inner;
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
        let state = ShellState::new(pty_id, pid, cwd.to_path_buf(), inner.integration);
        let actor = SessionActor {
            core: SessionCore::new(conversation, state, Arc::clone(&deps.observer)),
            clock: Arc::clone(&deps.clock),
            holder: Arc::clone(&deps.holder),
            writer: writer.clone(),
            screen: screen.clone(),
            tasks,
            life,
            startup: inner.integration.then(|| deps.clock.sleep(inner.config.startup_timeout)),
        };
        tokio::spawn(actor.run(messages));
        Ok(SessionHandle { conversation, pty_id, pid, screen, inbox, writer, life: lives })
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
        let order = RunOrder {
            id,
            command: request.command,
            mode: request.mode,
            output_limit: request.output_limit,
            token: sentinel::token(&*deps.rng),
            reply,
            progress: publish,
        };
        session.send(Msg::Run(order)).await?;

        let mut deadline = deps.clock.sleep(request.timeout);
        let mut updates_open = true;
        loop {
            tokio::select! {
                biased;
                ended = &mut answer => return finished(session, ended),
                changed = updates.changed(), if updates_open => match changed {
                    Ok(()) => {
                        let update = updates.borrow_and_update().update();
                        progress.update(&update);
                    }
                    Err(_) => updates_open = false,
                },
                () = &mut deadline => break,
            }
        }

        let (reply, detached) = oneshot::channel();
        session.send(Msg::Detach { id, reply }).await?;
        match detached.await.map_err(|_| session.exited())? {
            Detached::Gone => finished(session, answer.await),
            Detached::Unstarted => Err(ShellError::NotReady { conversation: session.conversation }),
            Detached::Running { captured, range, last_output, cwd, delimiter } => {
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
                let completion = if quiet && waits_for_input(&capture.snapshot) {
                    Completion::Interactive
                } else {
                    Completion::StillRunning
                };
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
}

impl fmt::Debug for ShellSessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShellSessions")
            .field("program", &self.inner.program)
            .field("integration", &self.inner.integration)
            .field("shells", &self.lock().len())
            .finish_non_exhaustive()
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

fn finished(
    session: &SessionHandle,
    ended: Result<Result<RunEnd, ShellError>, oneshot::error::RecvError>,
) -> Result<CommandResult, ShellError> {
    let RunEnd { output, cwd, delimiter } = ended.map_err(|_| session.exited())??;
    Ok(CommandResult {
        completion: output.completion,
        exit_code: output.exit_code,
        output: output.captured.text,
        truncated: output.captured.truncated,
        output_bytes: output.captured.bytes,
        output_range: output.range,
        cwd_after: cwd,
        screen_tail: None,
        delimiter,
    })
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
    let id = conversation.to_string();
    let tail = id.get(id.len().saturating_sub(8)..).unwrap_or(&id);
    format!("screen-{tail}")
}

#[cfg(test)]
mod tests;
