//! Fakes for the review of an exit: a judge that answers from a queue, a presence that a
//! test sets, and a model that follows a script of tool calls.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use efr_protocol::{
    CallId, ChainStep, ConversationId, ExitRecord, Judgement, Presence, Risk, UserAuthorization,
    Verdict,
};
use efr_provider::{
    ContentBlock, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request,
};
use serde_json::Value;

use super::{freeform_answer, text_answer, tool_answer};
use crate::{ExitJudge, JudgeInput, JudgeOutcome, PresenceReader};

/// What a fake judge read about one exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) call_id: CallId,
    pub(crate) turn_model: String,
    pub(crate) record: ExitRecord,
}

/// A judge that answers each exit with the next outcome of its queue, and with "the
/// chain ended at the user" when the queue is empty. It keeps what it read. An outcome
/// that is not cached waits for the runtime once before it comes, as a model call does,
/// so the turn sees the review start; a cached one comes at once.
#[derive(Debug, Default)]
pub(crate) struct FakeJudge {
    outcomes: Mutex<VecDeque<JudgeOutcome>>,
    seen: Mutex<Vec<Seen>>,
}

impl FakeJudge {
    /// A judge that gives `outcomes` in order.
    pub(crate) fn answering(outcomes: Vec<JudgeOutcome>) -> Arc<Self> {
        Arc::new(FakeJudge { outcomes: Mutex::new(outcomes.into()), seen: Mutex::default() })
    }

    /// Every exit that it judged, in order.
    pub(crate) fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// How many exits it judged.
    pub(crate) fn calls(&self) -> usize {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

#[async_trait]
impl ExitJudge for FakeJudge {
    async fn judge(&self, input: JudgeInput<'_>) -> JudgeOutcome {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).push(Seen {
            call_id: input.call_id,
            turn_model: input.turn_model.to_owned(),
            record: input.record.clone(),
        });
        let next = self.outcomes.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
        let outcome = next.unwrap_or_else(unanswered);
        if !outcome.cached {
            tokio::task::yield_now().await;
        }
        outcome
    }
}

/// The outcome of a model step of the chain: `verdict` at `risk`, with the user's
/// authorization `authorization`.
pub(crate) fn judged(
    verdict: Verdict,
    risk: Risk,
    authorization: UserAuthorization,
) -> JudgeOutcome {
    JudgeOutcome {
        judgement: Some(Judgement {
            verdict,
            risk,
            user_authorization: authorization,
            category: "fits the request".to_owned(),
            rationale: "the user asked for it".to_owned(),
        }),
        model: Some("small-model".to_owned()),
        chain_step: ChainStep::ClassifierModel,
        skipped: Vec::new(),
        latency_ms: 12,
        record_sha256: Some("ab".repeat(32)),
        cached: false,
        reasked: false,
    }
}

/// An allow at a low risk that the user asked for.
pub(crate) fn allows() -> JudgeOutcome {
    judged(Verdict::Allow, Risk::Low, UserAuthorization::High)
}

/// A deny of an action that nothing the user said relates to.
pub(crate) fn denies() -> JudgeOutcome {
    let mut outcome = judged(Verdict::Deny, Risk::High, UserAuthorization::None);
    if let Some(judgement) = outcome.judgement.as_mut() {
        judgement.category = "action outside the request".to_owned();
        judgement.rationale = "nothing the user said relates to it".to_owned();
    }
    outcome
}

/// The chain ended at the user: each model step failed.
pub(crate) fn unanswered() -> JudgeOutcome {
    JudgeOutcome {
        judgement: None,
        model: None,
        chain_step: ChainStep::User,
        skipped: vec!["small-model: timeout".to_owned(), "test-model: error".to_owned()],
        latency_ms: 40_000,
        record_sha256: None,
        cached: false,
        reasked: false,
    }
}

/// A presence that a test sets, which counts its reads and the pushes of away
/// questions.
#[derive(Debug, Default)]
pub(crate) struct FakePresence {
    away: Mutex<bool>,
    followed: Mutex<bool>,
    pushed: Mutex<Vec<CallId>>,
    reads: Mutex<usize>,
    read: tokio::sync::Notify,
}

impl FakePresence {
    /// Waits until a turn read the presence `count` times in all.
    pub(crate) async fn wait_for_reads(&self, count: usize) {
        loop {
            let read = self.read.notified();
            tokio::pin!(read);
            read.as_mut().enable();
            if *self.reads.lock().unwrap_or_else(PoisonError::into_inner) >= count {
                return;
            }
            read.await;
        }
    }

    /// Counts the user away from now on, or present.
    pub(crate) fn set_away(&self, away: bool) {
        *self.away.lock().unwrap_or_else(PoisonError::into_inner) = away;
    }

    /// Counts a client that can answer as following the conversation from now on, or
    /// none.
    pub(crate) fn set_followed(&self, followed: bool) {
        *self.followed.lock().unwrap_or_else(PoisonError::into_inner) = followed;
    }

    /// How many times a turn read the presence.
    pub(crate) fn reads(&self) -> usize {
        *self.reads.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The questions that it sent to the phones, in order.
    pub(crate) fn pushed(&self) -> Vec<CallId> {
        self.pushed.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl PresenceReader for FakePresence {
    fn presence(&self, _conversation_id: ConversationId) -> Presence {
        *self.reads.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        self.read.notify_waiters();
        if *self.away.lock().unwrap_or_else(PoisonError::into_inner) {
            Presence::Away
        } else {
            Presence::Present
        }
    }

    fn push(&self, _conversation_id: ConversationId, call_id: CallId) {
        self.pushed.lock().unwrap_or_else(PoisonError::into_inner).push(call_id);
    }

    fn followed(&self, _conversation_id: ConversationId) -> bool {
        *self.followed.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The tool of a [`Script`] entry that answers with text and ends the turn.
pub(crate) const DONE: &str = "";

/// A model that answers its requests in order: each `(tool, input)` of the script as
/// one tool call (a freeform one when the input is text), or the text `Done.` for an
/// entry whose tool is [`DONE`], then the text `Done.` for every later request. It keeps every
/// request, so a test reads what each call gave back without writing out the requests.
#[derive(Debug)]
pub(crate) struct Script {
    id: ProviderId,
    calls: Mutex<VecDeque<(String, Value)>>,
    requests: Mutex<Vec<Request>>,
}

impl Script {
    pub(crate) fn new(calls: Vec<(&str, Value)>) -> Arc<Self> {
        let calls = calls.into_iter().map(|(tool, input)| (tool.to_owned(), input)).collect();
        Arc::new(Script {
            id: ProviderId::new("replay").expect("provider id"),
            calls: Mutex::new(calls),
            requests: Mutex::default(),
        })
    }

    /// Every request that the model got, in order.
    pub(crate) fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The result of each tool call that the model read, in order, with whether it was
    /// an error.
    pub(crate) fn results(&self) -> Vec<(String, bool)> {
        let requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(last) = requests.last() else {
            return Vec::new();
        };
        last.messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::ToolResult { output, is_error, .. } => {
                    Some((output.clone(), *is_error))
                }
                _ => None,
            })
            .collect()
    }
}

#[async_trait]
impl Provider for Script {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        let mut requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
        let number = requests.len();
        requests.push(request);
        let next = self.calls.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
        let events: Vec<ProviderEvent> = match next {
            Some((tool, _)) if tool == DONE => text_answer("Done."),
            Some((tool, Value::String(text))) => {
                freeform_answer(&format!("call_{number}"), &tool, &text)
            }
            Some((tool, input)) => tool_answer(&format!("call_{number}"), &tool, &input),
            None => text_answer("Done."),
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}
