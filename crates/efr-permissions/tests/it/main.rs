//! The integration tests of the permission engine: one test binary (CONVENTIONS.md,
//! tests).

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

mod corpus;
