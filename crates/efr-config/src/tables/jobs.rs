//! `[jobs]`: background jobs, the long commands that the model starts with
//! `background: true` and that run on after their turn (efr's auto spec, sections 3.18
//! and 13.1, phase C).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[jobs]`: whether the model can start background jobs, their limits, and how often
/// their notices may start a turn. Every key applies from the next job or notice; a job
/// that runs keeps the limits of its start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct JobsSettings {
    /// `false`: a shell call with `background: true` fails with a reason, and the model
    /// runs the command in the foreground. Jobs that run go on.
    pub enabled: bool,
    /// The time limit of a job in minutes when the model names none, from 1 to
    /// `max_minutes`.
    pub default_minutes: u32,
    /// The longest time limit that the model can ask for, in minutes, from 1 to 1440.
    /// A longer one is cut to it.
    pub max_minutes: u32,
    /// The most jobs that run at one time in one conversation, from 1 to 64.
    pub max_running: u32,
    /// The most jobs that run at one time in all conversations, from 1 to 256.
    pub max_running_daemon: u32,
    /// The most turns per hour that job notices start in one conversation, from 0 to
    /// 60. 0: a notice never starts a turn; it waits for your next prompt.
    pub max_wakes_per_hour: u32,
    /// The most turns per hour that job notices start in all conversations, from 0 to
    /// 120. 0: a notice never starts a turn.
    pub max_wakes_per_hour_daemon: u32,
    /// The most times that one `watch` pattern of a job can wake the model, from 0 to
    /// 10. A higher `max_wakes` of the model is cut to it.
    pub max_wakes_per_watch: u32,
    /// The CPU priority (nice value) of every job, from 0 to 19; 19 is the lowest. A
    /// job never runs at a higher priority than efrd. The disk priority of a job is
    /// always the lowest (the idle class).
    pub nice: u8,
}

impl Default for JobsSettings {
    fn default() -> Self {
        JobsSettings {
            enabled: true,
            default_minutes: 60,
            max_minutes: 480,
            max_running: 4,
            max_running_daemon: 16,
            max_wakes_per_hour: 6,
            max_wakes_per_hour_daemon: 12,
            max_wakes_per_watch: 3,
            nice: 19,
        }
    }
}
