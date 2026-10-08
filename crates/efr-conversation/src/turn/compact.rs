//! The context of a running turn: the estimate before each model call, the guards, and
//! the compaction inside the turn.
//!
//! [`Turn::guard`] runs before each model call: it estimates the request, compacts at
//! the trigger when auto compaction is on and the breaker allows it, and refuses a
//! request above the hard cap. [`Turn::compact`] is the hook of an auto or an overflow
//! compaction: it runs between two model calls, records `conversation_compacted`, and
//! the turn goes on with the new window. The README, section "Context", is the
//! contract.

use std::sync::Arc;

use efr_protocol::{Compaction, CompactionId, CompactionTrigger, ErrorBody, Event};
use efr_provider::{Message, Request, TokenUsage};
use efr_stdx::id::uuid_v7;

use super::stream::Response;
use super::{Ending, Turn, context_tokens, provider_failure};
use crate::compaction::{
    self, Job, Outcome, Pruning, Window, cut_before, request_tokens, summary_message, turn_count,
    with_window,
};
use crate::context::{BREAKER_TRIES, ContextLimits, Full, context_full};
use crate::fresh::{self, Fresh};
use crate::{ConversationDeps, ConversationError};

/// What the guard before a model call decided.
#[derive(Debug)]
pub(super) enum Guard {
    /// Send the request; `estimate` is its estimated size.
    Send { estimate: u64 },
    /// The request does not fit and the turn cannot compact: the turn fails.
    Full(ErrorBody),
    /// The user interrupted the turn during a compaction.
    Interrupted,
}

/// What a compaction inside the turn did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Compacted {
    /// It recorded `conversation_compacted`; the window is the new one.
    Done,
    /// It freed nothing and recorded nothing: nothing lay before the tail, or the
    /// summary request failed. The window is as it was.
    NotDone,
    /// The user interrupted the turn during the summary call.
    Interrupted,
}

/// What a model call that the provider refused as too large ended with.
pub(super) enum Recovered {
    /// The turn compacted and the call went through.
    Done(Message),
    /// The turn ends.
    Stop(Ending),
}

impl Turn {
    /// The limits of the turn's model: its window from the model list, and the
    /// compaction settings of the turn.
    pub(super) fn limits(&self) -> ContextLimits {
        let model = self.model_key().model;
        let window = self
            .config
            .models
            .iter()
            .find(|info| info.id == model)
            .and_then(|info| info.context_window);
        ContextLimits::new(window, self.config.compaction)
    }

    /// The estimated context of `request`: the last real count plus the estimate of the
    /// messages added since, or the estimate of the whole request when no real count
    /// holds for this history. Live clients see it.
    pub(super) fn estimate(&mut self, request: &Request) -> u64 {
        let tokens = self.meter.estimate(request);
        self.estimated = Some(tokens);
        self.drafter.context(self.limits().gauge(tokens));
        tokens
    }

    /// The model call that just ended reported its usage: its input plus its output is
    /// the new base of the estimate, and live clients see it.
    pub(super) fn count_call(&mut self) {
        let Some(call) = self.call_usage.take() else {
            return;
        };
        let tokens = context_tokens(&call);
        self.meter.counted(tokens);
        self.last_call = Some(call);
        self.drafter.context(self.limits().gauge(tokens));
    }

    /// True while the turn may compact on its own: auto compaction is on and the
    /// breaker has not stopped it.
    pub(super) fn may_compact(&self) -> bool {
        self.limits().auto && self.misses < BREAKER_TRIES
    }

    /// The guard before a model call: compacts at the trigger (or above the hard cap)
    /// when the turn may, and refuses a request whose estimate is above the hard cap.
    pub(super) async fn guard(
        &mut self,
        window: &mut Window,
        base: &Request,
    ) -> Result<Guard, ConversationError> {
        let limits = self.limits();
        let mut estimate = self.estimate(&with_window(base, window));
        let full = estimate >= limits.trigger || estimate > limits.hard_cap;
        if full && self.may_compact() {
            match self.compact(CompactionTrigger::Auto, estimate, window, base).await? {
                Compacted::Done => estimate = self.estimate(&with_window(base, window)),
                Compacted::NotDone => {}
                Compacted::Interrupted => return Ok(Guard::Interrupted),
            }
        }
        if estimate <= limits.hard_cap {
            return Ok(Guard::Send { estimate });
        }
        let why = if limits.auto && !self.may_compact() { Full::Breaker } else { Full::Cap };
        tracing::warn!(
            tokens = estimate,
            hard_cap = limits.hard_cap,
            "the request would pass the hard cap of the context"
        );
        Ok(Guard::Full(context_full(why, estimate, &limits)))
    }

    /// The provider refused the request of `window`, estimated at `refused`, as larger
    /// than the model's context window: with auto compaction on, compacts once and
    /// sends the call again once. A second refusal, a compaction that frees nothing, an
    /// open breaker or auto compaction off ends the turn.
    pub(super) async fn recover(
        &mut self,
        window: &mut Window,
        base: &Request,
        refused: u64,
    ) -> Result<Recovered, ConversationError> {
        let limits = self.limits();
        tracing::warn!(
            tokens = refused,
            window = limits.window,
            "the provider refused the request as larger than the model's context window"
        );
        let stop = |why| Ok(Recovered::Stop(Ending::Failed(context_full(why, refused, &limits))));
        if !limits.auto {
            return stop(Full::Refused);
        }
        if !self.may_compact() {
            return stop(Full::Breaker);
        }
        self.catch_up(window);
        match self.compact(CompactionTrigger::Overflow, refused, window, base).await? {
            Compacted::Done => {}
            Compacted::NotDone => return stop(Full::CompactionFailed),
            Compacted::Interrupted => return Ok(Recovered::Stop(Ending::Interrupted)),
        }
        let request = with_window(base, window);
        let estimate = self.estimate(&request);
        if estimate > limits.hard_cap {
            let body = context_full(Full::Cap, estimate, &limits);
            return Ok(Recovered::Stop(Ending::Failed(body)));
        }
        match self.respond(request).await? {
            Response::Done(message) => Ok(Recovered::Done(message)),
            Response::Failed(error) if error.is_context_overflow() => {
                tracing::warn!(
                    tokens = estimate,
                    "the provider refused the request again after a compaction"
                );
                let body = context_full(Full::StillRefused, estimate, &limits);
                Ok(Recovered::Stop(Ending::Failed(body)))
            }
            Response::Failed(error) => {
                Ok(Recovered::Stop(Ending::Failed(provider_failure(&error))))
            }
            Response::Interrupted => Ok(Recovered::Stop(Ending::Interrupted)),
        }
    }

    /// The hook of a compaction inside the turn, between two model calls: prunes, and
    /// summarizes when pruning is not enough, records `conversation_compacted` and puts
    /// the new history in `window`. `tokens_before` is the estimate of the request that
    /// would have gone, or of the one the provider refused. The turn goes on after it.
    /// A compaction that leaves the context at or above the trigger, or that frees
    /// nothing, counts as a miss of the breaker.
    pub(super) async fn compact(
        &mut self,
        trigger: CompactionTrigger,
        tokens_before: u64,
        window: &mut Window,
        base: &Request,
    ) -> Result<Compacted, ConversationError> {
        let limits = self.limits();
        self.drafter.compacting(trigger);
        let provider = Arc::clone(&self.shared.deps.provider);
        let interrupt = self.control.interrupt.clone();
        let job = Job {
            provider: &*provider,
            base,
            window,
            limits,
            trigger,
            focus: None,
            interrupt: Some(&interrupt),
        };
        let compaction_id =
            CompactionId::from_uuid(uuid_v7(&*self.shared.deps.clock, &*self.shared.deps.rng));
        let built = match compaction::run(job).await {
            Outcome::Interrupted => return Ok(Compacted::Interrupted),
            Outcome::Pruned(pruning) => pruned(window, pruning),
            Outcome::Summarized { summary, usage, tail, pruned } => {
                let text = read_fresh(
                    &self.shared.deps,
                    self.shared.conversation_id,
                    &self.cwd,
                    self.logged_shell_cwd.clone(),
                )
                .await;
                let fresh = Fresh { compaction_id, text };
                let built = summarized(window, &fresh, summary, usage, tail, pruned);
                self.fresh = Some(fresh);
                built
            }
            Outcome::Nothing => {
                tracing::info!(
                    ?trigger,
                    "the context could not be compacted: nothing lies before the tail"
                );
                None
            }
            Outcome::Failed(error) => {
                tracing::warn!(?trigger, error = %error, "the summary request failed; nothing was compacted");
                None
            }
            Outcome::Empty => {
                tracing::warn!(?trigger, "the summary request gave no text; nothing was compacted");
                None
            }
        };
        let Some(built) = built else {
            self.miss(tokens_before, limits);
            return Ok(Compacted::NotDone);
        };
        let tokens_after = request_tokens(&with_window(base, &built.window));
        let compaction = Compaction {
            compaction_id,
            turn_id: Some(self.turn_id()),
            trigger,
            focus: None,
            model: base.model.clone(),
            window: limits.window,
            limit: limits.limit(),
            tokens_before,
            tokens_after,
            through_turn: built.cut.through_turn,
            through_message: built.cut.through_message,
            kept_turns: built.kept_turns,
            pruned_outputs: built.pruned_outputs,
            pruned_tokens: built.pruned_tokens,
            summary: built.summary,
            usage: built.usage.map(wire_usage),
        };
        self.record(vec![Event::ConversationCompacted(compaction)]).await?;
        *window = built.window;
        // NOTE: the real count of the last call held for the old history.
        self.meter.reset();
        if tokens_after >= limits.trigger {
            self.miss(tokens_after, limits);
        } else {
            self.misses = 0;
        }
        Ok(Compacted::Done)
    }

    /// A compaction did not bring the context of `tokens` below the trigger.
    fn miss(&mut self, tokens: u64, limits: ContextLimits) {
        self.misses += 1;
        if self.misses >= BREAKER_TRIES {
            tracing::warn!(
                tokens,
                trigger = limits.trigger,
                "compaction did not free enough room; the turn stops compacting on its own"
            );
        }
    }

    /// The fresh context block for a history that starts with the summary of
    /// `compaction_id`: the actor's copy when it belongs to that compaction, else read
    /// from disk now, as after a daemon restart.
    pub(super) async fn fresh_for(&mut self, compaction_id: CompactionId) -> String {
        if let Some(fresh) =
            self.fresh.as_ref().filter(|fresh| fresh.compaction_id == compaction_id)
        {
            return fresh.text.clone();
        }
        let text = read_fresh(
            &self.shared.deps,
            self.shared.conversation_id,
            &self.cwd,
            self.logged_shell_cwd.clone(),
        )
        .await;
        self.fresh = Some(Fresh { compaction_id, text: text.clone() });
        text
    }
}

/// The fresh context block as the model reads it, read from disk now.
pub(crate) async fn read_fresh(
    deps: &ConversationDeps,
    conversation_id: efr_protocol::ConversationId,
    cwd: &std::path::Path,
    logged_shell_cwd: Option<std::path::PathBuf>,
) -> String {
    fresh::read(deps, conversation_id, cwd, logged_shell_cwd).await.render()
}

/// The wire usage of the summary call: one call, so its context is its input plus its
/// output.
pub(crate) fn wire_usage(usage: TokenUsage) -> efr_protocol::Usage {
    let mut wire = efr_protocol::Usage::from(usage);
    wire.context_tokens = usage.input_tokens.saturating_add(usage.output_tokens);
    wire
}

/// A compaction ready to record.
pub(crate) struct Built {
    pub(crate) window: Window,
    pub(crate) cut: compaction::Cut,
    pub(crate) kept_turns: u32,
    pub(crate) pruned_outputs: u32,
    pub(crate) pruned_tokens: u64,
    pub(crate) summary: Option<String>,
    pub(crate) usage: Option<TokenUsage>,
}

/// The window after a pruning alone: the stubs, and the head as it was.
fn pruned(window: &Window, pruning: Pruning) -> Option<Built> {
    let cut = pruning.cut?;
    let kept_turns = turn_count(&pruning.placed[pruning.after.min(pruning.placed.len())..]);
    Some(Built {
        window: Window { head: window.head.clone(), placed: pruning.placed },
        cut,
        kept_turns,
        pruned_outputs: pruning.outputs,
        pruned_tokens: pruning.tokens,
        summary: None,
        usage: None,
    })
}

/// The window after a summary: the fresh block, the summary, then the verbatim tail
/// from `window.placed[tail]`.
pub(crate) fn summarized(
    window: &Window,
    fresh: &Fresh,
    summary: String,
    usage: Option<TokenUsage>,
    tail: usize,
    pruned: Option<Pruning>,
) -> Option<Built> {
    let cut = cut_before(&window.placed, tail)?;
    let placed = window.placed[tail..].to_vec();
    let head = vec![Message::user(fresh.text.clone()), summary_message(&summary)];
    let (pruned_outputs, pruned_tokens) =
        pruned.map_or((0, 0), |pruning| (pruning.outputs, pruning.tokens));
    Some(Built {
        kept_turns: turn_count(&placed),
        window: Window { head, placed },
        cut,
        pruned_outputs,
        pruned_tokens,
        summary: Some(summary),
        usage,
    })
}
