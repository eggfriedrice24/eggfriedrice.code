//! `prompt.withdraw`: take back a prompt that waits in the queue.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, Seq, TurnId};

/// The params of `prompt.withdraw` (Alt+Up in the input row of a turn).
///
/// The daemon removes one waiting prompt from the queue, records `prompt_withdrawn`
/// and returns the prompt's text, so the client can put it back where the user types.
/// The turn of a withdrawn prompt never starts; `prompt_withdrawn` is its last event.
/// A prompt that started, ended or was withdrawn gets `conflict`. A turn that this
/// conversation never queued, and a terminal without a waiting prompt, get
/// `not_found`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptWithdraw {
    /// Makes the request idempotent: a retry with the same id returns the first result.
    pub command_id: CommandId,
    /// The conversation whose queue holds the prompt.
    pub conversation_id: ConversationId,
    /// Which prompt to withdraw.
    pub target: WithdrawTarget,
}

/// Which waiting prompt `prompt.withdraw` takes back.
///
/// On the wire an object whose `kind` names the choice, such as
/// `{"kind": "turn", "turn_id": "..."}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WithdrawTarget {
    /// The prompt of this turn, as `prompt.send` returned it.
    Turn {
        /// The turn of the prompt.
        turn_id: TurnId,
    },
    /// The newest waiting prompt whose `prompt_queued` context names this terminal.
    NewestFromTty {
        /// The terminal, such as `/dev/pts/3`.
        tty: String,
    },
}

/// The result of `prompt.withdraw`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptWithdrawResult {
    /// The prompt that the daemon withdrew.
    pub withdrawn: WithdrawnPrompt,
}

/// A prompt that the daemon took out of the queue before it started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WithdrawnPrompt {
    /// The turn that would have answered it.
    pub turn_id: TurnId,
    /// The sequence number of the `prompt_withdrawn` event.
    pub seq: Seq,
    /// The prompt text, as `prompt_queued` recorded it.
    pub text: String,
}
