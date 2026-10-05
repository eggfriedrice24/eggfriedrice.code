//! The tracing subscriber of `efrd`: the one place the daemon's logs are set up.
//!
//! The filter is the config's `log` value (`EFR_LOG`, `--log`, the file, or `info`).
//! Under systemd, where `JOURNAL_STREAM` is set, events go to journald with their
//! fields; otherwise a compact layer writes to stderr, as under `just run`. The filter
//! sits in a reload layer, so a config reload changes it without a restart
//! ([`LogFilter::set`]).

use std::io::Write as _;

use efr_config::DEFAULT_LOG;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Registry, reload};

use crate::DaemonError;

/// Set by systemd for a service whose stdout or stderr is connected to the journal.
const JOURNAL_STREAM: &str = "JOURNAL_STREAM";

/// Where the logs went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogTarget {
    /// The systemd journal.
    Journald,
    /// Standard error.
    Stderr,
}

/// The running filter of the daemon's logs, which a config reload replaces.
#[derive(Debug, Clone)]
pub struct LogFilter {
    handle: reload::Handle<EnvFilter, Registry>,
}

impl LogFilter {
    /// Replaces the filter with `filter`, in `EnvFilter` syntax. An invalid filter
    /// changes nothing.
    pub(crate) fn set(&self, filter: &str) -> Result<(), DaemonError> {
        let filter = parse(filter)?;
        self.handle.reload(filter).map_err(|source| DaemonError::LogReload { source })
    }
}

/// `filter` as an `EnvFilter`, or why it is not one.
pub(crate) fn parse(filter: &str) -> Result<EnvFilter, DaemonError> {
    EnvFilter::try_new(filter)
        .map_err(|source| DaemonError::LogFilter { filter: filter.to_owned(), source })
}

/// Installs the subscriber for `filter` and returns where the logs go and the handle
/// that replaces the filter. An invalid filter falls back to `info` with a warning on
/// stderr, so a typo in `EFR_LOG` never leaves the daemon silent.
pub fn init(filter: &str) -> (LogTarget, LogFilter) {
    let env_filter = match parse(filter) {
        Ok(env_filter) => env_filter,
        Err(error) => {
            let reason = std::error::Error::source(&error).map(ToString::to_string);
            let _ = writeln!(
                std::io::stderr(),
                "efrd: the log filter {filter:?} is invalid ({}); using {DEFAULT_LOG:?}",
                reason.unwrap_or_default()
            );
            EnvFilter::new(DEFAULT_LOG)
        }
    };
    let (layer, handle) = reload::Layer::new(env_filter);
    let filter = LogFilter { handle };
    let registry = tracing_subscriber::registry().with(layer);
    // NOTE: JOURNAL_STREAM is a systemd convention, not an efr setting, so it is read
    // here and not through efr_stdx::env::Var.
    if std::env::var_os(JOURNAL_STREAM).is_some() {
        match tracing_journald::layer() {
            Ok(journald) => {
                // A second init fails only when a subscriber exists already, which is
                // then the one in charge.
                let _ =
                    registry.with(journald.with_syslog_identifier("efrd".to_owned())).try_init();
                return (LogTarget::Journald, filter);
            }
            Err(error) => {
                let _ = writeln!(
                    std::io::stderr(),
                    "efrd: the journal is not reachable ({error}); logging to stderr"
                );
            }
        }
    }
    let stderr =
        tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_target(true).compact();
    let _ = registry.with(stderr).try_init();
    (LogTarget::Stderr, filter)
}
