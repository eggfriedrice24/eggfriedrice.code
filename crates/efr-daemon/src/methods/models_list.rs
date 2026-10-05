//! `models.list`: the models that a prompt may name, with their reasoning efforts.
//!
//! NOTE: a stub. The wire types landed first so that the daemon and the CLI can be built
//! on them in parallel; until the model list is verified and the handler lands, the
//! method answers `internal`.

use efr_protocol::ModelsList;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    _state: &State,
    _params: ModelsList,
    _responder: &Responder,
) -> Result<(), DaemonError> {
    Err(DaemonError::NotWired { method: "models.list" })
}
