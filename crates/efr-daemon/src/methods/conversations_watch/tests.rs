//! The live side of a watch, without a daemon: the rows that a turn sends, a deleted
//! conversation, a broadcast that left it behind, a full queue and a read that fails.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use efr_protocol::{
    CommandId, ConversationId, ConversationStatus, ConversationsWatchItem, ErrorCode, Event,
    Origin, RecentConversation, Scope, Seq, ShellContext, TurnId, TurnSettings,
};
use efr_store::{Batch, Committed, Readers, Store, StoreError};
use efr_test_support::TestClock;
use efr_transport::{Delivery, SUBSCRIBER_QUEUE_FRAMES, SubscriptionReceiver};
use futures::FutureExt as _;
use pretty_assertions::assert_eq;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::{End, Live, ReadRows};
use crate::DaemonError;

/// An id of type `T` from a number, in the form of the fixtures.
fn id<T: std::str::FromStr>(n: u64) -> T
where
    T::Err: std::fmt::Debug,
{
    format!("0192f0c1-7a00-7000-8000-{n:012x}").parse().unwrap()
}

fn batch(conversation: ConversationId, events: Vec<Event>) -> Batch {
    events.into_iter().fold(Batch::new(), |batch, event| batch.event(conversation, event))
}

fn created() -> Event {
    Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None }
}

fn queued(turn_id: TurnId) -> Event {
    let command_id: CommandId = id(90);
    Event::PromptQueued {
        turn_id,
        command_id,
        text: "why is the disk full".to_owned(),
        origin: Origin::Shell,
        context: Some(ShellContext::new("/srv/p")),
        settings: TurnSettings::default(),
        steers: Vec::new(),
    }
}

fn started(turn_id: TurnId) -> Event {
    Event::TurnStarted {
        turn_id,
        cwd: "/srv/p".into(),
        scope: Scope::Path("/srv/p".into()),
        settings: None,
    }
}

fn answered(turn_id: TurnId) -> Event {
    Event::AssistantMessageCompleted { turn_id, index: 0, text: "journal files".to_owned() }
}

fn completed(turn_id: TurnId) -> Event {
    Event::TurnCompleted {
        turn_id,
        usage: None,
        context: None,
        changes: None,
        stop: None,
        undo: None,
    }
}

/// A reader of the rows of the store, as a list shows them without the registry and
/// the terminals, which count how often it reads.
fn store_reader(readers: &Readers, reads: &Arc<AtomicUsize>) -> ReadRows {
    let readers = readers.clone();
    let reads = Arc::clone(reads);
    Arc::new(move |ids| {
        let readers = readers.clone();
        reads.fetch_add(1, Ordering::AcqRel);
        async move {
            let rows = readers.with(move |conn| efr_store::conversations::rows(conn, &ids)).await?;
            Ok(rows
                .into_iter()
                .map(|row| RecentConversation {
                    summary: row.summary,
                    route: row.route,
                    project: None,
                    turns: row.turns,
                    open_in: None,
                    here: false,
                })
                .collect())
        }
        .boxed()
    })
}

/// A reader that answers every read with `answer`.
fn fixed_reader(
    answer: impl Fn(Vec<ConversationId>) -> Result<Vec<RecentConversation>, StoreError>
    + Send
    + Sync
    + 'static,
) -> ReadRows {
    let answer = Arc::new(answer);
    Arc::new(move |ids| {
        let answer = Arc::clone(&answer);
        async move { answer(ids) }.boxed()
    })
}

/// Starts a live side on `committed` with `rows`, whose snapshot is `snapshot` at `hwm`.
fn start(
    committed: broadcast::Receiver<Committed>,
    rows: ReadRows,
    snapshot: &[RecentConversation],
    hwm: Seq,
) -> (SubscriptionReceiver<ConversationsWatchItem>, Arc<End>, CancellationToken) {
    let (sender, receiver) = efr_transport::subscription();
    let end = Arc::new(End::default());
    let live = Live::new(committed, sender, rows, snapshot, hwm, &end);
    let cancelled = CancellationToken::new();
    tokio::spawn(live.produce(cancelled.clone()));
    (receiver, end, cancelled)
}

/// The next row of `receiver`.
async fn next_row(
    receiver: &mut SubscriptionReceiver<ConversationsWatchItem>,
) -> RecentConversation {
    match receiver.recv().await {
        Some(Delivery::Item { item: ConversationsWatchItem::Changed { conversation }, .. }) => {
            *conversation
        }
        other => panic!("expected a changed row, got {other:?}"),
    }
}

/// A row of `conversation` titled `title`, for the readers that make their rows.
fn row(conversation: ConversationId, title: &str) -> RecentConversation {
    RecentConversation {
        summary: efr_protocol::ConversationSummary {
            id: conversation,
            title: Some(title.to_owned()),
            status: ConversationStatus::Idle,
            created_at: jiff::Timestamp::UNIX_EPOCH,
            updated_at: jiff::Timestamp::UNIX_EPOCH,
            last_seq: Seq::new(1),
            cwd: None,
            scope: None,
            tty: None,
        },
        route: efr_protocol::RouteScope::Outside,
        project: None,
        turns: 0,
        open_in: None,
        here: false,
    }
}

#[tokio::test]
async fn a_turn_sends_its_queue_its_start_and_its_end_and_nothing_between() {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.shared()).await.unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let (mut receiver, _end, cancelled) =
        start(store.writer().subscribe(), store_reader(store.readers(), &reads), &[], Seq::ZERO);
    let conversation: ConversationId = id(1);
    let turn: TurnId = id(2);
    let writer = store.writer();

    writer.append(batch(conversation, vec![created(), queued(turn)])).await.unwrap();
    let first = next_row(&mut receiver).await;
    assert_eq!(first.summary.id, conversation, "a conversation that the stream did not show");
    assert_eq!((first.summary.status, first.turns), (ConversationStatus::Idle, 1));

    writer.append(batch(conversation, vec![started(turn)])).await.unwrap();
    assert_eq!(next_row(&mut receiver).await.summary.status, ConversationStatus::Running);

    // NOTE: the answer moves only the last activity, so the next row is the end.
    writer.append(batch(conversation, vec![answered(turn)])).await.unwrap();
    writer.append(batch(conversation, vec![completed(turn)])).await.unwrap();
    let end = next_row(&mut receiver).await;
    assert_eq!(end.summary.status, ConversationStatus::Idle);
    assert!(end.summary.last_seq.get() >= 5, "the row of the end is read after the end");
    cancelled.cancel();
}

#[tokio::test]
async fn the_commits_of_the_snapshot_are_not_read_again() {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.shared()).await.unwrap();
    let conversation: ConversationId = id(1);
    let turn: TurnId = id(2);
    let committed = store.writer().subscribe();
    let first = store.writer().append(batch(conversation, vec![created(), queued(turn)])).await;
    let hwm = first.unwrap().last_seq();
    let reads = Arc::new(AtomicUsize::new(0));
    let reader = store_reader(store.readers(), &reads);
    let snapshot = (reader)(vec![conversation]).await.unwrap();
    reads.store(0, Ordering::Release);
    let (mut receiver, _end, cancelled) = start(committed, reader, &snapshot, hwm);

    store.writer().append(batch(conversation, vec![started(turn)])).await.unwrap();
    assert_eq!(next_row(&mut receiver).await.summary.status, ConversationStatus::Running);
    assert_eq!(reads.load(Ordering::Acquire), 1, "the commit of the snapshot costs no read");
    cancelled.cancel();
}

#[tokio::test]
async fn a_conversation_without_a_row_is_removed() {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.shared()).await.unwrap();
    let conversation: ConversationId = id(1);
    let shown = [row(conversation, "old")];
    let (mut receiver, _end, cancelled) =
        start(store.writer().subscribe(), fixed_reader(|_| Ok(Vec::new())), &shown, Seq::ZERO);

    store.writer().append(batch(conversation, vec![created()])).await.unwrap();
    let removed = receiver.recv().await;
    assert_eq!(
        removed.map(|delivery| match delivery {
            Delivery::Item { item, .. } => item,
            other => panic!("{other:?}"),
        }),
        Some(ConversationsWatchItem::Removed { conversation_id: conversation })
    );
    cancelled.cancel();
}

#[tokio::test]
async fn a_broadcast_that_left_the_watch_behind_ends_it_with_overflow() {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.shared()).await.unwrap();
    let (commits, committed) = broadcast::channel(1);
    let mut last = Seq::ZERO;
    for n in 1..=3 {
        let conversation: ConversationId = id(n);
        let batch = store.writer().append(batch(conversation, vec![created()])).await.unwrap();
        last = batch.last_seq();
        commits.send(batch).unwrap();
    }
    let reader = fixed_reader(|ids| Ok(ids.into_iter().map(|id| row(id, "t")).collect()));
    let (mut receiver, end, _cancelled) = start(committed, reader, &[], Seq::ZERO);

    assert_eq!(receiver.recv().await, None, "the live side ends at once");
    let Err(DaemonError::Respond { source }) = end.result() else {
        panic!("a watch that fell behind ends with overflow");
    };
    assert!(
        matches!(source, efr_transport::TransportError::Overflow { last_seq } if last_seq <= last),
        "{source:?}"
    );
    assert_eq!(
        efr_protocol::ErrorFrame::from(DaemonError::Respond { source }).error.code,
        ErrorCode::Overflow
    );
}

#[tokio::test]
async fn a_full_queue_closes_the_watch_with_overflow() {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.shared()).await.unwrap();
    let reader = fixed_reader(|ids| Ok(ids.into_iter().map(|id| row(id, "t")).collect()));
    let (mut receiver, _end, _cancelled) =
        start(store.writer().subscribe(), reader, &[], Seq::ZERO);

    // NOTE: one commit that touches more conversations than the queue holds, which a
    // subscriber that does not read cannot take.
    let many = (1..=70_u64).fold(Batch::new(), |batch, n| batch.event(id(n), created()));
    store.writer().append(many).await.unwrap();
    for _ in 0..SUBSCRIBER_QUEUE_FRAMES {
        assert!(matches!(receiver.recv().await, Some(Delivery::Item { .. })));
    }
    assert!(matches!(receiver.recv().await, Some(Delivery::Overflowed { .. })));
    assert_eq!(receiver.recv().await, None);
}

#[tokio::test]
async fn a_read_that_fails_ends_the_watch_with_the_error_of_the_store() {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.shared()).await.unwrap();
    let reader = fixed_reader(|_| Err(StoreError::WriterStopped));
    let (mut receiver, end, _cancelled) = start(store.writer().subscribe(), reader, &[], Seq::ZERO);

    store.writer().append(batch(id(1), vec![created()])).await.unwrap();
    assert_eq!(receiver.recv().await, None);
    assert!(matches!(end.result(), Err(DaemonError::Store { .. })), "{:?}", end.result());
}
