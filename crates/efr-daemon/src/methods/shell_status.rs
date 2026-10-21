//! `shell.status`: the command that a `shell` call left running in the conversation's
//! hidden shell, if any, for `,shell` and for a client that did not see it start.
//!
//! It reads the shell's live state (`ShellSessions::busy`), never the event log: a
//! `shell_command_detached` of an efrd that stopped since, whose shell died with it,
//! never shows as busy. A conversation without a shell is free.

use efr_protocol::{ShellStatus, ShellStatusResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::methods::prompt_send::ensure_exists;
use crate::state::State;

#[tracing::instrument(skip_all, fields(conversation_id = %params.conversation_id))]
pub(crate) async fn handle(
    state: &State,
    params: ShellStatus,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let ShellStatus { conversation_id } = params;
    ensure_exists(state, conversation_id).await?;
    let busy = state.shells.busy(conversation_id).and_then(crate::tools::busy_shell);
    responder.item(&ShellStatusResult { busy }).await?;
    Ok(())
}
