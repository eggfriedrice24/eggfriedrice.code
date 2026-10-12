//! `efr-sbx run --call-dir DIR`: one call of the hidden shell's wrapper (the spec's
//! section 3.1, steps 7 to 11), or with `--job` one job that efrd starts itself
//! (`job.rs`).
//!
//! The launcher checks the call dir, then runs the line contained (bwrap, Landlock,
//! seccomp) or, for an approved exit, in the exit child. At the end it writes
//! `$CALL/apply` for the trusted shell, the sandbox state, and `$CALL/result.json`
//! last, by an atomic rename, after every process of the call is gone. It exits with
//! the call's status. It prints nothing to the terminal except when the call dir itself
//! is not to be trusted, because then it cannot write a result. A job writes no apply
//! and no state, and its spec must have a job plan; a call's spec must not.

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, Sender};

use efr_sandbox::{
    APPLY_FILE, EnvFilter, ExportFilter, FsView, JobPlan, MAX_STATE_BYTES, MountPlan, NetworkPlan,
    RESULT_FILE, Records, STARTED_FILE, STATE_JSON_FILE, STATE_ZSH_FILE, SandboxResult,
    SandboxState, SpecLaunch, StartDir, encode_apply, parse_records,
};

use crate::call_dir::{CallDir, PrivateDir};
use crate::error::SbxError;
use crate::finish::{self, FinalCwd};
use crate::guard::{self, Guard};
use crate::job::{self, Job};
use crate::launch::{self, Ending, Launch};
use crate::layer_lock::{self, LAYER_LOCK_WAIT, Locked};
use crate::real_fs::RealFs;
use crate::timings::Timings;
use crate::{deletes, exit_child, fds, signals};

/// The number the records pipe has in every child shell.
pub(crate) const RECORDS_FD: i32 = efr_sandbox::RECORDS_FD;

/// Runs `efr-sbx run`, with `job` for `--job`.
pub(crate) fn main(call_dir: &Path, job: bool) -> ExitCode {
    match run(call_dir, job) {
        Ok(status) => ExitCode::from(u8::try_from(status).unwrap_or(1)),
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "efr-sbx: {}", error.chain());
            ExitCode::from(125)
        }
    }
}

/// Makes fd 3 the launcher's own, so every descriptor it opens later is 4 or more and
/// the exit child's records pipe can take 3. Every inherited descriptor from 3 up is
/// marked close-on-exec first.
pub(crate) fn reserve_fds() -> Result<OwnedFd, SbxError> {
    fds::mark_inherited_cloexec(3)
        .map_err(|error| SbxError::os("mark the inherited descriptors", error))?;
    let null = File::open("/dev/null").map_err(|error| SbxError::io("open", "/dev/null", error))?;
    if null.as_raw_fd() == RECORDS_FD {
        // Descriptor 3 was free, and the kernel hands out the lowest free number.
        return Ok(OwnedFd::from(null));
    }
    fds::dup_to(null.as_fd(), RECORDS_FD, true)
        .map_err(|error| SbxError::os("reserve descriptor 3", error))
}

fn run(call_dir: &Path, job: bool) -> Result<i32, SbxError> {
    // The handlers only set a flag; holding them keeps the launcher alive through
    // Ctrl+C so it can write the result. They come first, so that the window in which
    // Ctrl+C ends the launcher itself is as short as it can be (efr-shell reports
    // that case as an interrupt too).
    signals::catch_job_control()?;
    if job {
        job::new_session()?;
    }
    let reserved = reserve_fds()?;
    let events = if job { Some(job::catch_stops()?) } else { None };
    let call = CallDir::open(call_dir)?;
    let result = match (events, call.spec.job) {
        (None, None) => launch(&call, reserved, None),
        (Some(events), Some(plan)) => run_job(&call, reserved, plan, events),
        (None, Some(_)) => mismatch("a job's spec needs run --job"),
        (Some(_), None) => mismatch("run --job needs a job's spec"),
    };
    let result = match signals::interrupted() {
        Some(signal) if result.setup_error.is_some() => stopped_before_run(result, signal),
        _ => result,
    };
    call.dir.write_atomic(RESULT_FILE, &result.to_json()?)?;
    Ok(result.status())
}

/// Runs the call's line contained or in the exit child, as a job when `job` is set.
fn launch(call: &CallDir, reserved: OwnedFd, job: Option<&Job>) -> SandboxResult {
    match call.spec.launch {
        SpecLaunch::Contained => {
            drop(reserved);
            contained(call, job).unwrap_or_else(|error| failure(call, error.chain()))
        }
        SpecLaunch::Unsandboxed => exit_child::run(call, reserved, job)
            .unwrap_or_else(|error| failure(call, error.chain())),
        _ => setup_failure("this launcher does not know the spec's launch kind".to_owned()),
    }
}

/// Runs a job: waits for its start byte, lowers its priority, starts its stop thread,
/// runs it, and names in the result why it was stopped and the nice value it got.
fn run_job(
    call: &CallDir,
    reserved: OwnedFd,
    plan: JobPlan,
    events: (Sender<job::Event>, Receiver<job::Event>),
) -> SandboxResult {
    match job::wait_for_start() {
        Ok(true) => {}
        Ok(false) => {
            return setup_failure("the lifeline ended before the job's start byte".to_owned());
        }
        Err(error) => return setup_failure(error.chain()),
    }
    let contained = call.spec.launch == SpecLaunch::Contained;
    let mut job = match Job::start(call, plan, events, contained) {
        Ok(job) => job,
        Err(error) => return setup_failure(error.chain()),
    };
    let mut result = launch(call, reserved, Some(&job));
    result.stop = job.finish();
    result.nice = Some(job.nice);
    result
}

/// A result for a launcher whose mode does not fit the spec: it starts nothing.
fn mismatch(problem: &'static str) -> SandboxResult {
    setup_failure(SbxError::JobMode { problem }.chain())
}

/// A result for a call whose sandbox did not start.
pub(crate) fn setup_failure(reason: String) -> SandboxResult {
    SandboxResult { setup_error: Some(reason), ..SandboxResult::default() }
}

/// The result of a call whose setup `signal` (SIGINT or SIGQUIT) broke or stopped:
/// the user stopped the call, and the sandbox did not fail. The command did not run,
/// and the status is 128 plus the signal, as for any job that Ctrl+C ends.
pub(crate) fn stopped_before_run(result: SandboxResult, signal: i32) -> SandboxResult {
    SandboxResult {
        started: result.started,
        signal: Some(signal),
        timings: result.timings,
        ..SandboxResult::default()
    }
}

/// Fails with [`SbxError::Interrupted`] when SIGINT or SIGQUIT came: the launcher then
/// does not start the call. It runs right before bwrap or the exit child starts.
pub(crate) fn stop_if_interrupted() -> Result<(), SbxError> {
    match signals::interrupted() {
        Some(signal) => Err(SbxError::Interrupted { signal }),
        None => Ok(()),
    }
}

/// A result for a launcher error: a setup failure before `$CALL/started`, and after it
/// a launch error, because bwrap or the exit child was started and the command may
/// have run.
fn failure(call: &CallDir, reason: String) -> SandboxResult {
    if call.dir.path().join(STARTED_FILE).exists() {
        SandboxResult { started: true, launch_error: Some(reason), ..SandboxResult::default() }
    } else {
        setup_failure(reason)
    }
}

/// The trusted shell's directory, as the kernel names it.
pub(crate) fn shell_pwd() -> Result<PathBuf, SbxError> {
    std::env::current_dir().map_err(|error| SbxError::os("read the working directory", error))
}

fn contained(call: &CallDir, job: Option<&Job>) -> Result<SandboxResult, SbxError> {
    let mut timings = Timings::start();
    let mut spec = call.spec.clone();
    if let NetworkPlan::Proxy { .. } = spec.network {
        return Err(SbxError::LaterPhase { what: "the network proxy" });
    }
    // The launcher runs with the trusted shell's environment, so its PATH is the
    // hidden shell's PATH; efrd's copy only served its own check before the call. A
    // job's launcher runs with efrd's environment, and the shell's is in `$CALL/env`.
    let env = match job {
        Some(job) => job.env.clone(),
        None => launch::own_env(),
    };
    spec.shell_path = launch::env_value(&env, "PATH")
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    prepare_sandbox_dir(&spec)?;
    timings.lap("prepare_dirs");
    let fs = RealFs;
    let plan = match MountPlan::build(&spec, &fs) {
        Ok(plan) => plan,
        Err(error) => {
            return Ok(setup_failure(format!("the sandbox cannot run here: {error}")));
        }
    };
    timings.lap("plan");
    let shell_dir = PrivateDir::open(&spec.runtime.shell_dir)?;
    let mut state = match shell_dir.read(STATE_JSON_FILE, MAX_STATE_BYTES)? {
        Some(bytes) => SandboxState::from_json(&bytes).unwrap_or_default(),
        None => SandboxState::default(),
    };
    timings.lap("state_read");
    let pwd = shell_pwd()?;
    let start = plan.start_dir(&pwd, state.sandbox_cwd.as_ref(), &spec.runtime.scratch, &fs);
    let start_host = match &start {
        StartDir::Private { path } => plan.private_host_path(path).unwrap_or_else(|| path.clone()),
        other => other.path().to_path_buf(),
    };
    let mut guard = Guard::before(&spec, &plan, &start_host, &shell_dir);
    // A job keeps the guard's record of its start on disk, for efr-sbx guard-check.
    let job_state = match job {
        Some(_) => {
            let dir = PrivateDir::open(&spec.runtime.job_state_dir(spec.call))?;
            let places = guard::JobPlaces {
                start: start_host.clone(),
                quarantine: spec.runtime.quarantine(spec.call),
            };
            guard.save(&dir, &places)?;
            Some(dir)
        }
        None => None,
    };
    timings.lap("guard_before");
    let (env, removed) = EnvFilter::new(&spec, &plan).apply(&env);
    let mut argv = plan.child_argv().to_vec();
    argv.push(spec.runtime.inside_dir().into_os_string());
    let launch = Launch {
        spec: &spec,
        plan: &plan,
        cwd: start.path(),
        argv,
        env,
        stdout: None,
        job: job.is_some(),
    };
    // NOTE: the overlays have no index, so the kernel would let two of them share an
    // upper dir; the lock holds until launch::run has torn the call's overlays down.
    let lock_dirs = layer_lock::lock_dirs(plan.layer_dirs());
    let layers_lock = match layer_lock::lock(&lock_dirs, LAYER_LOCK_WAIT)? {
        Locked::Held(lock) => lock,
        Locked::Busy { dir } => {
            return Ok(setup_failure(format!(
                "another call of this conversation still used its cache layers in {} after {} s",
                dir.display(),
                LAYER_LOCK_WAIT.as_secs()
            )));
        }
    };
    timings.lap("layer_lock");
    let mut outcome = launch::run(&launch, &mut || {
        stop_if_interrupted()?;
        if let Some(job) = job {
            job.stop_if_stopped()?;
        }
        call.dir.touch(STARTED_FILE)
    })?;
    timings.append(std::mem::take(&mut outcome.timings));
    // NOTE: under the layer lock, so in the `overlay` mode no other call of the
    // conversation changes an upper while the walk reads it. A job's cache deletes are
    // never applied, so the launcher does not list them.
    let deletes = match (&outcome.ending, plan.cache_layers()) {
        (Ending::Ran { .. }, Some(layers)) if job.is_none() => {
            Some(deletes::list(&spec, layers, std::mem::take(&mut outcome.uppers)))
        }
        _ => None,
    };
    drop(std::mem::take(&mut outcome.uppers));
    drop(layers_lock);
    timings.lap("cache_deletes");
    let mut result = SandboxResult {
        started: true,
        env_removed: removed,
        hidden_cwd: match &start {
            StartDir::Scratch { hidden, .. } => Some(hidden.clone()),
            _ => None,
        },
        cwd: Some(pwd.clone()),
        ..SandboxResult::default()
    };
    result.summary.confined = true;
    if let Some(deletes) = deletes {
        deletes.fill(&mut result.summary);
    }
    let code = match outcome.ending {
        Ending::SetupFailed { reason, .. } => {
            result.setup_error = Some(reason);
            result.timings = timings.into_steps();
            return Ok(result);
        }
        Ending::Ran { code } => code,
    };
    result.exit_code = Some(code);
    let records = outcome.records.and_then(|bytes| parse_records(&bytes, &spec.limits).ok());
    let records = match records {
        Some(records) if records.setup_error.is_some() => {
            if code == efr_sandbox::SETUP_FAILURE_STATUS {
                result.setup_error = records.setup_error;
                result.exit_code = None;
                result.timings = timings.into_steps();
                return Ok(result);
            }
            None
        }
        other => other,
    };
    if records.is_none() && code > 128 {
        result.signal = Some(code - 128);
    }
    let mut final_host = start_host;
    if let Some(records) = &records {
        let cwd = match &records.cwd {
            Some(cwd) => finish::contained_cwd(cwd, &plan, &fs),
            None => FinalCwd::Stay,
        };
        let cd = match &cwd {
            FinalCwd::Host(path) => {
                final_host.clone_from(path);
                result.cwd = Some(path.clone());
                // The shell is there already when the call ended where it started.
                (*path != pwd).then(|| path.clone())
            }
            FinalCwd::Private { inside, host } => {
                final_host.clone_from(host);
                result.cwd = Some(inside.clone());
                None
            }
            FinalCwd::Stay => None,
        };
        // A job changes no state of the hidden shell: no apply and no state files. Its
        // final dir only widens the guard's scan.
        if job.is_none() {
            result.summary.cwd_changed = cd.is_some();
            let filter = ExportFilter::from_spec(&spec, &plan, &final_host);
            let promotion = finish::promote(records, cd, &filter, &fs, &mut result.summary);
            finish::update_state(&mut state, records, &cwd, &pwd);
            // The command ran: a failure from here on loses the shell state of this
            // call, and must not turn into a setup failure.
            result.state_kept = write_apply(call, &promotion)
                .and_then(|()| shell_dir.write_atomic(STATE_JSON_FILE, &state.to_json()?))
                .and_then(|()| shell_dir.write_atomic(STATE_ZSH_FILE, state.render().as_bytes()))
                .is_ok();
        }
    }
    timings.lap("records_and_state");
    // NOTE: a job's end guard and efr-sbx guard-check take one lock, so neither takes
    // the other's half-moved quarantine as its view. The end guard runs also when the
    // lock stays busy: it must never be skipped.
    let _lock = match &job_state {
        Some(dir) => guard::lock(dir.path())?,
        None => None,
    };
    let mut changes = guard.after(&final_host);
    if job.is_none() {
        guard.keep_listings(&shell_dir);
    }
    if guard::quarantine(&mut changes, &spec.runtime.quarantine(spec.call)).is_err() {
        for change in &mut changes {
            change.quarantined = false;
        }
    }
    result.summary.surface_changes = changes;
    timings.lap("guard_after");
    result.timings = timings.into_steps();
    Ok(result)
}

/// Makes the conversation's private tmp, the cache layers and a job's state dir when
/// they are missing: they lie in `$SBX`, in efr's state root, where no call can reach.
fn prepare_sandbox_dir(spec: &efr_sandbox::SandboxSpec) -> Result<(), SbxError> {
    let mut dirs = vec![spec.runtime.private_tmp()];
    if spec.job.is_some() {
        dirs.push(spec.runtime.job_state_dir(spec.call));
    }
    if spec.cache_mode == efr_protocol::CacheMode::Overlay {
        for cache in &spec.caches {
            dirs.push(cache.upper.clone());
            dirs.push(cache.work.clone());
        }
    }
    for dir in dirs {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|error| SbxError::io("create", &dir, error))?;
    }
    Ok(())
}

/// Writes `$CALL/apply` for the trusted shell's `_efr_hs_sbx_apply`.
pub(crate) fn write_apply(call: &CallDir, promotion: &finish::Promotion) -> Result<(), SbxError> {
    let bytes = encode_apply(promotion.cd.as_deref(), &promotion.exports, &promotion.unsets);
    call.dir.write_atomic(APPLY_FILE, &bytes)
}

/// Parses records for the exit child, which never reports a setup error.
pub(crate) fn exit_records(bytes: Option<Vec<u8>>, call: &CallDir) -> Option<Records> {
    bytes
        .and_then(|bytes| parse_records(&bytes, &call.spec.limits).ok())
        .filter(|records| records.setup_error.is_none())
}

/// Reads a file through the real file system view, for the exit child's copies.
pub(crate) fn read_trusted(path: &Path, limit: usize) -> Option<Vec<u8>> {
    RealFs.read_file(path, limit).ok()
}

#[cfg(test)]
mod tests;
