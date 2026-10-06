//! The integration tests of efr-sbx: the escape suite, the behaviour tests, the probe
//! and the corpus run of the spec's section 16.2, on the real bwrap, Landlock and
//! seccomp. They need `EFR_TEST_SBX_BIN`, which `just test-sandbox` sets, and skip with
//! a reason otherwise.
#![cfg(test)]

mod behaviour;
mod corpus;
mod cost;
mod escape;
mod probe;
mod support;
