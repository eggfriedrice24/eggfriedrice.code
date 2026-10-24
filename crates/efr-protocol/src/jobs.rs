//! Background jobs on the wire (efr's auto spec, section 3.18, phase C): a `shell` call
//! with `background: true` starts a job that runs on after its turn, until it ends, its
//! time limit passes or someone stops it.
//!
//! efrd starts and stops jobs; the actor delivers their notices to the model. These
//! types are what the events, `jobs.list`, `jobs.stop` and the tools name. Every enum
//! here reads a value that this build does not know as its `Unknown` variant, so an old
//! client keeps working with a newer daemon.

use std::borrow::Cow;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use jiff::Timestamp;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

use crate::{CallId, ConversationId, ExitInfo, Launch, ProtocolError, TurnId};

/// The id of a job: `j1`, `j2`, ... in the order in which the jobs of one conversation
/// started. Ids repeat across conversations, so a job is named by its conversation and
/// its id.
///
/// On the wire a string. A string that is not `j` and a number from 1 to 999999999
/// without a leading zero is refused.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(String);

impl JobId {
    /// The largest job number.
    pub const MAX_NUMBER: u32 = 999_999_999;

    /// The id of the `n`th job of a conversation, `j{n}`. `n` is at least 1 and at most
    /// [`MAX_NUMBER`](Self::MAX_NUMBER); a value outside takes the nearest end.
    pub fn nth(n: u32) -> JobId {
        JobId(format!("j{}", n.clamp(1, Self::MAX_NUMBER)))
    }

    /// The number of the job, such as 3 for `j3`.
    pub fn number(&self) -> Option<u32> {
        self.0.strip_prefix('j').and_then(|digits| digits.parse().ok())
    }

    /// The id as text, such as `j3`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when `text` is a job id: `j`, then a digit from 1 to 9, then at most eight
    /// digits.
    fn valid(text: &str) -> bool {
        let Some(digits) = text.strip_prefix('j') else {
            return false;
        };
        (1..=9).contains(&digits.len())
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && !digits.starts_with('0')
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for JobId {
    type Err = ProtocolError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if JobId::valid(text) {
            Ok(JobId(text.to_owned()))
        } else {
            Err(ProtocolError::InvalidJobId { value: text.to_owned() })
        }
    }
}

impl Serialize for JobId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for JobId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(de::Error::custom)
    }
}

impl JsonSchema for JobId {
    fn schema_name() -> Cow<'static, str> {
        "JobId".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^j[1-9][0-9]{0,8}$",
            "description": "The id of a job of one conversation, such as j3."
        })
    }
}

/// Where a job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobState {
    /// The job runs.
    Running,
    /// The job's command ended by itself; `exit_code` says how.
    Exited,
    /// Someone or something stopped the job; `stopped_by` says who.
    Stopped,
    /// The job reached its time limit and efr stopped it.
    TimedOut,
    /// The job could not start, or its launcher failed.
    Failed,
    /// A state that this build does not know.
    #[serde(other)]
    Unknown,
}

impl JobState {
    /// Every state, in the order of the enum.
    pub const ALL: [JobState; 6] = [
        JobState::Running,
        JobState::Exited,
        JobState::Stopped,
        JobState::TimedOut,
        JobState::Failed,
        JobState::Unknown,
    ];

    /// The wire form, such as `timed_out`.
    pub const fn as_str(self) -> &'static str {
        match self {
            JobState::Running => "running",
            JobState::Exited => "exited",
            JobState::Stopped => "stopped",
            JobState::TimedOut => "timed_out",
            JobState::Failed => "failed",
            JobState::Unknown => "unknown",
        }
    }

    /// True for a job that no longer runs.
    pub const fn has_ended(self) -> bool {
        !matches!(self, JobState::Running)
    }
}

/// Who or what stopped a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobStopBy {
    /// The model, with the `job_stop` tool.
    Model,
    /// The user: `efr jobs stop` or the stop key of the view (`jobs.stop`).
    User,
    /// The job's time limit.
    TimeLimit,
    /// efrd stopped.
    DaemonStop,
    /// efrd stopped while the job ran, and its next start found the job.
    DaemonRestart,
    /// The surface guard found a git setting or another file that runs code, which the
    /// job changed.
    Guard,
    /// A call of full mode started the job, and a config reload turned full mode off
    /// (`permissions.allow_full`, `undo.enabled` or `snapshot.enabled`). Its notice
    /// starts no turn of its own and waits for the next prompt.
    FullOff,
    /// A cause that this build does not know.
    #[serde(other)]
    Unknown,
}

impl JobStopBy {
    /// Every cause, in the order of the enum.
    pub const ALL: [JobStopBy; 8] = [
        JobStopBy::Model,
        JobStopBy::User,
        JobStopBy::TimeLimit,
        JobStopBy::DaemonStop,
        JobStopBy::DaemonRestart,
        JobStopBy::Guard,
        JobStopBy::FullOff,
        JobStopBy::Unknown,
    ];

    /// The wire form, such as `time_limit`.
    pub const fn as_str(self) -> &'static str {
        match self {
            JobStopBy::Model => "model",
            JobStopBy::User => "user",
            JobStopBy::TimeLimit => "time_limit",
            JobStopBy::DaemonStop => "daemon_stop",
            JobStopBy::DaemonRestart => "daemon_restart",
            JobStopBy::Guard => "guard",
            JobStopBy::FullOff => "full_off",
            JobStopBy::Unknown => "unknown",
        }
    }
}

/// What a job notice tells the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NoticeKind {
    /// The job ended.
    Exited,
    /// A line of the job's output matched its `watch` pattern.
    Watch,
    /// A kind that this build does not know.
    #[serde(other)]
    Unknown,
}

impl NoticeKind {
    /// Every kind, in the order of the enum.
    pub const ALL: [NoticeKind; 3] = [NoticeKind::Exited, NoticeKind::Watch, NoticeKind::Unknown];
}

/// Why a job notice did not start a turn of its own: it waits for the next turn that a
/// prompt starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HeldReason {
    /// The conversation was not in `auto`. Only older builds record it: a wake turn now
    /// runs in the mode of the newest prompt.
    NotAuto,
    /// The conversation or the daemon reached its limit of turns that notices start
    /// per hour.
    WakeLimit,
    /// A turn that a notice started started the job, so its notices never start a
    /// turn.
    WakeTurnJob,
    /// efrd restarted while the job ran.
    DaemonRestart,
    /// `jobs.max_wakes_per_hour` is 0: notices never start a turn.
    WakesOff,
    /// A reason that this build does not know.
    #[serde(other)]
    Unknown,
}

impl HeldReason {
    /// Every reason, in the order of the enum.
    pub const ALL: [HeldReason; 6] = [
        HeldReason::NotAuto,
        HeldReason::WakeLimit,
        HeldReason::WakeTurnJob,
        HeldReason::DaemonRestart,
        HeldReason::WakesOff,
        HeldReason::Unknown,
    ];

    /// The wire form, such as `wake_limit`.
    pub const fn as_str(self) -> &'static str {
        match self {
            HeldReason::NotAuto => "not_auto",
            HeldReason::WakeLimit => "wake_limit",
            HeldReason::WakeTurnJob => "wake_turn_job",
            HeldReason::DaemonRestart => "daemon_restart",
            HeldReason::WakesOff => "wakes_off",
            HeldReason::Unknown => "unknown",
        }
    }
}

/// A pattern that wakes the model when a line of the job's output matches it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Watch {
    /// The pattern, a regular expression of at most
    /// [`MAX_PATTERN_CHARS`](Self::MAX_PATTERN_CHARS) characters.
    pub pattern: String,
    /// How many matches may wake the model, after efr's own limit
    /// (`jobs.max_wakes_per_watch`).
    pub max_wakes: u8,
}

impl Watch {
    /// The longest pattern, in characters.
    pub const MAX_PATTERN_CHARS: usize = 200;
}

/// A loopback port of a contained job that efr forwards to the same port of
/// `127.0.0.1` on the user's machine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PortForward {
    /// The address that the job listens on, as text: `127.0.0.1`, `::1`.
    pub listen: String,
    /// The port, the same in the job and on the user's machine.
    pub port: u16,
}

/// What efr did with a loopback port that a contained job listens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ForwardOutcome {
    /// efr forwards the port to the same port of `127.0.0.1`.
    Forwarded,
    /// The port is in use on the user's machine, or by another job.
    InUse,
    /// efr never forwards this port, such as a service port.
    Refused,
    /// The job or efrd forwards as many ports as it may.
    Limit,
    /// efr no longer forwards the port, such as after `jobs.forward_ports` turned off.
    Stopped,
    /// An outcome that this build does not know.
    #[serde(other)]
    Unknown,
}

impl ForwardOutcome {
    /// Every outcome, in the order of the enum.
    pub const ALL: [ForwardOutcome; 6] = [
        ForwardOutcome::Forwarded,
        ForwardOutcome::InUse,
        ForwardOutcome::Refused,
        ForwardOutcome::Limit,
        ForwardOutcome::Stopped,
        ForwardOutcome::Unknown,
    ];

    /// The wire form, such as `in_use`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ForwardOutcome::Forwarded => "forwarded",
            ForwardOutcome::InUse => "in_use",
            ForwardOutcome::Refused => "refused",
            ForwardOutcome::Limit => "limit",
            ForwardOutcome::Stopped => "stopped",
            ForwardOutcome::Unknown => "unknown",
        }
    }
}

/// One job, as `jobs.list`, `jobs.stop` and `job_started` show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobInfo {
    /// The job's id in its conversation.
    pub id: JobId,
    /// The conversation.
    pub conversation_id: ConversationId,
    /// The turn whose call started the job.
    pub turn_id: TurnId,
    /// The call that started the job.
    pub call_id: CallId,
    /// The command line, as the model wrote it.
    pub command: String,
    /// How the job runs: in the sandbox with its grants, or in the exit child.
    pub launch: Launch,
    /// Where the job is in its life.
    pub state: JobState,
    /// When the job started.
    pub started_at: Timestamp,
    /// When the job's time limit ends it, if it still runs then.
    pub ends_by: Timestamp,
    /// When the job ended. Absent while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    /// The exit status of the job's command, when it exited. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Who or what stopped the job. Absent for a job that runs or ended by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_by: Option<JobStopBy>,
    /// The output file, `$SCRATCH/jobs/<id>.out`, as a path on this machine.
    pub output: PathBuf,
    /// The bytes that the job wrote to its output so far, also the ones that the file
    /// left out in the middle.
    pub output_bytes: u64,
    /// The job's watch pattern. Absent when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<Watch>,
    /// How many lines of the output matched the watch pattern so far.
    #[serde(default)]
    pub wakes: u8,
    /// The ports that efr forwards for the job. Absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<PortForward>,
    /// The `shell` call that the user moved into this job with Ctrl+B. Absent for a job
    /// that the model started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<MovedCall>,
    /// True while the command of a running job that the user moved waits for input that
    /// is not a secret, such as `[Y/n]`: nobody can type it in a job (DS27). Absent when
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waits_input: bool,
    /// How efrd holds the processes of the job (v0.0.5): in a cgroup of the job's own,
    /// with its memory and CPU, or by the job's session only, with the reason. Absent
    /// from an efrd before v0.0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup: Option<JobCgroup>,
}

/// The cgroup of a job (v0.0.5): efrd makes one for each job below its own delegated
/// cgroup, so the stop of the job kills every process of the job, also one that left its
/// session, and the memory and the CPU time of the job can be seen.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct JobCgroup {
    /// Why the job has no cgroup of its own, and efrd holds its processes by its
    /// session only, such as `efrd's cgroup is not delegated (Delegate=yes)`. Absent
    /// when the job has its cgroup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<String>,
    /// The memory of the job's processes in bytes: what they use now while the job
    /// runs (`memory.current`), the most that they used at its end (`memory.peak`).
    /// Absent when the kernel does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// The CPU time of the job's processes so far, in microseconds (`usage_usec` of
    /// `cpu.stat`). Absent when the kernel does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_usec: Option<u64>,
}

/// The `shell` call that a job came from, when the user moved a running call into a job
/// (`call.background`). The job's `turn_id` and `call_id` name the same call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MovedCall {
    /// The call that the user moved.
    pub call_id: CallId,
    /// The turn of that call.
    pub turn_id: TurnId,
    /// How long the call ran before the move, in seconds. The job's time limit counts
    /// from the move.
    pub after_secs: u64,
    /// The exits of the job decision that the user's answer to the move covered. Absent
    /// when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<ExitInfo>,
    /// True when the call ran through efr's sandbox launcher, in the sandbox or in the
    /// exit child; false for a line typed into the hidden shell, which has no launcher
    /// to lose. Absent when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub launcher: bool,
}

/// What the summary of a question in a job notice turn starts with. efrd puts it in
/// front of the summary, so a present user sees that a notice, not the user, led to the
/// question; a client shows it in the question's title and reads the rest as any
/// summary.
pub const WAKE_QUESTION: &str = "(job notice turn) ";

#[cfg(test)]
mod tests;
