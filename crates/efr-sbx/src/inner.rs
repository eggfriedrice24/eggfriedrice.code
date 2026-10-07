//! `efr-sbx inner --policy-fd N`: the stage that bwrap starts inside the namespaces
//! (the spec's sections 3.1 step 8, 3.4, 3.6 and 3.10).
//!
//! It is trusted code in an untrusted place. In order:
//!
//! 1. Read the `InnerPolicy` from the policy descriptor and close it. When the call
//!    has cache overlays, tell the launcher that bwrap's setup is done, wait while its
//!    layer helper mounts them, and enter the start dir again.
//! 2. Move the records pipe to fd 3 and the terminal copy to fd 4; close every
//!    descriptor from 5 up, so nothing opened outside keeps its rights inside.
//! 3. (Phase 2: start `efr-sbx bridge` here, in a Landlock domain of its own.)
//! 4. Apply Landlock, then seccomp.
//! 5. Put the terminal copy on fd 2, so the child's stderr is the terminal again, and
//!    run the child shell.
//!
//! Until step 5, fd 2 is the pipe to the launcher that only bwrap and this stage hold,
//! so a failure there reaches the launcher as a setup error that no sandboxed code can
//! forge. A failure of the exec itself goes on fd 3 as a `setup-error` record.

use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::ExitCode;

use efr_sandbox::{InnerPolicy, LayersSync, MAX_POLICY_BYTES, RECORDS_FD, SETUP_FAILURE_STATUS};

use crate::error::SbxError;
use crate::{fds, landlock, os, seccomp};

/// The number the terminal copy waits on until Landlock and seccomp are in place.
const TERMINAL_PARKED_FD: RawFd = 4;

/// The lowest number the descriptors move to while 3 and 4 are filled.
const SCRATCH_FD_MIN: RawFd = 10;

/// The line the inner stage writes on the launcher's pipe before its error, so the
/// launcher can tell it from bwrap's own messages.
pub(crate) const SETUP_PREFIX: &str = "efr-sbx inner: ";

/// Runs the inner stage; returns only on failure.
pub(crate) fn main(policy_fd: RawFd) -> ExitCode {
    match prepare(policy_fd) {
        Ok(child) => exec_child(child),
        Err(error) => {
            // NOTE: until the child starts, fd 2 is bwrap's stderr, a pipe to the
            // launcher.
            let _ = writeln!(std::io::stderr(), "{SETUP_PREFIX}{}", error.chain());
        }
    }
    exit_code(SETUP_FAILURE_STATUS)
}

/// What is left to do after the sandbox is in place.
struct Child {
    argv: Vec<OsString>,
    terminal: Option<OwnedFd>,
    records: OwnedFd,
}

fn prepare(policy_fd: RawFd) -> Result<Child, SbxError> {
    let policy = read_policy(policy_fd)?;
    if let Some(sync) = policy.layers_sync {
        wait_for_layers(sync, policy.cwd.as_deref())?;
    }
    let records = fds::adopt(policy.records_fd)
        .and_then(|fd| park(fd, RECORDS_FD, false))
        .map_err(|error| SbxError::os("take the records pipe", error))?;
    let terminal = match policy.terminal_fd {
        Some(fd) => Some(
            fds::adopt(fd)
                .and_then(|fd| park(fd, TERMINAL_PARKED_FD, true))
                .map_err(|error| SbxError::os("take the terminal", error))?,
        ),
        None => None,
    };
    fds::close_from(TERMINAL_PARKED_FD + 1)
        .map_err(|error| SbxError::os("close the inherited descriptors", error))?;
    // NOTE: phase 2 starts the bridge here, before this stage's own Landlock domain.
    let (stdin, stdout) = (std::io::stdin(), std::io::stdout());
    let terminals: Vec<BorrowedFd<'_>> = policy
        .landlock
        .tty_fds
        .iter()
        .filter_map(|fd| match fd {
            0 => Some(stdin.as_fd()),
            1 => Some(stdout.as_fd()),
            2 => terminal.as_ref().map(AsFd::as_fd),
            _ => None,
        })
        .collect();
    landlock::restrict(&policy.landlock, &terminals)?;
    seccomp::install(&policy.seccomp)?;
    if policy.argv.is_empty() {
        return Err(SbxError::os("run the child", std::io::ErrorKind::InvalidInput.into()));
    }
    Ok(Child { argv: policy.argv, terminal, records })
}

/// Tells the launcher that bwrap's setup is done, with this process's id as the host
/// names it, and waits for its `g`: the layer helper mounts the cache overlays
/// meanwhile. Then enters the start dir again, because bwrap's `--chdir` left this
/// process in the directory below the overlay, where the masks inside the cache no
/// longer are.
fn wait_for_layers(sync: LayersSync, cwd: Option<&Path>) -> Result<(), SbxError> {
    let failed = |error| SbxError::os("wait for the cache overlays", error);
    let ready = fds::adopt(sync.ready_fd).map_err(failed)?;
    let go = fds::adopt(sync.go_fd).map_err(failed)?;
    // NOTE: /proc is the host's procfs (the plan never mounts a new one), so its
    // `self` link names this process as the launcher sees it.
    let pid = std::fs::read_link("/proc/self").map_err(failed)?;
    let mut line = pid.into_os_string().into_encoded_bytes();
    line.push(b'\n');
    File::from(ready).write_all(&line).map_err(failed)?;
    let mut answer = [0_u8; 1];
    let read = File::from(go).read(&mut answer).map_err(failed)?;
    if read != 1 || answer != *b"g" {
        return Err(failed(std::io::ErrorKind::BrokenPipe.into()));
    }
    match cwd {
        Some(cwd) => {
            rustix::process::chdir(cwd).map_err(|error| SbxError::io("enter", cwd, error.into()))
        }
        None => Ok(()),
    }
}

/// Reads and closes the policy descriptor.
fn read_policy(policy_fd: RawFd) -> Result<InnerPolicy, SbxError> {
    let fd = fds::adopt(policy_fd).map_err(|error| SbxError::os("take the policy", error))?;
    let mut bytes = Vec::new();
    let cap = u64::try_from(MAX_POLICY_BYTES).unwrap_or(u64::MAX).saturating_add(1);
    File::from(fd)
        .take(cap)
        .read_to_end(&mut bytes)
        .map_err(|error| SbxError::os("read the policy", error))?;
    Ok(InnerPolicy::from_json(&bytes)?)
}

/// `fd` at exactly `target`. It first moves to a high number, so that a descriptor
/// that sits at `target` already is closed before the copy lands there.
fn park(fd: OwnedFd, target: RawFd, cloexec: bool) -> std::io::Result<OwnedFd> {
    let high = rustix::io::fcntl_dupfd_cloexec(&fd, SCRATCH_FD_MIN)?;
    drop(fd);
    fds::dup_to(high.as_fd(), target, cloexec)
}

/// Puts the terminal on fd 2 and runs the child. When that fails, the error goes on
/// fd 3 as a `setup-error` record, because fd 2 may be the terminal by then.
fn exec_child(child: Child) {
    let error = match &child.terminal {
        Some(terminal) => match rustix::stdio::dup2_stderr(terminal) {
            Ok(()) => None,
            Err(error) => Some(SbxError::os("put the terminal on stderr", error.into())),
        },
        None => None,
    };
    drop(child.terminal);
    let error = error.unwrap_or_else(|| match child.argv.split_first() {
        Some((program, args)) => {
            let source = os::command(program).args(args).exec();
            SbxError::Spawn { program: program.into(), source }
        }
        None => SbxError::os("run the child", std::io::ErrorKind::InvalidInput.into()),
    });
    let mut record = efr_sandbox::RECORDS_HEADER.to_vec();
    record.extend_from_slice(b"setup-error\0");
    record.extend_from_slice(error.chain().replace('\0', " ").as_bytes());
    record.push(0);
    let _ = File::from(child.records).write_all(&record);
}

fn exit_code(status: i32) -> ExitCode {
    ExitCode::from(u8::try_from(status).unwrap_or(1))
}
