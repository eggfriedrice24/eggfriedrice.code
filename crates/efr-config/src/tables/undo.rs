//! `[undo]`: `efr undo`, which takes back the file and ref changes of a turn from efr's
//! snapshots (efr's auto spec, section 10.5, phase B).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[undo]`: whether `efr undo` can take back a turn. The snapshots themselves are in
/// `[snapshot]`. Every key applies from the next call or undo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct UndoSettings {
    /// `false` turns undo off: `efr undo` refuses, the end of a turn shows no undo
    /// hint, and in `auto` a line that deletes or overwrites files goes to the reviewer
    /// or to you, as with `sandbox.ask_destructive = "always"`.
    pub enabled: bool,
}

impl Default for UndoSettings {
    fn default() -> Self {
        UndoSettings { enabled: true }
    }
}
