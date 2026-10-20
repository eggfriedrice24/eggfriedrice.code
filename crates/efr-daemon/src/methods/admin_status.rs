//! `admin.status`: the daemon's health, for `efr status`, and its roots and config
//! file, for `efr paths` and `efr config show`.

use std::sync::Arc;

use efr_protocol::{AdminStatus, AdminStatusResult, PROTOCOL_VERSION};
use efr_transport::Responder;

use crate::state::State;
use crate::{DaemonError, reload};

pub(crate) async fn handle(
    state: &State,
    _params: AdminStatus,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let settings = Arc::clone(&state.settings.borrow());
    let mut sandbox = state.sandbox.reported(&settings);
    // NOTE: a disabled always rule (phase E, DE44) is a warning of the sandbox, which
    // `efr sandbox check` and `efr sandbox allow` show; efrd runs on.
    sandbox.warnings.extend(crate::always::disabled_warnings(&state.engine.borrow()));
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
        catalog: Some(state.providers.models().status()),
        roots: Some(state.roots.clone()),
        config: Some(reload::status(state).await),
        sandbox: Some(sandbox),
        sandbox_paths: Some(state.sandbox.paths().await),
        full_mode: None,
    };
    responder.item(&result).await?;
    Ok(())
}
