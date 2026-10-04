//! One long-lived hidden zsh per conversation, and running commands in it.
//!
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

mod config;
mod env;
mod error;
mod integration;
mod recording_sink;
mod screens;

pub use config::{ShellConfig, ShellDeps};
pub use error::ShellError;
pub use recording_sink::{Discard, RecordingSink, ShellNotice, ShellObserver};
pub use screens::ScreenFactory;
