//! `input.respond`: the line that the user typed for a running tool call that waits for
//! input.
//!
//! NOTE: a stub. The wire types landed first so that the daemon and the CLI can be built
//! on them in parallel; until the handler lands, every answer is refused with `internal`
//! and nothing reaches a PTY.

use efr_protocol::InputRespond;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    _state: &State,
    _params: InputRespond,
    _responder: &Responder,
) -> Result<(), DaemonError> {
    Err(DaemonError::NotWired { method: "input.respond" })
}
