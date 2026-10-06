//! The exit child (`Launch::Unsandboxed`, the spec's section 3.16): an approved
//! privilege, upload, persistence or outside run, without bwrap, Landlock or seccomp.
//!
//! It runs `zsh -f child.zsh DIR` with the trusted environment, where `DIR` holds only
//! the trusted `snapshot.zsh` and the line: never `state.zsh`, because nothing made in
//! the sandbox runs unsandboxed. It is a job of the same session on the same PTY, so
//! the input relay works as for any job. The launcher makes itself a child subreaper
//! first: orphans of the exit child, also after `setsid` or a double fork, reparent to
//! it. When the exit child ends, every descendant gets SIGTERM, SIGKILL after 2 s, and
//! only then is `result.json` written, so nothing of the call can read the next line
//! typed into the PTY.

use std::collections::BTreeSet;
use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use efr_sandbox::{LINE_FILE, SNAPSHOT_FILE, STARTED_FILE, SandboxResult};
use rustix::pipe::PipeFlags;
use rustix::process::{Pid, Signal, WaitOptions};

use crate::call;
use crate::call_dir::CallDir;
use crate::error::SbxError;
use crate::finish::{self, FinalCwd};
use crate::launch;
use crate::os;
use crate::real_fs::RealFs;

/// How long the descendants get between SIGTERM and SIGKILL.
const GRACE: Duration = Duration::from_secs(2);
/// How often the launcher looks for exited descendants while it waits.
const POLL: Duration = Duration::from_millis(10);
/// The dir in `$CALL` that holds the exit child's copies.
const EXIT_DIR: &str = "exit-child";
/// The most bytes of a snapshot or a line that the launcher copies.
const MAX_COPY_BYTES: usize = 64 * 1024 * 1024;

/// Runs the exit child; `records_slot` is the launcher's fd 3, which becomes the
/// child's records pipe.
pub(crate) fn run(call: &CallDir, mut records_slot: OwnedFd) -> Result<SandboxResult, SbxError> {
    let spec = &call.spec;
    rustix::process::set_child_subreaper(Some(rustix::process::getpid()))
        .map_err(|error| SbxError::os("become a child subreaper", error.into()))?;
    let dir = prepare_dir(call)?;
    let pwd = call::shell_pwd()?;
    let (records_read, records_write) = rustix::pipe::pipe_with(PipeFlags::CLOEXEC)
        .map_err(|error| SbxError::os("make the records pipe", error.into()))?;
    // dup2 clears close-on-exec, so the child gets the pipe at fd 3 and nothing else.
    rustix::io::dup2(&records_write, &mut records_slot)
        .map_err(|error| SbxError::os("put the records pipe on fd 3", error.into()))?;
    drop(records_write);
    call.dir.touch(STARTED_FILE)?;
    let spawned = os::command(&spec.runtime.zsh)
        .arg("-f")
        .arg(&spec.runtime.child_script)
        .arg(&dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();
    drop(records_slot);
    let mut child =
        spawned.map_err(|source| SbxError::Spawn { program: spec.runtime.zsh.clone(), source })?;
    let records = launch::reader("efr-sbx-records", records_read, spec.limits.max_bytes)?;
    let status = child.wait().map_err(|error| SbxError::os("wait for the exit child", error))?;
    let stopped = end_descendants();
    let records = call::exit_records(launch::join(records)?, call);
    let mut result = SandboxResult {
        started: true,
        exit_code: status.code(),
        signal: status.signal(),
        cwd: Some(pwd.clone()),
        state_kept: records.is_some(),
        ..SandboxResult::default()
    };
    result.summary.confined = false;
    result.summary.background_stopped = stopped;
    if let Some(records) = &records {
        let cd = match records.cwd.as_deref().map(|cwd| finish::exit_child_cwd(cwd, spec, &RealFs))
        {
            Some(FinalCwd::Host(path)) if path != pwd => Some(path),
            _ => None,
        };
        if let Some(cd) = &cd {
            result.cwd = Some(cd.clone());
            result.summary.cwd_changed = true;
        }
        let final_cwd = cd.clone().unwrap_or_else(|| pwd.clone());
        let filter = finish::exit_child_filter(spec, &final_cwd);
        let promotion = finish::promote(records, cd, &filter, &RealFs, &mut result.summary);
        call::write_apply(call, &promotion)?;
    }
    Ok(result)
}

/// `$CALL/exit-child/` with copies of the trusted snapshot (and its compiled form) and
/// of the line; no state.
fn prepare_dir(call: &CallDir) -> Result<PathBuf, SbxError> {
    let spec = &call.spec;
    let dir = call.dir.path().join(EXIT_DIR);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|error| SbxError::io("create", &dir, error))?;
    let copies = [
        (spec.runtime.shell_dir.join(SNAPSHOT_FILE), SNAPSHOT_FILE),
        (spec.runtime.shell_dir.join(format!("{SNAPSHOT_FILE}.zwc")), "snapshot.zsh.zwc"),
        (call.dir.path().join(LINE_FILE), LINE_FILE),
    ];
    for (source, name) in copies {
        let Some(bytes) = call::read_trusted(&source, MAX_COPY_BYTES) else { continue };
        let target = dir.join(name);
        fs::write(&target, bytes).map_err(|error| SbxError::io("write", &target, error))?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .map_err(|error| SbxError::io("protect", &target, error))?;
    }
    Ok(dir)
}

/// Every descendant of this process, as `(pid, name)`, found through the `children`
/// lists of `/proc`.
fn descendants() -> Vec<(Pid, String)> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let mut todo = vec![rustix::process::getpid().as_raw_nonzero().get()];
    while let Some(pid) = todo.pop() {
        let tasks = Path::new("/proc").join(pid.to_string()).join("task");
        let Ok(entries) = fs::read_dir(&tasks) else { continue };
        for task in entries.flatten() {
            let Ok(children) = fs::read_to_string(task.path().join("children")) else { continue };
            for child in children.split_whitespace().filter_map(|word| word.parse::<i32>().ok()) {
                if !seen.insert(child) {
                    continue;
                }
                let name = fs::read_to_string(format!("/proc/{child}/comm")).unwrap_or_default();
                if let Some(pid) = Pid::from_raw(child) {
                    found.push((pid, name.trim().to_owned()));
                }
                todo.push(child);
            }
        }
    }
    found
}

/// Reaps every child that exited, without waiting. `wait`, not `waitpid(None)`: an
/// orphan that called `setsid` is in another process group, which `waitpid(0)` skips.
fn reap() {
    while let Ok(Some(_)) = rustix::process::wait(WaitOptions::NOHANG) {}
}

/// Ends every descendant: SIGTERM, up to [`GRACE`] for them to go, then SIGKILL, and
/// reaps them all. Returns their names, each once, in the order found.
fn end_descendants() -> Vec<String> {
    reap();
    let first = descendants();
    let mut names: Vec<String> = Vec::new();
    for (_, name) in &first {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    if first.is_empty() {
        return names;
    }
    for (pid, _) in &first {
        let _ = rustix::process::kill_process(*pid, Signal::TERM);
    }
    let start = Instant::now();
    while start.elapsed() < GRACE {
        reap();
        if descendants().is_empty() {
            return names;
        }
        std::thread::sleep(POLL);
    }
    loop {
        let left = descendants();
        if left.is_empty() {
            break;
        }
        for (pid, _) in &left {
            let _ = rustix::process::kill_process(*pid, Signal::KILL);
        }
        // A killed child is reaped here; one that is not a direct child is reaped by
        // its own parent or, once orphaned, reparents here.
        let _ = rustix::process::wait(WaitOptions::empty());
        reap();
    }
    names
}
