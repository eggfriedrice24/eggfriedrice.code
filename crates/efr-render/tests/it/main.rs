//! The integration tests of the renderer: one test binary (CONVENTIONS.md, tests).
//! `design` compares the output with the frames of the terminal design, `perf` holds
//! the benches.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

mod design;
mod perf;
