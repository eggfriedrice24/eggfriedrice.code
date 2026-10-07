//! One contained launch: bwrap with the plan's arguments on an fd, the inner policy on
//! another, and the pipes back (the spec's sections 3.2 and 3.10).
//!
//! The launch opens every bind source (`FdTable`: one descriptor per bind), writes the
//! argument list and the policy to memfds, clears `FD_CLOEXEC` on exactly the
//! descriptors bwrap and the inner stage take by number, and starts bwrap with the
//! filtered environment. bwrap's stderr is a pipe to the launcher, never the terminal,
//! so its setup messages land in `setup_error` and never in the tool output. Three
//! threads drain the records, bwrap's stderr and the status stream while bwrap runs, so
//! a child that writes its records late never blocks on a full pipe.
//!
//! When the plan has cache overlays, the inner stage sends its process id once bwrap's
//! setup is done and waits. The launcher runs `efr-sbx layers` on it, which mounts the
//! overlays with `index=off` and `xino=off` (bwrap cannot), and then sends `g`. A
//! failed helper fails the launch as a setup failure, and the call does not run.

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

use efr_sandbox::{
    CacheLayers, FdTable, InnerPolicy, LaunchFds, LayersSync, MountPlan, SandboxSpec, encode_args,
};
use rustix::fs::{MemfdFlags, Mode, OFlags};
use rustix::io::FdFlags;
use rustix::pipe::PipeFlags;

pub(crate) use self::status::{BwrapExit, Ending, ending, parse_status};
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
    /// How the launch ended.
    pub(crate) ending: Ending,
    /// The records stream, or `None` when it passed the size limit.
    pub(crate) records: Option<Vec<u8>>,
}

/// Runs the launch. `opened` runs once every bind source is open and before bwrap
/// starts: the launcher writes `started` there.
pub(crate) fn run(
    launch: &Launch<'_>,
    opened: &mut dyn FnMut() -> Result<(), SbxError>,
) -> Result<Outcome, SbxError> {
    let fs = RealFs;
    let mut table = FdTable::new(&fs);
    let (status_read, status_write) = pipe("make the status pipe")?;
    let (records_read, records_write) = pipe("make the records pipe")?;
    let terminal = rustix::io::fcntl_dupfd_cloexec(std::io::stderr().as_fd(), 3)
        .map_err(|error| SbxError::os("copy the terminal", error.into()))?;
    let mut policy =
        InnerPolicy::new(launch.plan, records_write.as_raw_fd(), Some(terminal.as_raw_fd()));
    policy.argv.clone_from(&launch.argv);
    // The handshake with the layer helper: the inner stage writes on `ready` and reads
    // `go`; the launcher holds the other two ends.
    let handshake = match launch.plan.cache_layers() {
        Some(layers) => {
            let (ready, ready_inside) = pipe("make the layer pipes")?;
            let (go_inside, go) = pipe("make the layer pipes")?;
            policy.layers_sync = Some(LayersSync {
                ready_fd: ready_inside.as_raw_fd(),
                go_fd: go_inside.as_raw_fd(),
            });
            policy.cwd = Some(launch.cwd.to_path_buf());
            Some(Handshake { layers, ready, go, inside: [ready_inside, go_inside] })
        }
        None => None,
    };
    let policy = memfd("efr-sbx-policy", &policy.to_json()?)?;
    let fds = LaunchFds { status: status_write.as_raw_fd(), policy: policy.as_raw_fd() };
    let all = launch.plan.bwrap_args(&mut table, &fds, launch.cwd)?;
    // bwrap reads options from --args but drops a command that comes there, so the
    // command after `--` stays on the command line; it holds no path of the plan.
    let split = all.iter().position(|arg| arg == "--").unwrap_or(all.len());
    let (options, command_line) = all.split_at(split);
    let args = memfd("efr-sbx-args", &encode_args(options))?;
    opened()?;
    let mut passed: Vec<BorrowedFd<'_>> =
        [&status_write, &records_write, &terminal, &policy, &args].map(AsFd::as_fd).to_vec();
    if let Some(handshake) = &handshake {
        passed.extend(handshake.inside.iter().map(AsFd::as_fd));
    }
    for fd in &passed {
        inherit(*fd)?;
    }
    for number in table.numbers() {
        // FdTable owns these descriptors until it drops below, after bwrap started.
        fds::inherit_number(number)
            .map_err(|error| SbxError::os("pass a bind source to bwrap", error))?;
    }
    drop(passed);
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
    let helper = handshake.map(|Handshake { layers, ready, go, inside }| {
        drop(inside);
        (layers, ready, go)
    });
    let mut child = spawned
        .map_err(|source| SbxError::Spawn { program: launch.spec.runtime.bwrap.clone(), source })?;
    let stderr = child.stderr.take().map(OwnedFd::from);
    let records = reader("efr-sbx-records", records_read, launch.spec.limits.max_bytes)?;
    let errors = match stderr {
        Some(fd) => Some(reader("efr-sbx-stderr", fd, MAX_SIDE_BYTES)?),
        None => None,
    };
    let statuses = reader("efr-sbx-status", status_read, MAX_SIDE_BYTES)?;
    let mut mounts = None;
    let layers_failed = helper.and_then(|(layers, ready, go)| {
        let pid = read_line(ready)?;
        mounts = mount_namespace(&pid);
        mount_layers(launch, layers, &pid, go)
    });
    let exit = child.wait().map_err(|error| SbxError::os("wait for bwrap", error))?;
    // NOTE: the last reference to the call's mount namespace goes here, in a process
    // that is not exiting, so the kernel tears the overlays down before this returns.
    // When the last process inside drops it, the kernel does so a few milliseconds
    // later, and the next call of the conversation would find its upper dir in use.
    drop(mounts);
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
    let ending = match layers_failed {
        Some(reason) => Ending::SetupFailed {
            reason: format!("the cache overlays did not mount: {reason}"),
            inner: false,
        },
        None => ending(parse_status(&statuses), &String::from_utf8_lossy(&errors), bwrap),
    };
    Ok(Outcome { ending, records })
}

/// The layer helper's handshake of one launch.
struct Handshake<'a> {
    layers: &'a CacheLayers,
    /// The launcher's read end: the inner stage's process id comes here.
    ready: OwnedFd,
    /// The launcher's write end: one `g` lets the inner stage go on.
    go: OwnedFd,
    /// The ends that bwrap passes to the inner stage.
    inside: [OwnedFd; 2],
}

/// The mount namespace of the process that the inner stage named in `line`.
fn mount_namespace(line: &str) -> Option<OwnedFd> {
    let pid = line.trim().parse::<u32>().ok()?;
    let path = format!("/proc/{pid}/ns/mnt");
    rustix::fs::open(path.as_str(), OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty()).ok()
}

/// Runs the layer helper on the inner stage, which sent `line` (its process id) when
/// bwrap's setup was done, and lets the inner stage go on when the helper succeeded.
/// Returns the reason when the helper failed: then `go` closes unanswered, and the
/// inner stage stops before the call runs. When bwrap fails first, `ready` ends without
/// a line, the helper never runs, and bwrap's own message tells why.
fn mount_layers(
    launch: &Launch<'_>,
    layers: &CacheLayers,
    line: &str,
    go: OwnedFd,
) -> Option<String> {
    let Ok(pid) = line.trim().parse::<u32>() else {
        return Some(format!("the inner stage sent {line:?}, not a process id"));
    };
    let plan = match layers.to_json() {
        Ok(plan) => plan,
        Err(error) => return Some(error.to_string()),
    };
    let helper = os::command(&launch.spec.runtime.launcher)
        .arg("layers")
        .arg("--pid")
        .arg(pid.to_string())
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let mut helper = match helper {
        Ok(helper) => helper,
        Err(error) => return Some(format!("efr-sbx layers did not start: {error}")),
    };
    let sent = match helper.stdin.take() {
        Some(mut stdin) => stdin.write_all(&plan),
        None => Err(std::io::ErrorKind::BrokenPipe.into()),
    };
    let output = match helper.wait_with_output() {
        Ok(output) => output,
        Err(error) => return Some(format!("efr-sbx layers did not end: {error}")),
    };
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Some(if text.is_empty() {
            format!("efr-sbx layers ended with {}", output.status)
        } else {
            text
        });
    }
    if let Err(error) = sent {
        return Some(format!("efr-sbx layers did not get the plan: {error}"));
    }
    File::from(go).write_all(b"g").err().map(|error| format!("the inner stage left: {error}"))
}

/// The first line on `fd` without its newline, at most 64 bytes; `None` when the pipe
/// ends before a line.
fn read_line(fd: OwnedFd) -> Option<String> {
    let mut file = File::from(fd);
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    while line.len() < 64 {
        match file.read(&mut byte) {
            Ok(1) if byte == *b"\n" => return Some(String::from_utf8_lossy(&line).into_owned()),
            Ok(1) => line.extend_from_slice(&byte),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            _ => return None,
        }
    }
    Some(String::from_utf8_lossy(&line).into_owned())
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
