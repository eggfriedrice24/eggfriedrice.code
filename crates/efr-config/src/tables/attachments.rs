//! `[attachments]`: the files that go with a prompt (ADR 0026).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::AutoOnOff;

/// `[attachments]`: the files that you attach to a prompt with `@path` in a `,` line,
/// with a paste, or with the plus button of the phone, and the image files that the
/// model reads with `read_file`. Every key applies from the next upload or prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct AttachmentsSettings {
    /// `false`: efrd takes no file and tells no client that it takes files, an `@path`
    /// stays text, and `read_file` refuses an image file as a binary file.
    pub enabled: bool,
    /// The largest file of an attachment, in MiB, from 1 to 50. 50 MB is the file limit
    /// of the OpenAI Responses API. A larger value does not send a larger PDF as
    /// content: a PDF goes as content only up to 5 MiB and 100 pages.
    pub max_file_mib: u32,
    /// The most attachments of one prompt, from 1 to 20.
    pub max_per_message: u32,
    /// The longest edge in pixels of an image that a model gets, from 256 to 2576. efrd
    /// makes a larger PNG or JPEG smaller before a model gets it; 2576 is the largest
    /// edge that Claude reads.
    pub max_edge_px: u32,
    /// The paste of an image into a `,` line (OSC 5522). `auto`: in Ghostty 1.4.0 or
    /// later, never inside tmux, GNU screen or zellij. `on`: in every terminal. `off`:
    /// never.
    pub paste: AutoOnOff,
}

impl Default for AttachmentsSettings {
    fn default() -> Self {
        AttachmentsSettings {
            enabled: true,
            max_file_mib: 20,
            max_per_message: 10,
            max_edge_px: 2000,
            paste: AutoOnOff::Auto,
        }
    }
}
