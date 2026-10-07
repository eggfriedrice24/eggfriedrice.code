//! `conversation.subscribe`: a conversation's events, replayed or summarised, then live.
//!
//! The order is the one that loses nothing: subscribe to the store's commits, read the
//! high-water mark and the start in one read transaction, send the start, then forward
//! live events above the mark. The start is a replay of the events after `after_seq`
//! when that gap is at most 128 events and 1 MiB, and otherwise (or without
//! `after_seq`) a bounded snapshot: the summary, the newest events up to the same
//! bounds, and a history cursor for the older ones.
//!
//! Live events go through the transport's bounded subscriber queue (64 items). A
//! subscriber that falls behind, at the queue or at the store's broadcast, is closed
//! with `overflow` and the last sequence number it received.
//!
//! A subscription with `answers_input`, from a connection that holds the `terminal`
//! scope (the one `input.respond` needs), counts as one at which a person can answer a
//! waiting command, for as long as the request lasts. A command that waits for hidden
//! input while its conversation has none is stopped (`tools.rs`).
//!
//! A subscription with `drafts` also gets the drafts of the conversation's running
//! turn: what the model sent before the log has it. The live side subscribes to the
//! daemon's draft channel only after the read of the start, so no draft from before
//! the start follows it. Before it forwards a draft, it forwards every committed event
//! that is already waiting, so a draft never overtakes an event that the turn recorded
//! before it. It drops a draft that the turn made before an event that the subscriber
//! already got and that ends what the draft shows (`assistant_message_completed`,
//! `tool_call_started`, the end of a turn). A draft goes through the queue's lossy
//! room: a full room drops the draft, and a draft channel that left this subscriber
//! behind drops the oldest drafts. Neither closes the subscription.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use efr_conversation::ConversationDraft;
use efr_protocol::{
    ConversationId, ConversationSnapshot, ConversationSubscribe, ConversationSubscribeItem, Draft,
    Event, EventEnvelope, ScopeName, Seq,
};
use efr_store::Committed;
use efr_transport::{
    ConnectionContext, LossyOffer, Offer, Responder, SubscriptionSender, TransportError,
};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::methods::{cursor, granted, peer_side};
use crate::state::State;

/// The most events a resume replays, and the most a snapshot carries.
pub(crate) const MAX_EVENTS: usize = 128;
/// The most event bytes a resume replays, and the most a snapshot carries.
pub(crate) const MAX_BYTES: usize = 1024 * 1024;

/// How far the live side got, for the `overflow` of a subscriber the broadcast left
/// behind. `last` is the subscription guard's, so the notices learn how far this
/// terminal followed the conversation, while the subscription is open and after it
/// ends.
#[derive(Debug)]
struct Progress {
    last: Arc<AtomicU64>,
    lagged: AtomicBool,
}

impl Progress {
    fn raise(&self, seq: Seq) {
        self.last.fetch_max(seq.get(), Ordering::AcqRel);
    }
}

/// Stops the live side when the request ends, however it ends.
#[derive(Debug)]
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: ConversationSubscribe,
    responder: &Responder,
    cancelled: CancellationToken,
) -> Result<(), DaemonError> {
    let conversation_id = params.conversation_id;
    let answering = params.answers_input
        && granted(context.surface(), peer_side(state, context).await)
            .contains(&ScopeName::Terminal);
    let attached = state.connections.subscribe(context.conn_id(), conversation_id, answering);
    // Subscribing before the read means no commit falls between the two.
    let committed = state.writer.subscribe();
    let (sender, mut receiver) = efr_transport::subscription();
    let progress = Arc::new(Progress { last: attached.reached(), lagged: AtomicBool::new(false) });
    let (feed, drafts) = oneshot::channel();
    let live = Live {
        committed,
        sender,
        conversation_id,
        progress: Arc::clone(&progress),
        drafts: None,
        boundary: Seq::ZERO,
    };
    let _producer = AbortOnDrop(tokio::spawn(live.produce(drafts, cancelled)));
    let after = params.after_seq;
    let start = state
        .readers
        .with(move |conn| {
            let Some(summary) = efr_store::conversations::get(conn, conversation_id)? else {
                return Ok(None);
            };
            let hwm = efr_store::events::last_seq(conn)?;
            let bound = u32::try_from(MAX_EVENTS + 1).unwrap_or(u32::MAX);
            if let Some(after) = after {
                let gap = efr_store::events::read_conversation_after(
                    conn,
                    conversation_id,
                    after,
                    bound,
                )?;
                if fits(&gap) {
                    let items = gap.into_iter().map(ConversationSubscribeItem::Event).collect();
                    return Ok(Some(Start { items, hwm }));
                }
            }
            let newest =
                efr_store::events::read_conversation_before(conn, conversation_id, None, bound)?;
            let (events, older) = newest_that_fit(newest);
            let item = ConversationSubscribeItem::Snapshot(snapshot(summary, events, older, hwm));
            Ok(Some(Start { items: vec![item], hwm }))
        })
        .await?
        .ok_or(DaemonError::ConversationNotFound { conversation_id })?;
    let hwm = start.hwm;
    if params.drafts {
        // NOTE: subscribed after the read, so every draft that comes is newer than the
        // start. Without drafts the feed drops, and the live side never waits for it.
        let _ = feed.send(DraftFeed { receiver: state.drafts.subscribe() });
    }
    for item in start.items {
        responder.item(&item).await?;
    }
    receiver.skip_through(hwm);
    progress.raise(hwm);
    match responder.forward(receiver).await {
        Ok(()) if progress.lagged.load(Ordering::Acquire) => {
            let last_seq = Seq::new(progress.last.load(Ordering::Acquire));
            Err(DaemonError::Respond { source: TransportError::Overflow { last_seq } })
        }
        // The transport ends an overflowed subscription with `overflow` itself.
        Ok(()) | Err(TransportError::Overflow { .. }) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// What a subscription starts with, read in one transaction.
#[derive(Debug)]
pub(crate) struct Start {
    pub(crate) items: Vec<ConversationSubscribeItem>,
    pub(crate) hwm: Seq,
}

/// The draft channel of a subscription that asked for drafts, subscribed after the
/// read of its start.
#[derive(Debug)]
struct DraftFeed {
    receiver: broadcast::Receiver<ConversationDraft>,
}

/// The live side of one subscription.
#[derive(Debug)]
struct Live {
    committed: broadcast::Receiver<Committed>,
    sender: SubscriptionSender<ConversationSubscribeItem>,
    conversation_id: ConversationId,
    progress: Arc<Progress>,
    /// The draft channel, once the start is read; `None` without drafts.
    drafts: Option<broadcast::Receiver<ConversationDraft>>,
    /// The sequence number of the last event offered to the subscriber that ends what
    /// a draft shows ([`ends_drafts`]). A draft made before it is old.
    boundary: Seq,
}

/// What the live side does next.
enum Step {
    Batch(Result<Committed, RecvError>),
    Draft(Result<ConversationDraft, RecvError>),
}

impl Live {
    /// Offers the conversation's live events, and its drafts once `feed` brings their
    /// channel, to the subscriber until it closes or the request is cancelled.
    async fn produce(
        mut self,
        mut feed: oneshot::Receiver<DraftFeed>,
        cancelled: CancellationToken,
    ) {
        let mut feed_open = true;
        loop {
            // NOTE: biased, so a commit that waits goes before a draft that waits.
            let step = tokio::select! {
                biased;
                () = cancelled.cancelled() => return,
                batch = self.committed.recv() => Step::Batch(batch),
                fed = &mut feed, if feed_open => {
                    feed_open = false;
                    self.drafts = fed.ok().map(|DraftFeed { receiver }| receiver);
                    continue;
                }
                draft = next_draft(self.drafts.as_mut()) => Step::Draft(draft),
            };
            let open = match step {
                Step::Batch(batch) => self.forward(batch),
                Step::Draft(Ok(draft)) => self.forward_draft(draft),
                Step::Draft(Err(RecvError::Lagged(missed))) => {
                    tracing::debug!(missed, "a subscriber missed drafts");
                    true
                }
                Step::Draft(Err(RecvError::Closed)) => {
                    self.drafts = None;
                    true
                }
            };
            if !open {
                return;
            }
        }
    }

    /// Offers the conversation's events of `batch`; false when the subscription ended.
    fn forward(&mut self, batch: Result<Committed, RecvError>) -> bool {
        let batch = match batch {
            Ok(batch) => batch,
            Err(RecvError::Lagged(_)) => {
                // NOTE: stored before the sender drops, so the handler finds it when the
                // stream ends.
                self.progress.lagged.store(true, Ordering::Release);
                return false;
            }
            Err(RecvError::Closed) => return false,
        };
        for envelope in batch.events() {
            if envelope.conversation_id != Some(self.conversation_id) {
                continue;
            }
            let item = ConversationSubscribeItem::Event(envelope.clone());
            match self.sender.offer(envelope.seq, item) {
                Offer::Queued => {
                    self.progress.raise(envelope.seq);
                    if ends_drafts(&envelope.event) {
                        self.boundary = self.boundary.max(envelope.seq);
                    }
                }
                Offer::Overflowed | Offer::Closed => return false,
            }
        }
        true
    }

    /// Offers `draft` when it belongs to this conversation, after every commit that
    /// already waits, unless an event that ends what it shows went first; false when
    /// the subscription ended.
    fn forward_draft(&mut self, draft: ConversationDraft) -> bool {
        if draft.conversation_id != self.conversation_id {
            return true;
        }
        // NOTE: the turn sent the draft after the commits it made before it, so those
        // commits wait here already; they go first.
        loop {
            let batch = match self.committed.try_recv() {
                Ok(batch) => Ok(batch),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Lagged(missed)) => Err(RecvError::Lagged(missed)),
                Err(TryRecvError::Closed) => Err(RecvError::Closed),
            };
            if !self.forward(batch) {
                return false;
            }
        }
        if draft.after_seq < self.boundary {
            // NOTE: the turn made the draft before an event that the subscriber already
            // has ended its part: an old draft would only show the past again.
            return true;
        }
        let item = ConversationSubscribeItem::Draft(Draft {
            turn_id: draft.turn_id,
            after_seq: draft.after_seq,
            draft: draft.part,
        });
        match self.sender.offer_lossy(item) {
            LossyOffer::Queued | LossyOffer::Dropped => true,
            LossyOffer::Closed => false,
        }
    }
}

/// True for an event after which the drafts made before it show nothing new: the
/// end of the text of a message, the start of a tool call (after the whole answer of
/// the model), and the end of a turn.
fn ends_drafts(event: &Event) -> bool {
    matches!(
        event,
        Event::AssistantMessageCompleted { .. }
            | Event::ToolCallStarted { .. }
            | Event::TurnCompleted { .. }
            | Event::TurnFailed { .. }
            | Event::TurnInterrupted { .. }
            | Event::TurnCancelled { .. }
    )
}

/// The next draft of `drafts`, or never without a draft channel, so a `select!`
/// branch can be switched off without a guard.
async fn next_draft(
    drafts: Option<&mut broadcast::Receiver<ConversationDraft>>,
) -> Result<ConversationDraft, RecvError> {
    match drafts {
        Some(drafts) => drafts.recv().await,
        None => std::future::pending().await,
    }
}

/// True when `events` may be sent as they are: at most [`MAX_EVENTS`] and
/// [`MAX_BYTES`].
pub(crate) fn fits(events: &[EventEnvelope]) -> bool {
    events.len() <= MAX_EVENTS && bytes(events) <= MAX_BYTES
}

fn bytes(events: &[EventEnvelope]) -> usize {
    events
        .iter()
        .map(|envelope| serde_json::to_vec(envelope).map_or(usize::MAX, |json| json.len()))
        .fold(0, usize::saturating_add)
}

/// The newest of `events` (oldest first) that fit the bounds, and whether any older
/// one was left out.
pub(crate) fn newest_that_fit(mut events: Vec<EventEnvelope>) -> (Vec<EventEnvelope>, bool) {
    let mut cut = false;
    while !fits(&events) {
        events.remove(0);
        cut = true;
    }
    (events, cut)
}

/// A snapshot of `conversation` with its newest `events` (oldest first); `older` says
/// whether events before them exist.
pub(crate) fn snapshot(
    conversation: efr_protocol::ConversationSummary,
    events: Vec<EventEnvelope>,
    older: bool,
    hwm: Seq,
) -> ConversationSnapshot {
    let history_cursor = if older { events.first().map(|first| cursor(first.seq)) } else { None };
    ConversationSnapshot { conversation, events, history_cursor, hwm }
}
