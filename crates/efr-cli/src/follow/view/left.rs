//! A command that a shell call left running in the conversation's hidden shell: its
//! call answered the model, but the command goes on and the shell takes no other call
//! until it ends (`shell_command_detached`, `shell_command_input_changed`,
//! `shell_command_ended`). It belongs to the conversation, not to a turn, so the view
//! shows it while it follows any turn of the conversation.
//!
//! While it waits for input and keys can be read, the view asks for an answer line that
//! goes with `input.respond` and its call id, as for a running call. The line takes the
//! keys from the input row at once, so nothing typed for it is sent as a steer. A
//! command in the sandbox never gets an answer that may be a secret. Every text that
//! holds the command or its prompt row goes through `format::one_line`.

use efr_protocol::{ApprovedBy, CallId, InputWait};
use efr_render::RenderOptions;

use super::{AnswerKind, ECHO_PREFIX};
use crate::format::hidden_shell::{self, ELSEWHERE};
use crate::format::{self, Tone};

/// What the label of the answer line starts with.
const LABEL: &str = "the hidden shell";

/// The line under the answer line while the turn runs: Ctrl+C there stops the command,
/// not the turn.
pub(super) const ANSWER_HINT: &str = "enter answers \u{b7} ctrl+c stops the command, not the turn";

/// The line under the answer line after the turn ended, while `efr` stays open for it.
pub(super) const KEPT_HINT: &str = "enter answers \u{b7} ctrl+c stops it \u{b7} esc leaves it";

/// The line while the keys wait for the command's first wait: it may ask for a
/// password, which must never reach the input row.
pub(super) const HOLD_HINT: &str =
    "it may ask for a password: your keys wait here until efr knows \u{b7} ctrl+c stops it";

/// The note when the user leaves a command that waits, with Esc after the turn.
pub(super) const LEFT_WAITING: &str = "efr leaves the command waiting; ,shell answers or stops it";

/// A command left running in the hidden shell, as the view knows it.
#[derive(Debug)]
pub(super) struct Left {
    pub(super) call_id: CallId,
    /// The first characters of the command line, as the model wrote it. Empty when the
    /// view did not see the command start.
    command: String,
    /// It runs in the sandbox.
    pub(super) contained: bool,
    /// What it waits for, as the daemon said last.
    pub(super) wait: InputWait,
    /// A `visible` wait whose prompt looks like a password prompt.
    pub(super) looks_secret: bool,
    /// The row of the screen that holds the cursor while it waits.
    prompt: String,
    /// Who approved its call.
    approved_by: Option<ApprovedBy>,
    /// The answer that the user is asked for now, if any.
    pub(super) asking: Option<AnswerKind>,
    /// What the user typed so far, for a shown answer only.
    pub(super) typed: String,
    /// The keys are held for it until its first wait is known: its call may ask for a
    /// password outside the sandbox ([`Ask::Discard`](super::Ask::Discard)).
    pub(super) holding: bool,
    /// The view holds the keys for it now, with nothing to answer.
    pub(super) held: bool,
    /// It asked for an answer that may be a password: keys typed while it asks nothing
    /// are thrown away until it ends or the turn does.
    pub(super) guarding: bool,
    /// Keys can be read here, as the last event said.
    pub(super) can_ask: bool,
    /// The followed turn ended, and `efr` stays open for its answer line.
    pub(super) lingering: bool,
}

impl Left {
    /// The command of call `call_id`, as `shell_command_detached` reports it.
    pub(super) fn new(
        call_id: CallId,
        command: &str,
        contained: bool,
        approved_by: Option<ApprovedBy>,
    ) -> Left {
        Left {
            call_id,
            command: command.to_owned(),
            contained,
            wait: InputWait::None,
            looks_secret: false,
            prompt: String::new(),
            approved_by,
            asking: None,
            typed: String::new(),
            holding: false,
            held: false,
            guarding: false,
            can_ask: false,
            lingering: false,
        }
    }

    /// It waits for `wait` now, with the prompt row `prompt`.
    pub(super) fn waits(&mut self, wait: InputWait, looks_secret: bool, prompt: &str) {
        self.wait = wait;
        self.looks_secret = looks_secret && wait == InputWait::Visible;
        prompt.clone_into(&mut self.prompt);
    }

    /// True while keys are read for it, to answer or to hold.
    pub(super) fn reads_keys(&self) -> bool {
        self.asking.is_some() || self.held
    }

    /// True when its wait may be for a secret: a hidden one, or a visible one whose
    /// prompt looks like a password prompt.
    pub(super) fn secret_wait(&self) -> bool {
        hidden_shell::secret_wait(self.wait, self.looks_secret)
    }

    /// The answer line that its wait asks for; `None` without a wait, and for a wait in
    /// the sandbox that may be for a secret, which efr never types there.
    pub(super) fn kind(&self) -> Option<AnswerKind> {
        if self.contained && self.secret_wait() {
            return None;
        }
        match self.wait {
            InputWait::Hidden => Some(AnswerKind::Hidden),
            InputWait::Visible if self.looks_secret => Some(AnswerKind::Masked),
            InputWait::Visible => Some(AnswerKind::Visible),
            _ => None,
        }
    }

    /// The command on one line, or `the command` when the view did not see it.
    pub(super) fn command(&self) -> String {
        if self.command.trim().is_empty() {
            "the command".to_owned()
        } else {
            format::one_line(&self.command)
        }
    }

    /// Who let it run, in words; `None` when its call needed no approval.
    fn approver(&self) -> Option<&'static str> {
        hidden_shell::approver(self.approved_by?)
    }

    /// The label above its answer line, such as `the hidden shell: rsync -a ... (you
    /// approved it)`.
    pub(super) fn label(&self) -> String {
        let mut label = format!("{LABEL}: {}", self.command());
        if self.contained {
            label.push_str(" (sandbox)");
        } else if let Some(approver) = self.approver() {
            label.push_str(&format!(" ({approver})"));
        }
        label
    }

    /// What it waits for, in words: `a password` only outside the sandbox.
    fn waits_for(&self) -> Option<&'static str> {
        hidden_shell::waits_for(self.wait, self.looks_secret, self.contained)
    }

    /// The line at the end of the followed turn while it still runs. Without `kept`,
    /// `efr` ends, and the line says where to answer or stop it.
    pub(super) fn end_line(&self, kept: bool) -> String {
        let mut line = format!("the hidden shell still runs: {}", self.command());
        if let Some(what) = self.waits_for() {
            line.push_str(&format!(" (it waits for {what})"));
        }
        if !kept {
            line.push_str(" \u{b7} ");
            line.push_str(ELSEWHERE);
        }
        line
    }

    /// The note when it waits for input that `efr` cannot ask for here.
    pub(super) fn elsewhere_note(&self) -> String {
        let what = self.waits_for().unwrap_or("input");
        format!(
            "the hidden shell still runs {} and waits for {what}; efr cannot ask for it here; {ELSEWHERE}",
            self.command()
        )
    }

    /// The hint under its line now.
    fn hint(&self) -> &'static str {
        if self.asking.is_none() {
            HOLD_HINT
        } else if self.lingering {
            KEPT_HINT
        } else {
            ANSWER_HINT
        }
    }

    /// The rows of the live zone while keys are read for it: the label, the prompt row,
    /// how the answer goes, the echo of a shown answer and the hint. Each row is cut to
    /// the width on a terminal.
    pub(super) fn live(&self, options: &RenderOptions) -> String {
        if !self.reads_keys() {
            return String::new();
        }
        let mut out = String::new();
        let mut row = |text: &str, tone: Tone| {
            let text = match format::columns(options) {
                Some((width, method)) => format::cut(text, width, method),
                None => text.to_owned(),
            };
            out.push_str(&format::paint(&text, tone, options));
            out.push('\n');
        };
        row(&self.label(), Tone::Attention);
        if let Some(kind) = self.asking {
            if !self.prompt.trim().is_empty() {
                row(&format::one_line(&self.prompt), Tone::Plain);
            }
            row(kind.line(), Tone::Plain);
            if kind.shown() {
                row(&format!("{ECHO_PREFIX}{}", format::one_line(&self.typed)), Tone::Plain);
            }
        }
        row(self.hint(), Tone::Dim);
        out
    }

    /// The same rows for stderr when stdout is not a terminal, without the echo, which
    /// goes on a line of its own as it is typed.
    pub(super) fn raw(&self) -> String {
        let mut out = format!("{}\n", self.label());
        if let Some(kind) = self.asking {
            if !self.prompt.trim().is_empty() {
                out.push_str(&format::one_line(&self.prompt));
                out.push('\n');
            }
            out.push_str(kind.line());
            out.push('\n');
        }
        out.push_str(self.hint());
        out.push('\n');
        out
    }
}

#[cfg(test)]
mod tests;
