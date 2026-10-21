//! `call.background`: move the running `shell` call of a turn into a background job,
//! for Ctrl+B in the turn view of `efr` (v0.0.4 shell items, DS16-DS28 and DS53-DS56,
//! with the user's change of OQS2).
//!
//! The handler checks the call (`jobs/moves.rs`): it runs as a foreground `shell` call
//! of the conversation, jobs are on and have a free place, the line has no hard
//! `interactive` flag and no nested shell, a contained call holds no cache layer lock,
//! and the engine decides the line again as a job. Any `Deny` refuses.
//!
//! - A call in the sandbox moves at once: it keeps its sandbox, so the move opens
//!   nothing new, and the key press is the user's answer, also in `cautious` and
//!   `manual`.
//! - A call outside the sandbox (the exit child, a line typed into the hidden shell,
//!   full mode) gets a `preview` with a token and the exits of the job decision. Only a
//!   second call with that token in `confirm`, within 60 s and with the same decision,
//!   moves it.
//!
//! The move: efrd reserves the job's place and asks efr-shell to hand the run over;
//! efr-shell makes its own checks then (the command started, no sentinel run, no shell
//! or REPL, no input wait, no full-screen program). The tool path makes `$CALL/moved`
//! for a call through the launcher (DS23) once the run was handed over, adopts the run
//! into the job table and answers with the job; the answer `moved` comes after the job is recorded, at most
//! 10 s later on efrd's clock. The old shell's PTY is the job's from the hand-over on
//! (DS56): efr-shell's `ShellNotice::Adopted` marks it, whatever this handler does.
//!
//! The method needs `terminal`, which the phone and a model-side peer never hold. A
//! refusal is `invalid` with the reason alone as its message, and it gets a receipt.

use std::sync::Arc;

use efr_protocol::{CallBackground, CallBackgroundResult};
use efr_shell::ShellError;
use efr_stdx::time::Clock as _;
use efr_transport::Responder;
use tokio::sync::oneshot;

use crate::jobs::moves::{self, Confirmed};
use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "call.background";

#[tracing::instrument(skip_all, fields(conversation_id = %params.conversation_id, call_id = %params.call_id, confirm = params.confirm.is_some()))]
pub(crate) async fn handle(
    state: &State,
    params: CallBackground,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    match background(state, &params).await {
        Ok(result) => {
            responder.item(&result).await?;
            Ok(())
        }
        Err(error) => {
            tracing::info!(reason = %error, "a running call did not move to the background");
            Err(receipts::refuse(state, command_id, METHOD, error).await)
        }
    }
}

async fn background(
    state: &State,
    params: &CallBackground,
) -> Result<CallBackgroundResult, DaemonError> {
    let CallBackground { conversation_id, call_id, confirm, .. } = *params;
    ensure_exists(state, conversation_id).await?;
    let refuse = |reason| DaemonError::NotMoved { call_id, reason };
    let moves = state.jobs.moves();
    let call = moves.running(conversation_id, call_id).ok_or_else(|| refuse(moves::NOT_RUNNING))?;
    let settings = Arc::clone(&state.settings.borrow());
    let engine = Arc::clone(&state.engine.borrow());
    let check = moves::check(&engine, &call, &settings).map_err(refuse)?;
    // NOTE: the preview only checks the place; the move holds it from here on.
    let slot = state
        .jobs
        .reserve(conversation_id, &settings.jobs)
        .map_err(|_| refuse(moves::JOBS_FULL))?;
    let now = state.clock.now();
    if !call.contained {
        match confirm {
            None => {
                drop(slot);
                let token = moves
                    .preview(call_id, check.exits.clone(), now)
                    .ok_or_else(|| refuse(moves::ENDED))?;
                tracing::info!("a move of a call outside the sandbox waits for the user's answer");
                return Ok(CallBackgroundResult::Preview {
                    token,
                    command: call.command.clone(),
                    max_minutes: check.minutes,
                    exits: check.exits,
                });
            }
            Some(token) => moves.answered(call_id, token, &check.exits, now).map_err(refuse)?,
        }
    }
    let (to, adopted) = oneshot::channel();
    let (reply, answer) = oneshot::channel();
    let confirmed = Confirmed { slot, adopted, reply, check };
    if moves.confirm(call_id, confirmed).is_err() {
        return Err(refuse(moves::ENDED));
    }
    if let Err(error) = state.shells.request_move(conversation_id, call_id, to).await {
        moves.cancel(call_id);
        return Err(refuse(not_movable(&error)));
    }
    let moved = match state.clock.timeout(moves::MOVE_WAIT, answer).await {
        Ok(Ok(Ok(moved))) => moved,
        Ok(Ok(Err(reason))) => return Err(refuse(reason)),
        Ok(Err(_)) | Err(_) => return Err(refuse(moves::UNFINISHED)),
    };
    // NOTE: the old PTY was marked adopted at the hand-over (`ShellNotice::Adopted`),
    // also for a move whose answer did not come in time (DS56).
    tracing::info!(job = %moved.job, "a running call moved to the background");
    Ok(CallBackgroundResult::Moved { job_id: moved.job })
}

/// The reason of efr-shell's refusal of a hand-over, for the person.
fn not_movable(error: &ShellError) -> &'static str {
    match error {
        ShellError::NotMovable { reason, .. } => reason,
        _ => "the hidden shell could not hand the command over",
    }
}
