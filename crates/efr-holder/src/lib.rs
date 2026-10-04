//! The PTY holder contract: how the daemon gets a pseudo-terminal with a child process
//! on it without knowing who holds it.
//!
//! A [`SpawnSpec`] says what to run: an absolute program, its arguments, an absolute
//! working directory, the child's whole environment and the starting terminal size.
//!
//! Allowed dependencies: `efr-protocol` (for `PtyId` and `Size`) and `efr-stdx`. What
//! does not belong here: IO of any kind and anything outside safe Rust (opening PTYs is
//! `efr-pty`'s job, passing descriptors over a socket is `efr-fdpass`'s at milestone 5),
//! and anything a shell does with its PTY (`efr-shell`).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod error;
mod spec;

pub use error::HolderError;
pub use spec::SpawnSpec;
