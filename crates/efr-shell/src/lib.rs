//! One long-lived hidden zsh per conversation, and running commands in it.
//!
//! - [`ShellSessions`]: the manager. It spawns a conversation's shell through an
//!   `Arc<dyn PtyHolder>`, gives it a screen from the injected [`ScreenFactory`], reads
//!   and writes its PTY master through tokio's `AsyncFd`, and follows its state from
//!   the OSC 133 and OSC 7 marks ([`ShellState`], [`Phase`]).
//! - [`ShellSessions::run_command`] (also the [`CommandRunner`] trait): types a
//!   [`RunRequest`], waits for the output to start and end, and returns a
//!   [`CommandResult`] with the exit status, the output, the truncation flag and the
//!   directory after; or, at the timeout, [`Completion::Interactive`] with the screen's
//!   last lines when the command waits for input ([`Completion::FullScreen`] for a
//!   full-screen program). Output that moves the cursor is
//!   replayed on a short-lived capture screen from the same factory, so the text is
//!   what the screen shows. [`RunProgress`] hears the output as it grows. Shells
//!   without the integration are driven with random-token sentinels ([`RunMode`],
//!   [`Delimiter`]).
//! - [`ShellConfig`] and [`ShellDeps`]: what the daemon passes in. [`RecordingSink`]
//!   receives every byte for the PTY recording; [`ShellObserver`] hears
//!   [`ShellNotice`]s.
//! - The zsh integration (`assets/zsh/`): a ZDOTDIR shim that sources the user's own
//!   startup files and an original script that emits the marks, embedded with
//!   `include_str!` and written to [`ShellConfig::integration_dir`], next to the
//!   editor stub (`assets/efr-editor`) that every hidden shell gets as its editor.
//!
//! Allowed dependencies: `efr-holder`, `efr-screen`, `efr-protocol` and `efr-stdx`.
//! What does not belong here: opening PTYs (`efr-pty`), terminal emulation
//! (`efr-screen-vt100`, `efr-screen-ghostty`), storing the recording and turning
//! notices into events (`efr-daemon`), and anything about tools or permissions
//! (`efr-tools`, `efr-conversation`).

mod capture;
mod config;
#[cfg(test)]
mod e2e_zsh;
mod env;
mod error;
mod input;
mod integration;
mod modes;
mod nested_shell;
mod reader;
mod recording_sink;
mod replay;
mod run;
mod screens;
mod sentinel;
mod session;
mod sessions;
mod state;
#[cfg(test)]
mod testing;
mod writer;

pub use config::{ShellConfig, ShellDeps};
pub use error::ShellError;
pub use modes::{InputModes, TerminalModes, Termios};
pub use recording_sink::{Discard, RecordingSink, ShellNotice, ShellObserver};
pub use run::{
    CommandResult, Completion, Delimiter, NoProgress, OutputUpdate, RunMode, RunProgress,
    RunRequest,
};
pub use screens::ScreenFactory;
pub use sessions::{CommandRunner, ShellInfo, ShellSessions};
pub use state::{Phase, ShellState};
