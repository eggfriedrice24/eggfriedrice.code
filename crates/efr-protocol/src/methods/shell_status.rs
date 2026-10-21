//! `shell.status`: whether a command that a `shell` call left running holds the
//! conversation's hidden shell, for `,shell` and for a client that did not see the
//! command start.

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ApprovedBy, CallId, ConversationId, InputWait};

/// The params of `shell.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShellStatus {
    /// The conversation of the hidden shell.
    pub conversation_id: ConversationId,
}

/// The result of `shell.status`. The daemon reads its live state, so a
/// `shell_command_detached` event of a stopped efrd never shows as busy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShellStatusResult {
    /// The command that holds the hidden shell. Absent when the shell is free, or when
    /// the conversation has no hidden shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<BusyShell>,
}

/// A command that a `shell` call left running in the hidden shell: the shell takes no
/// other call until it ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BusyShell {
    /// The call that left the command running.
    pub call_id: CallId,
    /// The first 200 characters of the command line. The model wrote it, so a client
    /// shows it as text that a program wrote, on one line.
    pub command: String,
    /// When the command started.
    pub since: Timestamp,
    /// True when the command runs in the sandbox. A client never asks the user for a
    /// hidden answer, an answer to a wait that looks secret, or a manual answer to a
    /// command in the sandbox.
    pub contained: bool,
    /// What the command waits for now.
    pub input: InputWait,
    /// True for a `visible` wait whose prompt line looks like a password prompt. False
    /// when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub looks_secret: bool,
    /// The row of the screen that holds the cursor while the command waits, at most 200
    /// characters. The program wrote it, so a client shows it as text that a program
    /// wrote, on one line. Absent when the command does not wait.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// Who approved the call. Absent when the call needed no approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<ApprovedBy>,
}
