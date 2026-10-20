//! Full mode in a turn (efr's full mode, DF5, DF16 and the record of allowed exits).
//!
//! Full mode runs the model's commands as the user, with no sandbox and no question. The
//! engine decides a full call as an `auto` call and lets through what the sandbox would
//! hold; this file does what the turn adds:
//!
//! - Before each call of a full turn, [`Turn::full_gate`] reads the settings of that
//!   moment. When the user turned full mode off, or the daemon refuses it now (undo or
//!   snapshots went off), the call does not run and the turn stops with `forbidden`.
//! - A call that the engine allows with exits records `exit_requested` and one
//!   `exit_judged` with the judge `full` and the verdict `allow` before it runs
//!   ([`Turn::let_through`]), so the history shows what full mode let through. Nobody
//!   reviews it: the classifier and the always-allow rules read only the `auto` sandbox.
//!
//! What efr records covers only what the line shows: a script or a program that the line
//! starts is not read.

use efr_protocol::{ErrorBody, ErrorCode, Event, JudgeKind, Verdict};

use super::{Ruling, Turn};
use crate::exit;
use crate::{CallContext, ConversationError, ToolCall};

/// What the model reads, and the turn fails with, when the user turned full mode off
/// while the turn ran.
pub(crate) const FULL_TURNED_OFF: &str =
    "full mode was turned off while this turn ran; send the prompt again";

impl Turn {
    /// Why the call of `context` may not run, when its turn runs in full mode and the
    /// settings of this moment no longer allow it: the user turned the gate off, or the
    /// daemon refuses full mode now. `None` for a call that may run, and for every call
    /// of a turn in another mode.
    pub(super) fn full_gate(&self, context: &CallContext) -> Option<String> {
        if !context.is_full() {
            return None;
        }
        // NOTE: read at each call, as the approval timeout is, so a change of the gate or
        // of undo reaches the next call of a running turn (DF16).
        let config = self.shared.config.current();
        let full_mode = &config.full_mode;
        if !full_mode.allowed {
            return Some(FULL_TURNED_OFF.to_owned());
        }
        full_mode.refused.map(|refusal| format!("full mode cannot run: {}", refusal.describe()))
    }

    /// Stops the turn after the current call with `message`, which the model reads as the
    /// call's result: the call does not run.
    pub(super) fn stop_for_full(&mut self, message: &str) {
        tracing::warn!(reason = message, "a full mode turn stopped before its next call");
        self.stop = Some(ErrorBody::new(ErrorCode::Forbidden, message));
    }

    /// Records the exits of `ruling`, which full mode lets run with no question, before
    /// `call` runs: `exit_requested` with their record, and `exit_judged` with the judge
    /// `full` and the verdict `allow`.
    pub(super) async fn let_through(
        &mut self,
        call: &ToolCall,
        ruling: &Ruling,
    ) -> Result<(), ConversationError> {
        let turn_id = self.turn_id();
        let call_id = call.context.call_id;
        let kinds = exit::kinds(&ruling.decision);
        let record = self.exit_record(call, ruling);
        self.record(vec![
            Event::ExitRequested {
                turn_id,
                call_id,
                kinds: kinds.clone(),
                grants: ruling.launch.grants().to_vec(),
                source: exit::source(&ruling.decision),
                record: Box::new(record),
            },
            Event::ExitJudged {
                turn_id,
                call_id,
                judge: JudgeKind::Full,
                verdict: Verdict::Allow,
                model: None,
                latency_ms: None,
                risk: None,
                user_authorization: None,
                category: None,
                rationale: None,
                record_sha256: None,
                cached: false,
                chain_step: None,
                skipped: Vec::new(),
            },
        ])
        .await?;
        self.exits.judged(&kinds, Verdict::Allow);
        Ok(())
    }
}
