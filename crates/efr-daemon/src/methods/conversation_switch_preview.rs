//! `conversation.switch_preview`: what a switch of the model or the account of a
//! conversation would cost its next turn, for the model sheet of the phone.
//!
//! The target is `model` on the provider that `provider` names (an account with a
//! login, as a prompt names it), else on the provider that a prompt with `model` alone
//! takes. efrd compares it with the model and the provider of the conversation's newest
//! turn, which the next turn keeps:
//!
//! - `none`: the same model on the same provider, such as a change of the effort only,
//!   or a conversation with no turn yet, which has no cache to lose;
//! - `cold_cache`: another model, or the same model on another account, so the
//!   provider's cache does not serve the next request;
//! - `compacts`: as `cold_cache`, and the estimate of the next request on the target is
//!   at its compaction trigger (with `compaction.auto`) or above its hard cap, so the
//!   next turn compacts first. The window is the one of the target's entry in the model
//!   list, and the estimate comes from the context view of the conversation
//!   (`conversation.context`) on the target model.
//!
//! A `provider` that names no account with a login gets `unknown` and no provider. A
//! conversation that does not exist is `not_found`. The call changes nothing and
//! records nothing.

use std::sync::Arc;

use efr_conversation::{CompactionConfig, ContextLimits};
use efr_protocol::{
    ConversationContext, ConversationSwitchPreview, ConversationSwitchPreviewResult, SwitchCost,
};
use efr_transport::Responder;

use crate::DaemonError;
use crate::methods::conversation_context::newest_settings;
use crate::methods::prompt_send::ensure_exists;
use crate::state::State;

const METHOD: &str = "conversation.switch_preview";

pub(crate) async fn handle(
    state: &State,
    params: ConversationSwitchPreview,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let settings = Arc::clone(&state.settings.borrow());
    let target = match params.provider.as_deref() {
        Some(name) => state.providers.named(name),
        None => Some(state.providers.provider_of_model(&settings, &params.model)),
    };
    let cost = match target.as_deref() {
        Some(target) => cost(state, &params, target).await?,
        None => {
            ensure_exists(state, params.conversation_id).await?;
            SwitchCost::Unknown
        }
    };
    let result = ConversationSwitchPreviewResult { cost, provider: target };
    let value = serde_json::to_value(&result)
        .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?;
    responder.item(&value).await?;
    Ok(())
}

/// What a switch of the conversation of `params` to `params.model` on `target` costs.
async fn cost(
    state: &State,
    params: &ConversationSwitchPreview,
    target: &str,
) -> Result<SwitchCost, DaemonError> {
    let conversation_id = params.conversation_id;
    let Some(newest) = newest_settings(state, conversation_id).await else {
        ensure_exists(state, conversation_id).await?;
        return Ok(SwitchCost::None);
    };
    if newest.model == params.model && newest.provider.as_deref() == Some(target) {
        return Ok(SwitchCost::None);
    }
    let handle = match state.conversations.live(conversation_id) {
        Some(handle) => handle,
        None => state.conversations.open(conversation_id),
    };
    let measured = handle
        .context(ConversationContext { conversation_id, model: Some(params.model.clone()) })
        .await?;
    let settings = Arc::clone(&state.settings.borrow());
    let models = state.providers.catalogs().listed(&settings).models;
    let entries = || models.iter().filter(|info| info.id == params.model);
    let window = entries()
        .find(|info| info.provider.as_deref() == Some(target))
        .or_else(|| entries().next())
        .and_then(|info| info.context_window);
    let compaction = CompactionConfig::new(settings.compaction.auto, settings.compaction.auto_at);
    let limits = ContextLimits::new(window, compaction);
    let estimate = measured.estimate;
    Ok(if (limits.auto && estimate >= limits.trigger) || estimate > limits.hard_cap {
        SwitchCost::Compacts
    } else {
        SwitchCost::ColdCache
    })
}
