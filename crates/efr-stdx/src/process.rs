//! The one constructor of child processes.
//!
//! `clippy.toml` denies `std::process::Command::new` and `tokio::process::Command::new`
//! in every other place and names [`command`] instead. The caller must choose the
//! working directory, so no child runs in whatever directory the daemon started in, and
//! no child inherits the variables that systemd set for the daemon's own unit, nor what
//! the zsh plugin handed to `efr` in [`Var::PRIVATE`].

use std::ffi::OsStr;
use std::path::Path;

use crate::env::Var;

/// Variables that systemd sets for the daemon's own unit. A child that inherits them
/// can send readiness or status for `efrd.service`, claim the daemon's
/// socket-activation descriptors or feed its watchdog.
pub const SCRUBBED_ENV: &[&str] = &[
    // The sd_notify target of the unit.
    "NOTIFY_SOCKET",
    // Socket activation: the descriptors belong to the daemon process.
    "LISTEN_FDS",
    "LISTEN_PID",
    "LISTEN_FDNAMES",
    // The watchdog of the unit.
    "WATCHDOG_USEC",
    "WATCHDOG_PID",
    // It names the daemon's stderr, which a child's stderr is not.
    "JOURNAL_STREAM",
];

/// A tokio command for `program` that runs in `cwd` without the variables in
/// [`SCRUBBED_ENV`] and [`Var::PRIVATE`].
///
/// `PWD` is set to `cwd` when `cwd` is absolute and removed when it is relative: the
/// inherited value names the daemon's directory, and shells trust `PWD` when it looks
/// right. The child inherits the rest of the environment; the caller adds or removes
/// more, and sets the standard streams and `kill_on_drop` as it needs.
///
/// ```
/// let command = efr_stdx::process::command("zsh", "/home/user/project");
/// assert_eq!(command.as_std().get_current_dir(), Some("/home/user/project".as_ref()));
/// ```
#[expect(
    clippy::disallowed_methods,
    reason = "this is the constructor that clippy.toml names for every other crate"
)]
pub fn command(program: impl AsRef<OsStr>, cwd: impl AsRef<Path>) -> tokio::process::Command {
    let cwd = cwd.as_ref();
    let mut command = tokio::process::Command::new(program);
    command.current_dir(cwd);
    for name in SCRUBBED_ENV {
        command.env_remove(name);
    }
    for var in Var::PRIVATE {
        command.env_remove(var.name());
    }
    if cwd.is_absolute() {
        command.env("PWD", cwd);
    } else {
        command.env_remove("PWD");
    }
    command
}

#[cfg(test)]
mod tests;
