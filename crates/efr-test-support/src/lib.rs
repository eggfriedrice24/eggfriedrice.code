//! Light helpers for the tests of every efr crate.
//!
//! - [`TestClock`]: an `efr_stdx` clock that moves only when a test moves it.
//!
//! This is a dev crate: a crate names it under `[dev-dependencies]` only, and no
//! shipped binary links it.
//!
//! Allowed dependencies: `efr-protocol`, `efr-store`, `efr-provider` and `efr-stdx`.
//! What does not belong here: the daemon, the CLI or a holder binary (`TestDaemon` and
//! the scenario driver live in `efr-test-daemon`), and any provider's API, which is
//! replayed at the HTTP level with wiremock instead.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod clock;

pub use clock::TestClock;
