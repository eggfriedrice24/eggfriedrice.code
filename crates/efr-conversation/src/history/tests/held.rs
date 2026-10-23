//! The turns that the actor's cache holds: a turn's start never copies or decodes their
//! saved messages, and the snapshot takes the cache's copy. The window, the request
//! that the model gets and the context breakdown stay byte for byte the same as when
//! every saved turn is decoded: with the whole cache, with a part of it, with a turn
//! that the cache holds and the store never saved, after an idle trim and after a
//! restart (an empty cache).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use efr_protocol::{Event, TurnId};
use efr_provider::{ContentBlock, Message, Request, Role};
use efr_stdx::rng::Rng as _;
use efr_store::conversations::TurnStatus;
use efr_test_support::{TestRng, TestStore};
use pretty_assertions::assert_eq;
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};

use super::covered::{LIMITS, random_conversation};
use super::{conversation, key, turn};
use crate::actor::IDLE_TRIM;
use crate::compaction::with_window;
use crate::context::breakdown;
use crate::history::{CachedTurn, HistoryLimits, Snapshot};
use crate::testing::{Harness, Recorder, Setup, request};

/// The caches that an actor can hold for the conversation of `full`, a read with an
/// empty cache: none (after a trim or a restart), every saved turn, a random part of
/// them, and every saved turn with a cached copy of each finished turn that saved
/// nothing (its save failed) and of a turn that the conversation does not have.
fn caches(full: &Snapshot, rng: &TestRng) -> Vec<HashMap<TurnId, Arc<CachedTurn>>> {
    let all: HashMap<TurnId, Arc<CachedTurn>> =
        full.saved.iter().map(|(id, turn)| (*id, Arc::clone(turn))).collect();
    let part = all
        .iter()
        .filter(|_| rng.next_u64().is_multiple_of(2))
        .map(|(id, turn)| (*id, Arc::clone(turn)))
        .collect();
    let mut more = all.clone();
    let unsaved = full.turns.iter().filter(|t| !full.saved.contains_key(&t.id)).map(|t| t.id);
    for (number, turn_id) in unsaved.chain([turn(77_777)]).enumerate() {
        let messages = vec![
            Message::user(format!("cached only {number}")),
            Message::assistant(format!("kept in memory {number}")),
        ];
        more.insert(turn_id, Arc::new(CachedTurn { key: key("replay", "m"), messages }));
    }
    vec![HashMap::new(), all, part, more]
}

/// The window of `store`'s conversation for each of [`LIMITS`], each cache of
/// [`caches`] and each model key, from a read that holds the cache and from a read
/// that decodes every saved turn: the reads, the windows, the requests as JSON and the
/// context breakdowns must be equal. Gives the count of held turns that the read took
/// from the cache.
async fn assert_same(
    store: &TestStore,
    current: Option<TurnId>,
    running: Option<TurnId>,
    rng: &TestRng,
) -> usize {
    let base = request(Vec::new());
    let mut taken = 0;
    for (max_events, max_bytes) in LIMITS {
        let limits = HistoryLimits::new(max_events, max_bytes);
        let none = HashMap::new();
        let full = Snapshot::read(store.readers(), conversation(), limits, &none).await.unwrap();
        for cache in caches(&full, rng) {
            let mut held =
                Snapshot::read(store.readers(), conversation(), limits, &cache).await.unwrap();
            let what = format!("{limits:?}, {} cached", cache.len());
            assert_eq!(held.turns, full.turns, "{what}");
            assert_eq!(held.page, full.page, "{what}");
            assert_eq!(held.older, full.older, "the same ranges are read, {what}");
            assert_eq!(held.compactions, full.compactions, "{what}");
            assert_eq!(held.user_events, full.user_events, "{what}");
            assert_eq!(held.summary, full.summary, "{what}");
            let mut saved: Vec<TurnId> = held.saved.keys().copied().collect();
            let mut expected: Vec<TurnId> = full.saved.keys().copied().collect();
            saved.sort_unstable();
            expected.sort_unstable();
            assert_eq!(saved, expected, "the same turns count as saved, {what}");
            for (turn_id, turn) in &held.saved {
                match cache.get(turn_id) {
                    Some(cached) => {
                        assert!(Arc::ptr_eq(turn, cached), "a held turn was decoded, {what}");
                        taken += 1;
                    }
                    None => assert_eq!(turn, &full.saved[turn_id], "{what}"),
                }
            }
            let mut kept = full.clone();
            for view in [&mut held, &mut kept] {
                for turn in &mut view.turns {
                    if Some(turn.id) == running && turn.status == TurnStatus::Running {
                        turn.status = TurnStatus::Completed;
                    }
                }
            }
            let numbers: HashMap<TurnId, u32> = full
                .turns
                .iter()
                .filter(|turn| turn.started_at.is_some())
                .zip(1_u32..)
                .map(|(turn, number)| (turn.id, number))
                .collect();
            for model in [key("replay", "m"), key("other", "m")] {
                let window = |snapshot: &Snapshot| {
                    snapshot.window(current, &cache, &model, limits, Some("fresh"))
                };
                let (new, old) = (window(&held), window(&kept));
                assert_eq!(new, old, "{what}, {model:?}");
                let json =
                    |window| serde_json::to_string(&with_window(&base, window)).expect("json");
                assert_eq!(json(&new), json(&old), "the request, {what}, {model:?}");
                assert_eq!(
                    breakdown::measure(&base, &new, &numbers),
                    breakdown::measure(&base, &old, &numbers),
                    "the breakdown, {what}, {model:?}"
                );
            }
        }
    }
    taken
}

/// Random conversations (the ones of `covered.rs`: random bodies and ends, saved
/// messages for some turns, steers, every kind of compaction), each checked as a turn
/// starts and as the context breakdown reads it: the cache changes nothing that the
/// model or the user sees.
#[tokio::test]
async fn the_cache_changes_no_window_of_random_conversations() {
    let rng = TestRng::new(5);
    let mut taken = 0;
    for seed in 0..150 {
        let (store, running) = random_conversation(seed).await;
        taken += assert_same(&store, running, None, &rng).await;
        taken += assert_same(&store, None, running, &rng).await;
    }
    assert!(taken > 1000, "the reads took {taken} turns from the cache");
}

#[test]
fn a_cached_turn_gives_back_the_room_that_its_strings_do_not_use() {
    let mut output = String::with_capacity(64 * 1024);
    output.push_str(&"x".repeat(32 * 1024));
    let result = ContentBlock::ToolResult { call_id: "call_1".to_owned(), output, is_error: false };
    let mut content = Vec::with_capacity(8);
    content.push(result);
    let messages = vec![Message::new(Role::User, content)];
    let mut cached = CachedTurn { key: key("replay", "m"), messages };
    let before = cached.clone();

    cached.shrink();

    assert_eq!(cached, before, "the content stays");
    let message = &cached.messages[0];
    assert_eq!(message.content.capacity(), 1);
    let ContentBlock::ToolResult { output, .. } = &message.content[0] else {
        panic!("{message:?}");
    };
    assert_eq!(output.capacity(), 32 * 1024);
}

/// What one snapshot read reported: the saved turns that it decoded, the saved turns
/// that the cache held, and the bytes of JSON that it decoded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Read {
    decoded: u64,
    held: u64,
    bytes: u64,
}

impl Visit for Read {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "decoded" => self.decoded = value,
            "held" => self.held = value,
            "bytes" => self.bytes = value,
            _ => {}
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

/// The reads of the snapshots on this thread, from the history's debug event, and the
/// trims of the actor.
#[derive(Clone, Default)]
struct Reads {
    reads: Arc<Mutex<Vec<Read>>>,
    trims: Arc<Mutex<usize>>,
}

impl<S: Subscriber> Layer<S> for Reads {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        let fields = metadata.fields();
        if metadata.target() == "efr_conversation::history" && fields.field("held").is_some() {
            let mut read = Read::default();
            event.record(&mut read);
            self.reads.lock().unwrap_or_else(PoisonError::into_inner).push(read);
        }
        if metadata.target() == "efr_conversation::actor" && fields.field("turns").is_some() {
            *self.trims.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        }
    }
}

impl Reads {
    /// A counter of the events of this thread while the guard lives. A tokio test runs
    /// the actor's task on its own thread.
    fn install() -> (Reads, DefaultGuard) {
        let reads = Reads::default();
        let guard =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(reads.clone()));
        (reads, guard)
    }

    /// The reads since the last call.
    fn take(&self) -> Vec<Read> {
        std::mem::take(&mut *self.reads.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn trims(&self) -> usize {
        *self.trims.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Sends `text`, waits for the end of its turn, which must complete, and gives the
/// read of its start.
async fn turn_read(h: &mut Harness, reads: &Reads, text: &str) -> Read {
    let sent = h.prompt(text).await;
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "{text}: {end:?}");
    let taken = reads.take();
    let [read] = taken.as_slice() else {
        panic!("one read at the start of {text}: {taken:?}");
    };
    *read
}

/// Waits until the actor has started its idle period `count` times in all.
async fn armed(h: &Harness, count: usize) {
    let started = || h.clock.requested_sleeps().iter().filter(|sleep| **sleep == IDLE_TRIM).count();
    while started() < count {
        tokio::task::yield_now().await;
    }
}

/// The JSON of `request`'s messages before `count`, for a byte comparison.
fn bytes(request: &Request, count: usize) -> String {
    serde_json::to_string(&request.messages[..count]).expect("json")
}

/// The real actor: while its cache holds the earlier turns, a turn's start decodes no
/// saved turn; after an idle trim and after a restart it decodes each of them. Every
/// request starts with the bytes of the request before it, so the model reads the
/// same history from the cache and from the store.
#[tokio::test]
async fn a_turn_start_decodes_only_the_saved_turns_that_the_cache_lacks() {
    let (reads, _guard) = Reads::install();
    let setup = Setup::new();
    let read = format!("read {}", setup.home().join("big-32768").display());
    let model = Recorder::new(3);
    let mut h = setup.start_model(model.clone()).await;

    let one = turn_read(&mut h, &reads, &read).await;
    let two = turn_read(&mut h, &reads, "prompt 2").await;
    let three = turn_read(&mut h, &reads, &read).await;
    armed(&h, 3).await;
    h.clock.advance(IDLE_TRIM);
    let four = turn_read(&mut h, &reads, &read).await;
    assert_eq!(reads.trims(), 1, "the trim goes before the prompt");
    let five = turn_read(&mut h, &reads, "prompt 5").await;
    let mut h = h.restart_model(model.clone()).await;
    let six = turn_read(&mut h, &reads, "prompt 6").await;

    let held = |held| Read { decoded: 0, held, bytes: 0 };
    assert_eq!([one, two, three], [held(0), held(1), held(2)], "the cache holds each turn");
    assert_eq!((four.decoded, four.held), (3, 0), "after the trim");
    assert!(four.bytes > 2 * 32 * 1024, "two reads of 32 KiB: {four:?}");
    assert_eq!((five.decoded, five.held), (3, 1), "the cache holds the turn it ran since");
    assert_eq!((six.decoded, six.held), (5, 0), "after the restart");
    assert!(six.bytes > four.bytes, "{six:?}");

    let requests: Vec<Request> =
        model.requests().into_iter().filter(|request| !request.side_call).collect();
    assert_eq!(requests.len(), 9, "two requests for each read");
    for (at, pair) in requests.windows(2).enumerate() {
        let [before, after] = pair else { unreachable!("a window of two") };
        let count = before.messages.len();
        assert!(after.messages.len() > count, "request {at}");
        assert_eq!(
            bytes(before, count),
            bytes(after, count),
            "request {} edits request {at}",
            at + 1
        );
    }
    let last = requests.last().expect("a request");
    assert!(last.messages.iter().any(|message| message.provider_raw.is_some()));
    h.finish();
}
