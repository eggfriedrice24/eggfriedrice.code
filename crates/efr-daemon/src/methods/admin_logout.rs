//! `admin.logout`: forget the credentials of a provider, for `efr logout`.
//!
//! The provider's next request fails as not logged in. An API key stays valid at its
//! provider; only its owner can revoke it there.

use efr_protocol::{AdminLogout, AdminLogoutResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    params: AdminLogout,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let AdminLogout { provider } = params;
    let logged_out = state.providers.logout(&provider).await?;
    tracing::info!(provider = %provider, logged_out, "logout");
    responder.item(&AdminLogoutResult { provider, logged_out, active: false }).await?;
    Ok(())
}
