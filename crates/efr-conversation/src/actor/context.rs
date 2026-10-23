//! What fills the model's context (`conversation.context`): the request that the next
//! model call would send, built as a manual compaction builds it, and measured by
//! `context/breakdown.rs`.
//!
//! The actor starts it at any time, also while a turn runs, in a task of its own, so
//! the actor keeps answering. It only reads: the snapshot of the log that a turn reads,
//! the end of each turn and the compactions. While a turn runs, its messages up to now
//! join the history, rebuilt from its events, as a turn rebuilds a turn whose exact
//! messages are lost. The next prompt and its preamble are not known yet, so the
//! estimate leaves them out.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use efr_protocol::{
    Compaction, ConversationContext, ConversationContextResult, ConversationId, EventEnvelope,
    TurnId,
};
use efr_provider::Request;
use efr_store::Readers;
use efr_store::compactions;
use efr_store::conversations::{Turn, TurnStatus};
use efr_store::events;

use crate::ConversationError;
use crate::context::ContextLimits;
use crate::context::breakdown::{self, end_usage};
use crate::fresh::{self, Fresh};
use crate::history::{CachedTurn, ModelKey, Snapshot};
use crate::turn::Shared;

/// Answers `params` for the conversation of `shared`, with the actor's cached turns and
/// fresh block, while `running` runs.
pub(super) async fn run(
    shared: Arc<Shared>,
    params: ConversationContext,
    cache: HashMap<TurnId, Arc<CachedTurn>>,
    fresh: Option<Fresh>,
    running: Option<TurnId>,
) -> Result<ConversationContextResult, ConversationError> {
    let conversation_id = shared.conversation_id;
    let config = shared.config.current();
    let snapshot =
        Snapshot::read(&shared.deps.readers, conversation_id, config.history, &cache).await?;
    // NOTE: the model of the newest turn, as a manual compaction picks it: a model that
    // the model list no longer has gives way to the default.
    let listed =
        |model: &str| config.models.is_empty() || config.models.iter().any(|info| info.id == model);
    let newest = snapshot.newest_settings().filter(|settings| listed(&settings.model));
    let model = match params.model.as_deref().map(str::trim).filter(|model| !model.is_empty()) {
        Some(model) => model.to_owned(),
        None => newest.map_or_else(|| config.model.clone(), |settings| settings.model.clone()),
    };
    let effort = newest.map_or_else(|| config.effort.clone(), |settings| settings.effort.clone());
    let known = config
        .models
        .iter()
        .find(|info| info.id == model)
        .and_then(|info| info.context_window)
        .filter(|window| *window > 0);
    let limits = ContextLimits::new(known, config.compaction);
    let key = ModelKey::new(shared.deps.provider.id().clone(), model.clone());
    let head = snapshot.summary().and_then(|compaction| fresh::stored(compaction, fresh.as_ref()));

    // NOTE: the running turn counts as ended here, so the window rebuilds its messages
    // so far from the page, with a result for each call that still runs.
    let mut view = snapshot.clone();
    for turn in &mut view.turns {
        if Some(turn.id) == running && turn.status == TurnStatus::Running {
            turn.status = TurnStatus::Completed;
        }
    }
    let history = config.history.for_window(limits.window);
    let window = view.window(None, &cache, &key, history, head.as_deref());
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
        output: None,
        one_shot: false,
        provider_options: crate::turn::provider_options(&config, conversation_id),
    };
    let numbers: HashMap<TurnId, u32> = snapshot
        .turns
        .iter()
        .filter(|turn| turn.started_at.is_some())
        .zip(1_u32..)
        .map(|(turn, number)| (turn.id, number))
        .collect();
    let measured = breakdown::measure(&base, &window, &numbers);

    let ended = Ended::read(&shared.deps.readers, conversation_id, &snapshot).await?;
    Ok(ConversationContextResult {
        model,
        window: limits.window,
        window_known: known.is_some(),
        trigger: limits.trigger,
        hard_cap: limits.hard_cap,
        auto: limits.auto,
        counted: breakdown::counted(&snapshot.page),
        estimate: measured.estimate,
        running: running.is_some(),
        parts: measured.parts,
        largest: measured.largest,
        compactions: breakdown::compactions(&ended.compactions, ended.newest_at),
        cache: breakdown::cache(&ended.ends, &ended.compactions),
    })
}

/// The ends of the turns and the compactions of a conversation, from its start.
struct Ended {
    /// The end event of each ended turn, oldest first.
    ends: Vec<EventEnvelope>,
    /// Every compaction, oldest first.
    compactions: Vec<Compaction>,
    /// When efrd recorded the newest compaction.
    newest_at: Option<jiff::Timestamp>,
}

impl Ended {
    /// Reads them in one read transaction. The ends in the page of `snapshot` come from
    /// there; a turn that ended before the page is read by its newest event, or by its
    /// whole range when an event after its end is its newest.
    async fn read(
        readers: &Readers,
        conversation_id: ConversationId,
        snapshot: &Snapshot,
    ) -> Result<Self, ConversationError> {
        let mut ends: Vec<EventEnvelope> = snapshot
            .page
            .iter()
            .filter(|envelope| end_usage(&envelope.event).is_some())
            .cloned()
            .collect();
        let found: HashSet<TurnId> =
            ends.iter().filter_map(|envelope| envelope.event.turn_id()).collect();
        let missing: Vec<Turn> = snapshot
            .turns
            .iter()
            .filter(|turn| ended(turn.status) && !found.contains(&turn.id))
            .cloned()
            .collect();
        let mut read = readers
            .with(move |conn| {
                let mut older = Vec::new();
                for turn in &missing {
                    let newest = events::read_turn_range(
                        conn,
                        conversation_id,
                        turn.last_seq,
                        turn.last_seq,
                    )?;
                    let end = match is_end_of(&newest, turn.id) {
                        Some(end) => Some(end),
                        None => is_end_of(
                            &events::read_turn_range(
                                conn,
                                conversation_id,
                                turn.queued_seq,
                                turn.last_seq,
                            )?,
                            turn.id,
                        ),
                    };
                    older.extend(end);
                }
                let stored = compactions::of_conversation(conn, conversation_id)?;
                let newest_at = match stored.last() {
                    Some(newest) => {
                        events::read_turn_range(conn, conversation_id, newest.seq, newest.seq)?
                            .first()
                            .map(|envelope| envelope.at)
                    }
                    None => None,
                };
                let compactions = stored.into_iter().map(|stored| stored.compaction).collect();
                Ok(Ended { ends: older, compactions, newest_at })
            })
            .await
            .map_err(ConversationError::from_store)?;
        ends.append(&mut read.ends);
        ends.sort_by_key(|envelope| envelope.seq);
        read.ends = ends;
        Ok(read)
    }
}

/// True for the status of a turn that has an end event.
fn ended(status: TurnStatus) -> bool {
    matches!(status, TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Interrupted)
}

/// The end event of `turn` among `envelopes`.
fn is_end_of(envelopes: &[EventEnvelope], turn: TurnId) -> Option<EventEnvelope> {
    envelopes
        .iter()
        .find(|envelope| {
            envelope.event.turn_id() == Some(turn) && end_usage(&envelope.event).is_some()
        })
        .cloned()
}
