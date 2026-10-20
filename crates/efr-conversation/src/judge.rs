//! The judge of an exit: who answers a question about an action that leaves the `auto`
//! sandbox before the user does.
//!
//! In phase 1 the user answers every exit, so no judge is configured and the turn never
//! calls one. From phase A the daemon implements [`ExitJudge`] over a classifier
//! request, and `Turn::ask` asks it first when every deciding reason is an exit that a
//! classifier may judge. The record it reads ([`ExitRecord`]) holds user messages, the
//! action and facts that efr collected itself, never tool output or the model's reason.
//!
//! The daemon's judge owns the chain of models (the classifier model, then the turn's
//! model, then the user), the cache of verdicts of a turn and the hash of the record, so
//! this crate needs no hash function. The turn applies the allow rule and the naming
//! rule to the verdict it gets back.

use std::fmt;

use async_trait::async_trait;
use efr_permissions::Egress;
use efr_protocol::{CallId, ChainStep, ConversationId, ExitRecord, Judgement, TurnId};

/// What the judge reads about one exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JudgeInput<'a> {
    /// The conversation, whose chain of models and skip list the judge keeps.
    pub conversation_id: ConversationId,
    /// The turn, whose cache of verdicts the judge keeps.
    pub turn_id: TurnId,
    /// The call whose exit the record describes.
    pub call_id: CallId,
    /// The turn's model, the second step of the chain.
    pub turn_model: &'a str,
    /// The turn's reasoning effort; `None` leaves it to the backend.
    pub turn_effort: Option<&'a str>,
    /// The record of the exit.
    pub record: &'a ExitRecord,
    /// How contained commands of the call reach the network: the egress of the engine
    /// that decided it, which picks the classifier prompt's variant (phase D2, DP28).
    pub egress: Egress,
}

/// What the judge answers about one exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeOutcome {
    /// The verdict of a model; `None` when the chain ended at the user, after an error,
    /// a timeout or an answer that did not fit at each step. The user decides then.
    pub judgement: Option<Judgement>,
    /// The model that gave the verdict.
    pub model: Option<String>,
    /// The step of the chain that gave the verdict, or [`ChainStep::User`].
    pub chain_step: ChainStep,
    /// The models that the chain skipped, each as `<model>: <reason>`.
    pub skipped: Vec<String>,
    /// How long the judge took, in milliseconds, on the daemon's clock.
    pub latency_ms: u64,
    /// The SHA-256 of the record that the model saw, in hex.
    pub record_sha256: Option<String>,
    /// True when the verdict came from the turn's cache: no model saw the record again.
    pub cached: bool,
    /// True when the classifier model wrote no real reason and the judge asked it once
    /// more (`ExitJudged::reasked`).
    pub reasked: bool,
}

/// Judges one exit from its record: allow, deny or ask the user.
///
/// An error, a timeout or an answer that does not fit goes to the next step of the
/// chain, and at its end to the user; a judge never allows by failing, so it has no
/// error of its own.
#[async_trait]
pub trait ExitJudge: Send + Sync + fmt::Debug {
    /// The verdict about the exit that `input` describes.
    async fn judge(&self, input: JudgeInput<'_>) -> JudgeOutcome;
}

#[cfg(test)]
mod tests;
