//! The snapshot store end to end on real git: calls and turns in a git project and in
//! a plain directory, what is left out, the project's own `.git` untouched, the diff
//! of a turn, the collector, the cost per call and the log lines of a failed add.

#![cfg(test)]

mod bytes;
mod calls;
mod cost;
mod gc;
mod logs;
mod support;
mod turns;
