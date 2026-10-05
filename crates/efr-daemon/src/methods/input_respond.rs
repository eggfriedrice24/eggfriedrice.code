//! `input.respond`: the line that the user typed for a running tool call that waits for
//! input, typed into the conversation's hidden shell.
//!
//! The shell manager does the checks and the write in one step of the shell's actor:
//! the text is one line of at most 1024 bytes without control characters, the call's
//! command runs now, the terminal reads a line, and for a hidden answer echo is off.
//! Anything else writes nothing. The text is a `SecretText`, so it never reaches a log,
//! an error message, the event log or a receipt; this handler logs only its length.

use efr_protocol::{InputRespond, InputRespondResult};
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
    let InputRespond { conversation_id, call_id, text, hidden } = params;
    let not_running = || DaemonError::CallNotRunning { conversation_id, call_id };
    match state.shells.answer(conversation_id, call_id, &text, hidden).await {
        Ok(()) => {}
        Err(ShellError::NoShell { .. } | ShellError::NoCall { .. } | ShellError::Exited { .. }) => {
            return Err(not_running());
        }
        Err(ShellError::NotWaiting { reason, .. }) => {
            return Err(DaemonError::NotWaitingForInput { call_id, reason });
        }
        Err(ShellError::InvalidAnswer { reason }) => {
            return Err(DaemonError::InvalidAnswer { reason });
        }
        Err(error) => return Err(error.into()),
    }
    tracing::info!(bytes = text.expose_secret().len(), "an answer was typed for a waiting command");
    responder.item(&InputRespondResult {}).await?;
    Ok(())
}
