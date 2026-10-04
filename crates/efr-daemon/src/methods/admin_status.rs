//! `admin.status`: the daemon's health, for `efr status`.

use efr_protocol::{AdminStatus, AdminStatusResult, PROTOCOL_VERSION};
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: AdminStatus,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let result = AdminStatusResult {
        daemon_id: state.daemon_id,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol: PROTOCOL_VERSION,
        pid: state.pid,
        started_at: state.started_at,
        screen_backend: state.screen_backend.clone(),
        conversations: count(state.conversations.count()),
        shells: count(state.ptys.count()),
        providers: state.providers.status().await,
    };
    responder.item(&result).await?;
    Ok(())
}
