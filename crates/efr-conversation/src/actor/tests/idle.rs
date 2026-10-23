//! The idle trim: an actor that is idle for `IDLE_TRIM` drops the turns that it keeps
//! in memory, and the next request has the same bytes, from the saved messages. A turn
//! whose messages could not be saved is rebuilt as after a restart.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use efr_protocol::{Event, TurnId};
use efr_provider::Request;
use efr_store::Batch;
use efr_store::turn_messages::NewTurnMessages;
use pretty_assertions::assert_eq;
use tokio::sync::Notify;
use tracing::Subscriber;
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};

use crate::actor::IDLE_TRIM;
use crate::testing::{Harness, Recorder, Setup};

/// Counts the trims of the actors on this thread, from the actor's debug event.
#[derive(Clone, Default)]
struct Trims {
    count: Arc<AtomicUsize>,
    changed: Arc<Notify>,
}

impl<S: Subscriber> Layer<S> for Trims {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        if metadata.target() == "efr_conversation::actor"
            && metadata.fields().field("turns").is_some()
        {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.changed.notify_waiters();
        }
    }
}

impl Trims {
    /// A counter that sees the events of this thread while the guard lives. A tokio
    /// test runs the actor's task on its own thread.
    fn install() -> (Trims, DefaultGuard) {
        let trims = Trims::default();
        let guard =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(trims.clone()));
        (trims, guard)
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    /// Waits until the actors trimmed `count` times in all.
    async fn wait(&self, count: usize) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.count() >= count {
                return;
            }
            changed.await;
        }
    }
}

/// Waits until the actor has started its idle period `count` times in all, so a move
/// of the clock after it ends the period.
async fn armed(h: &Harness, count: usize) {
    let started = || h.clock.requested_sleeps().iter().filter(|sleep| **sleep == IDLE_TRIM).count();
    while started() < count {
        tokio::task::yield_now().await;
    }
}

/// Sends `text` and waits for the end of its turn, which must complete.
async fn turn(h: &mut Harness, text: &str) -> TurnId {
    let sent = h.prompt(text).await;
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "{text}: {end:?}");
    sent.turn_id
}

/// Takes the saved messages of `turn_id` out of the store, as for a turn whose
/// messages could not be saved: saving a turn again replaces its messages.
async fn unsave(h: &Harness, turn_id: TurnId) {
    let none = NewTurnMessages::new(h.conversation_id, turn_id, "replay", "none", Vec::new());
    h.store.writer().append(Batch::new().turn_messages(none)).await.expect("unsaved");
}

/// The requests of the turns, without the side calls.
fn turn_requests(model: &Recorder) -> Vec<Request> {
    model.requests().into_iter().filter(|request| !request.side_call).collect()
}

/// The JSON of `request`'s messages before `count`, for a byte comparison.
fn bytes(request: &Request, count: usize) -> String {
    serde_json::to_string(&request.messages[..count]).expect("json")
}

/// True when `after` starts with every message of `before`, byte for byte, with the
/// same model, system prompt and tools.
fn continues(before: &Request, after: &Request) -> bool {
    let count = before.messages.len();
    (&before.model, &before.system, &before.tools) == (&after.model, &after.system, &after.tools)
        && after.messages.len() >= count
        && bytes(before, count) == bytes(after, count)
}

#[tokio::test]
async fn the_request_after_an_idle_trim_has_the_bytes_of_the_request_before_it() {
    let (trims, _guard) = Trims::install();
    let setup = Setup::new();
    let model = Recorder::new(3);
    let mut h = setup.start_model(model.clone()).await;
    let notes = h.cwd.join("notes.txt");

    turn(&mut h, "prompt 1").await;
    turn(&mut h, &format!("read {}", notes.display())).await;
    turn(&mut h, "prompt 3").await;
    armed(&h, 3).await;
    // NOTE: a request that starts no work does not start the period again.
    h.clock.advance(IDLE_TRIM / 2);
    h.handle.state().await.expect("state");
    h.clock.advance(IDLE_TRIM / 2);
    trims.wait(1).await;
    turn(&mut h, "prompt 4").await;

    let requests = turn_requests(&model);
    let [.., before, after] = requests.as_slice() else {
        panic!("the requests: {}", requests.len());
    };
    assert!(continues(before, after), "the request after the trim edits the one before it");
    assert!(after.messages.iter().any(|message| message.provider_raw.is_some()));
    assert_eq!(trims.count(), 1);
    h.finish();
}

#[tokio::test]
async fn after_an_idle_trim_a_turn_without_saved_messages_is_rebuilt_as_after_a_restart() {
    let (trims, _guard) = Trims::install();
    let setup = Setup::new();
    let model = Recorder::new(3);
    let mut h = setup.start_model(model.clone()).await;

    let one = turn(&mut h, "prompt 1").await;
    turn(&mut h, "prompt 2").await;
    unsave(&h, one).await;
    armed(&h, 2).await;
    h.clock.advance(IDLE_TRIM);
    trims.wait(1).await;
    turn(&mut h, "prompt 3").await;
    let mut h = h.restart_model(model.clone()).await;
    turn(&mut h, "prompt 4").await;

    let requests = turn_requests(&model);
    let [_, two, three, four] = requests.as_slice() else {
        panic!("four requests: {}", requests.len());
    };
    // The actor kept turn 1 with its provider items until the trim; after it, turn 1
    // comes from its events, as after the restart.
    assert!(two.messages[1].provider_raw.is_some(), "{:?}", two.messages[1]);
    assert!(three.messages[1].provider_raw.is_none(), "{:?}", three.messages[1]);
    assert!(continues(three, four), "the restart rebuilds turn 1 as the trim did");
    h.finish();
}

#[tokio::test]
async fn the_idle_period_starts_again_at_the_end_of_each_turn() {
    let (trims, _guard) = Trims::install();
    let setup = Setup::new();
    let model = Recorder::new(3);
    let mut h = setup.start_model(model.clone()).await;
    let most = IDLE_TRIM - Duration::from_secs(60);

    let one = turn(&mut h, "prompt 1").await;
    unsave(&h, one).await;
    armed(&h, 1).await;
    h.clock.advance(most);
    turn(&mut h, "prompt 2").await;
    armed(&h, 2).await;
    h.clock.advance(most);
    turn(&mut h, "prompt 3").await;
    assert_eq!(trims.count(), 0, "no period lasted IDLE_TRIM");
    armed(&h, 3).await;
    h.clock.advance(IDLE_TRIM);
    trims.wait(1).await;
    turn(&mut h, "prompt 4").await;

    let requests = turn_requests(&model);
    let [_, _, three, four] = requests.as_slice() else {
        panic!("four requests: {}", requests.len());
    };
    assert!(three.messages[1].provider_raw.is_some(), "turn 1 is still in memory");
    assert!(four.messages[1].provider_raw.is_none(), "turn 1 comes from its events");
    h.finish();
}
