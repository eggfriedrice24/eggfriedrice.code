//! `conversation.subscribe`: a conversation's events, replayed or summarised, then live.
//!
//! The order is the one that loses nothing: subscribe to the store's commits, read the
//! high-water mark and the start in one read transaction, send the start, then forward
//! live events above the mark. The start is a replay of the events after `after_seq`
//! when that gap is at most 128 events and 1 MiB, and otherwise (or without
//! `after_seq`) a bounded snapshot: the summary, the newest events up to the same
//! bounds, and a history cursor for the older ones. An event that is over the bound by
//! itself is never in a snapshot: the cursor reaches it. When it is the newest event,
//! the snapshot has no events and a cursor before the number after the high-water mark.
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
//! `tool_call_started`, the end of a turn). The drafts start with the status of the
//! running turn (`ConversationHandle::live_drafts`), so a client that attaches during
//! a long model call or a compaction shows it at once.
//!
//! A draft goes through the queue's lossy room, and it never closes the subscription.
//! A draft that finds the room full waits in the live side, merged with the later
//! drafts of its part ([`pending`]), and goes at the next free place in the room. A
//! draft channel that left this subscriber behind lost drafts that never reached the
//! live side; it then builds them again from the turn's live parts
//! (`ConversationHandle::live_turn`). So a slow subscriber gets fewer, larger drafts,
//! and every byte of the text and the reasoning of the running message.
//!
//! A snapshot carries the live parts of the running turn (`live`): the text of the
//! message that streams and the reasoning of the turn so far, which the log does not
//! have yet. The handler reads them after the start, and after the draft channel when
//! the subscription has drafts, so a client that joins in the middle of a message
//! shows them at once and every later draft goes on past them: a draft that repeats
//! their bytes is cut to the new ones. A replay has no place for them, so a
//! subscription with drafts gets them as its first drafts, after the replay.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use efr_conversation::{ConversationDraft, LiveTurn};
use efr_protocol::{
    ConversationId, ConversationSnapshot, ConversationSubscribe, ConversationSubscribeItem, Draft,
    Event, EventEnvelope, ScopeName, Seq,
};
use efr_store::Committed;
use efr_transport::{
    ConnectionContext, LossyOffer, Offer, Responder, SubscriptionSender, TransportError,
};
use futures::FutureExt as _;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use self::pending::{Boundary, PendingDrafts};
use crate::DaemonError;
use crate::methods::{cursor, granted, peer_side};
use crate::state::State;

mod pending;

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
pub(crate) struct AbortOnDrop(pub(crate) JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) async fn handle(
    state: &Arc<State>,
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
        boundary: Boundary::default(),
        pending: PendingDrafts::default(),
        rebuild: None,
    };
    let _producer = AbortOnDrop(tokio::spawn(live.produce(drafts, cancelled)));
    let after = params.after_seq;
    let mut start = state
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
        let receiver = state.drafts.subscribe();
        // NOTE: read after the subscription, so no status draft falls between the two;
        // one that comes twice shows the same status again.
        let held = state
            .conversations
            .live(conversation_id)
            .map(|handle| handle.live_drafts())
            .unwrap_or_default();
        // NOTE: read after the subscription too, so every draft that comes later
        // continues what the start shows.
        let live = show_live(&mut start, running(state, conversation_id));
        let rebuild = live_parts(state, conversation_id);
        let _ = feed.send(DraftFeed { receiver, held, rebuild, live });
    } else {
        show_live(&mut start, running(state, conversation_id));
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

/// What the current model call of `conversation_id`'s running turn streamed so far;
/// `None` when no turn runs or its first model call has not started.
fn running(state: &State, conversation_id: ConversationId) -> Option<LiveTurn> {
    state.conversations.live(conversation_id).and_then(|handle| handle.live_turn())
}

/// Puts `live`, the live parts of the running turn, in the snapshot of `start`, so a
/// client that joins in the middle of a message shows its text and reasoning so far.
/// A replay has no place for them: they go as the first drafts. Returns what the live
/// side does with them.
fn show_live(start: &mut Start, live: Option<LiveTurn>) -> LiveStart {
    let Some(live) = live else {
        return LiveStart::Nothing;
    };
    let snapshot = start.items.iter_mut().find_map(|item| match item {
        ConversationSubscribeItem::Snapshot(snapshot) => Some(snapshot),
        _ => None,
    });
    match snapshot {
        Some(snapshot) => {
            snapshot.live = Some(wire_live(&live));
            LiveStart::Shown(live)
        }
        None => LiveStart::Draft(live),
    }
}

/// The wire form of `live`: the text of its current message and the reasoning of its
/// turn, each whole. The input of its tool calls goes as drafts.
fn wire_live(live: &LiveTurn) -> efr_protocol::LiveTurn {
    efr_protocol::LiveTurn {
        turn_id: live.turn_id,
        after_seq: live.after_seq,
        index: live.index,
        text: live.text.clone(),
        text_bytes: live.text_bytes(),
        reasoning: (!live.reasoning.is_empty()).then(|| live.reasoning.clone()),
        reasoning_title: live.title.clone(),
    }
}

/// What the start of a subscription did with the live parts of the running turn,
/// read after its draft channel.
#[derive(Debug)]
enum LiveStart {
    /// No model call of a turn runs.
    Nothing,
    /// The snapshot carries them: the drafts go on past them.
    Shown(LiveTurn),
    /// A replay has no place for them: they go first, as drafts, so a client that
    /// resumes in the middle of a message gets the reasoning and the text that the log
    /// does not have yet.
    Draft(LiveTurn),
}

/// The draft channel of a subscription that asked for drafts, subscribed after the
/// read of its start.
#[derive(Debug)]
struct DraftFeed {
    receiver: broadcast::Receiver<ConversationDraft>,
    /// The status of the running turn (its newest `context` draft, and `compacting`
    /// while it compacts), so a subscriber that attaches during a long model call or a
    /// compaction shows it at once.
    held: Vec<ConversationDraft>,
    /// Reads the live parts of the running turn, for the drafts that the channel lost.
    rebuild: Rebuild,
    /// What the start did with the live parts of the running turn.
    live: LiveStart,
}

/// Reads what the running turn of a conversation streamed so far and its status
/// drafts: what a subscriber needs to build the drafts it lost again.
struct Rebuild(Box<dyn Fn() -> (Option<LiveTurn>, Vec<ConversationDraft>) + Send>);

impl fmt::Debug for Rebuild {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Rebuild")
    }
}

/// The live parts and the status drafts of `conversation_id`'s running turn, read when
/// a subscriber lost drafts. It holds the state weakly, so a subscription never keeps
/// the daemon's state alive.
fn live_parts(state: &Arc<State>, conversation_id: ConversationId) -> Rebuild {
    let state = Arc::downgrade(state);
    Rebuild(Box::new(move || {
        Weak::upgrade(&state)
            .and_then(|state| state.conversations.live(conversation_id))
            .map(|handle| (handle.live_turn(), handle.live_drafts()))
            .unwrap_or_default()
    }))
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
    /// How far the events offered to the subscriber ended what drafts show. A draft
    /// made before them is old.
    boundary: Boundary,
    /// The drafts that wait for a free place in the lossy room, merged.
    pending: PendingDrafts,
    /// Reads the live parts of the running turn, once the draft channel came.
    rebuild: Option<Rebuild>,
}

/// What the live side does next.
enum Step {
    Batch(Result<Committed, RecvError>),
    Draft(Result<ConversationDraft, RecvError>),
    /// The lossy room has a free place for a held draft.
    Room,
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
                    let Ok(DraftFeed { receiver, held, rebuild, live }) = fed else {
                        continue;
                    };
                    self.drafts = Some(receiver);
                    self.rebuild = Some(rebuild);
                    for draft in held {
                        if !self.forward_draft(draft) {
                            return;
                        }
                    }
                    if !self.start_live(live) {
                        return;
                    }
                    continue;
                }
                draft = next_draft(self.drafts.as_mut()) => Step::Draft(draft),
                () = self.sender.lossy_room(), if !self.pending.is_empty() => Step::Room,
            };
            let open = match step {
                Step::Batch(batch) => self.forward(batch),
                Step::Draft(Ok(draft)) => self.forward_draft(draft),
                Step::Draft(Err(RecvError::Lagged(missed))) => {
                    tracing::debug!(missed, "a subscriber missed drafts and builds them again");
                    self.rebuild()
                }
                Step::Draft(Err(RecvError::Closed)) => {
                    self.drafts = None;
                    true
                }
                Step::Room => self.catch_up() && self.flush(),
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
                    if self.boundary.pass(&envelope.event, envelope.seq) {
                        self.pending.drop_ended(self.boundary);
                    }
                }
                Offer::Overflowed | Offer::Closed => return false,
            }
            // NOTE: nothing of a deleted conversation follows its tombstone, so the
            // subscription ends after it.
            if matches!(envelope.event, Event::ConversationDeleted { .. }) {
                return false;
            }
        }
        true
    }

    /// Offers `draft` when it belongs to this conversation, after every commit that
    /// already waits, unless an event that ends what it shows went first; false when
    /// the subscription ended. A draft that finds the lossy room full waits, merged.
    fn forward_draft(&mut self, draft: ConversationDraft) -> bool {
        if draft.conversation_id != self.conversation_id {
            return true;
        }
        // NOTE: the turn sent the draft after the commits it made before it, so those
        // commits wait here already; they go first.
        if !self.catch_up() {
            return false;
        }
        if self.boundary.ended(&draft) {
            // NOTE: the turn made the draft before an event that the subscriber already
            // has ended its part: an old draft would only show the past again.
            return true;
        }
        self.pending.push(draft);
        self.flush()
    }

    /// Takes what the start did with the live parts of the running turn, after the
    /// commits that wait: it holds them as shown, or offers them as drafts; false when
    /// the subscription ended.
    fn start_live(&mut self, live: LiveStart) -> bool {
        let conversation_id = self.conversation_id;
        match live {
            LiveStart::Nothing => return true,
            LiveStart::Shown(live) => {
                if !self.catch_up() {
                    return false;
                }
                self.pending.shown(conversation_id, live);
            }
            LiveStart::Draft(live) => {
                if !self.catch_up() {
                    return false;
                }
                self.pending.rebuild(conversation_id, Some(live), Vec::new());
            }
        }
        self.pending.drop_ended(self.boundary);
        self.flush()
    }

    /// The draft channel left this subscriber behind: holds again what the running
    /// turn streamed past what went here, after the commits that wait, and offers it.
    fn rebuild(&mut self) -> bool {
        if !self.catch_up() {
            return false;
        }
        let Some(rebuild) = &self.rebuild else {
            return true;
        };
        let (live, status) = (rebuild.0)();
        self.pending.rebuild(self.conversation_id, live, status);
        self.pending.drop_ended(self.boundary);
        self.flush()
    }

    /// Forwards every commit that already waits; false when the subscription ended.
    fn catch_up(&mut self) -> bool {
        loop {
            let batch = match self.committed.try_recv() {
                Ok(batch) => Ok(batch),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Lagged(missed)) => Err(RecvError::Lagged(missed)),
                Err(TryRecvError::Closed) => Err(RecvError::Closed),
            };
            if !self.forward(batch) {
                return false;
            }
        }
    }

    /// Offers the held drafts, oldest first, while the lossy room has a place; false
    /// when the subscription ended.
    fn flush(&mut self) -> bool {
        // NOTE: only this side offers lossy items, so a place that the check finds
        // stays free for the offer.
        while self.sender.lossy_room().now_or_never().is_some() {
            let Some(draft) = self.pending.pop() else {
                return true;
            };
            if self.boundary.ended(&draft) {
                continue;
            }
            let item = ConversationSubscribeItem::Draft(Draft {
                turn_id: draft.turn_id,
                after_seq: draft.after_seq,
                draft: draft.part,
            });
            match self.sender.offer_lossy(item) {
                LossyOffer::Queued | LossyOffer::Dropped => {}
                LossyOffer::Closed => return false,
            }
        }
        true
    }
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
/// [`MAX_BYTES`]. It writes each event as JSON at most once and stops at the bound.
pub(crate) fn fits(events: &[EventEnvelope]) -> bool {
    events.len() <= MAX_EVENTS
        && events
            .iter()
            .try_fold(0_usize, |total, envelope| {
                let total = total.saturating_add(json_len(envelope));
                (total <= MAX_BYTES).then_some(total)
            })
            .is_some()
}

/// The length of `envelope` as JSON; `usize::MAX`, which never fits, when it cannot be
/// written.
fn json_len(envelope: &EventEnvelope) -> usize {
    serde_json::to_vec(envelope).map_or(usize::MAX, |json| json.len())
}

/// The newest of `events` (oldest first) that fit the bounds, and whether any older
/// one was left out. When the newest event alone is over [`MAX_BYTES`], none fits.
pub(crate) fn newest_that_fit(events: Vec<EventEnvelope>) -> (Vec<EventEnvelope>, bool) {
    newest_within(events, json_len)
}

/// [`newest_that_fit`] with the length of each event from `len`. It walks from the
/// newest event back and stops at the first one that does not fit, so it measures
/// each event at most once.
pub(crate) fn newest_within(
    mut events: Vec<EventEnvelope>,
    mut len: impl FnMut(&EventEnvelope) -> usize,
) -> (Vec<EventEnvelope>, bool) {
    let mut start = events.len();
    let mut total = 0_usize;
    for envelope in events.iter().rev().take(MAX_EVENTS) {
        total = total.saturating_add(len(envelope));
        if total > MAX_BYTES {
            break;
        }
        start -= 1;
    }
    events.drain(..start);
    (events, start > 0)
}

/// A snapshot of `conversation` with its newest `events` (oldest first); `older` says
/// whether events before them exist. Without `events`, the cursor continues before
/// the number after `hwm`, so its first page holds the newest event.
pub(crate) fn snapshot(
    conversation: efr_protocol::ConversationSummary,
    events: Vec<EventEnvelope>,
    older: bool,
    hwm: Seq,
) -> ConversationSnapshot {
    // NOTE: a newest event over the bound leaves no event. A cursor that is absent
    // would say that the conversation has no older events, so it must be there.
    let before =
        events.first().map_or_else(|| Seq::new(hwm.get().saturating_add(1)), |first| first.seq);
    let history_cursor = older.then(|| cursor(before));
    ConversationSnapshot { conversation, events, history_cursor, hwm, live: None }
}

#[cfg(test)]
mod tests;
