//! `[compaction]`: when efrd compacts a conversation's context. The rules are in the
//! README of `efr-conversation`, section "Context".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The default of `compaction.auto_at`: compact at 76% of the model's context window,
/// so 24% of the window stays free for the next request and its answer.
pub const DEFAULT_AUTO_AT: u32 = 76;

/// `[compaction]`: when efrd compacts the context of a conversation on its own. Every
/// key applies from the next turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct CompactionSettings {
    /// `true` compacts the context on its own when it reaches `auto_at`, before a
    /// model call, and the turn goes on. `false` compacts only when you run `,compact`
    /// (`efr compact`), and a request that does not fit in the model's window fails
    /// the turn with a message that names `,compact`.
    pub auto: bool,
    /// The percent of the model's context window at which efr compacts on its own,
    /// from 1 to 99. `ctx 100%` in the status row means this point.
    pub auto_at: u32,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        CompactionSettings { auto: true, auto_at: DEFAULT_AUTO_AT }
    }
}
