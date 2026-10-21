//! `jobs` and `job_stop`: the model reads and stops the background jobs of its
//! conversation (efr's auto spec, section 13.8, phase C).
//!
//! Both are daemon tools, like the settings tool: they read efrd's job table, which
//! `efr-tools` cannot reach. Both declare no requirement, so the engine runs them with
//! no question in every mode for a turn from this machine; a phone turn gets the
//! question that every tool gets there. They act only on the jobs of the call's
//! conversation.
//!
//! The start of a job is here too ([`DaemonToolbox::start_job`]): a `shell` call with
//! `background: true` that the check point let through. Its line runs in the sandbox
//! with its grants in `auto`, and in the exit child after the user's yes in `manual` and
//! `cautious`. The toolbox clamps its time limit and its watch, gets the hidden shell's
//! environment, prepares its call dir under the plan lock with the snapshot before it,
//! and hands the launcher to the job table, which records `job_started` before the call
//! answers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_config::JobsSettings;
use efr_conversation::{CallContext, ToolCall, ToolOutcome, Toolbox as _};
use efr_permissions::CoverAsk;
use efr_permissions::Requirements;
use efr_protocol::{
    FileChanges, JobId, JobInfo, JobState, JobStopBy, Launch, SandboxSummary, Watch,
};
use efr_provider::ToolDefinition;
use efr_sandbox::JobPlan;
use efr_scope::Home;
use efr_shell::{NoProgress, RunRequest};
use efr_tools::{JobRequest, job_request, job_started_text};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{DaemonToolbox, SANDBOX_NOT_STARTED, for_model};
use crate::DaemonError;
use crate::jobs::{JobKey, JobTable, Matcher, NewJob, Slot, forward, job_line, output};
use crate::sandbox::{JobInput, PrepareInput, Prepared};
use crate::tools::CallSnapshots;

/// The name of the tool that lists the jobs and reads their output.
pub(crate) const JOBS: &str = "jobs";

/// The name of the tool that stops a job.
pub(crate) const JOB_STOP: &str = "job_stop";

/// What the model reads when this efrd runs no jobs, such as one with no sandbox
/// service.
pub(crate) const NOT_READY: &str =
    "background jobs do not run in this efrd; run the command in the foreground";

/// What the model reads while `jobs.enabled` is false.
pub(crate) const JOBS_OFF: &str = "background jobs are off here (jobs.enabled = false); run \
     the command in the foreground, or ask the user to turn jobs on";

/// What the model reads when efr could not get the hidden shell's environment for a job.
pub(crate) const ENV_NOT_READY: &str = "the shell environment is not ready; try again";

/// The line that efrd types into a hidden shell that has no `env` file: it changes
/// nothing but the zsh integration's record of the environment that it wrote last, so the
/// next prompt writes the file.
const ENV_REFRESH: &str = "_efr_hs_env=";

/// The lines of the output that `jobs` shows with an id and no `tail` or `offset`, and
/// that `job_stop` shows.
const DEFAULT_TAIL_LINES: usize = 50;

/// The most bytes of output that `jobs` and `job_stop` show.
const MAX_SHOWN_BYTES: usize = 32 * 1024;

/// How long a job start waits for a new hidden shell's first prompt, which writes the
/// shell's `env` file.
const ENV_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a job start looks for the `env` file after a prompt.
const ENV_FILE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// How often it looks.
const ENV_FILE_POLL: std::time::Duration = std::time::Duration::from_millis(10);

/// The most lines of output that `jobs` shows with `tail`.
pub(crate) const MAX_TAIL_LINES: u32 = 200;

/// What `jobs` tells the model about itself.
const JOBS_DESCRIPTION: &str = "List the background jobs of this conversation, or read \
    the output of one. Without id: one line per job with its id, state, command, start, \
    time left and output size. With id: that job's line, then its output from offset (in \
    bytes), or else its last tail lines (default 50, at most 200), at most 32 KiB. It only \
    reads. The notice of a job's end comes by itself, so do not poll with it.";

/// What `job_stop` tells the model about itself.
const JOB_STOP_DESCRIPTION: &str = "Stop one background job of this conversation, such as \
    j3. It answers with the job's state and the last 50 lines of its output; a job that \
    ended already answers with its final state. Stop a job when its work is done or no \
    longer needed.";

/// The input of `jobs`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobsInput {
    #[serde(default)]
    pub(crate) id: Option<JobId>,
    #[serde(default)]
    pub(crate) tail: Option<u32>,
    #[serde(default)]
    pub(crate) offset: Option<u64>,
}

/// The input of `job_stop`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobStopInput {
    pub(crate) id: JobId,
}

/// The schema of a job id, as both tools take it.
fn id_schema(description: &str) -> Value {
    json!({
        "type": "string",
        "pattern": "^j[1-9][0-9]{0,8}$",
        "description": description,
    })
}

/// The definition of `jobs` for the provider request.
pub(crate) fn jobs_definition() -> ToolDefinition {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": id_schema("The job, such as j3. Absent: list every job of this conversation."),
            "tail": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_TAIL_LINES,
                "description": "With id: how many of the last lines of the output to show; \
                    default 50."
            },
            "offset": {
                "type": "integer",
                "minimum": 0,
                "description": "With id: show the output from this byte on, in place of \
                    the last lines."
            }
        },
        "additionalProperties": false
    });
    ToolDefinition::function(JOBS, JOBS_DESCRIPTION, schema)
}

/// The definition of `job_stop` for the provider request.
pub(crate) fn job_stop_definition() -> ToolDefinition {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": id_schema("The job to stop, such as j3.")
        },
        "required": ["id"],
        "additionalProperties": false
    });
    ToolDefinition::function(JOB_STOP, JOB_STOP_DESCRIPTION, schema)
}

/// True for the name of one of the two job tools.
pub(crate) fn is_job_tool(name: &str) -> bool {
    name == JOBS || name == JOB_STOP
}

/// What a call of a job tool needs: nothing, for an input that matches its schema. An
/// input that does not is the text that the model reads.
pub(crate) fn requirements(call: &ToolCall) -> Result<Requirements, String> {
    match call.name.as_str() {
        JOBS => {
            let input = parse::<JobsInput>(JOBS, &call.input)?;
            if input.id.is_none() && (input.tail.is_some() || input.offset.is_some()) {
                return Err(format!("tail and offset of {JOBS} need an id"));
            }
            if input.tail.is_some_and(|tail| tail == 0 || tail > MAX_TAIL_LINES) {
                return Err(format!("tail of {JOBS} is from 1 to {MAX_TAIL_LINES}"));
            }
        }
        _ => {
            parse::<JobStopInput>(JOB_STOP, &call.input)?;
        }
    }
    Ok(Requirements::none())
}

/// Runs a call of a job tool on `jobs`, for the jobs of the call's conversation only.
pub(crate) async fn invoke(jobs: Option<&JobTable>, call: &ToolCall) -> ToolOutcome {
    let Some(jobs) = jobs else { return ToolOutcome::error(NOT_READY) };
    let conversation = call.context.conversation_id;
    let now = jobs.now();
    match call.name.as_str() {
        JOBS => {
            let input = match parse::<JobsInput>(JOBS, &call.input) {
                Ok(input) => input,
                Err(text) => return ToolOutcome::error(text),
            };
            let Some(id) = input.id else {
                return match jobs.list(Some(conversation)).await {
                    Ok(list) if list.is_empty() => {
                        ToolOutcome::ok("no background jobs in this conversation")
                    }
                    Ok(list) => ToolOutcome::ok(
                        list.iter().map(|job| job_line(job, now)).collect::<Vec<_>>().join("\n"),
                    ),
                    Err(error) => ToolOutcome::error(efr_stdx::with_causes(&error)),
                };
            };
            let job = match find(jobs, conversation, &id).await {
                Ok(job) => job,
                Err(text) => return ToolOutcome::error(text),
            };
            let lines =
                input.tail.map_or(DEFAULT_TAIL_LINES, |tail| tail.min(MAX_TAIL_LINES) as usize);
            let shown = shown_output(&job.output, input.offset, lines).await;
            ToolOutcome::ok(format!("{}\n{shown}", job_line(&job, now)))
        }
        _ => {
            let input = match parse::<JobStopInput>(JOB_STOP, &call.input) {
                Ok(input) => input,
                Err(text) => return ToolOutcome::error(text),
            };
            let before = match find(jobs, conversation, &input.id).await {
                Ok(job) => job,
                Err(text) => return ToolOutcome::error(text),
            };
            let Some(key) = JobKey::of(conversation, &input.id) else {
                return ToolOutcome::error(format!("no job {} in this conversation", input.id));
            };
            let (job, first) = if before.state == JobState::Running {
                match jobs.stop(key, JobStopBy::Model).await {
                    Ok(job) => (job, format!("job {} stopped", input.id)),
                    Err(error) => return ToolOutcome::error(efr_stdx::with_causes(&error)),
                }
            } else {
                (before, format!("job {} had ended already", input.id))
            };
            let shown = shown_output(&job.output, None, DEFAULT_TAIL_LINES).await;
            ToolOutcome::ok(format!("{first}\n{}\n{shown}", job_line(&job, now)))
        }
    }
}

/// The job `id` of `conversation`, or what the model reads when it has none.
async fn find(
    jobs: &JobTable,
    conversation: efr_protocol::ConversationId,
    id: &JobId,
) -> Result<JobInfo, String> {
    let missing = || format!("no job {id} in this conversation; call jobs to list them");
    let key = JobKey::of(conversation, id).ok_or_else(missing)?;
    match jobs.find(key).await {
        Ok(Some(job)) => Ok(job),
        Ok(None) => Err(missing()),
        Err(error) => Err(efr_stdx::with_causes(&error)),
    }
}

/// The output of a job as a tool shows it: from `offset`, or its last `lines` lines,
/// at most [`MAX_SHOWN_BYTES`].
async fn shown_output(path: &Path, offset: Option<u64>, lines: usize) -> String {
    let path = path.to_path_buf();
    let read =
        tokio::task::spawn_blocking(move || output::read(&path, offset, lines, MAX_SHOWN_BYTES))
            .await;
    match read {
        Ok(Ok(text)) if text.is_empty() => "(no output yet)".to_owned(),
        Ok(Ok(text)) => text,
        Ok(Err(error)) => format!("(the output could not be read: {error})"),
        Err(_) => "(the output could not be read)".to_owned(),
    }
}

/// The time limit in minutes of a job that asked for `asked`, and whether efr cut it:
/// the default when it asked for none, at most `jobs.max_minutes`.
pub(crate) fn job_minutes(asked: Option<u32>, settings: &JobsSettings) -> (u32, bool) {
    let max = settings.max_minutes.max(1);
    let wanted = asked.unwrap_or(settings.default_minutes).max(1);
    (wanted.min(max), wanted > max)
}

impl DaemonToolbox {
    /// Why a `shell` call with `background: true` cannot start a job here; `None` when
    /// it can, in every mode.
    pub(super) fn jobs_refused(&self) -> Option<&'static str> {
        if self.jobs.is_none() {
            return Some(NOT_READY);
        }
        if !self.settings.borrow().jobs.enabled {
            return Some(JOBS_OFF);
        }
        None
    }

    /// Starts the job of a `shell` call with `background: true` and answers at once
    /// with its id, its output file and its time limit.
    pub(super) async fn start_job(&self, call: ToolCall) -> ToolOutcome {
        let Some(jobs) = &self.jobs else { return ToolOutcome::error(NOT_READY) };
        let Some((sandbox, engine)) = &self.sandbox else {
            return ToolOutcome::error(SANDBOX_NOT_STARTED);
        };
        let request: JobRequest = match job_request(&call.input) {
            Ok(Some(request)) => request,
            Ok(None) => return ToolOutcome::error("this call does not start a background job"),
            Err(error) => return ToolOutcome::error(for_model(&error)),
        };
        // NOTE: before the settings: a reload that turns forwards off after this sends
        // its settings first, so a job that read the old ones still forwards nothing.
        let generation = jobs.forwards().generation();
        let settings = Arc::clone(&self.settings.borrow());
        if !settings.jobs.enabled {
            return ToolOutcome::error(JOBS_OFF);
        }
        let conversation = call.context.conversation_id;
        let slot = match jobs.reserve(conversation, &settings.jobs) {
            Ok(slot) => slot,
            Err(text) => return ToolOutcome::error(text),
        };
        let (minutes, clamped) = job_minutes(request.max_minutes, &settings.jobs);
        let (matcher, watch) = match &request.watch {
            Some(watch) => match Matcher::new(&watch.pattern) {
                Ok(matcher) => {
                    let most = u8::try_from(settings.jobs.max_wakes_per_watch).unwrap_or(u8::MAX);
                    let max_wakes = watch.max_wakes.unwrap_or(1).min(most);
                    (Some(matcher), Some(Watch { pattern: watch.pattern.clone(), max_wakes }))
                }
                Err(text) => return ToolOutcome::error(text),
            },
            None => (None, None),
        };
        let context = self.context(&call.context);
        let declared = match self.registry.requirements(&call.name, &context, &call.input) {
            Ok(declared) => declared,
            Err(error) => return ToolOutcome::error(for_model(&error)),
        };
        let Some(command) = declared.command.clone() else {
            return ToolOutcome::error("this call has no command line");
        };
        let cwd = match call.context.shell_cwd.clone() {
            Some(dir) => dir,
            None => self.shell_cwd(conversation).await.unwrap_or_else(|| call.context.cwd.clone()),
        };
        let env = match self.job_env(sandbox, conversation, &cwd).await {
            Ok(env) => env,
            Err(text) => return ToolOutcome::error(text),
        };
        let number = match jobs.number(conversation).await {
            Ok(number) => number,
            Err(error) => return ToolOutcome::error(efr_stdx::with_causes(&error)),
        };
        let id = JobId::nth(number);
        let launch = match &call.context.launch {
            Launch::Direct => Launch::Unsandboxed,
            launch => launch.clone(),
        };
        // NOTE: a job in the exit child gets efrd's roots (DV47); a contained one never.
        let env = jobs.job_env(env, launch == Launch::Unsandboxed);
        let output_path =
            call.context.scratch.join(efr_sandbox::JOB_DIR).join(output::file_name(&id));
        let now = self.clock.now();
        let info = JobInfo {
            id: id.clone(),
            conversation_id: conversation,
            turn_id: call.context.turn_id,
            call_id: call.context.call_id,
            command: command.clone(),
            launch,
            state: JobState::Running,
            started_at: now,
            ends_by: now,
            ended_at: None,
            exit_code: None,
            stopped_by: None,
            output: output_path.clone(),
            output_bytes: 0,
            watch,
            wakes: 0,
            forwards: Vec::new(),
        };
        let mut named_paths: Vec<PathBuf> =
            declared.paths.iter().map(|access| access.path.clone()).collect();
        named_paths.extend(declared.command_dir.clone());
        named_paths.extend(self.covered_root_paths(&call.context));
        let engine = Arc::clone(&engine.borrow());
        // NOTE: a job gets the snapshot of its start and keeps it.
        let mount_view = efr_permissions::exits::mount_view_line(&command);
        let input = PrepareInput { settings: &settings, engine: &engine, named_paths, mount_view };
        let limit_secs = u64::from(minutes) * 60;
        // NOTE: the launch of a job outside `auto` is the exit child, which gets no
        // forwards; the plan names the socket that efrd binds below.
        let socket = jobs
            .call_dir(conversation, call.context.call_id)
            .join(efr_sandbox::FORWARD_SOCKET_FILE);
        let forward_plan =
            forward::plan_of(&settings.jobs, &info.launch, engine.support().egress, socket);
        let job_input = JobInput {
            plan: JobPlan {
                forward: forward_plan.clone(),
                ..JobPlan::new(limit_secs, settings.jobs.nice)
            },
            line: &command,
            env: &env,
        };
        let prepared = match sandbox.prepare_job(&call.context, &input, job_input).await {
            Ok(prepared) => prepared,
            Err(error) => return prepare_failed(sandbox, &settings, &error, &call),
        };
        // NOTE: the socket exists before the launcher starts: the plan binds it into the
        // job, and refuses a forward plan without it.
        let forward = match forward_plan {
            Some(plan) => match forward::bind(&prepared.run.dir, plan, generation).await {
                Ok(socket) => Some(socket),
                Err(error) => {
                    tracing::warn!(%error, call_id = %call.context.call_id, "a job's forward socket could not be bound");
                    super::remove_call_dir(prepared.run.dir.clone()).await;
                    let reason = efr_stdx::with_causes(&error);
                    return ToolOutcome::error(format!(
                        "[the job could not start: {reason}. The command did not run.]"
                    ));
                }
            },
            None => None,
        };
        let forwards = forward.is_some();
        let new = NewJob {
            info,
            limit: std::time::Duration::from_secs(limit_secs),
            call_dir: prepared.run.dir.clone(),
            state_dir: prepared.state_dir.clone(),
            cwd,
            scratch: call.context.scratch.clone(),
            matcher,
            wakes: 0,
            guard_roots: prepared.guard_roots.clone(),
            write_roots: prepared.write_roots.clone(),
            contained: prepared.contained,
            proxy: None,
            bus: None,
            forward,
            full: call.context.is_full(),
        };
        let new =
            NewJob { wakes: new.info.watch.as_ref().map_or(0, |watch| watch.max_wakes), ..new };
        let notes = prepared.notes.clone();
        // NOTE: before the launch moves the proxy into the job's entry: the turn's trace
        // names the network of a turn that only starts a job (v0.0.4, DV21).
        let summary = prepared.contained.then(|| SandboxSummary {
            confined: true,
            proxied: prepared.proxy.is_some(),
            ..SandboxSummary::default()
        });
        let launch = JobLaunch {
            jobs: jobs.clone(),
            snapshots: self.snapshots.clone(),
            asks: self.covered_root_asks(&call.context),
            sandbox: sandbox.clone(),
            settings: Arc::clone(&settings),
            home: self.home.clone(),
            context: call.context.clone(),
            prepared,
            new,
            now,
            slot,
        };
        // NOTE: in a task of its own: an interrupt drops this call's future, and a start
        // cut in half would leave a launcher model-side, a snapshot pinned, or a job that
        // runs with no record or a record with no job. A job that starts after Esc runs
        // on, as every job does after Esc, and shows in `efr jobs`.
        let (info, changes) = match tokio::spawn(launch.run()).await {
            Ok(Ok(started)) => started,
            Ok(Err(outcome)) => return *outcome,
            Err(error) => {
                tracing::error!(%error, "the start of a job panicked");
                return ToolOutcome::error(
                    "[the job could not start: efr failed. The command did not run.]",
                );
            }
        };
        let mut text = job_started_text(
            id.as_str(),
            &command,
            &output_path.display().to_string(),
            minutes,
            clamped,
        );
        if forwards {
            text = format!("{text}\n{}", forward::START_NOTE);
        }
        if !notes.is_empty() {
            text = format!("{text}\n{}", notes.join("\n"));
        }
        let mut outcome = ToolOutcome::ok(text).with_job(info).with_sandbox(summary);
        outcome.changes = changes;
        outcome
    }

    /// The hidden shell's environment for a job of `conversation`, as the bytes of an
    /// `env` file: the test seam's, else the file that the zsh integration writes at
    /// each prompt. A shell with no file runs one line ([`ENV_REFRESH`]) to get a prompt;
    /// with no shell, efrd opens one in `start_dir` and waits for its first prompt.
    async fn job_env(
        &self,
        sandbox: &crate::sandbox::SandboxService,
        conversation: efr_protocol::ConversationId,
        start_dir: &Path,
    ) -> Result<Vec<u8>, String> {
        if let Some(env) = &self.job_env {
            return Ok(output::env_bytes(env));
        }
        let file = sandbox.shell_dir(conversation).join(efr_sandbox::ENV_FILE);
        // NOTE: the integration writes the file right after the prompt's marks, so a
        // read after a prompt looks again for a moment, in real time, as the wait for a
        // launcher's `started` does.
        let read = |file: PathBuf, wait: bool| async move {
            tokio::task::spawn_blocking(move || {
                let begin = std::time::Instant::now();
                loop {
                    let bytes =
                        crate::sandbox::fs::DaemonFs::read(&file, efr_sandbox::MAX_ENV_BYTES)
                            .ok()
                            .filter(|bytes| !bytes.is_empty());
                    if bytes.is_some() || !wait || begin.elapsed() >= ENV_FILE_WAIT {
                        return bytes;
                    }
                    std::thread::sleep(ENV_FILE_POLL);
                }
            })
            .await
            .ok()
            .flatten()
        };
        if let Some(bytes) = read(file.clone(), false).await {
            return Ok(bytes);
        }
        if self.shells.state(conversation).await.is_err() {
            if let Err(error) = self.shells.open(conversation, start_dir).await {
                tracing::info!(%conversation, %error, "a hidden shell for a job's environment did not start");
                return Err(ENV_NOT_READY.to_owned());
            }
            let _ = self.shells.until_free(conversation, ENV_WAIT).await;
            if let Some(bytes) = read(file.clone(), true).await {
                return Ok(bytes);
            }
        }
        // NOTE: the integration writes the file at a prompt only when the environment
        // changed since it wrote it last; a line that empties its own record of the last
        // write makes the next prompt write the file again, also one that went away.
        let request = RunRequest::new(ENV_REFRESH, start_dir);
        if let Err(error) = self.shells.run_command(conversation, request, &mut NoProgress).await {
            tracing::info!(%conversation, %error, "the line for a job's environment did not run");
        }
        read(file, true).await.ok_or_else(|| ENV_NOT_READY.to_owned())
    }
}

/// The part of a job's start that must not stop half way: from the guard check and
/// the snapshot under the plan lock to `job_started` and the job's task.
struct JobLaunch {
    jobs: JobTable,
    snapshots: Option<CallSnapshots>,
    /// The snapshot asks that the call's facts said are covered.
    asks: Vec<CoverAsk>,
    sandbox: crate::sandbox::SandboxService,
    settings: Arc<efr_config::Settings>,
    home: Home,
    context: CallContext,
    prepared: Prepared,
    new: NewJob,
    now: jiff::Timestamp,
    /// The job's place in the limits, until it runs or did not start.
    slot: Slot,
}

impl JobLaunch {
    /// Starts the job; `Err` is the outcome of a job that did not start.
    async fn run(self) -> Result<(JobInfo, Option<FileChanges>), Box<ToolOutcome>> {
        let JobLaunch {
            jobs,
            snapshots,
            asks,
            sandbox,
            settings,
            home,
            context,
            mut prepared,
            mut new,
            now,
            slot,
        } = self;
        // NOTE: the job's socket and bus filters live in its entry of the job table from
        // its start to its end; a job that does not start drops them here, which closes
        // them.
        new.proxy = prepared.proxy.take();
        new.bus = prepared.bus.take();
        // NOTE: before the launcher records the surface, so its guard never takes an
        // entry that a running job planted in these roots as its baseline.
        if prepared.contained {
            jobs.check_guards(Some(&prepared.guard_roots)).await;
        }
        let before = |snapshots: Option<CallSnapshots>| {
            let context = context.clone();
            let projects = prepared.projects.clone();
            async move {
                match snapshots {
                    Some(snapshots) => snapshots.before_call(&context, &projects).await,
                    None => None,
                }
            }
        };
        let snapshot = before(snapshots.clone()).await;
        let lost =
            super::recheck_covered(snapshots.as_ref(), &context, &asks, &prepared.projects).await;
        if let Some(stop) = lost {
            if let Some((taken, _)) = snapshot {
                taken.discard();
            }
            super::remove_call_dir(prepared.run.dir.clone()).await;
            return Err(Box::new(ToolOutcome::error(stop)));
        }
        sandbox
            .turns()
            .before_call(
                context.turn_id,
                now,
                &prepared.projects,
                &settings.sandbox.surface_files,
                sandbox.git(),
                &home,
            )
            .await;
        let mut spawned = match jobs.spawn(&new) {
            Ok(spawned) => spawned,
            Err(error) => {
                if let Some((taken, _)) = snapshot {
                    taken.discard();
                }
                super::remove_call_dir(prepared.run.dir.clone()).await;
                return Err(Box::new(ToolOutcome::error(format!(
                    "[the job could not start: {error}. The command did not run.]"
                ))));
            }
        };
        tracing::debug!(job = %new.info.id, pid = spawned.pid(), "a job's launcher waits for its start byte");
        if let Err(text) = jobs.start(&mut spawned, &new).await {
            if let Some((taken, _)) = snapshot {
                taken.discard();
            }
            return Err(Box::new(ToolOutcome::error(text)));
        }
        // NOTE: the launcher opened every bind source; the job blocks no plan any more.
        drop(prepared.guard);
        let snapshot = snapshot.map(|(taken, limits)| (taken.runs_on(true), limits));
        let changes = match &snapshots {
            Some(snapshots) => snapshots.after_call(snapshot).await,
            None => None,
        };
        let kept = before(snapshots).await;
        let info = match jobs.begin(spawned, new, kept).await {
            Ok(info) => info,
            Err(error) => return Err(Box::new(ToolOutcome::error(started_unrecorded(&error)))),
        };
        drop(slot);
        Ok((info, changes))
    }
}

/// What the model reads for a job whose call dir could not be prepared.
fn prepare_failed(
    sandbox: &crate::sandbox::SandboxService,
    settings: &Arc<efr_config::Settings>,
    error: &DaemonError,
    call: &ToolCall,
) -> ToolOutcome {
    tracing::warn!(error = %error, call_id = %call.context.call_id, "a job could not be prepared");
    if matches!(error, DaemonError::SandboxUnavailable { .. }) {
        sandbox.reprobe(Arc::clone(settings));
    }
    let reason = efr_stdx::with_causes(error);
    let summary = SandboxSummary { setup_error: Some(reason.clone()), ..SandboxSummary::default() };
    ToolOutcome::error(format!("[the job could not start: {reason}. The command did not run.]"))
        .with_sandbox(Some(summary))
}

/// What the model reads when the store did not take a job's start: efr stopped it.
fn started_unrecorded(error: &DaemonError) -> String {
    format!(
        "[the job started, but efr could not record it ({}), so efr stopped it. Try again.]",
        efr_stdx::with_causes(error)
    )
}

/// The input of the tool `name`, or the text that the model reads.
fn parse<T: for<'de> Deserialize<'de>>(name: &str, input: &Value) -> Result<T, String> {
    T::deserialize(input)
        .map_err(|error| format!("the input of the {name} tool does not match its schema: {error}"))
}

#[cfg(test)]
mod tests;
