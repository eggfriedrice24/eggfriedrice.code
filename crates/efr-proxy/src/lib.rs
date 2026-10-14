//! The network proxy of the `auto` sandbox (spec 8.2, phase D2): efrd serves one Unix
//! socket per contained call, and the call's bridge carries the tools' HTTPS and plain
//! HTTP connections to it.
//!
//! - [`head`] reads the request head: a `CONNECT host:port`, or an absolute-form `GET`
//!   or `HEAD` for plain HTTP. [`hello`] reads the TLS ClientHello of a tunnel for its
//!   server name. Both are hand parsers of a strict subset with hard caps: no TLS
//!   library and no third-party HTTP parser touches the untrusted bytes.
//! - [`classify`] and [`Guard`] are the address guard: the proxy connects only to a
//!   global address that is not the host's own and not on one of its links.
//!
//! Allowed dependencies: `efr-protocol` (the refusal reasons), `efr-permissions` (the
//! host list and the host syntax). What does not belong here: the config, the files of
//! a call, the bridge (`efr-sbx`), and any decision about which call gets the proxy
//! (efrd).

#![forbid(unsafe_code)]
// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod guard;
pub mod head;
pub mod hello;

pub use guard::{AddressClass, Guard, classify};
