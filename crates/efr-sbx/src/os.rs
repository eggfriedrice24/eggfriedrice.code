//! The few process, environment and time calls that the workspace routes through
//! `efr-stdx` elsewhere. efr-sbx must not reach tokio, and `efr-stdx` does, so the
//! launcher has its own narrow forms; each caller sets the environment and the working
//! directory of what it starts itself.

use std::ffi::OsStr;
use std::process::Command;
use std::time::Duration;

/// A command for `program`. The caller decides its environment and directory: the
/// launcher passes bwrap the filtered environment, the exit child the trusted one.
#[expect(
    clippy::disallowed_methods,
    reason = "efr_stdx::process::command builds a tokio command; the launcher has no runtime"
)]
pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}

/// The variable `name` of the launcher's own environment, when it is set and UTF-8.
#[expect(
    clippy::disallowed_methods,
    reason = "the launcher reads the trusted shell's environment, which efr_stdx::env does not type"
)]
pub(crate) fn var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The time since the epoch, for comparing file times with the start of a call.
#[expect(
    clippy::disallowed_methods,
    reason = "a file time is wall-clock time; the launcher schedules nothing with it"
)]
pub(crate) fn since_epoch() -> Duration {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default()
}
