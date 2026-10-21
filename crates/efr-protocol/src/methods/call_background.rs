//! `call.background`: move the running `shell` call of a turn into a background job,
//! for Ctrl+B in the turn view of `efr`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CallId, CommandId, ConversationId, ExitInfo, JobId};

/// The params of `call.background`.
///
/// The daemon checks the call before it moves it: the call is a running `shell` call of
/// the conversation, jobs are on and have a free place, the line can run as a job, the
/// job decision allows it, and the command does not wait for input now. A refusal is
/// `invalid` with a short reason for a person, such as `the command waits for input`.
///
/// Without `confirm`, a call that runs in the sandbox moves at once: it keeps its
/// sandbox, so the move opens nothing new, and the key press is the user's answer. The
/// answer is then `moved`. A call that runs outside the sandbox (in the exit child, in
/// full mode, or a line typed into the hidden shell in `cautious` and `manual`) does not
/// move yet: the answer is a `preview`, and the client asks the user one short question. Only a yes sends `call.background` again with the token of
/// the preview in `confirm`. The daemon checks the call again then, and refuses when the
/// job decision changed since the preview.
///
/// It needs the scope `terminal`, so the phone and the model's own commands can never
/// move a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallBackground {
    /// The conversation of the call.
    pub conversation_id: ConversationId,
    /// The running `shell` call.
    pub call_id: CallId,
    /// Makes the move idempotent.
    pub command_id: CommandId,
    /// Absent: check the call; move it when it runs in the sandbox, else answer a
    /// preview and move nothing. Present: the token of the preview that the user
    /// answered with yes; the call moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<u64>,
}

/// The result of `call.background`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CallBackgroundResult {
    /// The call runs outside the sandbox, so the move needs the user's yes. Nothing
    /// moved. The token is good for 60 seconds.
    Preview {
        /// The token that a confirm sends back in `confirm`.
        token: u64,
        /// The command line, as the model wrote it.
        command: String,
        /// The time limit of the job, in minutes, from the move.
        max_minutes: u32,
        /// The exits of the job decision, such as a delete that undo does not cover as a
        /// job. Absent when none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exits: Vec<ExitInfo>,
    },
    /// The call is a job now. The daemon answers after it recorded the job.
    Moved {
        /// The new job.
        job_id: JobId,
    },
    /// An answer from a newer daemon.
    #[serde(other)]
    Unknown,
}
