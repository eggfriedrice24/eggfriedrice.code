//! The snapshot store end to end on real git: calls and turns in a git project and in
//! a plain directory, what is left out, the project's own `.git` untouched, the diff
//! of a turn, the collector and the cost per call.

#![cfg(test)]

mod calls;
mod cost;
mod gc;
mod support;
mod turns;
