//! `presence.set`: tell efr that the user is away from a conversation, or back, for
//! `efr away` and `,away`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, Presence, Seq};

/// The params of `presence.set`. Any later input of an attached client sets the
/// conversation present again. The daemon refuses it from a process that the model's
/// commands started, so sandboxed code cannot make questions expire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceSet {
    /// Makes the change idempotent.
    pub command_id: CommandId,
    /// The conversation. Absent: every conversation that had input since the daemon
    /// started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// The new presence.
    pub presence: Presence,
}

/// The result of `presence.set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceSetResult {
    /// The presence that the conversations have now.
    pub presence: Presence,
    /// The conversations whose presence changed, each with one `presence_changed`
    /// event; absent when each one had that presence already.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<ConversationId>,
    /// The sequence number of the last `presence_changed` event; absent when nothing
    /// changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<Seq>,
}
