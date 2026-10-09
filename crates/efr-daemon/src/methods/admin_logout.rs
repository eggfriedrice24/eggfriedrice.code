//! `admin.logout`: forget the credentials of a provider, for `efr logout`.
//!
//! The provider's next request fails as not logged in. An API key stays valid at its
//! provider; only its owner can revoke it there. A logout that deleted credentials
//! records `logout_completed`, which names the provider and holds nothing of the
//! credentials. The answer says whether the provider is the one of new conversations,
//! whose turns now fail until a new login.

use efr_protocol::{AdminLogout, AdminLogoutResult, Event};
use efr_store::Batch;
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
    let active = state.providers.is_active(&provider);
    tracing::info!(provider = %provider, logged_out, active, "logout");
    if logged_out {
        let event = Event::LogoutCompleted { provider: provider.clone() };
        state.writer.append(Batch::new().global_event(event)).await?;
    }
    responder.item(&AdminLogoutResult { provider, logged_out, active }).await?;
    Ok(())
}
