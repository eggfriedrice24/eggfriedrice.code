//! `shell.interrupt`: stop the command that a `shell` call left running in the
//! conversation's hidden shell, as the user asks from `,shell` or the answer line of
//! `efr`.
//!
//! Without `close`, the shell manager sends `SIGINT` to the job of the command, only
//! while that job holds the terminal and is not the shell itself
//! (`ShellSessions::interrupt_left`). With `close`, it closes the hidden shell
//! (`SIGHUP`, then `SIGKILL` after the close grace), for a command that ignores
//! `SIGINT`; the next call starts a new shell. The command's end comes as
//! `shell_command_ended` with `how` `stopped` or `closed`.
//!
//! The method needs `terminal`, which the phone and a model-side peer never hold. A
//! command that no longer runs, or that another call left, is `not_found`; a job that
//! does not hold the terminal, or the shell itself in the foreground, is `conflict`.
//! A final refusal gets a receipt, so a retry of the command id gets it again; a stop
//! that went out has none, and a retry after the command's end is `not_found`.

use efr_protocol::{CallId, ConversationId, ShellInterrupt, ShellInterruptResult};
use efr_shell::ShellError;
use efr_transport::Responder;

use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "shell.interrupt";

#[tracing::instrument(skip_all, fields(conversation_id = %params.conversation_id, call_id = %params.call_id, close = params.close))]
pub(crate) async fn handle(
    state: &State,
    params: ShellInterrupt,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    match stop(state, &params).await {
        Ok(()) => {
            tracing::info!(
                close = params.close,
                "a command left running in the hidden shell was stopped by the user"
            );
            responder.item(&ShellInterruptResult {}).await?;
            Ok(())
        }
        Err(error) => Err(receipts::refuse(state, command_id, METHOD, error).await),
    }
}

async fn stop(state: &State, params: &ShellInterrupt) -> Result<(), DaemonError> {
    let ShellInterrupt { conversation_id, call_id, close, .. } = *params;
    ensure_exists(state, conversation_id).await?;
    state
        .shells
        .interrupt_left(conversation_id, call_id, close)
        .await
        .map_err(|error| refused(error, conversation_id, call_id))
}

/// The daemon error for a stop that the shell did not send. This is the one mapping of
/// the shell's stop errors; `error.rs` gives each its wire code.
fn refused(error: ShellError, conversation_id: ConversationId, call_id: CallId) -> DaemonError {
    match error {
        ShellError::NoShell { .. } | ShellError::NoCall { .. } | ShellError::Exited { .. } => {
            DaemonError::CallNotRunning { conversation_id, call_id }
        }
        ShellError::NotWaiting { reason, .. } => DaemonError::StopRefused { call_id, reason },
        error => error.into(),
    }
}
