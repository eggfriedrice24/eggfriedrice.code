//! `shell.interrupt`: stop the command that a `shell` call left running in the hidden
//! shell, for `,shell` and the answer line of `efr`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CallId, CommandId, ConversationId};

/// The params of `shell.interrupt`.
///
/// Without `close`, the daemon sends SIGINT to the process group of the command that
/// `call_id` left running in the conversation's hidden shell, and only while that group
/// holds the shell's terminal and is not the shell's own group. With `close`, it closes
/// the hidden shell (SIGHUP, then SIGKILL), for a command that ignores SIGINT; the next
/// call starts a new shell. The command's end then comes as `shell_command_ended` with
/// `how` `stopped` or `closed`.
///
/// The daemon answers `not_found` for an unknown conversation and when the call's
/// command no longer runs, and `conflict` when the shell itself holds the terminal.
/// It needs the scope `terminal`, as an answer to the command does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShellInterrupt {
    /// The conversation of the hidden shell.
    pub conversation_id: ConversationId,
    /// The call whose command still runs.
    pub call_id: CallId,
    /// Makes the stop idempotent.
    pub command_id: CommandId,
    /// True: close the hidden shell (SIGHUP, then SIGKILL), for a command that ignores
    /// SIGINT. False: SIGINT to the command's group. False when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub close: bool,
}

/// The result of `shell.interrupt`: the daemon sent the signal, or began to close the
/// shell. The command may still run for a moment; its end comes as an event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShellInterruptResult {}
