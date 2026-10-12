//! `jobs.stop`: stop one background job, for `efr jobs stop` and the stop key of the
//! view.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, JobId, JobInfo};

/// The params of `jobs.stop`.
///
/// The daemon refuses it from a process that the model's commands started: the model
/// stops its jobs with the `job_stop` tool, which records the stop as the model's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobsStop {
    /// Makes the stop idempotent.
    pub command_id: CommandId,
    /// The conversation of the job. Absent: the active conversation of the terminal
    /// that the connection's `hello` named. Job ids repeat across conversations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// The job.
    pub job_id: JobId,
}

/// The result of `jobs.stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobsStopResult {
    /// The job after the stop. A job that had ended already keeps its final state.
    pub job: JobInfo,
}
