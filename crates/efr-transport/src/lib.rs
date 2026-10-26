//! Both sides of the protocol's transport: the daemon's edge, without the engine, and
//! the client side in [`client`].
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
//! - [`serve_websocket`]: one WebSocket connection of the phone, on a stream that the
//!   daemon accepted and admitted: the bounded handshake (refusals are
//!   [`WebSocketRefusal`]), one JSON frame per text message, the server's pings, and
//!   the same connection loop as the Unix socket.
//! - [`ServerCodec`]: the tokio codec over `efr_protocol::framing`.
//! - [`ConnectionContext`]: `{ surface, peer, conn_id }`, carried by every request. The
//!   [`Peer`] is local (the kernel's uid and pid) or remote (a WebSocket peer).
//! - [`client`]: the client side for `efr`, the test daemon and later the PTY proxy:
//!   daemon discovery, the connection, hello, calls, streams and cancellation.
//!
//! Allowed dependencies: `efr-protocol` and `efr-stdx`. What does not belong here: the
//! meaning of any method (the daemon's `methods/`), the database (`efr-transport ->
//! efr-store` is a forbidden edge), the mapping from daemon errors to wire errors
//! (`efr-daemon/src/error.rs`), and the TCP listener and the peer check of the WebSocket
//! (the daemon's, because they need tailscaled).

pub mod client;
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
mod ws;

pub use codec::ServerCodec;
pub use context::{ConnId, ConnectionContext, Peer, PeerCred, RemotePeer};
pub use dispatch::{Dispatcher, Request, Responder};
pub use error::TransportError;
pub use subscriptions::{
    Delivery, LOSSY_QUEUE_FRAMES, LossyOffer, Offer, SUBSCRIBER_QUEUE_FRAMES, SubscriptionReceiver,
    SubscriptionSender, subscription,
};
pub use unix_listener::{Accepted, UnixListener};
pub use ws::{WebSocketCloseReason, WebSocketOptions, WebSocketRefusal, serve_websocket};
