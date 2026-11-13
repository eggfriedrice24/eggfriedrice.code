//! The review of an exit by the classifier, which decides alone: `auto` asks the user
//! nothing about an exit (efr's auto spec, sections 7.6, 9.1 and 9.5, as changed in
//! 0.0.9).
//!
//! [`Turn::review`] runs in `Turn::ask` when the turn runs in `auto`, a judge is
//! configured, `classifier.enabled` is on and every reason that asks is an exit
//! ([`Turn::reviews`]). From there no question follows: the call runs or the model
//! reads why it does not, and the user's next message is the way to ask for it. Without
//! a judge, or with `classifier.enabled = false`, the user answers each exit, as in
//! `cautious`. A write or a delete of a tree that the delete guard of full mode covers
//! too, such as the home directory ([`exit::guarded_tree`]), and a change of efr's
//! settings by the model get no review and no question: efr refuses them, and the model
//! gives the user the command ([`Turn::refused_in_auto`]).
//!
//! The steps, each of which ends the review:
//!
//! 1. After 3 denials in a row or 10 in the turn, the classifier stopped for the rest
//!    of the turn (`classifier_paused`). The model can finish the work that stays in the
//!    sandbox; its next exit stops the turn.
//! 2. An exit of the highest risk that no user message names
//!    ([`Approver::Guarded`]), and a payload that the classifier cannot read whole, do
//!    not run and get no review: `exit_judged` names a floor as the judge, and the
//!    reason names the words that a message can use. It counts as a denial.
//! 3. The judge answers. The turn records `exit_requested` with the record and
//!    `exit_review_started` in one batch, waits for the judge while the user can still
//!    interrupt, and records `exit_judged` with the verdict that efr applied. A verdict
//!    from the judge's cache of the turn comes at once and records no
//!    `exit_review_started`, so no client shows a review that never ran. efr applies
//!    the allow rule and the request rule ([`exit::effective`]): an allow runs the
//!    call, and anything else gives the model the reason and counts toward the pause.
//!    When no model of the chain answers, the call does not run; that counts as no
//!    denial, and the third such review in a row stops the turn, because the
//!    reviewer's model or the provider is down.
//!
//! A classifier allow resets the denials in a row.

use std::task::Poll;

use efr_permissions::{Approver, Cause, Effect, Subject};
use efr_protocol::{
    ApprovedBy, ErrorBody, ErrorCode, Event, ExitKind, ExitRecord, JudgeKind, Judgement, Verdict,
};

use super::{Authorization, Ruling, Turn};
use crate::exit::{self, Refusal, Unreviewed};
use crate::{ConversationError, JudgeInput, ToolCall};

/// What the review of an exit decided.
#[derive(Debug)]
pub(super) enum Reviewed {
    /// The classifier decides alone, and the call runs or does not; or the user
    /// interrupted the turn during the review.
    Decided(Authorization),
    /// The user answers: the classifier does not decide the exits of this call.
    Ask,
}

impl Turn {
    /// True when the reviewer decides alone for `call`: the turn runs in `auto`, a judge
    /// is configured and `classifier.enabled` is on. Then `auto` asks the user nothing
    /// that efr itself raised: an exit goes to the reviewer ([`Turn::reviews`]), and a
    /// guarded tree or a settings change is refused ([`Turn::refused_in_auto`]).
    pub(super) fn decides_alone(&self, call: &ToolCall) -> bool {
        call.context.sandboxed()
            && self.shared.deps.judge.is_some()
            && self.config.classifier_enabled
    }

    /// The refusal of `ruling` in place of a question, while the reviewer decides alone
    /// ([`Turn::decides_alone`]):
    ///
    /// - a write or a delete of a tree that only the user gives up
    ///   ([`exit::guarded_tree`]), such as `~`: the model reads the line, to give it to
    ///   the user, who runs it in their own shell. Full mode keeps its question.
    /// - a change of efr's settings by the model: the model tells the user the change.
    ///
    /// A rule of the user with the effect `ask` keeps its question, because the user
    /// chose it. Neither refusal counts toward the pause of the reviewer: no model
    /// decided it. `None` for any other ruling.
    pub(super) async fn refused_in_auto(
        &mut self,
        call: &ToolCall,
        ruling: &Ruling,
    ) -> Result<Option<Authorization>, ConversationError> {
        let decision = &ruling.decision;
        if !self.decides_alone(call)
            || decision.deciding().any(|reason| matches!(reason.cause, Cause::Rule { .. }))
        {
            return Ok(None);
        }
        if exit::guarded_tree(decision) {
            let trees = exit::guarded_trees(decision, ruling.engine.locations().home());
            let line = ruling.requirements.command.as_deref().unwrap_or_default();
            let turn_id = self.turn_id();
            let call_id = call.context.call_id;
            let kinds = exit::kinds(decision);
            let rationale = format!("only you give up {trees}; the model has the command for you");
            let record = self.exit_record(call, ruling);
            self.record(vec![
                Event::ExitRequested {
                    turn_id,
                    call_id,
                    kinds: kinds.clone(),
                    grants: ruling.launch.grants().to_vec(),
                    source: exit::source(decision),
                    record: Box::new(record),
                },
                Event::ExitJudged {
                    turn_id,
                    call_id,
                    judge: JudgeKind::Floor,
                    verdict: Verdict::Deny,
                    model: None,
                    latency_ms: None,
                    risk: None,
                    user_authorization: None,
                    category: kinds.first().map(|kind| kind.as_str().to_owned()),
                    rationale: Some(rationale.clone()),
                    record_sha256: None,
                    cached: false,
                    chain_step: None,
                    skipped: Vec::new(),
                    reasked: false,
                },
            ])
            .await?;
            let message = format!(
                "efr did not run this line: it writes or deletes {trees}, which only the user \
                 may give up. Do not try it another way. If the user wants it, they run the \
                 command in their own shell: {line}"
            );
            return Ok(Some(Authorization::Refused { message, refusal: Some(rationale) }));
        }
        let settings =
            decision.deciding().find_map(|reason| match (&reason.cause, &reason.subject) {
                (Cause::SettingsChange, Subject::Settings { summary, .. }) => Some(summary.clone()),
                _ => None,
            });
        if let Some(summary) = settings {
            let message = format!(
                "efr did not change its settings ({summary}): in auto only the user changes \
                 them. Tell the user the change; they make it with `efr config set` or in \
                 config.toml."
            );
            let refusal = Some("only you change efr's settings".to_owned());
            return Ok(Some(Authorization::Refused { message, refusal }));
        }
        Ok(None)
    }

    /// True when the classifier decides the exits of `ruling` alone, with no question
    /// to the user: see the module doc.
    ///
    /// Every reason that asks must be an exit. The ask of a privileged program, such as
    /// `sudo`, counts as one when the line has its `privilege` exit, which is the same
    /// fact in the record. Any other reason that asks, such as a rule of the user, keeps
    /// the question.
    pub(super) fn reviews(&self, call: &ToolCall, ruling: &Ruling) -> bool {
        let decision = &ruling.decision;
        let privilege = decision.exits().any(|need| need.kind == ExitKind::Privilege);
        self.decides_alone(call)
            && ruling.approver.is_some()
            && decision.effect() == Effect::Ask
            && decision.deciding().all(|reason| match reason.cause {
                Cause::Exit { .. } => true,
                Cause::Privileged { .. } => privilege,
                _ => false,
            })
            && !exit::guarded_tree(decision)
    }

    /// Decides the exits of `ruling`, whose record is `record`, when [`Turn::reviews`]
    /// says so.
    pub(super) async fn review(
        &mut self,
        call: &ToolCall,
        ruling: &Ruling,
        record: &ExitRecord,
    ) -> Result<Reviewed, ConversationError> {
        let Some(judge) = self.shared.deps.judge.clone().filter(|_| self.reviews(call, ruling))
        else {
            return Ok(Reviewed::Ask);
        };
        let turn_id = self.turn_id();
        let call_id = call.context.call_id;
        let decision = &ruling.decision;
        let kinds = exit::kinds(decision);
        let requested = Event::ExitRequested {
            turn_id,
            call_id,
            kinds: kinds.clone(),
            grants: ruling.launch.grants().to_vec(),
            source: exit::source(decision),
            record: Box::new(record.clone()),
        };
        // NOTE: the exit that stops the turn keeps its record, so the history shows
        // what nobody could decide.
        if let Some((reason, denials)) = self.exits.paused {
            self.record(vec![requested]).await?;
            let message = exit::reviewer_stopped(reason, denials);
            self.stop = Some(ErrorBody::new(ErrorCode::Forbidden, message.clone()));
            return Ok(Reviewed::Decided(Authorization::Denied { message, refusal: None }));
        }
        let unreviewed = if ruling.approver == Some(Approver::Guarded) {
            let names = exit::missing_names(decision, &self.user_messages.naming);
            Some(Unreviewed::Unnamed { names })
        } else if ruling.payload.as_ref().is_some_and(|payload| !payload.judgeable()) {
            Some(Unreviewed::Unread)
        } else {
            None
        };
        if let Some(refusal) = unreviewed {
            // NOTE: a rule of efr refused it, so `exit_judged` names a floor. It counts
            // as a denial, with the reviewer's own: an allow starts the count in a row
            // again, and the pause that follows stops the turn at its next exit.
            let mut events = vec![requested];
            events.push(Event::ExitJudged {
                turn_id,
                call_id,
                judge: JudgeKind::Floor,
                verdict: Verdict::Deny,
                model: None,
                latency_ms: None,
                risk: None,
                user_authorization: None,
                category: kinds.first().map(|kind| kind.as_str().to_owned()),
                rationale: Some(refusal.for_user()),
                record_sha256: None,
                cached: false,
                chain_step: None,
                skipped: Vec::new(),
                reasked: false,
            });
            self.exits.judged(&kinds, Verdict::Deny);
            let mut message = refusal.for_model();
            // NOTE: the model reads that the pause came, so that it writes its answer
            // before another exit ends the turn.
            if let Some((reason, denials)) = self.exits.denied() {
                events.push(Event::ClassifierPaused { turn_id, reason, denials });
                message.push_str(exit::PAUSED);
            }
            self.record(events).await?;
            return Ok(Reviewed::Decided(Authorization::Denied {
                message,
                refusal: Some(refusal.for_user()),
            }));
        }
        let key = self.model_key();
        let model = key.model;
        let provider = key.provider;
        let effort = self.settings.as_ref().and_then(|settings| settings.effort.clone());
        let input = JudgeInput {
            conversation_id: self.shared.conversation_id,
            turn_id,
            call_id,
            turn_model: &model,
            turn_provider: Some(provider.as_str()),
            turn_effort: effort.as_deref(),
            record,
            egress: ruling.engine.support().egress,
        };
        let mut judging = judge.judge(input);
        let started = Event::ExitReviewStarted { turn_id, call_id, kinds: kinds.clone() };
        // NOTE: a verdict of the judge's cache comes at the first poll; only a review
        // that waits for a model records its start.
        let outcome = match futures::poll!(&mut judging) {
            Poll::Ready(outcome) => {
                let mut events = vec![requested];
                if !outcome.cached {
                    events.push(started);
                }
                self.record(events).await?;
                outcome
            }
            Poll::Pending => {
                self.record(vec![requested, started]).await?;
                let interrupt = self.control.interrupt.clone();
                tokio::select! {
                    biased;
                    () = interrupt.raised() => {
                        return Ok(Reviewed::Decided(Authorization::Interrupted));
                    }
                    outcome = &mut judging => outcome,
                }
            }
        };
        drop(judging);
        let judgement = outcome.judgement.as_ref();
        // NOTE: `None` when no model of the chain answered: an error never allows, and
        // it is no denial of the reviewer.
        let applied: Option<(&Judgement, Result<(), Refusal>)> = judgement.map(|judgement| {
            let fetch = exit::fetch_asked(decision, &self.user_messages.naming);
            let strict = ruling.approver != Some(Approver::Classifier);
            (judgement, exit::effective(record, judgement, fetch, strict))
        });
        let verdict = match applied {
            Some((_, Ok(()))) => Verdict::Allow,
            _ => Verdict::Deny,
        };
        // NOTE: the event holds the verdict and the reason that efr applied: an allow
        // that broke a rule reads as refused, with the rule, never as allowed. With no
        // answer, it names the steps that failed, so the user reads why.
        let rationale = match applied {
            Some((judgement, Err(refusal))) => refusal.for_user(judgement),
            Some((judgement, Ok(()))) => judgement.rationale.clone(),
            None => silent_reason(&outcome.skipped),
        };
        let mut events = vec![Event::ExitJudged {
            turn_id,
            call_id,
            judge: JudgeKind::Classifier,
            verdict,
            model: outcome.model.clone(),
            latency_ms: Some(outcome.latency_ms),
            risk: judgement.map(|judgement| judgement.risk),
            user_authorization: judgement.map(|judgement| judgement.user_authorization),
            category: judgement.map(|judgement| judgement.category.clone()),
            rationale: Some(rationale),
            record_sha256: outcome.record_sha256.clone(),
            cached: outcome.cached,
            chain_step: Some(outcome.chain_step),
            skipped: outcome.skipped.clone(),
            reasked: outcome.reasked,
        }];
        let authorization = match applied {
            Some((_, Ok(()))) => {
                self.exits.answered_by_reviewer();
                self.exits.allowed();
                self.exits.judged(&kinds, Verdict::Allow);
                Authorization::Allowed {
                    approved_by: Some(ApprovedBy::Reviewer),
                    launch: ruling.launch.clone(),
                }
            }
            Some((judgement, Err(refusal))) => {
                self.exits.answered_by_reviewer();
                self.exits.judged(&kinds, Verdict::Deny);
                let mut message = refusal.for_model(judgement);
                if let Some((reason, denials)) = self.exits.denied() {
                    events.push(Event::ClassifierPaused { turn_id, reason, denials });
                    message.push_str(exit::PAUSED);
                }
                Authorization::Denied { message, refusal: None }
            }
            // NOTE: nobody judged these exits, so the next record does not list them as
            // denied. The third review in a row with no answer stops the turn: the
            // reviewer's model or the provider is down, and each further exit would only
            // wait for the whole chain and fail.
            None => {
                if self.exits.silent() {
                    let message = exit::reviewer_down();
                    self.stop = Some(ErrorBody::new(ErrorCode::Forbidden, message));
                }
                Authorization::Denied { message: exit::reviewer_silent(), refusal: None }
            }
        };
        self.record(events).await?;
        Ok(Reviewed::Decided(authorization))
    }
}

/// The reason in `exit_judged` when no model of the reviewer's chain answered, with the
/// steps that the chain skipped and why, such as `small-model: timeout`, cut to the
/// length of a rationale.
fn silent_reason(skipped: &[String]) -> String {
    let text = if skipped.is_empty() {
        "no reviewer model answered, so nothing ran".to_owned()
    } else {
        format!("no reviewer model answered ({}), so nothing ran", skipped.join(", "))
    };
    text.chars().take(exit::RATIONALE_CHARS).collect()
}
