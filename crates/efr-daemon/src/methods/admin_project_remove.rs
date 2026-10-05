//! `admin.project_remove`: take a project out of the registry, for
//! `efr project remove`.

use efr_protocol::{AdminProjectRemove, AdminProjectRemoveResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::projects;
use crate::reload;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    params: AdminProjectRemove,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let (project, file) = projects::remove(state, params).await?;
    tracing::info!(root = %project.root.display(), "project removed");
    let reload = reload::reload(state, "admin.project_remove").await?;
    responder.item(&AdminProjectRemoveResult { project, file, reload }).await?;
    Ok(())
}
