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
mod connections;
mod conversations;
mod discovery;
mod error;
mod gc;
mod lock;
mod methods;
mod notices;
mod providers;
mod ptys;
mod receipts;
mod reconcile;
mod run;
mod screens;
mod shells;
mod signals;
mod state;
mod telemetry;
#[cfg(test)]
mod testing;
mod tools;

pub use config::{
    CONFIG_FILE, Config, ConversationSettings, DEFAULT_LOG, DEFAULT_PROVIDER,
    DEFAULT_SYSTEM_PROMPT, Flags, OpenAiSettings, PermissionSettings, ScreenChoice, ShellSettings,
    Source,
};
pub use efr_conversation::HostInfo;
// NOTE: the trait's vocabulary comes with it, so efr-test-daemon, whose allowlist has
// no efr-holder edge, can inject a holder of its own through `Deps::with_holder`.
pub use efr_holder::{
    ChildStatus, HolderError, PtyHandle, PtyHolder, PtyInfo, Signal, SignalTarget, SpawnSpec,
};
pub use efr_provider::Provider;
pub use efr_shell::ScreenFactory;
pub use error::DaemonError;
pub use providers::{API, ProviderFactory, SUBSCRIPTION};
pub use run::{Daemon, Deps, run, start};
pub use signals::shutdown_on_signals;
pub use telemetry::{LogTarget, init as init_telemetry};
