//! `pty.write`: input for a hidden shell's PTY, as if typed. Answers to terminal
//! queries never come this way; the daemon's own screen gives them.

use bytes::Bytes;
use efr_protocol::{PtyWrite, PtyWriteResult};
use efr_shell::ShellError;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    params: PtyWrite,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let pty_id = params.pty_id;
    let conversation =
        state.ptys.conversation(pty_id).ok_or(DaemonError::PtyNotFound { pty_id })?;
    let bytes = params.data.into_bytes();
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    match state.shells.write(conversation, Bytes::from(bytes)).await {
        Ok(()) => {}
        Err(ShellError::NoShell { .. } | ShellError::Exited { .. }) => {
            return Err(DaemonError::PtyNotFound { pty_id });
        }
        Err(error) => return Err(error.into()),
    }
    state.ptys.written(pty_id, len);
    responder.item(&PtyWriteResult {}).await?;
    Ok(())
}
