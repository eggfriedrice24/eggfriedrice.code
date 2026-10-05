//! The integration tests of efr-cli: one test binary with one module per area
//! (CONVENTIONS.md, tests). `binary` runs the built `efr` against a fake daemon,
//! `plugin` drives the zsh plugin and `smoke` runs `efr` against a real daemon.
//! nextest names a test `efr-cli::it <module>::<test>`; `-E 'test(/^plugin::/)'`
//! picks one module.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

mod binary;
mod plugin;
mod smoke;
