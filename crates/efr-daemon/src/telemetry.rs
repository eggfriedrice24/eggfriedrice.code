//! The tracing subscriber of `efrd`: the one place the daemon's logs are set up.
//!
//! The filter is the config's `log` value (`EFR_LOG`, `--log`, the file, or `info`).
//! Under systemd, where `JOURNAL_STREAM` is set, events go to journald with their
//! fields; otherwise a compact layer writes to stderr, as under `just run`.

use std::io::Write as _;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::config::DEFAULT_LOG;

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

/// Installs the subscriber for `filter` and returns where the logs go. An invalid
/// filter falls back to `info` with a warning on stderr, so a typo in `EFR_LOG` never
/// leaves the daemon silent.
pub fn init(filter: &str) -> LogTarget {
    let env_filter = match EnvFilter::try_new(filter) {
        Ok(env_filter) => env_filter,
        Err(error) => {
            let _ = writeln!(
                std::io::stderr(),
                "efrd: the log filter {filter:?} is invalid ({error}); using {DEFAULT_LOG:?}"
            );
            EnvFilter::new(DEFAULT_LOG)
        }
    };
    let registry = tracing_subscriber::registry().with(env_filter);
    // NOTE: JOURNAL_STREAM is a systemd convention, not an efr setting, so it is read
    // here and not through efr_stdx::env::Var.
    if std::env::var_os(JOURNAL_STREAM).is_some() {
        match tracing_journald::layer() {
            Ok(journald) => {
                // A second init fails only when a subscriber exists already, which is
                // then the one in charge.
                let _ =
                    registry.with(journald.with_syslog_identifier("efrd".to_owned())).try_init();
                return LogTarget::Journald;
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
    LogTarget::Stderr
}
