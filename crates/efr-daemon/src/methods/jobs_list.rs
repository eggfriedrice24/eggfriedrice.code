//! `jobs.list`: the background jobs of one conversation or of all of them
//! (`efr jobs list`).
//!
//! Until the job runtime exists, no job ever starts, so the list is always empty.

use efr_protocol::{JobsList, JobsListResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    _state: &State,
    _params: JobsList,
    responder: &Responder,
) -> Result<(), DaemonError> {
    responder.item(&JobsListResult { jobs: Vec::new() }).await?;
    Ok(())
}
