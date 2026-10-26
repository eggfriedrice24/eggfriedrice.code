//! The real efr daemon in-process, for the integration tests of `efr-daemon` and
//! `efr-cli`.
//!
//! - [`TestDaemon`]: `efr_daemon::start` on temporary XDG roots with a socket in the
//!   temporary runtime directory, vt100 screens, a `TestClock`, a seeded `TestRng`, an
//!   in-memory (or file) store, the [`FakePtyHolder`] or the build's own PTY holder,
//!   and a `ReplayProvider` or the real OpenAI provider against a [`ResponsesServer`].
//!   It hands out `efr_transport::client::Client`s and reads the event log back.
//! - [`MessagesServer`]: a stand-in for Anthropic's Messages API and its model list,
//!   for a daemon that runs the Anthropic provider.
//! - [`FakeLauncher`]: a `/bin/sh` script in place of `efr-sbx`, which runs calls and
//!   background jobs with no sandbox; [`TestDaemonBuilder::job_env`] gives every job an
//!   environment, and [`TestDaemon::peer_side`] says where a process stands on the socket.
//! - [`FakeForwarder`]: the forwarder of a job's port forwards, which a test plays on the
//!   job's forward socket, with an [`echo_server`] behind each port;
//!   [`TestDaemon::host_ports`] says which free port stands for a forwarded port.
//! - [`FakeReach`]: the reach helper of a contained call, which a test plays on the
//!   call's reach socket with no TCP port.
//! - [`FakeProcView`]: the process facts that routing reads (which shell runs, which
//!   terminal a peer runs on), set by the test; [`TestDaemonBuilder::proc_view`].
//! - [`FakePtyHolder`]: a `PtyHolder` over socketpairs, whose shell the test plays
//!   with scripted bytes ([`PtyScript`], [`FakeTerminal`]).
//! - [`FakePhone`]: the phone app on efrd's remote listener, a WebSocket on a real
//!   loopback TCP stream, with a software P-256 [`PhoneKey`];
//!   [`TestDaemonBuilder::remote`] turns the listener on with a fake tailscaled.
//! - [`Replay`]: drives a [`Scenario`], an NDJSON transcript from `fixtures/`, through a
//!   test daemon and checks every outbound record; [`Replay::bless`] rewrites one.
//!
//! This is a dev crate (tier T): only the `tests/` targets of `efr-daemon` and
//! `efr-cli` depend on it, never a `src/` file, because a library's unit tests that
//! linked it would hold two copies of the daemon's types.
//!
//! Allowed dependencies: `efr-daemon` (without its default features),
//! `efr-test-support`, `efr-transport` (its `client` module) and `efr-protocol`. What
//! does not belong here: assertions about one scenario (they live in the tests that
//! replay it) and helpers that a leaf crate's tests need (those belong in
//! `efr-test-support`).

mod error;
mod fake_forwarder;
mod fake_launcher;
mod fake_phone;
mod fake_procs;
mod fake_reach;
mod pty_script;
mod replay;
mod test_daemon;

// NOTE: the client types are part of this crate's API (a test daemon hands out
// clients), so its users need no efr-transport edge of their own.
pub use efr_transport::client::{Client, ClientError, ConnectOptions, ItemStream};
// NOTE: the hook of `TestDaemonBuilder::search_hook` is efrd's own type, and the tests
// of efr-cli may not name efr-daemon.
pub use efr_daemon::SearchRowHook;
pub use error::TestDaemonError;
pub use fake_forwarder::{FakeForwarder, connect_socket, echo_server};
pub use fake_launcher::FakeLauncher;
pub use fake_phone::{FakePhone, PhoneKey};
pub use fake_procs::FakeProcView;
pub use fake_reach::{FakeReach, Told};
pub use pty_script::{
    FakePtyHolder, FakeTerminal, PROMPT, PtyScript, PtyStep, command_output, typed_command,
};
pub use replay::{
    Bindings, REFRESHED_ACCESS_TOKEN, REFRESHED_REFRESH_TOKEN, Replay, SCENARIOS, Scenario,
    ScenarioSpec,
};
pub use test_daemon::{
    ANTHROPIC_API_KEY, ANTHROPIC_CREDENTIAL, API_KEY, DEFAULT_SEED, HOME_PLACEHOLDER, HOSTNAME,
    MessagesAnswer, MessagesRequest, MessagesServer, ModelsAnswer, ModelsPageRequest,
    ModelsRequest, OS, PeerSide, ReceivedRequest, ResponsesAnswer, ResponsesServer,
    SUBSCRIPTION_ACCESS_TOKEN, SUBSCRIPTION_ACCOUNT, SUBSCRIPTION_CREDENTIAL,
    SUBSCRIPTION_REFRESH_TOKEN, SYSTEM_PROMPT, TTY, TestDaemon, TestDaemonBuilder, TokenRequest,
    command_id, events_until,
};
