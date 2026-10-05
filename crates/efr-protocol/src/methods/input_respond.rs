//! `input.respond`: send the line that the user typed to a running tool call that waits
//! for input.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CallId, ConversationId, SecretText};

/// The params of `input.respond`: one line that the user typed for a running tool call
/// whose last `tool_call_input_changed` event said that it waits for input.
///
/// The daemon writes `text` and then a carriage return to the call's PTY, but only while
/// that call's command runs and reads a line, and with `hidden` only while the PTY also
/// has echo off. It answers `not_found` for an unknown conversation or when no call's
/// command runs in it (the call's command ended, or was left at its timeout and goes on
/// without a call), `conflict` when another call's command runs or the call's command
/// does not wait for that input, and `invalid` for a text that is not one line. Then it
/// writes nothing.
///
/// There is no command id: like `pty.write`, an answer is never retried, so the daemon
/// keeps no receipt, which would also store the text. `Debug` never shows the text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputRespond {
    /// The conversation of the running call.
    pub conversation_id: ConversationId,
    /// The call that waits for input.
    pub call_id: CallId,
    /// The line without its line ending: at most 1024 bytes and no control characters
    /// (U+0000 to U+001F and U+007F). The daemon never logs or records it.
    pub text: SecretText,
    /// True when the client asked the user for hidden input, because the event said
    /// `hidden`. The daemon then requires the PTY's echo to be off when it writes, so the
    /// terminal does not put the text in the output that the model reads.
    pub hidden: bool,
}

/// The result of `input.respond`: the line was written to the call's PTY.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputRespondResult {}
