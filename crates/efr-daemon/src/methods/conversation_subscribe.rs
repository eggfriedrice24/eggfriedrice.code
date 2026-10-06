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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use efr_protocol::{
    ConversationId, ConversationSnapshot, ConversationSubscribe, ConversationSubscribeItem,
    EventEnvelope, ScopeName, Seq,
};
use efr_store::Committed;
use efr_transport::{ConnectionContext, Offer, Responder, SubscriptionSender, TransportError};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::methods::{cursor, granted};
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
    let answering =
        params.answers_input && granted(context.surface()).contains(&ScopeName::Terminal);
    let attached = state.connections.subscribe(context.conn_id(), conversation_id, answering);
    // Subscribing before the read means no commit falls between the two.
    let committed = state.writer.subscribe();
    let (sender, mut receiver) = efr_transport::subscription();
    let progress = Arc::new(Progress { last: attached.reached(), lagged: AtomicBool::new(false) });
    let _producer = AbortOnDrop(tokio::spawn(produce(
        committed,
        sender,
        conversation_id,
        Arc::clone(&progress),
        cancelled,
    )));
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

/// Offers the conversation's live events to the subscriber until it closes or the
/// request is cancelled.
async fn produce(
    mut committed: broadcast::Receiver<Committed>,
    mut sender: SubscriptionSender<ConversationSubscribeItem>,
    conversation_id: ConversationId,
    progress: Arc<Progress>,
    cancelled: CancellationToken,
) {
    loop {
        let batch = tokio::select! {
            () = cancelled.cancelled() => return,
            batch = committed.recv() => batch,
        };
        match batch {
            Ok(batch) => {
                for envelope in batch.events() {
                    if envelope.conversation_id != Some(conversation_id) {
                        continue;
                    }
                    let item = ConversationSubscribeItem::Event(envelope.clone());
                    match sender.offer(envelope.seq, item) {
                        Offer::Queued => progress.raise(envelope.seq),
                        Offer::Overflowed | Offer::Closed => return,
                    }
                }
            }
            Err(RecvError::Lagged(_)) => {
                // NOTE: stored before the sender drops, so the handler finds it when the
                // stream ends.
                progress.lagged.store(true, Ordering::Release);
                return;
            }
            Err(RecvError::Closed) => return,
        }
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
