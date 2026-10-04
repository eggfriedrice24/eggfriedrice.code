//! What a hidden shell is doing, as its OSC 133 and OSC 7 marks say.

use std::path::PathBuf;

use efr_protocol::PtyId;
use efr_screen::{PromptKind, SemanticPromptEvent, ShellMarkKind};

/// Where a hidden shell is in its prompt cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Phase {
    /// The shell was spawned and has shown no marked prompt yet.
    Starting,
    /// A prompt is being drawn: `A` or `P` arrived, `B` not yet.
    Prompting {
        /// True for a continuation prompt (`P;k=s`).
        continuation: bool,
    },
    /// At a primary prompt, waiting for a command line.
    Ready,
    /// At a continuation prompt: an unfinished command line waits for more input.
    Continuation,
    /// A command line runs: `C` arrived, `D` not yet.
    Running,
    /// A command line ended (`D`) and the next prompt is not drawn yet.
    Finished,
    /// No marks arrived in time (or the shell is not a zsh), so runs are delimited
    /// with sentinels and the shell's phase is unknown.
    Unmarked,
}

/// A snapshot of one hidden shell.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShellState {
    /// The shell's PTY.
    pub pty_id: PtyId,
    /// The shell's process id.
    pub pid: u32,
    /// Where it is in its prompt cycle.
    pub phase: Phase,
    /// Its working directory: the start directory until the first OSC 7 report.
    pub cwd: PathBuf,
    /// The host of the last OSC 7 report, if it named one.
    pub host: Option<String>,
    /// The exit status of the last command line that ran.
    pub last_exit: Option<i32>,
    /// True once an integration mark has arrived.
    pub integration: bool,
}

impl ShellState {
    pub(crate) fn new(pty_id: PtyId, pid: u32, cwd: PathBuf, integration_expected: bool) -> Self {
        ShellState {
            pty_id,
            pid,
            phase: if integration_expected { Phase::Starting } else { Phase::Unmarked },
            cwd,
            host: None,
            last_exit: None,
            integration: false,
        }
    }

    /// True when a command can be typed now and delimited by marks or sentinels.
    pub fn accepts_command(&self) -> bool {
        matches!(self.phase, Phase::Ready | Phase::Unmarked)
    }

    /// True while a command line runs or an unfinished one waits for input.
    pub fn is_busy(&self) -> bool {
        matches!(self.phase, Phase::Running | Phase::Continuation)
    }

    /// Applies one mark. Returns true when the working directory changed.
    pub(crate) fn apply(&mut self, kind: &ShellMarkKind) -> bool {
        match kind {
            ShellMarkKind::SemanticPrompt(event) => {
                self.integration = true;
                self.phase = next_phase(self.phase, event, &mut self.last_exit);
                false
            }
            ShellMarkKind::CwdChanged { host, path } => {
                self.host.clone_from(host);
                if self.cwd == *path {
                    return false;
                }
                self.cwd.clone_from(path);
                true
            }
            _ => false,
        }
    }

    /// The first marked prompt did not come in time: runs use sentinels from now on,
    /// until a mark proves the integration loaded after all.
    pub(crate) fn startup_expired(&mut self) {
        if self.phase == Phase::Starting {
            self.phase = Phase::Unmarked;
        }
    }
}

fn next_phase(phase: Phase, event: &SemanticPromptEvent, last_exit: &mut Option<i32>) -> Phase {
    match event {
        SemanticPromptEvent::PromptStart { kind, .. } => {
            Phase::Prompting { continuation: *kind == PromptKind::Secondary }
        }
        SemanticPromptEvent::InputStart => match phase {
            Phase::Prompting { continuation: true } => Phase::Continuation,
            Phase::Prompting { continuation: false } => Phase::Ready,
            // A `B` without its prompt is a redraw; the phase stands.
            other => other,
        },
        SemanticPromptEvent::OutputStart { .. } => Phase::Running,
        SemanticPromptEvent::CommandEnd { exit_code, .. } => {
            if phase == Phase::Running {
                *last_exit = *exit_code;
            }
            Phase::Finished
        }
        _ => phase,
    }
}

#[cfg(test)]
mod tests;
