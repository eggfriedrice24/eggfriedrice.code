//! The words for a command that a shell call left running in a conversation's hidden
//! shell, for the turn view and for `efr shell`: who let it run, what it waits for and
//! how it ended. The command comes from the model, so a caller passes it through
//! [`one_line`](super::one_line) first.

use efr_protocol::{ApprovedBy, InputWait, ShellCommandEnd};

/// Where the user answers or stops the command after `efr` ends.
pub(crate) const ELSEWHERE: &str = ",shell answers or stops it";

/// Who let the command run, in words; `None` for a source that this build does not
/// know.
pub(crate) fn approver(approved_by: ApprovedBy) -> Option<&'static str> {
    match approved_by {
        ApprovedBy::You => Some("you approved it"),
        ApprovedBy::YourRule => Some("your rule allowed it"),
        ApprovedBy::Reviewer => Some("the reviewer allowed it"),
        ApprovedBy::FullMode => Some("full mode let it run"),
        _ => None,
    }
}

/// True when a wait may be for a secret: a hidden one, or a visible one whose prompt
/// looks like a password prompt.
pub(crate) fn secret_wait(wait: InputWait, looks_secret: bool) -> bool {
    wait == InputWait::Hidden || (wait == InputWait::Visible && looks_secret)
}

/// What the command waits for, in words: `a password` only for a command outside the
/// sandbox; `None` without a wait.
pub(crate) fn waits_for(
    wait: InputWait,
    looks_secret: bool,
    contained: bool,
) -> Option<&'static str> {
    match wait {
        InputWait::None => None,
        _ if !contained && secret_wait(wait, looks_secret) => Some("a password"),
        _ => Some("input"),
    }
}

/// The note when the command ended, as `how` says. `secret_in_sandbox` is true when the
/// command runs in the sandbox and its last wait may be for a secret: efr stops such a
/// wait at once and types nothing (DS50), and its end is `limit` too.
pub(crate) fn ended(
    command: &str,
    exit_code: Option<i32>,
    how: ShellCommandEnd,
    secret_in_sandbox: bool,
) -> String {
    match how {
        ShellCommandEnd::Ended => match exit_code {
            Some(code) if code != 0 => {
                format!("{command} ended in the hidden shell with exit {code}")
            }
            _ => format!("{command} ended in the hidden shell"),
        },
        ShellCommandEnd::Stopped => format!("{command} stopped in the hidden shell"),
        ShellCommandEnd::Limit if secret_in_sandbox => format!(
            "efr stopped {command} in the hidden shell: it asked for a secret in the sandbox, \
             where efr types none"
        ),
        ShellCommandEnd::Limit => {
            format!("efr stopped {command} in the hidden shell: nobody answered its input in time")
        }
        ShellCommandEnd::Closed => format!(
            "the hidden shell closed with {command} in it; the next call starts a new shell"
        ),
        ShellCommandEnd::ShellExited => format!("the hidden shell exited with {command} in it"),
        ShellCommandEnd::Moved => format!("{command} moved to a background job"),
        _ => format!("{command} no longer runs in the hidden shell"),
    }
}

#[cfg(test)]
mod tests;
