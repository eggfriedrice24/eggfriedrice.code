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

mod catalog;
mod config;
mod connections;
mod conversations;
mod discovery;
mod engine;
mod error;
mod gc;
mod lock;
mod methods;
mod notices;
mod projects;
mod providers;
mod ptys;
mod receipts;
mod reconcile;
mod reload;
mod run;
mod runtime;
mod sandbox;
mod screens;
mod settings;
mod shells;
mod signals;
mod state;
mod telemetry;
#[cfg(test)]
mod testing;
mod tools;

pub use config::{Flags, load_settings, resolve_settings};
// NOTE: the settings types come with the daemon, so efr-test-daemon, whose allowlist
// has no efr-config edge, can start a daemon with settings of its own.
pub use efr_config::{
    CONFIG_FILE, ConversationSettings, DEFAULT_LOG, DEFAULT_PROVIDER, DEFAULT_SYSTEM_PROMPT,
    ModelSettings, OpenAiSettings, PermissionSettings, ScreenChoice, Settings, ShellSettings,
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
pub use runtime::build_runtime;
pub use sandbox::seams::TEST_SEAMS;
pub use signals::shutdown_on_signals;
pub use telemetry::{LogFilter, LogTarget, init as init_telemetry};
