//! `admin.project_add`: register a project, for `efr project add`.
//!
//! The answer carries the reload that follows the write: when `config.toml` has an
//! error, the old engine stays and the project counts from the next reload that
//! succeeds.

use efr_protocol::{AdminProjectAdd, AdminProjectAddResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::projects;
use crate::reload;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    params: AdminProjectAdd,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let (project, file) = projects::add(state, params).await?;
    tracing::info!(root = %project.root.display(), "project registered");
    // NOTE: read once now, from the files the user just named; before each call efrd
    // compares them with this record, never trusting what a call may have rewritten.
    state.sandbox.register_project(&project.root).await;
    let reload = reload::reload(state, "admin.project_add").await?;
    responder.item(&AdminProjectAddResult { project, file, reload }).await?;
    Ok(())
}
