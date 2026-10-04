//! One long-lived hidden zsh per conversation, and running commands in it.
//!
//! - [`ShellState`] and [`Phase`]: what a hidden shell is doing, followed from its
//!   OSC 133 and OSC 7 marks.
//! - [`RunRequest`], [`CommandResult`], [`Completion`], [`RunProgress`]: one command
//!   line, delimited by OSC 133 marks or, in shells without the integration, by
//!   random-token sentinels ([`RunMode`], [`Delimiter`]).
//! - [`ShellConfig`] and [`ShellDeps`]: what the daemon passes in. [`ScreenFactory`]
//!   builds each shell's screen, [`RecordingSink`] receives every byte for the PTY
//!   recording and [`ShellObserver`] hears [`ShellNotice`]s.
//! - The zsh integration (`assets/zsh/`): a ZDOTDIR shim that sources the user's own
//!   startup files and an original script that emits the marks, embedded with
//!   `include_str!` and written to [`ShellConfig::integration_dir`].
//!
//! Allowed dependencies: `efr-holder`, `efr-screen`, `efr-protocol` and `efr-stdx`.
//! What does not belong here: opening PTYs (`efr-pty`), terminal emulation
//! (`efr-screen-vt100`, `efr-screen-ghostty`), storing the recording and turning
//! notices into events (`efr-daemon`), and anything about tools or permissions
//! (`efr-tools`, `efr-conversation`).

mod capture;
mod config;
mod env;
mod error;
mod integration;
mod recording_sink;
mod run;
mod screens;
mod sentinel;
mod state;

pub use config::{ShellConfig, ShellDeps};
pub use error::ShellError;
pub use recording_sink::{Discard, RecordingSink, ShellNotice, ShellObserver};
pub use run::{
    CommandResult, Completion, Delimiter, NoProgress, OutputUpdate, RunMode, RunProgress,
    RunRequest,
};
pub use screens::ScreenFactory;
pub use state::{Phase, ShellState};
