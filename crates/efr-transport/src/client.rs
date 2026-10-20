//! The client side of the efr protocol, for `efr`, the test daemon and later the PTY
//! proxy.
//!
//! - [`discover`]: finds the daemon's socket from `daemon.json` ([`DaemonInfo`]) or the
//!   default path, and refuses a daemon of another protocol version before connecting.
//! - [`Client::connect`]: connects over the Unix socket and says hello with
//!   [`ConnectOptions`]; [`Client::call`] runs a unary method, [`Client::stream`] a
//!   streaming one as an [`ItemStream`], and [`Client::cancel`] stops a request.
//! - [`ClientCodec`]: the tokio codec over `efr_protocol::framing`.
//!
//! This module uses the daemon's side of the crate only in its tests, whose fake
//! daemons are built from the real server side. What does not belong here: the
//! daemon's side of the protocol (the rest of this crate), command-line parsing and
//! output (`efr-cli`), and the WebSocket transport, which arrives with the phone
//! milestone as `ws.rs`.

mod codec;
mod connection;
mod discovery;
mod error;
#[cfg(test)]
mod testing;
mod unix;

pub use codec::ClientCodec;
pub use connection::{Client, ConnectOptions, ItemStream};
pub use discovery::{DaemonInfo, Discovered, discover, read_daemon_json};
pub use error::ClientError;
