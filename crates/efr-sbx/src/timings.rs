//! How long each step of a contained launch took, for `result.json` and so for efrd's
//! debug log (the `launcher_*` phase lines of docs/sandbox.md).

use std::time::Instant;

use efr_sandbox::LaunchTiming;

use crate::os;

/// The steps of one launch so far.
#[derive(Debug)]
pub(crate) struct Timings {
    last: Instant,
    steps: Vec<LaunchTiming>,
}

impl Timings {
    /// Starts the first step now.
    pub(crate) fn start() -> Timings {
        Timings { last: os::now(), steps: Vec::new() }
    }

    /// Ends the step `phase` now and starts the next one.
    pub(crate) fn lap(&mut self, phase: &str) {
        let now = os::now();
        let us = u64::try_from(now.duration_since(self.last).as_micros()).unwrap_or(u64::MAX);
        self.steps.push(LaunchTiming { phase: phase.to_owned(), us });
        self.last = now;
    }

    /// Adds the steps that another part timed, such as the launch, and starts the next
    /// step now.
    pub(crate) fn append(&mut self, steps: Vec<LaunchTiming>) {
        self.steps.extend(steps);
        self.last = os::now();
    }

    /// The steps, in order.
    pub(crate) fn into_steps(self) -> Vec<LaunchTiming> {
        self.steps
    }
}

#[cfg(test)]
mod tests;
