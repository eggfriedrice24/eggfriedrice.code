//! The integration tests of efr-daemon: one test binary with one module per area
//! (CONVENTIONS.md, tests), so the daemon is linked once instead of once per file.
//! nextest names a test `efr-daemon::it <module>::<test>`; `-E 'test(/^hello::/)'`
//! picks one module.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

mod anthropic;
mod apply_patch;
mod approvals;
mod backtrace;
mod bus;
mod cache_prefix;
mod catalog;
mod classifier_eval;
mod compact;
mod context;
mod drafts;
mod full;
mod hello;
mod home;
mod input_respond;
mod interrupt;
mod jobs;
mod jobs_forward;
mod jobs_moved;
mod jobs_undo;
mod jobs_wake;
mod login;
mod netview;
mod presence;
mod prompt_send;
mod proxy;
mod pty_attach;
mod reach;
mod receipts;
mod reconcile;
mod resume;
mod routing;
mod sandbox;
mod scope_marker;
mod shell_tool;
mod snapshots;
mod subscribe;
mod support;
mod turn_input;
mod undo;
mod undo_method;
