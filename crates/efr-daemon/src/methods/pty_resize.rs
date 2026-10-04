//! `pty.resize`: a new size for a hidden shell's PTY and its screen.
//!
//! A size with no rows or no columns is refused here, at the protocol edge, before it
//! reaches the holder or the screen. A very large size is clamped, and the answer says
//! the size the PTY has now.

use efr_protocol::{PtyResize, PtyResizeResult, Size};
use efr_shell::ShellError;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

/// The largest size a PTY gets; more columns or rows than any screen shows.
pub(crate) const MAX_SIZE: Size = Size { cols: 1000, rows: 500 };

/// The size to apply for a request of `size`.
pub(crate) fn clamp(size: Size) -> Result<Size, DaemonError> {
    if size.cols == 0 || size.rows == 0 {
        return Err(DaemonError::InvalidParams {
            reason: "a PTY needs at least one row and one column",
        });
    }
    Ok(Size { cols: size.cols.min(MAX_SIZE.cols), rows: size.rows.min(MAX_SIZE.rows) })
}

pub(crate) async fn handle(
    state: &State,
    params: PtyResize,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let pty_id = params.pty_id;
    let size = clamp(params.size)?;
    let conversation =
        state.ptys.conversation(pty_id).ok_or(DaemonError::PtyNotFound { pty_id })?;
    match state.shells.resize(conversation, size).await {
        Ok(()) => {}
        Err(ShellError::NoShell { .. } | ShellError::Exited { .. }) => {
            return Err(DaemonError::PtyNotFound { pty_id });
        }
        Err(error) => return Err(error.into()),
    }
    state.ptys.resized(pty_id, size);
    responder.item(&PtyResizeResult { size }).await?;
    Ok(())
}
