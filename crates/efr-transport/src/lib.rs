//! The daemon's protocol edge, without the engine.
//!
//! - [`UnixListener`]: the socket at `$XDG_RUNTIME_DIR/efr/daemon.sock`, created with
//!   mode 0600; every peer's uid is checked with `SO_PEERCRED`.
//! - [`subscription`]: bounded per-subscriber queues of [`SUBSCRIBER_QUEUE_FRAMES`]
//!   items; overflow closes that subscription with `overflow` and `last_seq`, and never
//!   slows the producer.
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
mod subscriptions;
mod unix_listener;

pub use codec::ServerCodec;
pub use context::{ConnId, ConnectionContext, PeerCred};
pub use error::TransportError;
pub use subscriptions::{
    Delivery, Offer, SUBSCRIBER_QUEUE_FRAMES, SubscriptionReceiver, SubscriptionSender,
    subscription,
};
pub use unix_listener::{Accepted, UnixListener};
