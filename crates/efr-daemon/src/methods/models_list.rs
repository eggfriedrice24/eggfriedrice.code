//! `models.list`: the models that a prompt may name, with their reasoning efforts.
//!
//! The list is the effective one of the latest settings: the built-in models of the
//! provider, then the ids of `[openai] models`, with the default model marked. A turn
//! checks its model and effort against the same list when it starts.

use efr_protocol::{ModelsList, ModelsListResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::providers::effective_models;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: ModelsList,
    responder: &Responder,
) -> Result<(), DaemonError> {
    // NOTE: the settings are cloned out so the watch's read lock is not held while the
    // answer is sent.
    let settings = std::sync::Arc::clone(&state.settings.borrow());
    let result = ModelsListResult { models: effective_models(&settings) };
    responder.item(&result).await?;
    Ok(())
}
