//! `turn.undo`: take back the file and ref changes of a turn (`efr undo`).
//!
//! This build has no undo yet, so every request is refused with `conflict` and the
//! refusal `off` in the error's data. Nothing is read, written or recorded. The handler
//! of phase B resolves the conversation as `conversation.diff` does, builds the
//! candidates from the turns of the log, refuses a phone itself, and asks the
//! conversation's actor.

use efr_protocol::{ErrorBody, ErrorCode, TurnUndo, UndoRefused};
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    _state: &State,
    params: TurnUndo,
    _responder: &Responder,
) -> Result<(), DaemonError> {
    tracing::debug!(command_id = %params.command_id, "turn.undo refused: this build has no undo");
    Err(DaemonError::Rejected { body: refusal(UndoRefused::Off) })
}

/// The wire error of a refused undo: `conflict`, with the refusal as `refused` in its
/// data.
fn refusal(refused: UndoRefused) -> ErrorBody {
    let data = serde_json::json!({ "refused": refused });
    ErrorBody::new(ErrorCode::Conflict, refused.to_string()).with_data(data)
}

#[cfg(test)]
mod tests;
