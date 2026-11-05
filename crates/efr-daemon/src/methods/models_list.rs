//! `models.list`: the models that a prompt may name, with their reasoning efforts and
//! windows, and where each model catalog came from.
//!
//! The list is the effective one of the latest settings over each current catalog:
//! the models of the provider of new conversations first (its catalog's models, best
//! priority first, then the ids of the config's list of its company), with the default
//! model marked; then the models of each other provider with a login, the same way but
//! never marked as the default. Each model names its provider (`catalogs.rs`). A turn
//! checks its model and effort against the same list when it starts, and goes to the
//! first provider with a login that lists its model (`Catalogs::routed`). Everything comes from memory; the answer never
//! waits for a fetch.

use efr_protocol::{ModelsList, ModelsListResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::catalogs::Listed;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: ModelsList,
    responder: &Responder,
) -> Result<(), DaemonError> {
    // NOTE: the settings are cloned out so the watch's read lock is not held while the
    // answer is sent.
    let settings = std::sync::Arc::clone(&state.settings.borrow());
    let Listed { models, mut catalogs } = state.providers.catalogs().listed(&settings);
    let catalog = catalogs.first().cloned();
    // NOTE: with one provider the answer stays as before: `catalog` says it all.
    if catalogs.len() == 1 {
        catalogs.clear();
    }
    responder.item(&ModelsListResult { models, catalog, catalogs }).await?;
    Ok(())
}
