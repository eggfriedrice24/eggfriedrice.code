//! The in-process PTY holder.
//!
//! `LocalPtyHolder` implements `efr_holder::PtyHolder`: it opens a pseudo-terminal
//! (`posix_openpt`, `grantpt`, `unlockpt`, `ptsname`), starts the spec's program on it
//! as a session leader with the PTY as its controlling terminal, sets and changes the
//! window size, delivers signals, and reaps every child so that `wait` and `list` agree
//! on how it ended. The daemon uses it through its `local-pty` feature at milestone 1;
//! from milestone 5 `efr-ptyd` and the PTY proxy use it instead. [`PtyError`] names the
//! step of a spawn that failed, inside the `HolderError` that the trait returns.
//!
//! Allowed dependencies: `efr-holder` and `efr-stdx`. What does not belong here:
//! anything a shell does with its PTY (reading, writing, screens, marks: `efr-shell`),
//! the choice of holder (`efr-daemon`), and passing descriptors between processes
//! (`efr-fdpass`, milestone 5).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod child;
mod error;
mod termios;

pub use error::PtyError;
