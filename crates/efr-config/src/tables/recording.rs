//! `[recording]`: how long efrd keeps the recordings of the hidden shells' terminals,
//! which `pty.attach` resumes from (v0.0.5).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[recording]`: the age and the size after which efrd deletes the recordings of the
/// hidden shells' terminals, once an hour. It always keeps the newest segment, and at
/// least the last 1 MiB, of a shell that still runs. Every key applies from the next
/// pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct RecordingSettings {
    /// Days after its shell exited after which the recording of a hidden shell is
    /// deleted, from 0 to 3650; 0 keeps it.
    pub max_age_days: u32,
    /// The most GiB that all recordings may take, from 0 to 10000; 0 sets no limit. Past
    /// it, efrd deletes the oldest segments first, but never the newest segment or the
    /// last 1 MiB of a shell that still runs.
    pub max_gib: u32,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        RecordingSettings { max_age_days: 14, max_gib: 2 }
    }
}
