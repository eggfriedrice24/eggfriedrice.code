//! Light helpers for the tests of every efr crate.
//!
//! - [`TestClock`]: an `efr_stdx` clock that moves only when a test moves it.
//! - [`TestRng`]: a seeded `efr_stdx` generator whose sequence never changes.
//! - [`TestDirs`]: the four efr roots and a home directory in a throwaway tree.
//! - [`Transcript`]: the NDJSON transcript reader and validator, with its [`Record`]s.
//! - [`fixtures`]: a crate's fixture files, found from `file!()`.
//!
//! This is a dev crate: a crate names it under `[dev-dependencies]` only, and no
//! shipped binary links it.
//!
//! Allowed dependencies: `efr-protocol`, `efr-store`, `efr-provider` and `efr-stdx`.
//! What does not belong here: the daemon, the CLI or a holder binary (`TestDaemon` and
//! the scenario driver live in `efr-test-daemon`), and any provider's API, which is
//! replayed at the HTTP level with wiremock instead.
//!
//! `fixtures` is a public module because its path is the one the structure document
//! names for every crate's tests: `efr_test_support::fixtures::path(file!(), "...")`.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod clock;
mod dirs;
mod error;
pub mod fixtures;
mod ndjson;
mod rng;

pub use clock::TestClock;
pub use dirs::TestDirs;
pub use error::TestSupportError;
pub use ndjson::{Entry, Inbound, Outbound, Record, Transcript};
pub use rng::TestRng;
