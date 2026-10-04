//! The state of the user's shell at the moment a `,` line is sent.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the zsh plugin observed in the user's shell when it sent a prompt.
///
/// The plugin sends it with every `,` line and Ctrl+Space line, and the daemon computes
/// the git root and the [`Scope`](crate::Scope) from it, so the plugin stays fast. It is
/// context for the model, never a sandbox root or a process working directory.
///
/// It holds no command line. The last command can hold a secret such as
/// `export TOKEN=...`, and a context is recorded in the `prompt_queued` event, which the
/// event log keeps forever and every subscriber receives. The last command therefore
/// travels as [`PromptSend::last_command`](crate::PromptSend::last_command) and reaches
/// only the turn's preamble. A `last_command` member that a client still puts here is
/// ignored like any unknown member, so it can never reach an event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[cfg(test)]
mod tests;
