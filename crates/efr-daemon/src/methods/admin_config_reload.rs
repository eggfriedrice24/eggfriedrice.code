//! `admin.config_reload`: read `config.toml` again now, for `efr config reload`.
//!
//! NOTE: a stub. The wire types landed first so that the daemon and the CLI can be built
//! on them in parallel; until live reload lands, the method answers `internal` and the
//! daemon keeps the config it read at start.

use efr_protocol::AdminConfigReload;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    _state: &State,
    _params: AdminConfigReload,
    _responder: &Responder,
) -> Result<(), DaemonError> {
    Err(DaemonError::NotWired { method: "admin.config_reload" })
}
