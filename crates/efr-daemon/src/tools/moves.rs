//! The tool path's side of a move of a running `shell` call into a background job
//! (Ctrl+B, `call.background`; the table and its checks are in `jobs/moves.rs`).
//!
//! Each foreground `shell` call registers while it runs, with what the turn judged of
//! it. When the handler of `call.background` confirmed a move and efr-shell handed the
//! run over, the run answers its call with `Completion::Moved`. The tool path, which
//! still holds the call's proxy, bus, reach and snapshot, then takes the adopted run:
//! the reach ends with `Unreach` for every port (DS25), the job table adopts the run
//! with the proxy and the bus (DS22), and the model reads the output so far and
//! `job_moved_text` (DS29) in place of the run's own answer.

use std::path::{Path, PathBuf};
use std::time::Duration;

use efr_config::Settings;

use efr_conversation::{CallContext, ToolOutcome};
use efr_protocol::{CacheMode, JobId, JobInfo, JobState, MovedCall, SandboxSummary};
use efr_shell::AdoptedRun;
use efr_tools::job_moved_text;
use serde_json::Value;

use super::DaemonToolbox;
use crate::jobs::forward::reach::ReachCall;
use crate::jobs::moves::{self, Confirmed, Moved, Registration, RunningCall};
use crate::jobs::{AdoptedJob, output, stop_adopted};
use crate::sandbox::bus::BusCall;
use crate::sandbox::proxy::ProxyCall;

/// What the model reads when efr could not record the job of a moved call.
const MOVE_FAILED: &str = "[efr could not move the command to the background and stopped it. \
     Run it again if it is still needed.]";

/// How a foreground `shell` call runs, for its registration.
#[derive(Debug, Default)]
pub(super) struct RunKind {
    /// True for a call in the sandbox.
    pub(super) contained: bool,
    /// The call's dir, for a call through the launcher.
    pub(super) call_dir: Option<PathBuf>,
    /// True when the call's plan holds the cache layer lock.
    pub(super) holds_cache: bool,
}

/// What a moved call hands its job besides the run.
#[derive(Debug, Default)]
pub(super) struct MoveParts {
    pub(super) proxy: Option<ProxyCall>,
    pub(super) bus: Option<BusCall>,
    pub(super) reach: Option<ReachCall>,
    pub(super) guard_roots: Vec<PathBuf>,
    pub(super) write_roots: Vec<PathBuf>,
}

/// A run that efr-shell handed over for a move.
pub(super) struct Adopted {
    running: RunningCall,
    confirmed: Confirmed,
    run: AdoptedRun,
}

/// True when a contained call with `settings` holds the cache layer lock for its life
/// (`efr-sbx/src/layer_lock.rs`): a cache of `sandbox.caches` that exists keeps a
/// private layer, in the `overlay` mode, or in the `tmp` mode with `keep_downloads`
/// for a download cache. Such a call cannot move in v0.0.4 (DS19.7, OQS7): a second
/// call of the conversation would wait for the lock and fail.
pub(super) async fn holds_cache_lock(settings: &Settings, home: &Path) -> bool {
    let sandbox = &settings.sandbox;
    let downloads: Vec<PathBuf> = if sandbox.keep_downloads {
        let downloads = efr_config::DOWNLOAD_CACHES.iter();
        downloads.map(|cache| efr_sandbox::expand_home(Path::new(cache), home)).collect()
    } else {
        Vec::new()
    };
    let kept: Vec<PathBuf> = sandbox
        .caches
        .iter()
        .map(|cache| efr_sandbox::expand_home(cache, home))
        .filter(|cache| match sandbox.cache_mode {
            CacheMode::Overlay => true,
            CacheMode::Tmp => downloads.contains(cache),
            _ => false,
        })
        .collect();
    if kept.is_empty() {
        return false;
    }
    // NOTE: the plan skips a cache that does not exist, and so does the lock.
    tokio::task::spawn_blocking(move || kept.iter().any(|cache| cache.is_dir()))
        .await
        .unwrap_or(true)
}

impl DaemonToolbox {
    /// Registers the foreground `shell` call `call` with `input` while it runs, so the
    /// user can move it into a job; `None` when this efrd runs no jobs.
    pub(super) fn register_move(
        &self,
        call: &CallContext,
        input: &Value,
        kind: RunKind,
    ) -> Option<Registration> {
        let jobs = self.jobs.as_ref()?;
        let requirements = self.judged_requirements(call.call_id);
        let command = requirements
            .as_ref()
            .and_then(|requirements| requirements.command.clone())
            .or_else(|| input.get("command").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default();
        let running = RunningCall {
            context: call.clone(),
            requirements,
            command,
            started: self.clock.now(),
            contained: kind.contained,
            call_dir: kind.call_dir,
            holds_cache: kind.holds_cache,
        };
        Some(jobs.moves().register(running))
    }

    /// The run that efr-shell handed over for the move of the call of `registration`,
    /// once the call answered; `None` when no move was under way. A move that the run
    /// did not take (its command ended first) answers the handler and frees its place.
    pub(super) async fn moved_run(&self, registration: Option<Registration>) -> Option<Adopted> {
        let (running, mut confirmed) = registration?.finish()?;
        match (&mut confirmed.adopted).await {
            Ok(run) => Some(Adopted { running, confirmed, run }),
            Err(_) => {
                let _ = confirmed.reply.send(Err(moves::ENDED));
                None
            }
        }
    }

    /// After the snapshot after a moved call: a new snapshot of the call's roots (the
    /// turn's project and `named`), which the job of `job` keeps to its end. Then what
    /// the command writes after the move joins the turn that started the call (DS28),
    /// as for a job that `background: true` started.
    pub(super) async fn keep_for_job(
        &self,
        context: &CallContext,
        named: &[PathBuf],
        job: Option<&JobInfo>,
    ) {
        let (Some(jobs), Some(job)) = (&self.jobs, job) else { return };
        let Some(kept) = self.snapshot_before(context, named).await else { return };
        jobs.keep_snapshot(job, kept).await;
    }

    /// Makes the adopted run of `adopted` a background job with `parts` (DS22, DS25), and
    /// answers the call: the output so far and the moved text. A job that could not be
    /// recorded stops the command.
    pub(super) async fn finish_move(&self, adopted: Adopted, parts: MoveParts) -> ToolOutcome {
        let Adopted { running, confirmed, run } = adopted;
        let Confirmed { slot, reply, check, .. } = confirmed;
        // NOTE: only now that efr-shell handed the run over: a launcher that ended
        // before the hand-over wrote the shell state of a call that did not move. One
        // that ends after it but before the mark writes its state before any later call
        // of the conversation can run, since this call has not answered yet (DS23).
        if let Some(dir) = &running.call_dir {
            make_moved_file(dir.join(efr_sandbox::MOVED_FILE)).await;
        }
        let MoveParts { proxy, bus, reach, guard_roots, write_roots } = parts;
        // NOTE: a job gets no reach (DS34, OQS10); the helper closes its listeners first.
        if let Some(reach) = reach {
            reach.unreach_all().await;
        }
        let context = &running.context;
        let Some(jobs) = &self.jobs else {
            let _ = reply.send(Err(moves::NO_JOBS));
            return ToolOutcome::error(MOVE_FAILED);
        };
        let conversation = context.conversation_id;
        let number = match jobs.number(conversation).await {
            Ok(number) => number,
            Err(error) => {
                tracing::warn!(%conversation, error = %error, "a moved call got no job number; efr stops it");
                stop_adopted(&run).await;
                let _ = reply.send(Err(moves::NOT_RECORDED));
                return ToolOutcome::error(MOVE_FAILED);
            }
        };
        let id = JobId::nth(number);
        let now = self.clock.now();
        let after = Duration::try_from(now.duration_since(running.started)).unwrap_or_default();
        let head = run.head().to_owned();
        let output_path = context.scratch.join(efr_sandbox::JOB_DIR).join(output::file_name(&id));
        let proxied = proxy.is_some();
        let info = JobInfo {
            id,
            conversation_id: conversation,
            turn_id: context.turn_id,
            call_id: context.call_id,
            command: running.command.clone(),
            launch: check.launch.clone(),
            state: JobState::Running,
            started_at: now,
            ends_by: now,
            ended_at: None,
            exit_code: None,
            stopped_by: None,
            output: output_path,
            output_bytes: 0,
            watch: None,
            wakes: 0,
            forwards: Vec::new(),
            moved_from: Some(MovedCall {
                call_id: context.call_id,
                turn_id: context.turn_id,
                after_secs: after.as_secs(),
                exits: check.exits.clone(),
                launcher: running.call_dir.is_some(),
            }),
            waits_input: false,
        };
        let job = AdoptedJob {
            info,
            limit: Duration::from_secs(u64::from(check.minutes) * 60),
            scratch: context.scratch.clone(),
            call_dir: running.call_dir.clone(),
            guard_roots,
            write_roots,
            contained: running.contained,
            proxy,
            bus,
            full: context.is_full(),
        };
        let info = match jobs.adopt(run, slot, job).await {
            Ok(info) => info,
            Err(error) => {
                tracing::warn!(%conversation, error = %error, "a moved call could not become a job; efr stopped it");
                let _ = reply.send(Err(moves::NOT_RECORDED));
                return ToolOutcome::error(MOVE_FAILED);
            }
        };
        let _ = reply.send(Ok(Moved { job: info.id.clone() }));
        // NOTE: the new hidden shell starts where the old one was at the move: its
        // directory changes only at a prompt, so it is where the call started.
        let cwd = context.shell_cwd.clone().unwrap_or_else(|| context.cwd.clone());
        let text = job_moved_text(&info, after, &cwd, running.contained);
        let output = match head.as_str() {
            "" => text,
            head if head.ends_with('\n') => format!("{head}{text}"),
            head => format!("{head}\n{text}"),
        };
        let summary = running.contained.then(|| SandboxSummary {
            confined: true,
            proxied,
            ..SandboxSummary::default()
        });
        ToolOutcome::ok(output).with_job(info).with_sandbox(summary)
    }
}

/// Makes `$CALL/moved` as an empty file in one call (`O_CREAT | O_EXCL`, 0600), so the
/// launcher of a moved call writes no shell state at its end (DS23).
async fn make_moved_file(path: PathBuf) {
    let made = tokio::task::spawn_blocking(move || {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)
    })
    .await;
    if !matches!(made, Ok(Ok(_))) {
        tracing::warn!(
            "the moved mark of a call could not be made; its launcher may write the shell state of its start"
        );
    }
}
