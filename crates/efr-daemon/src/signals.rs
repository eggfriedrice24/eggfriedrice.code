//! SIGTERM and SIGINT start a graceful shutdown; SIGHUP reloads the config.
//!
//! systemd stops the unit with SIGTERM; Ctrl+C under `just run` sends SIGINT. Either
//! cancels the token that `run` serves until, so the daemon stops accepting, drains its
//! connections and actors, and closes the database cleanly. `systemctl --user reload
//! efrd` sends SIGHUP (`ExecReload` in the unit), which reads `config.toml` again.

use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio_util::sync::CancellationToken;

use crate::DaemonError;

/// The stream of SIGHUPs. Installing it keeps SIGHUP from ending the process. Must be
/// called inside the runtime.
pub(crate) fn hangups() -> Result<Signal, DaemonError> {
    signal(SignalKind::hangup()).map_err(|source| DaemonError::Signals { source })
}

/// Installs the handlers and returns a token that is cancelled at the first SIGTERM or
/// SIGINT. Must be called inside the runtime.
pub fn shutdown_on_signals() -> Result<CancellationToken, DaemonError> {
    let mut terminate =
        signal(SignalKind::terminate()).map_err(|source| DaemonError::Signals { source })?;
    let mut interrupt =
        signal(SignalKind::interrupt()).map_err(|source| DaemonError::Signals { source })?;
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        let name = tokio::select! {
            _ = terminate.recv() => "SIGTERM",
            _ = interrupt.recv() => "SIGINT",
        };
        tracing::info!(signal = name, "shutting down");
        cancel.cancel();
    });
    Ok(token)
}
