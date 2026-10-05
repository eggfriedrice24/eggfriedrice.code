//! `input.respond`: the line that the user typed for a running tool call that waits for
//! input, typed into the conversation's hidden shell.
//!
//! The shell manager does the checks and the write in one step of the shell's actor:
//! the text is one line of at most [`InputRespond::MAX_TEXT_BYTES`] bytes without
//! control characters, the call's command runs now, a wait of it of the answer's kind
//! was reported and the job that waited still holds the terminal, and for a hidden
//! answer the terminal reads a line with echo off. Anything else writes nothing. The
//! text is a `SecretText`, so it never reaches a log, an error message, the event log
//! or a receipt; this handler logs only its length.

use efr_protocol::{CallId, ConversationId, InputRespond, InputRespondResult};
use efr_shell::ShellError;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

#[tracing::instrument(skip_all, fields(conversation_id = %params.conversation_id, call_id = %params.call_id, hidden = params.hidden))]
pub(crate) async fn handle(
    state: &State,
    params: InputRespond,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let InputRespond { conversation_id, call_id, text, hidden, manual: _ } = params;
    state
        .shells
        .answer(conversation_id, call_id, &text, hidden)
        .await
        .map_err(|error| refused(error, conversation_id, call_id))?;
    tracing::debug!(
        bytes = text.expose_secret().len(),
        "an answer was typed for a waiting command"
    );
    responder.item(&InputRespondResult {}).await?;
    Ok(())
}

/// The daemon error for an answer that the shell did not type. This is the one mapping
/// of the shell's answer errors; the daemon errors it makes name the call, and
/// `error.rs` gives each its wire code.
fn refused(error: ShellError, conversation_id: ConversationId, call_id: CallId) -> DaemonError {
    match error {
        ShellError::NoShell { .. } | ShellError::NoCall { .. } | ShellError::Exited { .. } => {
            DaemonError::CallNotRunning { conversation_id, call_id }
        }
        ShellError::NotWaiting { reason, .. } => {
            DaemonError::NotWaitingForInput { call_id, reason }
        }
        ShellError::InvalidAnswer { reason } => DaemonError::InvalidAnswer { reason },
        error => error.into(),
    }
}

#[cfg(test)]
mod tests;
