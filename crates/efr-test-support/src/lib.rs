//! Light helpers for the tests of every efr crate.
//!
//! - [`bench`](mod@bench): samples and their percentiles, the report of a `bench_perf_*` bench
//!   with its JSON lines, the slope of a cost over its input size, and the resident
//!   memory of a process.
//! - [`TestClock`]: an `efr_stdx` clock that moves only when a test moves it.
//! - [`TestRng`]: a seeded `efr_stdx` generator whose sequence never changes.
//! - [`TestDirs`]: the four efr roots and a home directory in a throwaway tree.
//! - [`TestStore`]: the real `efr-store` over an in-memory database.
//! - [`Transcript`]: the NDJSON transcript reader and validator, with its [`Record`]s.
//! - [`fixtures`]: a crate's fixture files, found from `file!()`.
//! - [`Redactor`]: placeholders for the temporary cwd, the scratch path, the host name
//!   and timestamps, and back.
//! - [`ReplayProvider`]: an `efr_provider::Provider` that answers from a transcript and
//!   checks every request against it.
//! - [`SearchCorpus`]: a seeded corpus of conversations of a [`CorpusSize`], as events,
//!   for the search benches.
//! - [`PerfCorpus`]: a seeded corpus of long conversations of a [`PerfSize`] or a
//!   [`PerfShape`], with the saved messages and a compaction, written into a real store
//!   for the `bench_perf_*` benches.
//! - [`Wait`]: polls a condition that another task or thread makes true, and gives up
//!   after a real time limit ([`WAIT_LIMIT`]) with an error that names it.
//!
//! This is a dev crate: a crate names it under `[dev-dependencies]` only, and no
//! shipped binary links it.
//!
//! Allowed dependencies: `efr-protocol`, `efr-store`, `efr-provider` and `efr-stdx`.
//! What does not belong here: the daemon, the CLI or a holder binary (`TestDaemon` and
//! the scenario driver live in `efr-test-daemon`), and any provider's API, which is
//! replayed at the HTTP level with wiremock instead.
//!
//! `bench` is a public module because its names (`run`, `slope`, `Report`) read as
//! `bench::run` at the call site.
//!
//! `fixtures` is a public module because its path is the one the structure document
//! names for every crate's tests: `efr_test_support::fixtures::path(file!(), "...")`.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

pub mod bench;
mod clock;
mod dirs;
mod error;
pub mod fixtures;
mod ndjson;
mod perf_corpus;
mod redact;
mod replay_provider;
mod rng;
mod search_corpus;
mod store;
mod wait;

pub use clock::TestClock;
pub use dirs::TestDirs;
pub use error::TestSupportError;
pub use ndjson::{Entry, Inbound, Outbound, Record, Transcript};
pub use perf_corpus::{
    PERF_CORPUS_VERSION, PERF_MODEL, PERF_PROVIDER, PerfCorpus, PerfShape, PerfSize, PerfTurn,
    PerfWritten, open_store_file,
};
pub use redact::Redactor;
pub use replay_provider::ReplayProvider;
pub use rng::TestRng;
pub use search_corpus::{CORPUS_VERSION, CorpusConversation, CorpusSize, SearchCorpus};
pub use store::TestStore;
pub use wait::{WAIT_LIMIT, Wait};
