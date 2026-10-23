//! The integration tests of efr-sbx: the escape suite, the network view's escape tests,
//! the behaviour tests, the forwarder of a job with port forwards, the reach helper of
//! a call, the end of a call that efrd moved into a job, the probe and the corpus run of the spec's section 16.2, on the real bwrap,
//! Landlock and seccomp. They need `EFR_TEST_SBX_BIN`, which `just test-sandbox` sets,
//! and skip with a reason otherwise.
#![cfg(test)]

mod behaviour;
mod corpus;
mod cost;
mod dbus;
mod escape;
mod forward;
mod job;
mod mount_view;
mod moved;
mod netview;
mod perf;
mod probe;
mod proxy;
mod reach;
mod support;
