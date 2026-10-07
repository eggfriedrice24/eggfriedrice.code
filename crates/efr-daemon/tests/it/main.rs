//! The integration tests of efr-daemon: one test binary with one module per area
//! (CONVENTIONS.md, tests), so the daemon is linked once instead of once per file.
//! nextest names a test `efr-daemon::it <module>::<test>`; `-E 'test(/^hello::/)'`
//! picks one module.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

mod approvals;
mod backtrace;
mod hello;
mod input_respond;
mod interrupt;
mod login;
mod prompt_send;
mod pty_attach;
mod receipts;
mod reconcile;
mod sandbox;
mod shell_tool;
mod subscribe;
mod support;
