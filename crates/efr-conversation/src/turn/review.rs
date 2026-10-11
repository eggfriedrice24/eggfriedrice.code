//! The review of an exit by the classifier, before any question to the user (efr's auto
//! spec, sections 7.6, 9.1 and 9.5).
//!
//! [`Turn::review`] runs in `Turn::ask` when the turn runs in `auto`, a judge is
//! configured, `classifier.enabled` is on, the classifier has not stopped for the turn,
//! every reason that asks is an exit, and every exit goes to the classifier once the
//! user's messages are known ([`exit::route`]). It records `exit_requested` with the
//! record and `exit_review_started` in one batch, waits for the judge while the user
//! can still interrupt, and records `exit_judged`. A verdict from the judge's cache of
//! the turn comes at once and records no `exit_review_started`, so no client shows a
//! review that never ran. efr applies the allow rule and the naming rule
//! ([`exit::effective`]) to the verdict: an allow runs the call, a deny gives the model
//! the reviewer's text and counts toward the pause, and anything else asks the user,
//! whose question shows the classifier's answer.
//!
//! After 3 denials in a row or 10 in the turn, the classifier stops for the rest of
//! the turn (`classifier_paused`): each later exit that it would judge asks the user,
//! and stops the turn when the user is away ([`Turn::paused_and_away`]). A classifier
//! allow or any answer of the user resets the denials in a row; an error, a step of the
//! chain and an expired question count as none.

use std::task::Poll;

use efr_permissions::{Approver, Cause, Effect};
use efr_protocol::{
    ErrorBody, ErrorCode, Event, ExitRecord, JudgeKind, Judgement, Presence, Verdict,
};

use super::{Authorization, Ruling, Turn};
use crate::exit;
use crate::{ConversationError, JudgeInput, ToolCall};

/// What the review of an exit decided.
#[derive(Debug)]
pub(super) enum Reviewed {
    /// The classifier decided, or the user interrupted the turn during the review.
    Decided(Authorization),
    /// The user decides. `recorded` is true when the review recorded `exit_requested`
    /// already, and `judgement` is the classifier's answer for the question.
    Ask { recorded: bool, judgement: Option<Judgement> },
}

impl Turn {
    /// True when the classifier judges the exits of `ruling` before the user: see the
    /// module doc.
    pub(super) fn reviews(&self, call: &ToolCall, ruling: &Ruling) -> bool {
        call.context.auto
            && self.shared.deps.judge.is_some()
            && self.config.classifier_enabled
            && !self.exits.classifier_off()
            && self.goes_to_classifier(ruling)
    }

    /// True when every reason of `ruling` that asks is an exit, and the classifier may
    /// judge each exit; whether it judges now is [`Turn::reviews`].
    pub(super) fn goes_to_classifier(&self, ruling: &Ruling) -> bool {
        ruling.decision.effect() == Effect::Ask
            && ruling.decision.deciding().all(|reason| matches!(reason.cause, Cause::Exit { .. }))
            && ruling.approver == Some(Approver::Classifier)
    }

    /// Stops the turn when the classifier stopped for it and would have judged the exits
    /// of `ruling`, and nobody is there to decide them (efr's auto spec, section 9.5):
    /// the call does not run, and the turn ends after it. `None` when the user is
    /// present, or the classifier still judges, or would not have judged them.
    pub(super) fn paused_and_away(
        &mut self,
        call: &ToolCall,
        ruling: &Ruling,
    ) -> Option<Authorization> {
        let (reason, denials) = self.exits.paused?;
        let judged = call.context.auto
            && self.shared.deps.judge.is_some()
            && self.config.classifier_enabled
            && self.goes_to_classifier(ruling);
        let away =
            self.shared.deps.presence.presence(self.shared.conversation_id) == Presence::Away;
        if !judged || !away {
            return None;
        }
        let message = exit::away_stopped(reason, denials);
        self.stop = Some(ErrorBody::new(ErrorCode::Forbidden, message.clone()));
        Some(Authorization::Denied { message, refusal: None })
    }

    /// Asks the judge about the exits of `ruling`, whose record is `record`, when
    /// [`Turn::reviews`] says so.
    pub(super) async fn review(
        &mut self,
        call: &ToolCall,
        ruling: &Ruling,
        record: &ExitRecord,
    ) -> Result<Reviewed, ConversationError> {
        let Some(judge) = self.shared.deps.judge.clone().filter(|_| self.reviews(call, ruling))
        else {
            return Ok(Reviewed::Ask { recorded: false, judgement: None });
        };
        let turn_id = self.turn_id();
        let call_id = call.context.call_id;
        let decision = &ruling.decision;
        let kinds = exit::kinds(decision);
        let model = self.model_key().model;
        let effort = self.settings.as_ref().and_then(|settings| settings.effort.clone());
        let input = JudgeInput {
            conversation_id: self.shared.conversation_id,
            turn_id,
            call_id,
            turn_model: &model,
            turn_effort: effort.as_deref(),
            record,
        };
        let mut judging = judge.judge(input);
        let requested = Event::ExitRequested {
            turn_id,
            call_id,
            kinds: kinds.clone(),
            grants: ruling.launch.grants().to_vec(),
            source: exit::source(decision),
            record: Box::new(record.clone()),
        };
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
        let verdict = match &outcome.judgement {
            Some(judgement) => exit::effective(record, judgement, exit::package_fetch(decision)),
            None => Verdict::AskUser,
        };
        let judgement = outcome.judgement.as_ref();
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
            rationale: judgement.map(|judgement| judgement.rationale.clone()),
            record_sha256: outcome.record_sha256.clone(),
            cached: outcome.cached,
            chain_step: Some(outcome.chain_step),
            skipped: outcome.skipped.clone(),
        }];
        let reviewed = match (verdict, judgement) {
            (Verdict::Allow, _) => {
                self.exits.allowed();
                self.exits.judged(&kinds, Verdict::Allow);
                // NOTE: never `approved_interactive`: nobody follows the call, so it
                // keeps the model's timeout.
                Reviewed::Decided(Authorization::Allowed {
                    approved_interactive: false,
                    launch: ruling.launch.clone(),
                })
            }
            (Verdict::Deny, Some(judgement)) => {
                self.exits.judged(&kinds, Verdict::Deny);
                if let Some((reason, denials)) = self.exits.denied() {
                    events.push(Event::ClassifierPaused { turn_id, reason, denials });
                }
                Reviewed::Decided(Authorization::Denied {
                    message: exit::reviewer_denied(judgement),
                    refusal: None,
                })
            }
            // NOTE: the question shows the verdict that efr applied, as `exit_judged`
            // does: an allow that broke a rule reads as unsure, never as allowed.
            _ => Reviewed::Ask {
                recorded: true,
                judgement: outcome
                    .judgement
                    .clone()
                    .map(|judgement| Judgement { verdict, ..judgement }),
            },
        };
        self.record(events).await?;
        Ok(reviewed)
    }
}
