//! The PTY holder contract: how the daemon gets a pseudo-terminal with a child process
//! on it without knowing who holds it.
//!
//! [`PtyHolder`] spawns a child on a new PTY from a [`SpawnSpec`] and returns a
//! [`PtyHandle`]: the master side as an `OwnedFd`, the child's pid and the PTY's id. It
//! also resizes, signals ([`Signal`], [`SignalTarget`]), lists ([`PtyInfo`],
//! [`ChildStatus`]) and releases the PTYs it holds, and waits until a child has been
//! reaped. At milestone 1 the holder is
//! `efr_pty::LocalPtyHolder` inside the daemon; at milestone 5 it is `efr-ptyd`, a
//! separate service reached over the holder socket, whose messages are
//! [`RequestFrame`] and [`ResponseFrame`]. `efr-shell` only ever sees
//! `Arc<dyn PtyHolder>`, so that switch changes no caller.
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
mod holder;
mod info;
mod signal;
mod spec;
mod wire;

pub use error::HolderError;
pub use holder::{PtyHandle, PtyHolder};
pub use info::{ChildStatus, PtyInfo};
pub use signal::{Signal, SignalTarget};
pub use spec::SpawnSpec;
pub use wire::{
    HOLDER_PROTOCOL_VERSION, HolderErrorCode, HolderRequest, HolderResponse, RequestFrame,
    ResponseFrame, WireError,
};
