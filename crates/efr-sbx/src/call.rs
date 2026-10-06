//! `efr-sbx run --call-dir DIR`: one call of the hidden shell's wrapper (the spec's
//! section 3.1, steps 7 to 11).
//!
//! The launcher checks the call dir, then runs the line contained (bwrap, Landlock,
//! seccomp) or, for an approved exit, in the exit child. At the end it writes
//! `$CALL/apply` for the trusted shell, the sandbox state, and `$CALL/result.json`
//! last, by an atomic rename, after every process of the call is gone. It exits with
//! the call's status. It prints nothing to the terminal except when the call dir itself
//! is not to be trusted, because then it cannot write a result.

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use efr_sandbox::{
    APPLY_FILE, EnvFilter, ExportFilter, FsView, MAX_STATE_BYTES, MountPlan, NetworkPlan,
    RESULT_FILE, Records, STARTED_FILE, STATE_JSON_FILE, STATE_ZSH_FILE, SandboxResult,
    SandboxState, SpecLaunch, StartDir, encode_apply, parse_records,
};

use crate::call_dir::{CallDir, PrivateDir};
use crate::error::SbxError;
use crate::finish::{self, FinalCwd};
use crate::guard::{self, Guard};
use crate::launch::{self, Ending, Launch};
use crate::real_fs::RealFs;
use crate::{exit_child, fds, signals};

/// The number the records pipe has in every child shell.
pub(crate) const RECORDS_FD: i32 = efr_sandbox::RECORDS_FD;

/// Runs `efr-sbx run`.
pub(crate) fn main(call_dir: &Path) -> ExitCode {
    match run(call_dir) {
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

fn run(call_dir: &Path) -> Result<i32, SbxError> {
    let reserved = reserve_fds()?;
    // The handlers only set a flag; holding them keeps the launcher alive through
    // Ctrl+C so it can write the result.
    signals::catch_job_control()?;
    let call = CallDir::open(call_dir)?;
    let result = match call.spec.launch {
        SpecLaunch::Contained => {
            drop(reserved);
            contained(&call).unwrap_or_else(|error| setup_failure(error.chain()))
        }
        SpecLaunch::Unsandboxed => {
            exit_child::run(&call, reserved).unwrap_or_else(|error| setup_failure(error.chain()))
        }
        _ => setup_failure("this launcher does not know the spec's launch kind".to_owned()),
    };
    call.dir.write_atomic(RESULT_FILE, &result.to_json()?)?;
    Ok(result.status())
}

/// A result for a call whose sandbox did not start.
pub(crate) fn setup_failure(reason: String) -> SandboxResult {
    SandboxResult { setup_error: Some(reason), ..SandboxResult::default() }
}

/// The trusted shell's directory, as the kernel names it.
pub(crate) fn shell_pwd() -> Result<PathBuf, SbxError> {
    std::env::current_dir().map_err(|error| SbxError::os("read the working directory", error))
}

fn contained(call: &CallDir) -> Result<SandboxResult, SbxError> {
    let mut spec = call.spec.clone();
    if let NetworkPlan::Proxy { .. } = spec.network {
        return Err(SbxError::LaterPhase { what: "the network proxy" });
    }
    // The launcher runs with the trusted shell's environment, so its PATH is the
    // hidden shell's PATH; efrd's copy only served its own check before the call.
    let env = launch::own_env();
    spec.shell_path = launch::env_value(&env, "PATH")
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    prepare_sandbox_dir(&spec)?;
    let fs = RealFs;
    let plan = match MountPlan::build(&spec, &fs) {
        Ok(plan) => plan,
        Err(error) => {
            return Ok(setup_failure(format!("the sandbox cannot run here: {error}")));
        }
    };
    let shell_dir = PrivateDir::open(&spec.runtime.shell_dir)?;
    let mut state = match shell_dir.read(STATE_JSON_FILE, MAX_STATE_BYTES)? {
        Some(bytes) => SandboxState::from_json(&bytes).unwrap_or_default(),
        None => SandboxState::default(),
    };
    let pwd = shell_pwd()?;
    let start = plan.start_dir(&pwd, state.sandbox_cwd.as_ref(), &spec.runtime.scratch, &fs);
    let start_host = match &start {
        StartDir::Private { path } => plan.private_host_path(path).unwrap_or_else(|| path.clone()),
        other => other.path().to_path_buf(),
    };
    let guard = Guard::before(&spec, &plan, &start_host);
    let (env, removed) = EnvFilter::new(&spec, &plan).apply(&env);
    let mut argv = plan.child_argv().to_vec();
    argv.push(spec.runtime.inside_dir().into_os_string());
    let launch = Launch { spec: &spec, plan: &plan, cwd: start.path(), argv, env, stdout: None };
    let outcome = launch::run(&launch, &mut || call.dir.touch(STARTED_FILE))?;
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
    let code = match outcome.ending {
        Ending::SetupFailed { reason, .. } => {
            result.setup_error = Some(reason);
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
        result.summary.cwd_changed = cd.is_some();
        let filter = ExportFilter::from_spec(&spec, &plan, &final_host);
        let promotion = finish::promote(records, cd, &filter, &fs, &mut result.summary);
        finish::update_state(&mut state, records, &cwd, &pwd);
        write_apply(call, &promotion)?;
        shell_dir.write_atomic(STATE_JSON_FILE, &state.to_json()?)?;
        shell_dir.write_atomic(STATE_ZSH_FILE, state.render().as_bytes())?;
        result.state_kept = true;
    }
    let mut changes = guard.after(&final_host);
    guard::quarantine(&mut changes, &spec.runtime.quarantine(spec.call))?;
    result.summary.surface_changes = changes;
    Ok(result)
}

/// Makes the conversation's private tmp and the cache layers when they are missing:
/// they lie in `$SBX`, in efr's state root, where no call can reach.
fn prepare_sandbox_dir(spec: &efr_sandbox::SandboxSpec) -> Result<(), SbxError> {
    let mut dirs = vec![spec.runtime.private_tmp()];
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
