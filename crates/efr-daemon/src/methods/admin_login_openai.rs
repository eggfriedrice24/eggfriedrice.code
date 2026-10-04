//! `admin.login_openai`: the ChatGPT subscription login, driven by the daemon for
//! `efr login openai`.
//!
//! The first item is the authorize URL; the stream ends when the browser came back, the
//! tokens are saved and `login_completed` is recorded. The callback listener lives only
//! as long as this request: a client that goes away cancels the login and frees the
//! port, and a second login while one runs is refused with `busy`.

use efr_protocol::{AdminLoginOpenAi, AdminLoginOpenAiItem, Event};
use efr_store::Batch;
use efr_transport::Responder;

use crate::DaemonError;
use crate::providers::SUBSCRIPTION;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: AdminLoginOpenAi,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let pending = state.providers.start_login().await?;
    let url = pending.authorize_url().to_string();
    responder.item(&AdminLoginOpenAiItem::AuthorizeUrl { url }).await?;
    let completed = pending.complete().await.map_err(|source| DaemonError::Login { source })?;
    // The running provider must not keep the previous account's cached token.
    state.providers.login_completed();
    tracing::info!(
        provider = SUBSCRIPTION,
        has_account = completed.account_id.is_some(),
        "login completed"
    );
    let event = Event::LoginCompleted { provider: SUBSCRIPTION.to_owned() };
    state.writer.append(Batch::new().global_event(event)).await?;
    responder.item(&AdminLoginOpenAiItem::Completed { provider: SUBSCRIPTION.to_owned() }).await?;
    Ok(())
}
