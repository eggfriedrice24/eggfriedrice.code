//! `turn.steer`: add guidance to the running turn.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, Seq, ShellContext, TurnId, TurnSettings};

/// The params of `turn.steer` (`,!` in the shell, Enter in the input row of a turn).
/// Unlike `prompt.send`, it does not queue: the text joins the running turn, and the
/// next model call of the turn reads it.
///
/// A steer is late when the turn will make no more model calls: the model answered
/// without a tool call, the user asked to interrupt the turn, `turn_id` names a turn
/// that is not running, or no turn runs. Without `if_late`, the daemon refuses a late
/// steer with `conflict`, as a daemon from before `if_late` does. A late steer is never
/// recorded as `turn_steered`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnSteer {
    /// Makes the request idempotent.
    pub command_id: CommandId,
    /// The conversation whose running turn to steer.
    pub conversation_id: ConversationId,
    /// The turn the client means. When it is given and is not the running turn, the
    /// steer is late.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// The guidance.
    pub text: String,
    /// What the daemon does with a late steer. Absent: it refuses it with `conflict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_late: Option<LateSteer>,
}

/// What the daemon does with a late steer, instead of refusing it.
///
/// On the wire an object whose `kind` names the choice, such as `{"kind": "queue"}`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LateSteer {
    /// Turn the text into a prompt, in the same actor step that finds the steer late,
    /// as `prompt.send` with these members would: the daemon records `prompt_queued`
    /// (with the steer's `command_id`) and no `turn_steered`. The prompt waits behind
    /// the running turn and the prompts already queued, or starts at once when nothing
    /// runs.
    ///
    /// `Debug` leaves out `last_command`, as for [`PromptSend`](crate::PromptSend).
    Queue {
        /// The user's shell when the steer was sent, for the prompt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ShellContext>,
        /// The last command line from the user's shell history, for the turn's
        /// live-state preamble only; never recorded, as in `prompt.send`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_command: Option<String>,
        /// The settings that the prompt asks for. Absent means none.
        #[serde(default, skip_serializing_if = "TurnSettings::is_empty")]
        settings: TurnSettings,
    },
}

impl fmt::Debug for LateSteer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // NOTE: last_command is deliberately missing; a command line can hold a
            // secret, and requests are logged.
            LateSteer::Queue { context, settings, .. } => f
                .debug_struct("Queue")
                .field("context", context)
                .field("settings", settings)
                .finish_non_exhaustive(),
        }
    }
}

/// The result of `turn.steer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnSteerResult {
    /// The turn that took the guidance. When `queued` is true, the new turn of the
    /// prompt that the late steer became.
    pub turn_id: TurnId,
    /// The sequence number of the `turn_steered` event. When `queued` is true, of the
    /// `prompt_queued` event. A client that shows the steer as unread waits for a
    /// `steering_delivered` event that names this number.
    pub seq: Seq,
    /// True when the steer was late and `if_late` turned it into a prompt. Absent
    /// means false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
}
