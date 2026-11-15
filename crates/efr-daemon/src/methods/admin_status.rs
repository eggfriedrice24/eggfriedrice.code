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
    // NOTE: a credential that changed outside efrd shows at once, with the list of a
    // new account.
    state.providers.sync_logins_and_lists().await;
    let settings = Arc::clone(&state.settings.borrow());
    // NOTE: the accounts in the order of `models.list`, then the ones that it does not
    // list, each inside its kind: `efr status` and the phone show one order.
    let order = state.providers.catalogs().order(&settings);
    let order: Vec<&str> = order.iter().map(String::as_str).collect();
    let mut providers = state.providers.status().await;
    efr_protocol::order_by_list(&mut providers, |provider| provider.provider.as_str(), &order);
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
        providers,
        catalog: Some(state.providers.catalogs().status_of(&state.providers.models(), &settings)),
        roots: Some(state.roots.clone()),
        config: Some(reload::status(state).await),
        sandbox: Some(sandbox),
        sandbox_paths: Some(state.sandbox.paths().await),
        // NOTE: the same value that a turn's resolve reads.
        full_mode: Some(crate::full::status(&settings, state.runs_as_root)),
    };
    responder.item(&result).await?;
    Ok(())
}
