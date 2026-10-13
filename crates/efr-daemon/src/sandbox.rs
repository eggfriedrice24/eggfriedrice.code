//! The sandbox service of the `auto` mode: the launcher's copy, the probe and its
//! status, the spec of each call, the plan lock, the turn-end report and the cache
//! layers' collector (efr's auto spec, sections 5, 12 and 15.2).
//!
//! At start efrd copies the installed `efr-sbx` to `$R/bin/efr-sbx` and checks its
//! SHA-256, then runs the probe; the status goes into a `watch` that the conversations
//! read when a prompt arrives and a turn starts, and an `auto` turn runs as `cautious`
//! while it says unavailable. The probe runs again after a reload that changes
//! `[sandbox]` or the projects, after a call whose sandbox could not start, before an
//! `auto` prompt while the last probe failed, and for `admin.sandbox_check`.
//!
//! [`SandboxService::prepare`] turns a call that the check point let through the
//! launcher into its call dir: `$R/sbx/<conversation>/<call>` (0700) with `spec.json`
//! and `nonce` (0600). It takes the plan lock of the call's projects, which the caller
//! holds until the launcher writes `started`.

use std::collections::HashMap;
use std::io::Write as _;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use efr_config::Settings;
use efr_conversation::CallContext;
use efr_permissions::{Engine, ExitNeed, WriteBind};
use efr_protocol::{
    ConversationId, DesktopEnv, DesktopKind, Event, Grant, Launch, SandboxPaths, SandboxStatus,
    Scope,
};
use efr_sandbox::{
    DesktopSocket, JobPlan, NONCE_FILE, ProbeReport, SPEC_FILE, SpecLaunch, expand_home, nonce_hex,
    too_wide,
};
use efr_scope::{Git, Home};
use efr_shell::SandboxRun;
use efr_stdx::paths::Dirs;
use efr_stdx::rng::Rng;
use efr_stdx::time::{Clock, Stopwatch};
use efr_store::{Batch, WriterHandle};
use tokio::sync::{Semaphore, watch};

use crate::DaemonError;
use crate::engine::load_registry;

pub(crate) mod desktop;
mod explain;
pub(crate) mod facts;
pub(crate) mod fs;
pub(crate) mod gc;
pub(crate) mod launcher;
pub(crate) mod links;
pub(crate) mod lock;
pub(crate) mod mounts;
pub(crate) mod peers;
pub(crate) mod plan;
pub(crate) mod probe;
pub(crate) mod projects;
pub(crate) mod quarantine;
pub(crate) mod report;
pub(crate) mod seams;
pub(crate) mod units;

pub(crate) use lock::PlanGuard;
pub(crate) use plan::HostFacts;
pub(crate) use seams::Seams;

/// The reason of the status before the first probe.
pub(crate) const NOT_PROBED: &str = "the sandbox probe has not run yet";

/// The bubblewrap that the spec of an exit child job names when the probe found none:
/// the exit child never runs it, and the spec must name an absolute path.
const EXIT_CHILD_BWRAP: &str = "/usr/bin/bwrap";

/// The directory of the probe's fake call below efr's state root.
const PROBE_DIR: &str = "sandbox/probe";

/// The note of a call whose line runs `df` or `findmnt` when efrd could not take the
/// snapshot of the host's mounts (efr's auto spec, 13.7).
pub(crate) const MOUNT_VIEW_FAILED: &str = "the mount view failed; df shows the sandbox's mounts";

/// The file in a conversation's sandbox dir where its hidden shell writes its `PATH`.
const PATH_FILE: &str = "path";

/// The most bytes of a `PATH` that efrd reads from [`PATH_FILE`].
const MAX_PATH_BYTES: usize = 64 * 1024;

/// What the service is built from.
pub(crate) struct ServiceParts {
    pub(crate) dirs: Dirs,
    pub(crate) home: Home,
    pub(crate) host: HostFacts,
    /// The installed launcher; `None` when none was found.
    pub(crate) source: Option<PathBuf>,
    pub(crate) seams: Seams,
    /// The project registry file.
    pub(crate) registry: PathBuf,
    pub(crate) writer: WriterHandle,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) rng: Arc<dyn Rng>,
    pub(crate) git: Git,
}

/// The sandbox service. Cheap to clone; clones share everything.
#[derive(Clone)]
pub(crate) struct SandboxService {
    inner: Arc<Inner>,
}

struct Inner {
    dirs: Dirs,
    home: Home,
    host: HostFacts,
    /// `$R/bin/efr-sbx`.
    copy: PathBuf,
    source: Option<PathBuf>,
    seams: Seams,
    registry: PathBuf,
    status: watch::Sender<SandboxStatus>,
    /// The last probe's report, for `admin.sandbox_check`.
    last: Mutex<Option<ProbeReport>>,
    /// Runs one probe at a time.
    probing: Semaphore,
    locks: lock::PlanLocks,
    turns: report::Turns,
    /// The calls through the launcher that run now, per conversation, so the cache
    /// collector leaves their layers alone.
    running: Mutex<HashMap<ConversationId, usize>>,
    /// The `PATH` that a hidden shell reported last.
    reported_path: Mutex<Option<String>>,
    /// The desktop variables of the terminal of each conversation's last prompt that
    /// sent any, for its desktop grants.
    desktops: Mutex<HashMap<ConversationId, DesktopEnv>>,
    /// True when efrd shares its parent's mount namespace, checked once at start.
    mount_view: bool,
    /// The snapshots of the host's mounts for the lines that run `df` or `findmnt`.
    mounts: mounts::MountView,
    writer: WriterHandle,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
    git: Git,
}

impl std::fmt::Debug for SandboxService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxService")
            .field("copy", &self.inner.copy)
            .field("source", &self.inner.source)
            .field("status", &*self.inner.status.borrow())
            .finish_non_exhaustive()
    }
}

/// What [`SandboxService::prepare`] needs besides the call.
pub(crate) struct PrepareInput<'a> {
    pub(crate) settings: &'a Settings,
    pub(crate) engine: &'a Engine,
    /// The paths that the call names: its declared paths and where it starts.
    pub(crate) named_paths: Vec<PathBuf>,
    /// True when the line runs `df` or `findmnt` (`exits::mount_view_line`): the call
    /// gets the snapshot of the host's mounts.
    pub(crate) mount_view: bool,
}

/// A prepared call: the run for the shell, the plan lock, and what the tool result says.
#[derive(Debug)]
pub(crate) struct Prepared {
    pub(crate) run: SandboxRun,
    /// Held until the launcher writes `started`, or the call ends.
    pub(crate) guard: PlanGuard,
    /// `$CALL/started`.
    pub(crate) started: PathBuf,
    /// The project roots of the call, which the turn-end report reads.
    pub(crate) projects: Vec<PathBuf>,
    /// Notes for the model.
    pub(crate) notes: Vec<&'static str>,
    /// The roots that the surface guard scans.
    pub(crate) guard_roots: Vec<PathBuf>,
    /// The write roots of the spec.
    pub(crate) write_roots: Vec<PathBuf>,
    /// The state dir of a job, `$SBX/jobs/<call>`.
    pub(crate) state_dir: PathBuf,
    /// True for a contained launch, false for the exit child.
    pub(crate) contained: bool,
    /// Counts the call as running until it drops; `None` for a job.
    _running: Option<Running>,
}

/// What [`SandboxService::prepare_job`] needs besides a call's input.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JobInput<'a> {
    pub(crate) plan: JobPlan,
    /// The line, which efrd writes to `$CALL/line`.
    pub(crate) line: &'a str,
    /// The job's environment, the bytes of `$CALL/env`.
    pub(crate) env: &'a [u8],
}

/// One running call of a conversation, counted until it drops.
#[derive(Debug)]
struct Running {
    service: SandboxService,
    conversation: ConversationId,
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut running = self.service.running();
        if let Some(count) = running.get_mut(&self.conversation) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                running.remove(&self.conversation);
            }
        }
    }
}

impl SandboxService {
    /// The service, with the launcher copied; the status says [`NOT_PROBED`] until the
    /// first [`probe`](Self::probe).
    pub(crate) async fn new(parts: ServiceParts) -> SandboxService {
        let ServiceParts { dirs, home, mut host, source, seams, registry, writer, clock, rng, git } =
            parts;
        let source = seams.launcher().map(Path::to_path_buf).or(source);
        let copy = dirs.runtime().join(launcher::BIN_DIR).join(launcher::LAUNCHER);
        if let Some(source) = &source {
            let (from, to) = (source.clone(), copy.clone());
            match tokio::task::spawn_blocking(move || launcher::install(&from, &to)).await {
                Ok(Ok(())) => {
                    tracing::info!(source = %source.display(), copy = %copy.display(), "sandbox launcher copied")
                }
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "the sandbox launcher could not be copied")
                }
                Err(_) => tracing::warn!("copying the sandbox launcher panicked"),
            }
            host.binaries.push(source.clone());
        }
        let (status, _) = watch::channel(SandboxStatus::unavailable(NOT_PROBED));
        let mount_view =
            tokio::task::spawn_blocking(mounts::same_mount_namespace).await.unwrap_or(false);
        let helper = seams.statvfs_helper().map_or_else(|| copy.clone(), Path::to_path_buf);
        let mounts = mounts::MountView::new(helper, Arc::clone(&clock));
        SandboxService {
            inner: Arc::new(Inner {
                dirs,
                home,
                host,
                copy,
                source,
                seams,
                registry,
                status,
                last: Mutex::new(None),
                probing: Semaphore::new(1),
                locks: lock::PlanLocks::default(),
                turns: report::Turns::default(),
                running: Mutex::new(HashMap::new()),
                reported_path: Mutex::new(None),
                desktops: Mutex::new(HashMap::new()),
                mount_view,
                mounts,
                writer,
                clock,
                rng,
                git,
            }),
        }
    }

    /// The status that the conversations read.
    pub(crate) fn status(&self) -> watch::Receiver<SandboxStatus> {
        self.inner.status.subscribe()
    }

    /// The status now.
    pub(crate) fn current(&self) -> SandboxStatus {
        self.inner.status.borrow().clone()
    }

    /// The status now as `efr status` and `efr sandbox check` get it: with
    /// `netview_off` when `settings` turn the network view off, so the client does not
    /// show the view as on while the engine does not use it.
    pub(crate) fn reported(&self, settings: &Settings) -> SandboxStatus {
        SandboxStatus { netview_off: !settings.network.netview, ..self.current() }
    }

    /// True when efrd's mount namespace is the one of its parent, so the mounts that
    /// efrd reads are the user's: `df` and `findmnt` then run in the sandbox with the
    /// snapshot. When false, they stay `outside` exits.
    pub(crate) fn mount_view(&self) -> bool {
        self.inner.mount_view
    }

    /// The plan locks, which `write_file` takes for its own write.
    pub(crate) fn locks(&self) -> &lock::PlanLocks {
        &self.inner.locks
    }

    /// The turns that the turn-end report follows.
    pub(crate) fn turns(&self) -> &report::Turns {
        &self.inner.turns
    }

    /// The `PATH` where a program word of the next call of `conversation` resolves.
    /// The user's startup files set it, so it is what the conversation's hidden shell
    /// reported last (its integration writes it at each prompt when it changed), else
    /// what any hidden shell reported last, since they all run the same startup files,
    /// else the environment that efrd starts them with.
    pub(crate) fn shell_path(&self, conversation: ConversationId) -> String {
        let file = self
            .inner
            .dirs
            .runtime()
            .join(plan::SHELL_DIR)
            .join(conversation.to_string())
            .join(PATH_FILE);
        let reported = fs::DaemonFs::read(&file, MAX_PATH_BYTES)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|text| text.trim_end_matches('\n').to_owned())
            .filter(|path| !path.is_empty());
        // NOTE: the value is replaced whole, so a poisoned lock still holds a usable one.
        let mut last = self.inner.reported_path.lock().unwrap_or_else(PoisonError::into_inner);
        match reported {
            Some(path) => {
                *last = Some(path.clone());
                path
            }
            None => last.clone().unwrap_or_else(|| self.inner.host.path.clone()),
        }
    }

    /// Keeps the desktop variables that a prompt of `conversation` sent with its shell
    /// context, `None` when its terminal has none of them; the desktop grants of its
    /// calls then use efrd's own start environment.
    pub(crate) fn set_desktop(&self, conversation: ConversationId, desktop: Option<&DesktopEnv>) {
        // NOTE: the value is replaced whole, so a poisoned lock still holds usable ones.
        let mut desktops = self.inner.desktops.lock().unwrap_or_else(PoisonError::into_inner);
        match desktop.filter(|env| !env.is_empty()) {
            Some(env) => {
                desktops.insert(conversation, env.clone());
            }
            None => {
                desktops.remove(&conversation);
            }
        }
    }

    /// The control sockets of the desktop grants of `call`, found and checked now for
    /// the variables of the terminal of the conversation's last prompt, else efrd's
    /// own. A grant without a socket that a call may use refuses the call with
    /// [`SandboxError::NoDesktop`](efr_sandbox::SandboxError::NoDesktop).
    fn desktop_sockets(&self, call: &CallContext) -> Result<Vec<DesktopSocket>, DaemonError> {
        let wanted: Vec<DesktopKind> = call
            .launch
            .grants()
            .iter()
            .filter_map(|grant| match grant {
                Grant::Desktop { desktop } => Some(*desktop),
                _ => None,
            })
            .collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let client = self
            .inner
            .desktops
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&call.conversation_id)
            .cloned();
        let host = &self.inner.host;
        let env = desktop::chosen(client.as_ref(), &host.desktop);
        let cookie = self.inner.home.path().join(".Xauthority");
        let facts = desktop::Facts {
            user_runtime: &host.user_runtime,
            uid: rustix::process::getuid().as_raw(),
            needs_cookie: desktop::needs_cookie(
                client.as_ref(),
                host.xauthority.is_some(),
                desktop::lstat(&cookie).is_some(),
            ),
        };
        let mut sockets = Vec::with_capacity(wanted.len());
        for kind in wanted {
            let socket = desktop::find(kind, env, &facts, &desktop::lstat)
                .map_err(|source| DaemonError::SandboxSpec { source })?;
            tracing::debug!(
                desktop = kind.name(),
                socket = %socket.path.display(),
                from_prompt = client.is_some(),
                "a desktop grant binds its socket"
            );
            sockets.push(socket);
        }
        Ok(sockets)
    }

    /// `$R/sbx/<conversation>`, where the conversation's hidden shell writes its `PATH`,
    /// its `env` and `snapshot.zsh`.
    pub(crate) fn shell_dir(&self, conversation: ConversationId) -> PathBuf {
        self.inner.dirs.runtime().join(plan::SHELL_DIR).join(conversation.to_string())
    }

    /// The launcher's copy, `$R/bin/efr-sbx`, which starts every job.
    pub(crate) fn launcher(&self) -> &Path {
        &self.inner.copy
    }

    /// The machine facts of the hidden shells.
    pub(crate) fn host(&self) -> &HostFacts {
        &self.inner.host
    }

    /// The hardened git runner.
    pub(crate) fn git(&self) -> &Git {
        &self.inner.git
    }

    /// The home directory.
    pub(crate) fn home(&self) -> &Home {
        &self.inner.home
    }

    /// True while a launcher of `conversation` still runs: a call dir with `started`
    /// and no `result.json`, also of a call that answered at its timeout. It reads one
    /// small directory.
    pub(crate) fn launcher_runs(&self, conversation: ConversationId) -> bool {
        let dir = self.inner.dirs.runtime().join(plan::SHELL_DIR).join(conversation.to_string());
        gc::launcher_running(&dir)
    }

    /// efr's own programs that the probe found: efrd, efr and the launcher. Undo never
    /// writes them (plan PB14).
    pub(crate) fn host_binaries(&self) -> Vec<PathBuf> {
        self.inner.host.binaries.clone()
    }

    /// The masks of the machine that every contained call gets besides the engine's
    /// and the user's: a `$XAUTHORITY` and `$HISTFILE` outside their usual place. efrd
    /// never copies or writes them for undo either (plan PB14).
    pub(crate) fn host_masks(&self) -> Vec<PathBuf> {
        let host = &self.inner.host;
        [&host.xauthority, &host.histfile].into_iter().flatten().cloned().collect()
    }

    /// The registered project roots that may be write roots, and the user's extra
    /// roots: the probe checks that no program of the sandbox lies in one.
    async fn write_roots(&self, settings: &Settings) -> Vec<PathBuf> {
        let home = self.inner.home.path();
        let registry = load_registry(&self.inner.registry).await;
        let mut roots: Vec<PathBuf> = registry
            .projects()
            .iter()
            .map(|project| project.root().to_path_buf())
            .filter(|root| !too_wide(root, home))
            .collect();
        roots.extend(
            settings
                .sandbox
                .write_roots
                .iter()
                .map(|root| expand_home(root, home))
                .filter(|root| root.is_absolute() && !too_wide(root, home)),
        );
        roots
    }

    /// Runs the probe now with `settings`, keeps its report and sends the new status.
    /// A change to unavailable after an earlier probe is recorded as
    /// `sandbox_unavailable`.
    pub(crate) async fn probe(&self, settings: &Settings) -> ProbeReport {
        // NOTE: the semaphore is never closed, so the acquire cannot fail; without the
        // permit the probe still runs, only not alone.
        let _alone = self.inner.probing.acquire().await.ok();
        let report = self.run_probe(settings).await;
        let status = match self.inner.seams.probe() {
            Some(status) => status.clone(),
            None => report.status(),
        };
        let had_report = self.lock_last().replace(report.clone()).is_some();
        let previous = self.inner.status.send_replace(status.clone());
        if status.available {
            tracing::info!(
                cache_mode = status.cache_mode.as_str(),
                "the auto sandbox is available"
            );
        } else {
            let reason = status.reason.clone().unwrap_or_default();
            tracing::warn!(reason = %reason, "the auto sandbox is unavailable; auto turns run as cautious");
            if had_report && (previous.available || previous.reason != status.reason) {
                let event = Event::SandboxUnavailable { reason };
                if let Err(error) = self.inner.writer.append(Batch::new().global_event(event)).await
                {
                    tracing::warn!(error = %error, "sandbox_unavailable could not be recorded");
                }
            }
        }
        report
    }

    async fn run_probe(&self, settings: &Settings) -> ProbeReport {
        let inner = &self.inner;
        let input = probe::ProbeInput {
            enabled: settings.sandbox.enabled,
            source: inner.source.clone(),
            copy: inner.copy.clone(),
            dir: inner.dirs.state().join(PROBE_DIR),
            bwrap: settings.sandbox.bwrap.clone(),
            zsh: inner.host.zsh.clone(),
            home: inner.home.path().to_path_buf(),
            user_runtime: inner.host.user_runtime.clone(),
            cache_mode: settings.sandbox.cache_mode,
            write_roots: self.write_roots(settings).await,
            shell_path: inner.host.path.clone(),
        };
        if let Some(status) = inner.seams.probe() {
            // NOTE: a test that replaced the probe gets its status in the report too,
            // so admin.sandbox_check lists what the fallback reads.
            return ProbeReport {
                bwrap: status.bwrap.clone(),
                bwrap_version: status.bwrap_version.clone(),
                landlock_abi: status.landlock_abi,
                errata: status.errata,
                cache_mode: status.cache_mode,
                warnings: status.warnings.clone(),
                ..ProbeReport::default()
            };
        }
        let copy_matches = match &input.source {
            Some(source) => {
                let (source, copy) = (source.clone(), input.copy.clone());
                tokio::task::spawn_blocking(move || launcher::matches(&source, &copy))
                    .await
                    .unwrap_or(false)
            }
            None => false,
        };
        if let Some(report) = probe::own_checks(&input, copy_matches) {
            return report;
        }
        let report = probe::run_launcher(&input, &inner.clock).await;
        probe::with_own_checks(report, &input)
    }

    /// Runs the probe again in the background, as after a call whose sandbox could not
    /// start.
    pub(crate) fn reprobe(&self, settings: Arc<Settings>) {
        let service = self.clone();
        tokio::spawn(async move {
            service.probe(&settings).await;
        });
    }

    /// Where the sandbox keeps its files, for `admin.status` and `efr paths`.
    pub(crate) async fn paths(&self) -> SandboxPaths {
        let inner = &self.inner;
        let copy = inner.copy.clone();
        let source = inner.source.clone();
        let (exists, matches) = {
            let (copy, source) = (copy.clone(), source.clone());
            tokio::task::spawn_blocking(move || {
                let exists = copy.is_file();
                let matches =
                    source.as_deref().map(|source| exists && launcher::matches(source, &copy));
                (exists, matches)
            })
            .await
            .unwrap_or((false, None))
        };
        SandboxPaths {
            launcher: exists.then_some(copy),
            launcher_source: source,
            launcher_sha256_ok: matches,
            state: inner.dirs.state().join(plan::SANDBOX_DIR),
            runtime: inner.dirs.runtime().join(plan::SHELL_DIR),
        }
    }

    /// Keeps the worktree record of the project at `root`, when its `.git` is a file,
    /// after `efr project add` registered it. A record that cannot be written leaves
    /// the project's git dirs read-only in the sandbox, and costs a warning.
    pub(crate) async fn register_project(&self, root: &Path) {
        let (state, root) = (self.inner.dirs.state().to_path_buf(), root.to_path_buf());
        let written = tokio::task::spawn_blocking(move || projects::register(&state, &root)).await;
        match written {
            Ok(Ok(Some(record))) => {
                tracing::info!(root = %record.root.display(), git_dir = %record.git_dir.display(), "worktree project recorded")
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "the worktree record could not be written")
            }
            Err(_) => tracing::warn!("writing the worktree record panicked"),
        }
    }

    /// The registered project roots.
    pub(crate) async fn projects(&self) -> Vec<PathBuf> {
        load_registry(&self.inner.registry)
            .await
            .projects()
            .iter()
            .map(|project| project.root().to_path_buf())
            .collect()
    }

    /// Prepares `call` for the launcher: takes the plan lock of its projects, makes the
    /// targets of its approved `MakeFile` and `MakeDir` grants, makes `$SCRATCH/jobs`,
    /// and writes its call dir with `spec.json` and `nonce`.
    pub(crate) async fn prepare(
        &self,
        call: &CallContext,
        input: &PrepareInput<'_>,
    ) -> Result<Prepared, DaemonError> {
        self.prepare_with(call, input, None).await
    }

    /// Prepares the background job of `call`, as [`prepare`](Self::prepare) does a
    /// call, with its job plan: its call dir is `$R/sbx/<conversation>/jobs/<call>`,
    /// and efrd writes `line` and `env` there itself. A job of a `manual` or `cautious`
    /// turn runs in the exit child, which needs no bubblewrap. The job does not count
    /// as a running call of the conversation: its layers are its own.
    pub(crate) async fn prepare_job(
        &self,
        call: &CallContext,
        input: &PrepareInput<'_>,
        job: JobInput<'_>,
    ) -> Result<Prepared, DaemonError> {
        self.prepare_with(call, input, Some(job)).await
    }

    async fn prepare_with(
        &self,
        call: &CallContext,
        input: &PrepareInput<'_>,
        job: Option<JobInput<'_>>,
    ) -> Result<Prepared, DaemonError> {
        let status = self.current();
        // NOTE: a job outside `auto` runs in the exit child: no bubblewrap, Landlock or
        // seccomp, only the launcher, which the spawn needs.
        let launch = match (&job, &call.launch) {
            (Some(_), Launch::Direct) => Launch::Unsandboxed,
            (_, launch) => launch.clone(),
        };
        let exit_child_job = job.is_some() && launch == Launch::Unsandboxed;
        if !status.available && !exit_child_job {
            return Err(DaemonError::SandboxUnavailable {
                reason: status.reason.clone().unwrap_or_else(|| NOT_PROBED.to_owned()),
            });
        }
        let bwrap = match status.bwrap.clone() {
            Some(bwrap) => bwrap,
            None if exit_child_job => PathBuf::from(EXIT_CHILD_BWRAP),
            None => {
                return Err(DaemonError::SandboxUnavailable {
                    reason: "no bubblewrap was found".to_owned(),
                });
            }
        };
        let desktops = self.desktop_sockets(call)?;
        let projects = self.projects().await;
        let turn_project = match &call.scope {
            Scope::Project(id) => input.engine.locations().project_root(id).map(Path::to_path_buf),
            _ => None,
        };
        let protected_config = {
            let dir = self.inner.dirs.config().to_path_buf();
            tokio::task::spawn_blocking(move || crate::engine::protected_config(&dir))
                .await
                .unwrap_or_default()
        };
        let secrets = input.engine.locations().secret_paths();
        let plan_input = plan::PlanInput {
            conversation: call.conversation_id,
            call: call.call_id,
            launch: &launch,
            turn_project: turn_project.as_deref(),
            projects: &projects,
            named_paths: &input.named_paths,
            scratch: &call.scratch,
            settings: &input.settings.sandbox,
            secrets: &secrets,
            protected_config: &protected_config,
            host: &self.inner.host,
            dirs: &self.inner.dirs,
            home: self.inner.home.path(),
            bwrap: &bwrap,
            cache_mode: status.cache_mode,
            launcher: &self.inner.copy,
            desktops: &desktops,
            job: job.as_ref().map(|job| job.plan),
        };
        let roots: Vec<PathBuf> =
            plan::project_roots(&plan_input).into_iter().map(|root| root.path).collect();
        // NOTE: the snapshot runs beside the plan lock and the plan, so a mount that
        // hangs for its deadline never holds the lock of the call's projects.
        let snapshot = (input.mount_view && self.mount_view() && launch != Launch::Unsandboxed)
            .then(|| {
                let view = self.inner.mounts.clone();
                tokio::spawn(async move { view.snapshot().await })
            });
        let watch = Stopwatch::start();
        let guard = self.inner.locks.lock(&roots).await;
        tracing::debug!(phase = "plan_lock", elapsed_ms = %watch, "phase=plan_lock elapsed_ms={}", watch);
        // NOTE: the plan reads records, links and project dirs: a few small reads, done
        // here under the plan lock.
        let watch = Stopwatch::start();
        let planned = plan::build(&plan_input);
        tracing::debug!(phase = "plan_build", elapsed_ms = %watch, "phase=plan_build elapsed_ms={}", watch);
        let spec = planned.spec;
        let mut notes = planned.notes;
        make_targets(&call.exits).await?;
        let call_dir = spec.runtime.call_dir.clone();
        let mut nonce = [0_u8; 16];
        self.inner.rng.fill_bytes(&mut nonce);
        let bytes = spec.to_json().map_err(|source| DaemonError::SandboxSpec { source })?;
        let mut dirs =
            vec![self.inner.dirs.runtime().join(plan::SHELL_DIR), spec.runtime.shell_dir.clone()];
        if job.is_some() {
            dirs.push(spec.runtime.shell_dir.join(efr_sandbox::JOB_DIR));
        }
        dirs.extend([call_dir.clone(), spec.runtime.sandbox_dir.clone()]);
        if job.is_some() {
            dirs.push(spec.runtime.job_state_dir(call.call_id));
        }
        let mut files: Vec<(&'static str, Vec<u8>)> =
            vec![(SPEC_FILE, bytes), (NONCE_FILE, nonce_hex(&nonce).into_bytes())];
        if let Some(job) = &job {
            files.push((efr_sandbox::LINE_FILE, job.line.as_bytes().to_vec()));
            files.push((efr_sandbox::ENV_FILE, job.env.to_vec()));
        }
        if let Some(snapshot) = snapshot {
            files.extend(mount_files(snapshot, &spec, &mut notes).await);
        }
        let dir = call_dir.clone();
        let stamp = spec.runtime.sandbox_dir.join(gc::LAST_CALL_FILE);
        let scratch = call.scratch.clone();
        let watch = Stopwatch::start();
        tokio::task::spawn_blocking(move || {
            write_call_dir(&dirs, &dir, &files)?;
            // NOTE: before any command of the call runs, so no command can make the dir
            // of the job outputs first and plant a file or a link there.
            crate::jobs::output::make_dir(&scratch).map_err(|source| DaemonError::Io {
                path: scratch.join(efr_sandbox::JOB_DIR),
                source,
            })?;
            // The cache collector counts a conversation's idle days from this file.
            std::fs::write(&stamp, b"").map_err(|source| DaemonError::Io { path: stamp, source })
        })
        .await
        .map_err(|_| DaemonError::TaskPanicked { task: "sandbox call dir" })??;
        tracing::debug!(phase = "call_dir_write", elapsed_ms = %watch, "phase=call_dir_write elapsed_ms={}", watch);
        let running = match job {
            Some(_) => None,
            None => {
                self.running()
                    .entry(call.conversation_id)
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
                Some(Running { service: self.clone(), conversation: call.conversation_id })
            }
        };
        let launch = match spec.launch {
            SpecLaunch::Unsandboxed => SpecLaunch::Unsandboxed,
            _ => SpecLaunch::Contained,
        };
        let run = SandboxRun::new(call_dir.clone(), call.call_id, nonce, launch);
        Ok(Prepared {
            run,
            guard,
            started: call_dir.join(efr_sandbox::STARTED_FILE),
            projects: roots,
            notes,
            guard_roots: spec.guard_roots.clone(),
            write_roots: spec.write_roots.iter().map(|root| root.path.clone()).collect(),
            state_dir: spec.runtime.job_state_dir(call.call_id),
            contained: spec.launch == SpecLaunch::Contained,
            _running: running,
        })
    }

    /// Deletes idle cache layers by `settings`, leaving those of a conversation with a
    /// running call.
    pub(crate) async fn gc(&self, settings: &Settings) {
        let root = self.inner.dirs.state().join(plan::SANDBOX_DIR);
        let now = std::time::SystemTime::from(self.inner.clock.now());
        let shells = self.inner.dirs.runtime().join(plan::SHELL_DIR);
        let max_idle = Duration::from_secs(u64::from(settings.sandbox.cache_days) * 24 * 3600);
        let max_bytes = u64::from(settings.sandbox.cache_max_gib) * 1024 * 1024 * 1024;
        let service = self.clone();
        let done = tokio::task::spawn_blocking(move || {
            let busy = |running: &HashMap<ConversationId, usize>, name: &str| {
                running.keys().any(|known| known.to_string() == name)
                    || gc::launcher_running(&shells.join(name))
            };
            let layers = gc::scan(&root, now, &|name| busy(&service.running(), name));
            let mut aside = gc::left_aside(&root);
            for dir in gc::pick(&layers, max_idle, max_bytes) {
                let Some(name) = dir.parent().and_then(Path::file_name) else { continue };
                let name = name.to_string_lossy();
                // NOTE: checked again and moved under the lock that each plan takes to
                // count its call: a call that started since the scan keeps its layers,
                // and one that starts later finds no layers to lose.
                let running = service.running();
                if busy(&running, &name) {
                    continue;
                }
                aside.extend(gc::set_aside(&dir, now));
                drop(running);
            }
            gc::remove(&aside);
        })
        .await;
        if done.is_err() {
            tracing::warn!("the cache layers' collector panicked");
        }
    }

    /// The quarantine of the call `call` of `conversation`, `$SBX/quarantine/<call>`.
    pub(crate) fn quarantine_dir(
        &self,
        conversation: ConversationId,
        call: efr_protocol::CallId,
    ) -> PathBuf {
        self.inner
            .dirs
            .state()
            .join(plan::SANDBOX_DIR)
            .join(conversation.to_string())
            .join("quarantine")
            .join(call.to_string())
    }

    fn running(&self) -> std::sync::MutexGuard<'_, HashMap<ConversationId, usize>> {
        // Counts only; a poisoned lock still holds usable ones.
        self.inner.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_last(&self) -> std::sync::MutexGuard<'_, Option<ProbeReport>> {
        // The report is replaced whole, so a poisoned lock still holds a usable one.
        self.inner.last.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The `mounts` and `mounts.json` of a call from the snapshot that `task` takes, with
/// no sizes below the masks of `spec` ([`mount_masks`]). When the snapshot failed, no
/// file and the note [`MOUNT_VIEW_FAILED`]: the call's `df` then shows the sandbox's
/// mounts.
async fn mount_files(
    task: tokio::task::JoinHandle<Option<Arc<mounts::Taken>>>,
    spec: &efr_sandbox::SandboxSpec,
    notes: &mut Vec<&'static str>,
) -> Vec<(&'static str, Vec<u8>)> {
    match snapshot_files(task, spec).await {
        Some((table, sizes)) => {
            vec![(efr_sandbox::MOUNTS_FILE, table), (efr_sandbox::SIZES_FILE, sizes)]
        }
        None => {
            tracing::warn!(call_id = %spec.call, "the mount view failed; df shows the sandbox's mounts");
            notes.push(MOUNT_VIEW_FAILED);
            Vec::new()
        }
    }
}

async fn snapshot_files(
    task: tokio::task::JoinHandle<Option<Arc<mounts::Taken>>>,
    spec: &efr_sandbox::SandboxSpec,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let taken = task.await.ok()??;
    let (masks, below) = mount_masks(spec);
    tokio::task::spawn_blocking(move || {
        // NOTE: the plan masks the real target of a link, and a mount's target is a
        // real path, so each mask counts in both forms.
        let real = |paths: Vec<PathBuf>| -> Vec<PathBuf> {
            let mut all = Vec::with_capacity(paths.len());
            for path in paths {
                if let Ok(target) = std::fs::canonicalize(&path)
                    && target != path
                {
                    all.push(target);
                }
                all.push(path);
            }
            all
        };
        taken.files(&real(masks), &real(below))
    })
    .await
    .ok()?
}

/// The masks of the mount view of a call: its read masks but those that an `Unmask`
/// grant opens, at or below which a mount gets no sizes; and the dirs that the sandbox
/// replaces with its own (the runtime dirs, efr's data and state roots), strictly below
/// which a mount gets none.
fn mount_masks(spec: &efr_sandbox::SandboxSpec) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let unmasked: Vec<&PathBuf> = spec
        .grants
        .iter()
        .filter_map(|grant| match grant {
            Grant::Unmask { path } => Some(path),
            _ => None,
        })
        .collect();
    let masks = spec
        .masks
        .iter()
        .filter(|mask| !unmasked.contains(&&mask.path))
        .map(|mask| mask.path.clone())
        .collect();
    let runtime = &spec.runtime;
    let below = vec![
        runtime.user_runtime.clone(),
        runtime.runtime.clone(),
        runtime.data.clone(),
        runtime.state.clone(),
    ];
    (masks, below)
}

/// Makes the targets that approved write grants bind and that their program would
/// make: an empty file or directory, through its parent with no link on the way.
async fn make_targets(exits: &[ExitNeed]) -> Result<(), DaemonError> {
    let targets: Vec<(PathBuf, bool)> = exits
        .iter()
        .filter_map(|need| match (&need.bind, &need.target) {
            (Some(WriteBind::MakeFile), Some(target)) => Some((target.clone(), false)),
            (Some(WriteBind::MakeDir), Some(target)) => Some((target.clone(), true)),
            _ => None,
        })
        .collect();
    if targets.is_empty() {
        return Ok(());
    }
    tokio::task::spawn_blocking(move || {
        for (target, dir) in &targets {
            fs::DaemonFs::make(target, *dir)
                .map_err(|source| DaemonError::Io { path: target.clone(), source })?;
        }
        Ok(())
    })
    .await
    .map_err(|_| DaemonError::TaskPanicked { task: "sandbox grant targets" })?
}

/// Makes `dirs` (0700, in order) and writes `files` (0600, new files) into `call_dir`:
/// `spec.json` and `nonce`, and for a job `line` and `env`. It blocks.
fn write_call_dir(
    dirs: &[PathBuf],
    call_dir: &Path,
    files: &[(&str, Vec<u8>)],
) -> Result<(), DaemonError> {
    for dir in dirs {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|source| DaemonError::Io { path: dir.clone(), source })?;
    }
    for (name, bytes) in files {
        let path = call_dir.join(name);
        let mut file = efr_stdx::fs::create_private(&path).map_err(|source| DaemonError::Io {
            path: path.clone(),
            source: std::io::Error::other(source),
        })?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| DaemonError::Io { path: path.clone(), source })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
