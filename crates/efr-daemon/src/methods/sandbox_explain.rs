//! `sandbox.explain`: what a contained call of the `auto` mode can do with one path,
//! for `efr sandbox explain`. A read method: it changes nothing and says no more than
//! the user's own config does.

use std::sync::Arc;

use efr_protocol::SandboxExplain;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    params: SandboxExplain,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let settings = Arc::clone(&state.settings.borrow());
    let engine = Arc::clone(&state.engine.borrow());
    let result =
        state.sandbox.explain(&params.path, params.cwd.as_deref(), &settings, &engine).await?;
    responder.item(&result).await?;
    Ok(())
}
