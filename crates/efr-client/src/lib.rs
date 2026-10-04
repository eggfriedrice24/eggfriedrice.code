//! The client side of the efr protocol, for `efr`, the test daemon and later the PTY
//! proxy.
//!
//! - [`ClientCodec`]: the tokio codec over `efr_protocol::framing`.
//!
//! Allowed dependencies: `efr-protocol` and `efr-stdx`. What does not belong here: the
//! daemon's side of the protocol (`efr-transport`, which this crate may use only in its
//! tests), command-line parsing and output (`efr-cli`), and the WebSocket transport,
//! which arrives with the phone milestone as `ws.rs`.

mod codec;
mod error;

pub use codec::ClientCodec;
pub use error::ClientError;
