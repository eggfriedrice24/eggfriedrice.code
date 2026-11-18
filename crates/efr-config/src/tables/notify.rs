//! `[notify]`: the desktop notices that efr sends through the terminal (OSC 777).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::AutoOnOff;

/// `[notify]`: when efr sends a desktop notice through your terminal. A notice has the
/// title `efr` and a body such as `app: a question waits`. The body never holds a
/// prompt, a command line, a title or text of the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct NotifySettings {
    /// The desktop notice of the terminal (OSC 777): when a question waited 10 seconds
    /// and you typed no key, when efrd shows a notice above a waiting prompt that needs
    /// you, and at the end of a long turn (`after_secs`). `auto`: only in Ghostty, never
    /// inside tmux, GNU screen or zellij. `on`: in every terminal. `off`: never. efrd
    /// gives it to the shell at the next prompt; `efr` reads it at its next run.
    pub terminal: AutoOnOff,
    /// The shortest turn, in seconds, whose end sends a desktop notice, from 0 to 86400;
    /// 0: the end of each turn. It is also the shortest turn of the phone whose end
    /// sends a push to the phone (`presence.push`). An interrupted turn sends no
    /// notice. A question does not wait for this time. If Ghostty also tells you the end
    /// of a command (`notify-on-command-finish`), set that option to `never`, or you get
    /// two notices.
    pub after_secs: u32,
}

impl Default for NotifySettings {
    fn default() -> Self {
        NotifySettings { terminal: AutoOnOff::Auto, after_secs: 30 }
    }
}
