//! One contained launch: bwrap with the plan's arguments on an fd, the inner policy on
//! another, and the pipes back (the spec's sections 3.2 and 3.10).
//!
//! Each attempt opens every bind source again (`FdTable`: one descriptor per bind),
//! writes the argument list and the policy to memfds, clears `FD_CLOEXEC` on exactly
//! the descriptors bwrap and the inner stage take by number, and starts bwrap with the
//! filtered environment. bwrap's stderr is a pipe to the launcher, never the terminal,
//! so its setup messages land in `setup_error` and never in the tool output. Three
//! threads drain the records, bwrap's stderr and the status stream while bwrap runs, so
//! a child that writes its records late never blocks on a full pipe.

mod status;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Stdio;
use std::thread::JoinHandle;

use efr_sandbox::{FdTable, InnerPolicy, LaunchFds, MountPlan, SandboxSpec, encode_args};
use rustix::fs::MemfdFlags;
use rustix::io::FdFlags;
use rustix::pipe::PipeFlags;

pub(crate) use self::status::{
    BwrapExit, Ending, OVERLAY_RETRY_PAUSE, ending, parse_status, retry_overlay,
};
use crate::error::SbxError;
use crate::real_fs::RealFs;
use crate::{fds, os};

/// The most bytes of bwrap's stderr and of its status stream that the launcher keeps.
const MAX_SIDE_BYTES: usize = 64 * 1024;

/// What one launch needs besides the plan.
pub(crate) struct Launch<'a> {
    /// The call's spec.
    pub(crate) spec: &'a SandboxSpec,
    /// Its checked plan.
    pub(crate) plan: &'a MountPlan,
    /// Where the call starts inside.
    pub(crate) cwd: &'a Path,
    /// What the inner stage runs once the sandbox is in place.
    pub(crate) argv: Vec<OsString>,
    /// The environment of bwrap and everything inside.
    pub(crate) env: BTreeMap<OsString, OsString>,
    /// Where the child's stdout goes; `None` keeps the launcher's.
    pub(crate) stdout: Option<OwnedFd>,
}

impl std::fmt::Debug for Launch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // NOTE: the environment's values never reach a log.
        f.debug_struct("Launch").field("cwd", &self.cwd).field("argv", &self.argv).finish()
    }
}

/// How a launch went.
#[derive(Debug)]
pub(crate) struct Outcome {
    /// The ending of the last attempt.
    pub(crate) ending: Ending,
    /// The records stream, or `None` when it passed the size limit.
    pub(crate) records: Option<Vec<u8>>,
}

/// Runs the launch, trying again while an overlay is busy. `opened` runs once, after
/// the first attempt opened every bind source and before bwrap starts: the launcher
/// writes `started` there.
pub(crate) fn run(
    launch: &Launch<'_>,
    opened: &mut dyn FnMut() -> Result<(), SbxError>,
) -> Result<Outcome, SbxError> {
    let start = os::now();
    let mut attempts = 0;
    let mut opened = Some(opened);
    loop {
        attempts += 1;
        let (ending, records) = attempt(launch, opened.take())?;
        if retry_overlay(&ending, attempts, start.elapsed()) {
            os::sleep(OVERLAY_RETRY_PAUSE);
            continue;
        }
        return Ok(Outcome { ending, records });
    }
}

fn attempt(
    launch: &Launch<'_>,
    opened: Option<&mut dyn FnMut() -> Result<(), SbxError>>,
) -> Result<(Ending, Option<Vec<u8>>), SbxError> {
    let fs = RealFs;
    let mut table = FdTable::new(&fs);
    let (status_read, status_write) = pipe("make the status pipe")?;
    let (records_read, records_write) = pipe("make the records pipe")?;
    let terminal = rustix::io::fcntl_dupfd_cloexec(std::io::stderr().as_fd(), 3)
        .map_err(|error| SbxError::os("copy the terminal", error.into()))?;
    let mut policy =
        InnerPolicy::new(launch.plan, records_write.as_raw_fd(), Some(terminal.as_raw_fd()));
    policy.argv.clone_from(&launch.argv);
    let policy = memfd("efr-sbx-policy", &policy.to_json()?)?;
    let fds = LaunchFds { status: status_write.as_raw_fd(), policy: policy.as_raw_fd() };
    let all = launch.plan.bwrap_args(&mut table, &fds, launch.cwd)?;
    // bwrap reads options from --args but drops a command that comes there, so the
    // command after `--` stays on the command line; it holds no path of the plan.
    let split = all.iter().position(|arg| arg == "--").unwrap_or(all.len());
    let (options, command_line) = all.split_at(split);
    let args = memfd("efr-sbx-args", &encode_args(options))?;
    if let Some(opened) = opened {
        opened()?;
    }
    let passed: Vec<BorrowedFd<'_>> =
        [&status_write, &records_write, &terminal, &policy, &args].map(AsFd::as_fd).to_vec();
    for fd in &passed {
        inherit(*fd)?;
    }
    for number in table.numbers() {
        // FdTable owns these descriptors until it drops below, after bwrap started.
        fds::inherit_number(number)
            .map_err(|error| SbxError::os("pass a bind source to bwrap", error))?;
    }
    let mut command = os::command(&launch.spec.runtime.bwrap);
    command
        .arg("--args")
        .arg(args.as_raw_fd().to_string())
        .args(command_line)
        .env_clear()
        .envs(&launch.env)
        .stdin(Stdio::inherit())
        .stderr(Stdio::piped());
    if let Some(stdout) = &launch.stdout {
        let copy = stdout.try_clone().map_err(|error| SbxError::os("copy stdout", error))?;
        command.stdout(Stdio::from(copy));
    }
    let spawned = command.spawn();
    // The launcher's copies of what bwrap took close now, so the readers see the end
    // of each pipe when the last process inside is gone.
    drop((status_write, records_write, terminal, policy, args, table));
    let mut child = spawned
        .map_err(|source| SbxError::Spawn { program: launch.spec.runtime.bwrap.clone(), source })?;
    let stderr = child.stderr.take().map(OwnedFd::from);
    let records = reader("efr-sbx-records", records_read, launch.spec.limits.max_bytes)?;
    let errors = match stderr {
        Some(fd) => Some(reader("efr-sbx-stderr", fd, MAX_SIDE_BYTES)?),
        None => None,
    };
    let statuses = reader("efr-sbx-status", status_read, MAX_SIDE_BYTES)?;
    let exit = child.wait().map_err(|error| SbxError::os("wait for bwrap", error))?;
    let records = join(records)?;
    let errors = match errors {
        Some(errors) => join(errors)?.unwrap_or_default(),
        None => Vec::new(),
    };
    let statuses = join(statuses)?.unwrap_or_default();
    let bwrap = match (exit.code(), exit.signal()) {
        (Some(code), _) => BwrapExit::Code(code),
        (None, signal) => BwrapExit::Signal(signal.unwrap_or_default()),
    };
    let ending = ending(parse_status(&statuses), &String::from_utf8_lossy(&errors), bwrap);
    Ok((ending, records))
}

fn pipe(what: &'static str) -> Result<(OwnedFd, OwnedFd), SbxError> {
    rustix::pipe::pipe_with(PipeFlags::CLOEXEC).map_err(|error| SbxError::os(what, error.into()))
}

/// A memfd holding `bytes`, read from its start.
fn memfd(name: &str, bytes: &[u8]) -> Result<OwnedFd, SbxError> {
    let fd = rustix::fs::memfd_create(name, MemfdFlags::CLOEXEC)
        .map_err(|error| SbxError::os("make a memfd", error.into()))?;
    let mut file = File::from(fd);
    file.write_all(bytes).map_err(|error| SbxError::os("fill a memfd", error))?;
    file.rewind().map_err(|error| SbxError::os("rewind a memfd", error))?;
    Ok(OwnedFd::from(file))
}

/// Clears `FD_CLOEXEC`, so bwrap inherits the descriptor.
fn inherit(fd: BorrowedFd<'_>) -> Result<(), SbxError> {
    rustix::io::fcntl_setfd(fd, FdFlags::empty())
        .map_err(|error| SbxError::os("pass a descriptor to bwrap", error.into()))
}

pub(crate) type ReadTask = JoinHandle<std::io::Result<Option<Vec<u8>>>>;

/// Reads `fd` to its end on a thread of its own: at most `limit` bytes are kept, and
/// `None` means there were more. The rest is read and dropped, so the writer never
/// blocks.
pub(crate) fn reader(name: &str, fd: OwnedFd, limit: usize) -> Result<ReadTask, SbxError> {
    let task = move || {
        let mut file = File::from(fd);
        let mut kept = Vec::new();
        let mut chunk = [0_u8; 16 * 1024];
        let mut over = false;
        loop {
            let read = match file.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if !over && kept.len() + read <= limit {
                kept.extend_from_slice(chunk.get(..read).unwrap_or_default());
            } else {
                over = true;
            }
        }
        Ok((!over).then_some(kept))
    };
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(task)
        .map_err(|error| SbxError::os("start a reader thread", error))
}

pub(crate) fn join(handle: ReadTask) -> Result<Option<Vec<u8>>, SbxError> {
    match handle.join() {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => Err(SbxError::os("read a pipe from bwrap", error)),
        Err(_) => Err(SbxError::os("read a pipe from bwrap", std::io::ErrorKind::Other.into())),
    }
}

/// The variables of the launcher's own environment.
pub(crate) fn own_env() -> BTreeMap<OsString, OsString> {
    os::vars()
}

/// The value of `name` in `env`.
pub(crate) fn env_value<'a>(
    env: &'a BTreeMap<OsString, OsString>,
    name: &str,
) -> Option<&'a OsStr> {
    env.get(OsStr::new(name)).map(OsString::as_os_str)
}
