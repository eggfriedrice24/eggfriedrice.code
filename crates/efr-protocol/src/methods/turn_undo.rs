//! `turn.undo`: take back the file and ref changes of one turn, from efr's own
//! snapshot stores, for `efr undo`.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::compaction::is_zero_u32;
use crate::{CommandId, ConversationId, RefChange, TurnId, UndoEntry, UndoId, UndoableTurn};

/// The params of `turn.undo`.
///
/// `efr undo` sends it twice: first with `dry_run` for the preview, then without it to
/// apply. The apply checks each path again. A phone cannot undo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnUndo {
    /// Makes the undo idempotent.
    pub command_id: CommandId,
    /// The conversation. Absent: the conversation of `turn_id`, else the active
    /// conversation of the terminal that the connection's `hello` named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// The turn. Absent: the newest turn of the conversation that is not undone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// True shows what the undo does and changes nothing. False when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    /// True also overwrites a path that changed after the turn, and undoes a turn while
    /// a newer turn is not undone. False when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
    /// True takes back the newest undo of the conversation instead. False when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redo: bool,
    /// True lists the turns that can be undone in `turns` and changes nothing. False
    /// when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub list: bool,
}

/// The result of `turn.undo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnUndoResult {
    /// The conversation.
    pub conversation_id: ConversationId,
    /// The turn that the undo takes back, or whose undo a redo takes back. Absent with
    /// `list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// The first line of the turn's prompt. Absent when the log has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The undo. Absent in a dry run and with `list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_id: Option<UndoId>,
    /// True when the undo changed files or refs; false in a dry run, with `list`, and
    /// when nothing was to do.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub applied: bool,
    /// True when the undo went on with an earlier undo that stopped half way. False
    /// when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resumed: bool,
    /// What the undo does or did with each path, at most
    /// [`MAX_UNDO_ENTRIES`](crate::MAX_UNDO_ENTRIES). Absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<UndoEntry>,
    /// How many entries `entries` leaves out. Absent when none.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub entries_more: u32,
    /// The refs that the undo moves or moved. Absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<RefChange>,
    /// The directory of the undo trash that holds every file that the undo overwrote
    /// or removed. Absent when it holds none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trash: Option<PathBuf>,
    /// Notes for the user, each one line: that the git index stays as it is, to run
    /// `systemctl --user daemon-reload`, that empty directories and special files do
    /// not come back. Absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// With `list`: the turns that can be undone, newest first. Absent otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<UndoableTurn>,
}
