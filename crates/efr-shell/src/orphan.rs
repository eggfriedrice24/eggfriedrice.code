//! A command left running in the hidden shell: a run whose timeout passed while its
//! command went on. The shell takes no other marked line until it ends.

use efr_protocol::{ApprovedBy, CallId, InputWait};
use jiff::Timestamp;

/// A command left running in a conversation's hidden shell, as
/// [`ShellSessions::busy`](crate::ShellSessions::busy) tells it. The daemon maps it to
/// the one wire type, `efr_protocol::BusyShell`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftRunning {
    /// The tool call whose command it is, when the run named one.
    pub call: Option<CallId>,
    /// The first 200 characters of the command line, as the model wrote it.
    pub command: String,
    /// When its run was asked for.
    pub since: Timestamp,
    /// True when the command runs in the auto mode's sandbox.
    pub contained: bool,
    /// What it waits for now.
    pub input: InputWait,
    /// True for a visible wait whose prompt reads like a password prompt while the
    /// terminal is not in line mode.
    pub looks_secret: bool,
    /// The row of the screen that holds the cursor while the command waits, at most 200
    /// characters; empty while it does not wait.
    pub prompt: String,
    /// Who approved the call; `None` when it needed no approval.
    pub approved_by: Option<ApprovedBy>,
}
