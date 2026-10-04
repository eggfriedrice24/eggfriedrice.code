//! The efr wire contract: everything that the daemon and its clients exchange.
//!
//! Frames, the `Method` enum with one params type per method, results and stream items,
//! `Event` and its envelope, ids, `Scope`, `ShellContext`, screen snapshots, wire errors,
//! the pure length-prefix framing and [`PROTOCOL_VERSION`]. The daemon, `efr`, the tests,
//! the PTY proxy and the WebSocket clients compile against these types; the phone app
//! reads `docs/protocol.md` and the frozen fixtures in `fixtures/v1/`.
//!
//! Allowed dependencies: the allowlist permits `efr-stdx`, but this crate uses no
//! workspace crate, because `efr-stdx` depends on tokio and this crate must never reach
//! tokio. What does not belong here: IO of any kind, tokio, clocks and random generators,
//! and any knowledge of what the daemon does with a request.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod capabilities;
mod error;
mod event;
mod ids;
mod scope;
mod screen;
mod shell_context;
mod version;

pub use capabilities::Capabilities;
pub use error::{ErrorBody, ErrorCode, ErrorFrame, ProtocolError};
pub use event::{ApprovalDecision, Event, EventEnvelope, Usage};
pub use ids::{
    CallId, CommandId, ConversationId, DaemonId, DeviceId, PtyId, RequestId, Seq, TurnId,
};
pub use scope::{Origin, ProjectId, Scope, ScopeName};
pub use screen::{Cell, Color, Cursor, RowCells, ScreenSnapshot, Size};
pub use shell_context::ShellContext;
pub use version::PROTOCOL_VERSION;
