//! The daemon's protocol edge, without the engine.
//!
//! - [`UnixListener`]: the socket at `$XDG_RUNTIME_DIR/efr/daemon.sock`, created with
//!   mode 0600; every peer's uid is checked with `SO_PEERCRED`. [`UnixListener::serve`]
//!   runs one task per connection.
//! - Each connection checks `hello` and the protocol version before any method runs,
//!   keeps a request-id table, cancels requests on a cancel frame, and cancels every
//!   request still in flight when it closes.
//! - [`Dispatcher`]: the trait `efr-daemon` implements to answer methods and to learn
//!   when a connection closes. A handler gets a [`Request`] and answers through its
//!   [`Responder`].
//! - [`subscription`]: bounded per-subscriber queues of [`SUBSCRIBER_QUEUE_FRAMES`]
//!   items; overflow closes that subscription with `overflow` and `last_seq`, and never
//!   slows the producer. A lossy item, such as a draft, is dropped when its own room
//!   of [`LOSSY_QUEUE_FRAMES`] is full, and never closes the subscription.
//! - [`ServerCodec`]: the tokio codec over `efr_protocol::framing`.
//! - [`ConnectionContext`]: `{ surface, uid, pid, conn_id }`, carried by every request.
//!
//! Allowed dependencies: `efr-protocol` and `efr-stdx`. What does not belong here: the
//! meaning of any method (the daemon's `methods/`), the database (`efr-transport ->
//! efr-store` is a forbidden edge), and the mapping from daemon errors to wire errors
//! (`efr-daemon/src/error.rs`). The WebSocket listener for the phone lands here later.

mod codec;
mod connection;
mod context;
mod dispatch;
mod error;
mod hello;
mod subscriptions;
#[cfg(test)]
mod testing;
mod unix_listener;

pub use codec::ServerCodec;
pub use context::{ConnId, ConnectionContext, PeerCred};
pub use dispatch::{Dispatcher, Request, Responder};
pub use error::TransportError;
pub use subscriptions::{
    Delivery, LOSSY_QUEUE_FRAMES, LossyOffer, Offer, SUBSCRIBER_QUEUE_FRAMES, SubscriptionReceiver,
    SubscriptionSender, subscription,
};
pub use unix_listener::{Accepted, UnixListener};
