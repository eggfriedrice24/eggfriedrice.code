//! `projects.list`: the registered projects, for `efr project list`.

use efr_protocol::{ProjectsList, ProjectsListResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::projects;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: ProjectsList,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let projects = projects::list(state).await?;
    let result = ProjectsListResult { file: state.engine_parts.registry.clone(), projects };
    responder.item(&result).await?;
    Ok(())
}
