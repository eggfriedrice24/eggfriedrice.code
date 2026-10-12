//! `jobs.list`: the background jobs of one conversation or of all of them, for
//! `efr jobs list`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, JobInfo};

/// The params of `jobs.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobsList {
    /// The conversation. Absent: the jobs of every conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
}

/// The result of `jobs.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobsListResult {
    /// The jobs, newest first, at most [`MAX_JOBS`](Self::MAX_JOBS). Absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<JobInfo>,
}

impl JobsListResult {
    /// The most jobs that one answer holds.
    pub const MAX_JOBS: usize = 200;
}
