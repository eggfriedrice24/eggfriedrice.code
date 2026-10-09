//! `admin.login_api_key`: store an API key for `openai-api` or `anthropic-api`, for
//! `efr login openai-api` and `efr login anthropic`.
//!
//! The key is checked with its provider unless the client says not to, stored as the
//! provider's credential and `login_completed` is recorded. The provider of new
//! conversations stays the one of `[model] provider`: the answer says whether the key
//! belongs to it, and another provider needs that key and a restart. The key reaches
//! no event, no log line and no error.

use efr_protocol::{AdminLoginApiKey, AdminLoginApiKeyResult, Event};
use efr_store::Batch;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    params: AdminLoginApiKey,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let AdminLoginApiKey { provider, key, check } = params;
    let login = state.providers.login_api_key(&provider, &key, check).await?;
    drop(key);
    tracing::info!(provider = %provider, checked = check, active = login.active, "login completed");
    let event = Event::LoginCompleted { provider: provider.clone() };
    state.writer.append(Batch::new().global_event(event)).await?;
    let result = AdminLoginApiKeyResult {
        provider,
        key_hint: login.hint,
        checked: check,
        active: login.active,
    };
    responder.item(&result).await?;
    Ok(())
}
