//! The snapshot store end to end on real git: calls and turns in a git project and in
//! a plain directory, what is left out, the project's own `.git` untouched, the diff
//! of a turn, the collector, the cost per call, the log lines of a failed add, and the
//! `bench_perf_*` benches (ignored; `just bench-perf` runs them).

#![cfg(test)]

mod bytes;
mod calls;
mod cost;
mod cover;
mod gc;
mod home;
mod jobs;
mod logs;
mod mirror;
mod perf;
mod refresh;
mod support;
mod turns;
mod undo;
