//! The daemon's protocol edge, without the engine.
//!
//! - [`ServerCodec`]: the tokio codec over `efr_protocol::framing`.
//! - [`ConnectionContext`]: `{ surface, uid, pid, conn_id }`, carried by every request.
//!
//! Allowed dependencies: `efr-protocol` and `efr-stdx`. What does not belong here: the
//! meaning of any method (the daemon's `methods/`), the database (`efr-transport ->
//! efr-store` is a forbidden edge), and the mapping from daemon errors to wire errors
//! (`efr-daemon/src/error.rs`). The WebSocket listener for the phone lands here later.

mod codec;
mod context;
mod error;

pub use codec::ServerCodec;
pub use context::{ConnId, ConnectionContext, PeerCred};
pub use error::TransportError;
