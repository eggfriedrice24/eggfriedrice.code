//! `efrd`, the efr daemon: the composition root.
//!
//! The daemon owns the conversations (one `efr-conversation` actor each), the hidden
//! shells (`efr-shell` over a PTY holder, with an `efr-screen` per shell), the event
//! log (`efr-store`), the providers and the login, and serves the protocol on the Unix
//! socket through `efr-transport`. [`run`] starts it in the order ARCHITECTURE.md
//! gives: the lock, the config, the migrated database, reconciliation, the actors, the
//! socket and `daemon.json`, then `READY=1`. One file per protocol method lives in
//! `methods/`, with the exhaustive scope match in `methods.rs`, and the one mapping of
//! daemon errors to wire errors is `From<DaemonError> for ErrorFrame` in `error.rs`.
//!
//! The library exists so that `efr-test-daemon` can run the real daemon in-process
//! with injected [`Deps`]; `main.rs` only parses flags, sets up tracing and calls
//! [`run`].
//!
//! Allowed dependencies: every library crate except `efr-client` and the test crates.
//! Feature-gated code lives only in `screens.rs` (`screen-ghostty`) and `shells.rs`
//! (`local-pty`). What does not belong here: logic that a library crate can own, such
//! as the turn loop, permission decisions, SQL, terminal emulation or the wire format.

mod config;
mod discovery;
mod error;
mod lock;

pub use config::{
    CONFIG_FILE, Config, ConversationSettings, DEFAULT_LOG, DEFAULT_PROVIDER,
    DEFAULT_SYSTEM_PROMPT, Flags, OpenAiSettings, ScreenChoice, ShellSettings, Source,
};
pub use error::DaemonError;
