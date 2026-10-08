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

use efr_protocol::{
    Compaction, CompactionId, CompactionTrigger, DraftPart, ErrorBody, ErrorCode, Event,
};
use efr_provider::{Request, TokenUsage};
use efr_stdx::id::uuid_v7;

use super::Turn;
use crate::compaction::{
    self, Job, Outcome, Pruning, Window, cut_before, request_tokens, summary_message, turn_count,
    with_window,
};
use crate::context::{BREAKER_TRIES, ContextLimits};
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
    Done { tokens_after: u64 },
    /// It freed nothing and recorded nothing: nothing lay before the tail, or the
    /// summary request failed. The window is as it was.
    NotDone,
    /// The user interrupted the turn during the summary call.
    Interrupted,
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

    /// The estimated context of `request`, whose messages are those of `window`: the
    /// last real count plus the estimate of the messages added since that call, or the
    /// estimate of the whole request when no real count holds for this window.
    pub(super) fn estimate(&self, window: &Window, request: &Request) -> u64 {
        match self.counted {
            Some((tokens, len)) if len <= window.len() => {
                tokens.saturating_add(window.tokens_from(len))
            }
            _ => request_tokens(request),
        }
    }

    /// Takes the usage of the call that just ended as the new base of the estimate:
    /// `window` holds its request and its answer.
    pub(super) fn count(&mut self, window: &Window) {
        if let Some(usage) = self.last_call.take() {
            let tokens = usage.input_tokens.saturating_add(usage.output_tokens);
            self.counted = Some((tokens, window.len()));
        }
    }

    /// True while the turn may compact on its own: auto compaction is on and the
    /// breaker has not stopped it.
    pub(super) fn may_compact(&self) -> bool {
        self.limits().auto && self.misses < BREAKER_TRIES
    }

    /// The guard before a model call: compacts at the trigger when the turn may, and
    /// refuses a request whose estimate is above the hard cap.
    pub(super) async fn guard(
        &mut self,
        window: &mut Window,
        base: &Request,
    ) -> Result<Guard, ConversationError> {
        let limits = self.limits();
        let mut estimate = self.estimate(window, &with_window(base, window));
        if estimate >= limits.trigger && self.may_compact() {
            match self.compact(CompactionTrigger::Auto, estimate, window, base).await? {
                Compacted::Done { tokens_after } => estimate = tokens_after,
                Compacted::NotDone => {}
                Compacted::Interrupted => return Ok(Guard::Interrupted),
            }
        }
        if estimate > limits.hard_cap {
            return Ok(Guard::Full(self.full(estimate)));
        }
        Ok(Guard::Send { estimate })
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
        self.drafter.part(DraftPart::Compacting { trigger });
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
            self.misses += 1;
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
        // NOTE: the real count of the last call held for the old window.
        self.counted = None;
        if tokens_after >= limits.trigger {
            self.misses += 1;
        } else {
            self.misses = 0;
        }
        Ok(Compacted::Done { tokens_after })
    }

    /// The failure of a turn whose request does not fit: `tokens` is the estimate, or
    /// the size that the provider refused.
    pub(super) fn full(&self, tokens: u64) -> ErrorBody {
        full(tokens, self.limits(), self.misses >= BREAKER_TRIES)
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

/// The failure body of a request that does not fit in the model's window.
fn full(tokens: u64, limits: ContextLimits, breaker: bool) -> ErrorBody {
    let window = limits.window;
    let message = if breaker {
        format!(
            "the context is full: compaction did not free enough room (still {} of {} \
             tokens); run ,compact or start a new conversation",
            thousands(tokens),
            thousands(window)
        )
    } else {
        format!(
            "the context is full: {} of {} tokens; run ,compact or start a new conversation",
            thousands(tokens),
            thousands(window)
        )
    };
    ErrorBody::new(ErrorCode::Internal, message).with_data(serde_json::json!({
        "cause": "context_overflow",
        "tokens": tokens,
        "window": window,
    }))
}

/// `tokens` in thousands, rounded, such as `281k`.
fn thousands(tokens: u64) -> String {
    format!("{}k", tokens.saturating_add(500) / 1000)
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
    let head = vec![efr_provider::Message::user(fresh.text.clone()), summary_message(&summary)];
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
