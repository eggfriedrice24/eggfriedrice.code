//! The real efr daemon in-process, for the integration tests of `efr-daemon` and
//! `efr-cli`.
//!
//! - [`TestDaemon`]: `efr_daemon::start` on temporary XDG roots with a socket in the
//!   temporary runtime directory, vt100 screens, a `TestClock`, a seeded `TestRng`, an
//!   in-memory (or file) store, the [`FakePtyHolder`] or the build's own PTY holder,
//!   and a `ReplayProvider` or the real OpenAI provider against a [`ResponsesServer`].
//!   It hands out `efr_client::Client`s and reads the event log back.
//! - [`FakePtyHolder`]: a `PtyHolder` over socketpairs, whose shell the test plays
//!   with scripted bytes ([`PtyScript`], [`FakeTerminal`]).
//! - [`Replay`]: drives a [`Scenario`], an NDJSON transcript from `fixtures/`, through a
//!   test daemon and checks every outbound record; [`Replay::bless`] rewrites one.
//!
//! This is a dev crate (tier T): only the `tests/` targets of `efr-daemon` and
//! `efr-cli` depend on it, never a `src/` file, because a library's unit tests that
//! linked it would hold two copies of the daemon's types.
//!
//! Allowed dependencies: `efr-daemon` (without its default features),
//! `efr-test-support`, `efr-client` and `efr-protocol`. What does not belong here:
//! assertions about one scenario (they live in the tests that replay it) and helpers
//! that a leaf crate's tests need (those belong in `efr-test-support`).

mod error;
mod pty_script;

pub use error::TestDaemonError;
pub use pty_script::{
    FakePtyHolder, FakeTerminal, PROMPT, PtyScript, PtyStep, command_output, typed_command,
};
