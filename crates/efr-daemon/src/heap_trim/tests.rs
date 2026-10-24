//! The heap trim's debounce on a manual clock, with a trimmer that only counts: one
//! heap trim after the quiet period of the actor trims, none while a turn runs, and at
//! most one in `MIN_GAP`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use efr_protocol::{
    CommandId, ConversationId, Event, Origin, Scope, ShellContext, TurnId, TurnSettings,
};
use efr_stdx::time::Clock as _;
use efr_store::{Batch, Store};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{HeapTrimParts, HeapTrimStatus, MIN_GAP, QUIET, Trimmer, follow, status_channel};

/// A trimmer that counts its calls and gives nothing back.
#[derive(Debug, Default)]
struct Counting {
    calls: AtomicU64,
}

impl Trimmer for Counting {
    fn trim(&self) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        false
    }
}

/// The task over an in-memory store, run by a test.
struct Harness {
    store: Store,
    clock: TestClock,
    trimmer: Arc<Counting>,
    actor_trims: watch::Sender<u64>,
    status: watch::Receiver<HeapTrimStatus>,
    stop: CancellationToken,
    task: JoinHandle<()>,
    conversation: ConversationId,
    next: u64,
}

impl Harness {
    async fn start() -> Self {
        let clock = TestClock::new();
        let store = Store::open_in_memory(clock.shared()).await.unwrap();
        let trimmer = Arc::new(Counting::default());
        let (actor_trims, actor_trims_watch) = watch::channel(0);
        let (status_sender, status) = status_channel();
        let stop = CancellationToken::new();
        let parts = HeapTrimParts {
            actor_trims: actor_trims_watch,
            committed: store.writer().subscribe(),
            readers: store.readers().clone(),
            clock: clock.shared(),
            status: status_sender,
            stop: stop.clone(),
        };
        let task = tokio::spawn(follow(parts, Arc::clone(&trimmer)));
        let harness = Harness {
            store,
            clock,
            trimmer,
            actor_trims,
            status,
            stop,
            task,
            conversation: id(1),
            next: 1,
        };
        let created = Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None };
        harness.append(vec![created]).await;
        harness
    }

    fn id<T: std::str::FromStr>(&mut self) -> T
    where
        T::Err: std::fmt::Debug,
    {
        self.next += 1;
        id(self.next)
    }

    async fn append(&self, events: Vec<Event>) {
        let batch = events
            .into_iter()
            .fold(Batch::new(), |batch, event| batch.event(self.conversation, event));
        self.store.writer().append(batch).await.unwrap();
    }

    /// Queues and starts a turn, and waits until the task sees it run.
    async fn start_turn(&mut self) -> TurnId {
        let turn_id: TurnId = self.id();
        let command_id: CommandId = self.id();
        let running = self.status.borrow().running;
        self.append(vec![
            Event::PromptQueued {
                turn_id,
                command_id,
                text: "a prompt".to_owned(),
                origin: Origin::Shell,
                context: Some(ShellContext::new("/srv/p")),
                settings: TurnSettings::default(),
                steers: Vec::new(),
            },
            Event::TurnStarted {
                turn_id,
                cwd: "/srv/p".into(),
                scope: Scope::Path("/srv/p".into()),
                settings: None,
            },
        ])
        .await;
        self.wait(|status| status.running == running + 1).await;
        turn_id
    }

    /// Ends `turn_id`, and waits until the task sees it end.
    async fn end_turn(&mut self, turn_id: TurnId) {
        let running = self.status.borrow().running;
        self.append(vec![Event::TurnCompleted {
            turn_id,
            usage: None,
            context: None,
            changes: None,
            stop: None,
            undo: None,
        }])
        .await;
        self.wait(|status| status.running + 1 == running).await;
    }

    /// One actor trim, as an actor adds it, and waits until the task sees it.
    async fn actor_trim(&mut self) {
        self.actor_trims.send_modify(|count| *count += 1);
        let count = *self.actor_trims.borrow();
        self.wait(|status| status.actor_trims == count).await;
    }

    async fn wait(&mut self, done: impl FnMut(&HeapTrimStatus) -> bool) -> HeapTrimStatus {
        *self.status.wait_for(done).await.unwrap()
    }

    fn calls(&self) -> u64 {
        self.trimmer.calls.load(Ordering::SeqCst)
    }

    fn due(&self) -> Option<jiff::Timestamp> {
        self.status.borrow().due
    }

    async fn finish(self) {
        self.stop.cancel();
        self.task.await.unwrap();
    }
}

/// An id of type `T` from a number, in the form of the fixtures.
fn id<T: std::str::FromStr>(n: u64) -> T
where
    T::Err: std::fmt::Debug,
{
    format!("0192f0c1-7a00-7000-8000-{n:012x}").parse().unwrap()
}

fn after(at: jiff::Timestamp, by: Duration) -> jiff::Timestamp {
    at.checked_add(by).unwrap()
}

#[tokio::test]
async fn the_heap_is_trimmed_once_after_the_quiet_period_of_the_actor_trims() {
    let mut h = Harness::start().await;
    assert_eq!(h.due(), None, "no timer without an actor trim");

    h.actor_trim().await;
    h.clock.advance(Duration::from_secs(10));
    h.actor_trim().await;
    h.clock.advance(Duration::from_secs(10));
    h.actor_trim().await;
    // NOTE: the quiet period counts from the last actor trim.
    let last = h.clock.now();
    assert_eq!(h.due(), Some(after(last, QUIET)));

    h.clock.advance(QUIET - Duration::from_secs(1));
    assert_eq!(h.clock.next_deadline(), Some(after(last, QUIET)));
    assert_eq!((h.calls(), h.status.borrow().trims), (0, 0));

    h.clock.advance(Duration::from_secs(1));
    h.wait(|status| status.trims == 1 && status.due.is_none()).await;
    assert_eq!(h.calls(), 1);

    // NOTE: without a new actor trim, no time gives a second heap trim.
    h.clock.advance(Duration::from_secs(3600));
    tokio::task::yield_now().await;
    assert_eq!((h.calls(), h.due()), (1, None));
    h.finish().await;
}

#[tokio::test]
async fn the_heap_is_not_trimmed_while_a_turn_runs() {
    let mut h = Harness::start().await;
    let turn = h.start_turn().await;
    h.actor_trim().await;
    assert_eq!(h.due(), None, "no heap trim is due while a turn runs");

    h.clock.advance(Duration::from_secs(3600));
    tokio::task::yield_now().await;
    assert_eq!((h.calls(), h.due()), (0, None));

    // NOTE: the quiet period counts from the end of the turn.
    h.end_turn(turn).await;
    let ended = h.clock.now();
    assert_eq!(h.due(), Some(after(ended, QUIET)));

    // NOTE: a turn that starts in the quiet period stops the wait, and its end starts
    // the period again.
    h.clock.advance(QUIET / 2);
    let second = h.start_turn().await;
    assert_eq!(h.due(), None);
    h.clock.advance(QUIET);
    tokio::task::yield_now().await;
    assert_eq!(h.calls(), 0);
    h.end_turn(second).await;
    let ended = h.clock.now();
    assert_eq!(h.due(), Some(after(ended, QUIET)));

    h.clock.advance(QUIET);
    h.wait(|status| status.trims == 1).await;
    assert_eq!(h.calls(), 1);
    h.finish().await;
}

#[tokio::test]
async fn a_second_heap_trim_waits_for_the_gap_after_the_first() {
    let mut h = Harness::start().await;
    h.actor_trim().await;
    h.clock.advance(QUIET);
    h.wait(|status| status.trims == 1).await;
    let first = h.clock.now();

    h.actor_trim().await;
    assert_eq!(h.due(), Some(after(first, MIN_GAP)), "the gap is longer than the quiet");
    h.clock.advance(MIN_GAP - Duration::from_secs(1));
    tokio::task::yield_now().await;
    assert_eq!(h.calls(), 1);

    h.clock.advance(Duration::from_secs(1));
    h.wait(|status| status.trims == 2).await;
    assert_eq!(h.calls(), 2);
    h.finish().await;
}
