//! `[resume]`: how `,resume` and `efr resume` show the conversations to continue.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What `,resume` and `efr resume` show when they are not given a row or an id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ResumePicker {
    /// A live search picker in fzf, below the command line, when a new enough fzf is
    /// installed and the terminal allows it. Else the numbered list.
    #[default]
    Auto,
    /// The numbered list, also when fzf is installed.
    List,
}

impl ResumePicker {
    /// Every choice, in the order the docs list them.
    pub const ALL: [ResumePicker; 2] = [ResumePicker::Auto, ResumePicker::List];

    /// The name in the file, such as `auto`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ResumePicker::Auto => "auto",
            ResumePicker::List => "list",
        }
    }
}

/// `[resume]`: how `,resume` and `efr resume` show the conversations to continue.
/// Only `efr` reads it; the daemon accepts it so one file serves both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ResumeSettings {
    /// `auto` opens a live search picker in fzf, below the command line and never full
    /// screen, when a new enough fzf is on the `PATH`. Type to search the titles and the
    /// text of your conversations. Without fzf, `auto` shows the numbered list. `list`
    /// always shows the numbered list.
    pub picker: ResumePicker,
}
