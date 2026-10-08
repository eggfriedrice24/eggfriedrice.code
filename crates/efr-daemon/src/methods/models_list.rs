//! `models.list`: the models that a prompt may name, with their reasoning efforts and
//! windows, and where the model catalog came from.
//!
//! The list is the effective one of the latest settings over the current catalog: the
//! catalog's models, best priority first, then the ids of `[openai] models`, with the
//! default model marked. A turn checks its model and effort against the same list when
//! it starts. Both come from memory; the answer never waits for a fetch.

use efr_protocol::{ModelsList, ModelsListResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: ModelsList,
    responder: &Responder,
) -> Result<(), DaemonError> {
    // NOTE: the settings are cloned out so the watch's read lock is not held while the
    // answer is sent.
    let settings = std::sync::Arc::clone(&state.settings.borrow());
    let models = state.providers.models();
    let result =
        ModelsListResult { models: models.effective(&settings), catalog: Some(models.status()) };
    responder.item(&result).await?;
    Ok(())
}
