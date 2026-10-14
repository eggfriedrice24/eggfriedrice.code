//! The network proxy of the `auto` sandbox (spec 8.2, phase D2): efrd serves one Unix
//! socket per contained call, and the call's bridge carries the tools' HTTPS and plain
//! HTTP connections to it.
//!
//! - [`Proxy`] holds the [`Deps`] and the live global host list; [`Proxy::serve`] starts
//!   one call's listener with its [`CallPolicy`] and gives a [`CallProxy`], whose
//!   [`close`](CallProxy::close) gives the [`CallReport`] with the refusals.
//! - A `CONNECT` goes in the order of DP6: the host form, the budget, the lists and the
//!   port rule before any lookup, then the lookup, the address guard, the connect,
//!   `200`, the ClientHello and its server name, and the bytes both ways under the
//!   call's upload budget. A plain `GET` or `HEAD` goes to an http host with a head
//!   that the proxy writes itself (DP8).
//! - [`head`] and [`hello`] are hand parsers of a strict subset with hard caps: no TLS
//!   library and no third-party HTTP parser touches the untrusted bytes.
//! - [`classify`] and [`Guard`] are the address guard: the proxy connects only to a
//!   global address that is not the host's own and not on one of its links (DP7).
//! - [`Resolver`], [`Connector`] and [`OwnAddresses`] are the seams to the system;
//!   [`SystemResolver`], [`SystemConnector`] and [`SystemAddresses`] are the real ones,
//!   which only efrd builds.
//!
//! Allowed dependencies: `efr-protocol` (the call id, the refusal reasons),
//! `efr-permissions` (the host list and the host syntax), `efr-stdx` (the clock).
//! What does not belong here: the config, the files of a call, the bridge (`efr-sbx`),
//! and any decision about which call gets the proxy (efrd).

#![forbid(unsafe_code)]
// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod call;
mod conn;
mod deps;
mod guard;
pub mod head;
pub mod hello;
mod plain;
mod proxy;
mod records;
mod reply;
mod system;
mod tunnel;
mod upstream;

pub use call::{CallProxy, CallReport};
pub use deps::{Connecting, Connector, Deps, Limits, OwnAddresses, Resolver, Resolving};
pub use guard::{AddressClass, Guard, classify};
pub use proxy::{CallPolicy, Proxy};
pub use system::{SystemAddresses, SystemConnector, SystemResolver};
