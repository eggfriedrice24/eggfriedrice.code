//! `conversation.diff`: what a turn changed in files, from efr's own snapshot store,
//! for `efr diff`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, FileChanges, TurnId};

/// The params of `conversation.diff`, a read method.
///
/// The daemon compares the turn's first snapshot (before its first call that can
/// write) with its last (at turn end), in each registered project and `$SCRATCH` that
/// the turn wrote through its calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationDiff {
    /// The conversation. Absent: the active conversation of the terminal that the
    /// connection's `hello` named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// The turn. Absent: the newest turn of the conversation that has snapshots, that
    /// is the last turn that ran a call that can write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// True asks for the list of files only, without the unified diff.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stat: bool,
}

/// The result of `conversation.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationDiffResult {
    /// The turn that the answer is about.
    pub turn_id: TurnId,
    /// The changed files.
    pub changes: FileChanges,
    /// The unified diff of every changed file, with the paths of
    /// [`FileChange::path`](crate::FileChange::path) after `a/` and `b/`, at most
    /// [`MAX_TURN_DIFF_LINES`](crate::MAX_TURN_DIFF_LINES) lines and then a line
    /// `... N more lines`. Absent with `stat`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}
