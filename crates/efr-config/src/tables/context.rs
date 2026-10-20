//! `[context]`: what a prompt tells the model about your terminal, besides its
//! directory and its last command.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[context]`: the facts of your terminal that a prompt carries to the model. Every
/// key applies from the next prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ContextSettings {
    /// How many of the terminal's recent commands a prompt shows the model, from 0 to
    /// 10; 0 shows none. Each one comes with its exit status, its directory when that
    /// is not the current one, and how long ago it ended. The zsh plugin keeps the last
    /// 10 command lines of each terminal. It does not keep a `,` line, an `efr` command
    /// or a line that starts with a space.
    pub recent_commands: u32,
}

impl Default for ContextSettings {
    fn default() -> Self {
        ContextSettings { recent_commands: 8 }
    }
}
