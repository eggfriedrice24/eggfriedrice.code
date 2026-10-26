//! `conversations.watch`: the rows of the recent conversations of the machine, then a
//! row each time a commit changes what a list shows of a conversation.
//!
//! The order is the one of `conversation.subscribe`: subscribe to the store's commits,
//! read the rows and the high-water mark in one read transaction, send the snapshot,
//! then look at the commits above the mark. The live side takes every commit that
//! already waits with the one that came, reads the rows of the conversations that they
//! touched in one read, and sends a row only when it differs from the row that the
//! stream sent last for that conversation, without the time and the sequence number of
//! the last activity. So the events of a running turn send nothing, and its start, an
//! approval request, its answer and its end each send one row. A conversation whose row
//! is gone was deleted: the stream sends `removed`.
//!
//! The rows are the rows of `conversations.recent` without a `tty` and a `pwd`
//! (`recent_rows.rs`), with the title that a search row shows (`search.rs`), so a list
//! that the watch feeds shows what a list that asks shows.
//!
//! Rows go through the transport's bounded subscriber queue (64 items). A subscriber
//! that falls behind, at the queue or at the store's broadcast, is closed with
//! `overflow`; it watches again and gets a new snapshot. A read that fails ends the
//! stream with the error of the store.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use efr_protocol::{
    ConversationId, ConversationsWatch, ConversationsWatchItem, RecentConversation, RouteScope, Seq,
};
use efr_scope::Registry;
use efr_store::conversations::RecentRow;
use efr_store::{Committed, StoreError};
use efr_transport::{Offer, Responder, SubscriptionSender, TransportError};
use futures::FutureExt as _;
use futures::future::BoxFuture;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::engine::load_registry;
use crate::methods::conversation_subscribe::AbortOnDrop;
use crate::methods::page_size;
use crate::methods::recent_rows::recent_rows;
use crate::methods::search::titled;
use crate::state::State;

/// Rows of the snapshot when the request names no limit.
const DEFAULT_ROWS: u32 = 20;
/// The most rows of a snapshot.
const MAX_ROWS: u32 = 50;

/// Reads the wire rows of the conversations of `ids` now, in their order. An id with
/// no conversation has no row.
pub(crate) type ReadRows = Arc<
    dyn Fn(Vec<ConversationId>) -> BoxFuture<'static, Result<Vec<RecentConversation>, StoreError>>
        + Send
        + Sync,
>;

pub(crate) async fn handle(
    state: &Arc<State>,
    params: ConversationsWatch,
    responder: &Responder,
    cancelled: CancellationToken,
) -> Result<(), DaemonError> {
    let limit = page_size(params.limit, DEFAULT_ROWS, MAX_ROWS);
    // Subscribing before the read means no commit falls between the two.
    let committed = state.writer.subscribe();
    let (stored, titles, hwm) = state
        .readers
        .with(move |conn| {
            let rows = efr_store::conversations::recent(conn, None, limit)?;
            let ids: Vec<ConversationId> = rows.iter().map(|row| row.summary.id).collect();
            let titles = efr_store::search::titles(conn, &ids)?;
            let hwm = efr_store::events::last_seq(conn)?;
            Ok((rows, titles, hwm))
        })
        .await?;
    let conversations = wire_rows(state, stored, titles).await;
    let (sender, receiver) = efr_transport::subscription();
    let end = Arc::new(End::default());
    let live = Live::new(committed, sender, store_rows(state), &conversations, hwm, &end);
    let _producer = AbortOnDrop(tokio::spawn(live.produce(cancelled)));
    responder.item(&ConversationsWatchItem::Snapshot { conversations }).await?;
    match responder.forward(receiver).await {
        Ok(()) => end.result(),
        // The transport ends an overflowed subscription with `overflow` itself.
        Err(TransportError::Overflow { .. }) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// The wire rows of `stored`, with the search title of each from `titles`.
async fn wire_rows(
    state: &State,
    stored: Vec<RecentRow>,
    titles: Vec<Option<String>>,
) -> Vec<RecentConversation> {
    // NOTE: only a project's name needs the registry, so a list without projects
    // never reads the file.
    let registry = if stored.iter().any(|row| matches!(row.route, RouteScope::Project(_))) {
        load_registry(&state.engine_parts.registry).await
    } else {
        Registry::empty()
    };
    recent_rows(state, stored, &registry, None)
        .into_iter()
        .zip(titles)
        .map(|(conversation, title)| titled(conversation, title))
        .collect()
}

/// The reader of the live side: the rows of the store now. It holds the state weakly,
/// so a watch never keeps the daemon's state alive; without the state it reads no row.
fn store_rows(state: &Arc<State>) -> ReadRows {
    let state = Arc::downgrade(state);
    Arc::new(move |ids| {
        let state = Weak::clone(&state);
        async move {
            let Some(state) = state.upgrade() else {
                return Err(StoreError::WriterStopped);
            };
            let (stored, titles) = state
                .readers
                .with(move |conn| {
                    let rows = efr_store::conversations::rows(conn, &ids)?;
                    let found: Vec<ConversationId> =
                        rows.iter().map(|row| row.summary.id).collect();
                    let titles = efr_store::search::titles(conn, &found)?;
                    Ok((rows, titles))
                })
                .await?;
            Ok(wire_rows(&state, stored, titles).await)
        }
        .boxed()
    })
}

/// How the live side ended, for the handler once the queue is empty.
#[derive(Debug, Default)]
struct End {
    /// The store's broadcast left the live side behind.
    lagged: AtomicBool,
    /// The sequence number of the last commit that the live side looked at.
    last: AtomicU64,
    /// A read of the rows failed.
    failed: Mutex<Option<StoreError>>,
}

impl End {
    /// What the request ends with after the live side ended and the queue went out.
    fn result(&self) -> Result<(), DaemonError> {
        if let Some(source) = self.failed.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Err(source.into());
        }
        if self.lagged.load(Ordering::Acquire) {
            let last_seq = Seq::new(self.last.load(Ordering::Acquire));
            return Err(DaemonError::Respond { source: TransportError::Overflow { last_seq } });
        }
        Ok(())
    }
}

/// The conversations that some commits touched, each with the sequence number of its
/// last event in them.
#[derive(Debug, Default)]
struct Touched {
    last: HashMap<ConversationId, Seq>,
}

impl Touched {
    fn add(&mut self, conversation_id: ConversationId, seq: Seq) {
        let last = self.last.entry(conversation_id).or_insert(seq);
        *last = (*last).max(seq);
    }

    fn ids(&self) -> Vec<ConversationId> {
        self.last.keys().copied().collect()
    }

    /// The conversations in the order of their last events, so the queue gets rising
    /// sequence numbers: it drops an item whose number is not above the one before.
    fn in_order(self) -> Vec<(ConversationId, Seq)> {
        let mut touched: Vec<(ConversationId, Seq)> = self.last.into_iter().collect();
        touched.sort_by_key(|(_, seq)| *seq);
        touched
    }
}

/// The live side of one watch.
struct Live {
    committed: broadcast::Receiver<Committed>,
    sender: SubscriptionSender<ConversationsWatchItem>,
    rows: ReadRows,
    /// The row that the stream sent last for each conversation, as [`compared`] gives
    /// it.
    shown: HashMap<ConversationId, RecentConversation>,
    /// The high-water mark of the snapshot: the commits at or below it are in it.
    hwm: Seq,
    end: Arc<End>,
}

impl Live {
    fn new(
        committed: broadcast::Receiver<Committed>,
        sender: SubscriptionSender<ConversationsWatchItem>,
        rows: ReadRows,
        snapshot: &[RecentConversation],
        hwm: Seq,
        end: &Arc<End>,
    ) -> Self {
        let shown = snapshot.iter().map(|row| (row.summary.id, compared(row))).collect();
        Live { committed, sender, rows, shown, hwm, end: Arc::clone(end) }
    }

    /// Sends the rows that the commits change until the subscriber closes, the request
    /// is cancelled or the live side falls behind.
    async fn produce(mut self, cancelled: CancellationToken) {
        loop {
            let batch = tokio::select! {
                biased;
                () = cancelled.cancelled() => return,
                batch = self.committed.recv() => batch,
            };
            let mut touched = Touched::default();
            if !self.take(batch, &mut touched) {
                return;
            }
            // NOTE: the commits that already wait join this one, so a burst of commits
            // costs one read.
            loop {
                let batch = match self.committed.try_recv() {
                    Ok(batch) => Ok(batch),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Lagged(missed)) => Err(RecvError::Lagged(missed)),
                    Err(TryRecvError::Closed) => Err(RecvError::Closed),
                };
                if !self.take(batch, &mut touched) {
                    return;
                }
            }
            if touched.last.is_empty() {
                continue;
            }
            let read = (self.rows)(touched.ids());
            let rows = tokio::select! {
                biased;
                () = cancelled.cancelled() => return,
                rows = read => rows,
            };
            match rows {
                Ok(rows) => {
                    if !self.send(touched, rows) {
                        return;
                    }
                }
                Err(source) => {
                    tracing::warn!(error = %source, "a watch of the conversations could not read their rows");
                    *self.end.failed.lock().unwrap_or_else(PoisonError::into_inner) = Some(source);
                    return;
                }
            }
        }
    }

    /// Adds the conversations of `batch` above the snapshot to `touched`; false when
    /// the live side must end.
    fn take(&mut self, batch: Result<Committed, RecvError>, touched: &mut Touched) -> bool {
        let batch = match batch {
            Ok(batch) => batch,
            Err(RecvError::Lagged(_)) => {
                // NOTE: stored before the sender drops, so the handler finds it when the
                // stream ends.
                self.end.lagged.store(true, Ordering::Release);
                return false;
            }
            Err(RecvError::Closed) => return false,
        };
        for envelope in batch.events() {
            if envelope.seq <= self.hwm {
                continue;
            }
            if let Some(conversation_id) = envelope.conversation_id {
                touched.add(conversation_id, envelope.seq);
            }
        }
        self.end.last.fetch_max(batch.last_seq().get(), Ordering::AcqRel);
        true
    }

    /// Offers a row for each conversation of `touched` whose row differs from the one
    /// sent last, and `removed` for each one without a row; false when the subscription
    /// ended.
    fn send(&mut self, touched: Touched, rows: Vec<RecentConversation>) -> bool {
        let mut rows: HashMap<ConversationId, RecentConversation> =
            rows.into_iter().map(|row| (row.summary.id, row)).collect();
        for (conversation_id, seq) in touched.in_order() {
            let item = match rows.remove(&conversation_id) {
                Some(row) => {
                    let key = compared(&row);
                    if self.shown.get(&conversation_id) == Some(&key) {
                        continue;
                    }
                    self.shown.insert(conversation_id, key);
                    ConversationsWatchItem::Changed { conversation: Box::new(row) }
                }
                None => {
                    self.shown.remove(&conversation_id);
                    ConversationsWatchItem::Removed { conversation_id }
                }
            };
            match self.sender.offer(seq, item) {
                Offer::Queued => {}
                Offer::Overflowed | Offer::Closed => return false,
            }
        }
        true
    }
}

/// `row` without the time and the sequence number of its last activity, which every
/// event moves: what decides whether a list shows something new.
fn compared(row: &RecentConversation) -> RecentConversation {
    let mut row = row.clone();
    row.summary.updated_at = jiff::Timestamp::UNIX_EPOCH;
    row.summary.last_seq = Seq::ZERO;
    row
}

#[cfg(test)]
mod tests;
