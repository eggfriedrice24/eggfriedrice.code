//! bwrap's `--json-status-fd` stream and what it says about a launch.
//!
//! bwrap writes `child-pid` right after `clone`, before it mounts anything, and
//! `exit-code` only when the command it started has exited. So "`child-pid`, no
//! `exit-code`" means that the mounts or the exec failed: a setup failure, not a
//! failure of the command. The inner stage reports its own failures on bwrap's stderr
//! with [`SETUP_PREFIX`](crate::inner::SETUP_PREFIX), before the child exists.

use std::time::Duration;

use serde_json::Value;

use crate::inner::SETUP_PREFIX;

/// What the status stream held.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BwrapStatus {
    /// The pid of the namespace's init, as the host sees it.
    pub(crate) child_pid: Option<i32>,
    /// The command's exit code, 128 plus the signal for a signal.
    pub(crate) exit_code: Option<i32>,
}

/// Parses the status stream: one JSON object per line. Lines that are not objects of
/// the two kinds are skipped.
pub(crate) fn parse_status(bytes: &[u8]) -> BwrapStatus {
    let mut status = BwrapStatus::default();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(line) else { continue };
        let number = |name: &str| {
            fields.get(name).and_then(Value::as_i64).and_then(|value| i32::try_from(value).ok())
        };
        if let Some(pid) = number("child-pid") {
            status.child_pid = Some(pid);
        }
        if let Some(code) = number("exit-code") {
            status.exit_code = Some(code);
        }
    }
    status
}

/// How a launch ended before the result is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ending {
    /// The command ran and exited with this code.
    Ran {
        /// The code, 128 plus the signal for a signal.
        code: i32,
    },
    /// The sandbox did not start; the text says why, for a person.
    SetupFailed {
        /// bwrap's or the inner stage's message.
        reason: String,
        /// True when the inner stage failed, after every mount was in place.
        inner: bool,
    },
}

/// How bwrap itself ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BwrapExit {
    /// It exited with this code.
    Code(i32),
    /// A signal killed it: Ctrl+C or efr's interrupt reached the whole job.
    Signal(i32),
}

/// Decides how a launch ended from bwrap's status stream, its stderr and how bwrap
/// itself ended.
pub(crate) fn ending(status: BwrapStatus, stderr: &str, bwrap: BwrapExit) -> Ending {
    if let Some(reason) = inner_failure(stderr) {
        return Ending::SetupFailed { reason, inner: true };
    }
    match (status.child_pid, status.exit_code, bwrap) {
        (Some(_), Some(code), _) => Ending::Ran { code },
        // The namespace existed and a signal ended the job before bwrap could report
        // the command's code: the call was interrupted, it did not fail to start.
        (Some(_), None, BwrapExit::Signal(signal)) => Ending::Ran { code: 128 + signal },
        _ => {
            let text = first_lines(stderr);
            let reason = if text.is_empty() {
                match bwrap {
                    BwrapExit::Code(code) => {
                        format!("bwrap exited with {code} before the command started")
                    }
                    BwrapExit::Signal(signal) => {
                        format!("bwrap got signal {signal} before the command started")
                    }
                }
            } else {
                text
            };
            Ending::SetupFailed { reason, inner: false }
        }
    }
}

/// The inner stage's own report on bwrap's stderr, when there is one.
fn inner_failure(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| line.strip_prefix(SETUP_PREFIX)).map(str::to_owned)
}

/// The first lines of bwrap's messages, joined, at most 1000 characters.
fn first_lines(stderr: &str) -> String {
    let text: Vec<&str> =
        stderr.lines().map(str::trim).filter(|line| !line.is_empty()).take(5).collect();
    text.join("; ").chars().take(1000).collect()
}

/// The most attempts of one launch when an overlay is busy.
pub(crate) const OVERLAY_ATTEMPTS: u32 = 25;
/// How long a launch may keep trying when an overlay is busy.
pub(crate) const OVERLAY_RETRY_WINDOW: Duration = Duration::from_millis(100);
/// The pause between two attempts.
pub(crate) const OVERLAY_RETRY_PAUSE: Duration = Duration::from_millis(2);

/// True when a failed attempt should be tried again: the overlay was busy (an upper
/// dir stays in use for a short time after the previous call's namespace died), and
/// attempts and time are left. A launch never tries again without the overlay.
pub(crate) fn retry_overlay(ending: &Ending, attempts: u32, elapsed: Duration) -> bool {
    let Ending::SetupFailed { reason, inner: false } = ending else { return false };
    reason.to_ascii_lowercase().contains("overlay")
        && attempts < OVERLAY_ATTEMPTS
        && elapsed < OVERLAY_RETRY_WINDOW
}

#[cfg(test)]
mod tests;
