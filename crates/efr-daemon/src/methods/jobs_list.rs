//! `jobs.list`: the background jobs of one conversation or of all of them
//! (`efr jobs list`), newest first, at most 200. A job that runs shows the bytes of its
//! output so far and the ports that efr forwards for it now, from the job table; an
//! ended one shows what its `job_ended` recorded, from the moment that event commits.

use efr_protocol::{JobsList, JobsListResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::methods::prompt_send::ensure_exists;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    params: JobsList,
    responder: &Responder,
) -> Result<(), DaemonError> {
    if let Some(conversation_id) = params.conversation_id {
        ensure_exists(state, conversation_id).await?;
    }
    let jobs = state.jobs.list(params.conversation_id).await?;
    responder.item(&JobsListResult { jobs }).await?;
    Ok(())
}
