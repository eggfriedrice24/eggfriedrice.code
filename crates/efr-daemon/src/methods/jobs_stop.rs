//! `jobs.stop`: stop one background job (`efr jobs stop`, the stop key of the view).
//!
//! The conversation is the one that the params name, else the active conversation of
//! the terminal that the connection's `hello` named. The method needs `operate`, which a
//! model-side peer never holds: the model stops its jobs with the `job_stop` tool, and
//! a process of a job can never stop a job through the socket. The stop counts as the
//! user's (`stopped_by: user`) and the answer waits for the job's end. A job that ended
//! already answers with its final state, so a retry gets the same job; an unknown job
//! is `not_found`, which a retried command id gets again from its receipt.

use efr_protocol::{JobStopBy, JobsStop, JobsStopResult};
use efr_transport::{ConnectionContext, Responder};

use crate::jobs::JobKey;
use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "jobs.stop";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: JobsStop,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    match stop(state, context, params).await {
        Ok(result) => {
            responder.item(&result).await?;
            Ok(())
        }
        Err(error) => Err(receipts::refuse(state, command_id, METHOD, error).await),
    }
}

async fn stop(
    state: &State,
    context: &ConnectionContext,
    params: JobsStop,
) -> Result<JobsStopResult, DaemonError> {
    let conversation_id = match params.conversation_id {
        Some(conversation_id) => conversation_id,
        None => state
            .connections
            .tty(context.conn_id())
            .and_then(|tty| state.conversations.current(&tty))
            .map(|(_, slot)| slot.conversation_id)
            .ok_or(DaemonError::NoActiveConversation)?,
    };
    ensure_exists(state, conversation_id).await?;
    let not_found = || DaemonError::JobNotFound { conversation_id, job_id: params.job_id.clone() };
    let key = JobKey::of(conversation_id, &params.job_id).ok_or_else(not_found)?;
    let job = state.jobs.stop(key, JobStopBy::User).await?;
    tracing::info!(conversation = %conversation_id, job = %job.id, state = job.state.as_str(), "jobs.stop answered");
    Ok(JobsStopResult { job })
}

#[cfg(test)]
mod tests;
