//! The state of the user's shell at the moment a `,` line is sent.

use std::fmt;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the zsh plugin observed in the user's shell when it sent a prompt.
///
/// The plugin sends it with every `,` line and Ctrl+Space line, and the daemon computes
/// the git root and the [`Scope`](crate::Scope) from it, so the plugin stays fast. It is
/// context for the model, never a sandbox root or a process working directory.
///
/// `Debug` leaves out `last_command`, because a command line can hold a secret such as
/// `export TOKEN=...`, and contexts are logged.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShellContext {
    /// The shell's `$PWD`.
    pub pwd: PathBuf,
    /// The shell's `$OLDPWD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldpwd: Option<PathBuf>,
    /// The terminal device, `$TTY`. The daemon keeps one active conversation per tty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// The process id of the user's shell, `$$`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_pid: Option<u32>,
    /// The exit status of the last command, `$?`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<i32>,
    /// The last command line, from the shell history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_command: Option<String>,
    /// The nesting depth of the shell, `$SHLVL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shlvl: Option<u32>,
    /// `$SSH_CONNECTION`, set when the shell runs over SSH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_connection: Option<String>,
    /// The host name of the machine the shell runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

impl ShellContext {
    /// A context that knows only the working directory.
    pub fn new(pwd: impl Into<PathBuf>) -> Self {
        ShellContext { pwd: pwd.into(), ..ShellContext::default() }
    }
}

impl fmt::Debug for ShellContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: last_command is deliberately missing; see the type's doc comment.
        f.debug_struct("ShellContext")
            .field("pwd", &self.pwd)
            .field("oldpwd", &self.oldpwd)
            .field("tty", &self.tty)
            .field("shell_pid", &self.shell_pid)
            .field("last_status", &self.last_status)
            .field("shlvl", &self.shlvl)
            .field("ssh_connection", &self.ssh_connection)
            .field("hostname", &self.hostname)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
