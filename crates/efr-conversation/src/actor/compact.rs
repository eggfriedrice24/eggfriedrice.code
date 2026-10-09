//! A manual compaction (`conversation.compact`): the steps of the README's "Context"
//! section between turns, as the actor's one job.
//!
//! The actor starts it only while no turn runs and refuses it with `CompactionBusy`
//! otherwise. It runs in a task of its own, so the actor keeps answering; a prompt that
//! arrives meanwhile waits in the queue and starts after it. A manual compaction always
//! writes a summary, records `conversation_compacted` with trigger `manual`, no turn and
//! the user's focus, together with the command's receipt, and never starts a turn.

use std::collections::HashMap;
use std::sync::Arc;

use efr_protocol::{
    Compaction, CompactionId, CompactionTrigger, ConversationCompact, ConversationCompactResult,
    Event, Seq, TurnId,
};
use efr_provider::Request;
use efr_stdx::id::uuid_v7;
use efr_store::Batch;

use super::receipt;
use crate::ConversationError;
use crate::compaction::{self, Job, Outcome, request_tokens, with_window};
use crate::context::ContextLimits;
use crate::fresh::{self, Fresh};
use crate::history::{CachedTurn, ModelKey, Snapshot};
use crate::turn::{Shared, read_fresh, summarized, wire_usage};

/// The method of the receipt.
pub(super) const CONVERSATION_COMPACT: &str = "conversation.compact";

/// What a manual compaction hands back to the actor: its answer, and the fresh context
/// block of the new compaction for the next turns.
#[derive(Debug)]
pub(super) struct Compacted {
    pub(super) result: Result<ConversationCompactResult, ConversationError>,
    pub(super) fresh: Option<Fresh>,
}

/// Runs the manual compaction `params` of the conversation of `shared`, with the
/// actor's cached turns and fresh block.
pub(super) async fn run(
    shared: Arc<Shared>,
    params: ConversationCompact,
    cache: HashMap<TurnId, Arc<CachedTurn>>,
    fresh: Option<Fresh>,
) -> Compacted {
    let mut fresh = fresh;
    let result = compact(&shared, params, &cache, &mut fresh).await;
    Compacted { result, fresh }
}

async fn compact(
    shared: &Shared,
    params: ConversationCompact,
    cache: &HashMap<TurnId, Arc<CachedTurn>>,
    fresh: &mut Option<Fresh>,
) -> Result<ConversationCompactResult, ConversationError> {
    let conversation_id = shared.conversation_id;
    let config = shared.config.current();
    let snapshot = Snapshot::read(&shared.deps.readers, conversation_id, config.history).await?;
    // NOTE: the model and the effort of the newest turn, so the summary request shares
    // the prefix of the last request and hits the prompt cache.
    let settings = snapshot.newest_settings();
    let model = settings.map_or_else(|| config.model.clone(), |settings| settings.model.clone());
    let effort = settings.map_or_else(|| config.effort.clone(), |settings| settings.effort.clone());
    let cwd = snapshot
        .summary
        .as_ref()
        .and_then(|summary| summary.cwd.clone())
        .unwrap_or_else(|| shared.deps.home.path().to_path_buf());
    let logged_shell_cwd = snapshot.agent_cwd();
    // NOTE: the stored block, so the summary request starts with the bytes of the last
    // request; only a compaction from before the stored block reads the disk again.
    let head = match snapshot.summary() {
        Some(compaction) => match fresh::stored(compaction, fresh.as_ref()) {
            Some(text) => Some(text),
            None => Some(
                read_fresh(&shared.deps, conversation_id, &cwd, logged_shell_cwd.clone()).await,
            ),
        },
        None => None,
    };
    let context_window =
        config.models.iter().find(|info| info.id == model).and_then(|info| info.context_window);
    let limits = ContextLimits::new(context_window, config.compaction);
    let key = ModelKey::new(shared.deps.provider.id().clone(), model.clone());
    let history = config.history.for_window(limits.window);
    let window = snapshot.window(None, cache, &key, history, head.as_deref());
    let base = Request {
        model: model.clone(),
        system: config.system_prompt.clone().filter(|system| !system.is_empty()),
        messages: Vec::new(),
        tools: shared
            .deps
            .toolbox
            .definitions(crate::turn::edit_tool(&*shared.deps.provider, &model)),
        max_output_tokens: config.max_output_tokens,
        effort,
        side_call: false,
        provider_options: crate::turn::provider_options(&config, shared.conversation_id),
    };
    let tokens_before = request_tokens(&with_window(&base, &window));
    let job = Job {
        provider: &*shared.deps.provider,
        base: &base,
        window: &window,
        limits,
        trigger: CompactionTrigger::Manual,
        focus: params.focus.as_deref(),
        interrupt: None,
        gap: &shared.gap,
        clock: &*shared.deps.clock,
    };
    let compaction_id = CompactionId::from_uuid(uuid_v7(&*shared.deps.clock, &*shared.deps.rng));
    let (summary, usage, tail, pruned, omitted) = match compaction::run(job).await {
        Outcome::Summarized { summary, usage, tail, pruned, omitted } => {
            (summary, usage, tail, pruned, omitted)
        }
        Outcome::Nothing => return Err(ConversationError::NothingToCompact { conversation_id }),
        Outcome::Failed(source) => {
            return Err(ConversationError::Summary { source: Arc::new(source) });
        }
        Outcome::Empty => return Err(ConversationError::EmptySummary),
        Outcome::Incomplete(stop) => {
            tracing::warn!(?stop, "the summary was cut off; nothing was compacted");
            return Err(ConversationError::IncompleteSummary);
        }
        Outcome::Pruned(_) | Outcome::Interrupted => {
            unreachable!("a manual compaction always summarizes and has no interrupt")
        }
    };
    let text = read_fresh(&shared.deps, conversation_id, &cwd, logged_shell_cwd).await;
    let new_fresh = Fresh { compaction_id, text };
    let Some(built) = summarized(&window, &new_fresh, summary, usage, tail, pruned, omitted) else {
        return Err(ConversationError::NothingToCompact { conversation_id });
    };
    let compaction = Compaction {
        compaction_id,
        turn_id: None,
        trigger: CompactionTrigger::Manual,
        focus: params.focus.clone().filter(|focus| !focus.trim().is_empty()),
        model,
        window: limits.window,
        limit: limits.limit(),
        tokens_before,
        tokens_after: request_tokens(&with_window(&base, &built.window)),
        through_turn: built.cut.through_turn,
        through_message: built.cut.through_message,
        kept_turns: built.kept_turns,
        pruned_outputs: built.pruned_outputs,
        pruned_tokens: built.pruned_tokens,
        omitted_turns: built.omitted_turns,
        omitted_messages: built.omitted_messages,
        fresh: built.fresh,
        summary: built.summary,
        usage: built.usage.map(wire_usage),
    };
    let result = ConversationCompactResult { seq: Seq::ZERO, compaction: compaction.clone() };
    let receipt = receipt(params.command_id, CONVERSATION_COMPACT, &result)?;
    let batch = Batch::new()
        .event(conversation_id, Event::ConversationCompacted(compaction))
        .receipt(receipt);
    let committed =
        shared.deps.writer.append(batch).await.map_err(ConversationError::from_store)?;
    *fresh = Some(new_fresh);
    Ok(ConversationCompactResult { seq: committed.last_seq(), ..result })
}
