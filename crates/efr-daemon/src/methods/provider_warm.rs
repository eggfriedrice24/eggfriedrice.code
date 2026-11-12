//! `provider.warm`: make the connection of a terminal's next prompt ready while the user
//! types it (`efr warm`, which the zsh plugin starts once for each prompt line).
//!
//! The handler picks what the next prompt of the terminal would use and starts the
//! warm-up in a task of its own (`warmup.rs`), then answers at once with the provider.
//! It records nothing and sends no model request. The terminal is the params' `tty`,
//! else the hello's. The method needs `operate`, as `prompt.send` does, so a process of
//! the model's commands cannot make efrd talk to a provider.

use std::sync::Arc;

use efr_protocol::{ProviderWarm, ProviderWarmResult};
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::state::State;
use crate::warmup::{self, Next, Trigger};

const METHOD: &str = "provider.warm";

pub(crate) async fn handle(
    state: &Arc<State>,
    context: &ConnectionContext,
    params: ProviderWarm,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let ProviderWarm { tty, pwd, model, provider, new_conversation } = params;
    // NOTE: a name such as `work` becomes the id of the account, as for a prompt; a
    // name that no provider with a login has leaves the choice to the model.
    let provider = provider.and_then(|name| state.providers.named(&name));
    let next = Next {
        tty: tty.or_else(|| state.connections.tty(context.conn_id())),
        pwd,
        model,
        provider,
        new_conversation,
        conversation: None,
    };
    let target = warmup::target(state, &next).await;
    let provider = target.provider();
    warmup::start(state.providers.warmups(), target, Trigger::Typing);
    let result = ProviderWarmResult { provider: Some(provider) };
    let value = serde_json::to_value(&result)
        .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?;
    responder.item(&value).await?;
    Ok(())
}
