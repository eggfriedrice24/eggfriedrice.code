use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::{
    CallId, CommandId, ConversationId, Event, Origin, Scope, Seq, ShellContext, TurnId,
    TurnSettings,
};
use efr_stdx::time::Clock as _;
use efr_store::search::{DocKind, Pending, SearchApplied, SearchPage, SearchState};
use efr_store::{Batch, Committed, Readers, Store, StoreError, WriterHandle};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{
    DEBOUNCE, IndexStore, Indexer, MIN_PAUSE, Rules, SearchIndexStatus, StoreIndex,
    TURN_PAGE_INTERVAL, index_text, status_channel,
};

/// An id of type `T` from a number, in the form of the fixtures.
fn id<T: std::str::FromStr>(n: u64) -> T
where
    T::Err: std::fmt::Debug,
{
    format!("0192f0c1-7a00-7000-8000-{n:012x}").parse().unwrap()
}

/// Builds the events of a log, with ids from a counter.
struct Log {
    next: u64,
}

/// The ids of one turn.
#[derive(Clone, Copy)]
struct Turn {
    conversation: ConversationId,
    turn: TurnId,
}

impl Log {
    fn new() -> Self {
        Log { next: 1 }
    }

    fn id<T: std::str::FromStr>(&mut self) -> T
    where
        T::Err: std::fmt::Debug,
    {
        self.next += 1;
        id(self.next)
    }

    fn conversation(&mut self) -> ConversationId {
        self.id()
    }

    /// A queued prompt of a new turn, with the conversation created first when `first`.
    fn prompt(&mut self, conversation: ConversationId, text: &str, first: bool) -> (Turn, Batch) {
        let (turn, events) = self.prompt_events(conversation, text, first);
        (turn, batch(conversation, events))
    }

    fn prompt_events(
        &mut self,
        conversation: ConversationId,
        text: &str,
        first: bool,
    ) -> (Turn, Vec<Event>) {
        let turn = Turn { conversation, turn: self.id() };
        let mut events = Vec::new();
        if first {
            events.push(Event::ConversationCreated {
                origin: Origin::Shell,
                tty: None,
                route: None,
            });
        }
        let command_id: CommandId = self.id();
        events.push(Event::PromptQueued {
            turn_id: turn.turn,
            command_id,
            text: text.to_owned(),
            origin: Origin::Shell,
            context: Some(ShellContext::new("/srv/p")),
            settings: TurnSettings::default(),
            steers: Vec::new(),
        });
        (turn, events)
    }

    fn start(turn: Turn) -> Batch {
        batch(turn.conversation, vec![start_event(turn)])
    }

    /// An answer and a `shell` call of a running turn.
    fn work(&mut self, turn: Turn, answer: &str, command: &str) -> Vec<Event> {
        let call_id: CallId = self.id();
        vec![
            Event::AssistantMessageCompleted {
                turn_id: turn.turn,
                index: 0,
                text: answer.to_owned(),
            },
            Event::ToolCallStarted {
                turn_id: turn.turn,
                call_id,
                tool: "shell".to_owned(),
                input: json!({ "command": command }),
                freeform: false,
                manual_input: false,
                launch: None,
            },
        ]
    }

    fn end(turn: Turn) -> Batch {
        batch(turn.conversation, vec![end_event(turn)])
    }

    /// A whole turn in one batch: prompt, start, answer, command, end.
    fn turn(&mut self, conversation: ConversationId, first: bool, texts: [&str; 3]) -> Batch {
        let (turn, mut events) = self.prompt_events(conversation, texts[0], first);
        events.push(start_event(turn));
        events.extend(self.work(turn, texts[1], texts[2]));
        events.push(end_event(turn));
        batch(conversation, events)
    }
}

fn start_event(turn: Turn) -> Event {
    Event::TurnStarted {
        turn_id: turn.turn,
        cwd: "/srv/p".into(),
        scope: Scope::Path("/srv/p".into()),
        settings: None,
    }
}

fn end_event(turn: Turn) -> Event {
    Event::TurnCompleted {
        turn_id: turn.turn,
        usage: None,
        context: None,
        changes: None,
        stop: None,
        undo: None,
    }
}

/// One batch of `events` of `conversation`.
fn batch(conversation: ConversationId, events: Vec<Event>) -> Batch {
    events.into_iter().fold(Batch::new(), |batch, event| batch.event(conversation, event))
}

/// The texts of the index, in sequence order.
async fn texts(readers: &Readers) -> Vec<String> {
    docs(readers).await.into_iter().map(|doc| doc.2).collect()
}

/// Every doc of the index: its seq, kind code and text.
async fn docs(readers: &Readers) -> Vec<(u64, i64, String)> {
    readers
        .with(|conn| {
            let mut stmt = conn.prepare("SELECT seq, kind, text FROM search_docs ORDER BY seq")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<Vec<(i64, i64, String)>, _>>()?;
            Ok(rows.into_iter().map(|(seq, kind, text)| (seq as u64, kind, text)).collect())
        })
        .await
        .unwrap()
}

/// The newest event of the log.
async fn newest(readers: &Readers) -> Seq {
    readers.with(efr_store::events::last_seq).await.unwrap()
}

/// What a test can change in the store of the indexer.
#[derive(Default)]
struct Knobs {
    /// The reads that fail before one works.
    failing_reads: AtomicU32,
    /// The reads so far.
    reads: AtomicU32,
    /// How far a writer job moves the test clock: the time it takes.
    job_millis: AtomicU32,
}

/// The store of the tests: the real one, a clock that a writer job moves, and reads
/// that fail on demand.
#[derive(Clone)]
struct TestStore {
    inner: StoreIndex,
    clock: TestClock,
    knobs: Arc<Knobs>,
    /// The broadcast that the indexer reads; a test may give a small one.
    broadcast: Option<broadcast::Sender<Committed>>,
}

impl TestStore {
    fn job(&self) {
        let millis = self.knobs.job_millis.load(Ordering::SeqCst);
        if millis > 0 {
            self.clock.advance(Duration::from_millis(u64::from(millis)));
        }
    }
}

#[async_trait]
impl IndexStore for TestStore {
    async fn read(&self, redaction: u32) -> Result<(SearchState, Option<Pending>), StoreError> {
        self.knobs.reads.fetch_add(1, Ordering::SeqCst);
        let failing = self.knobs.failing_reads.load(Ordering::SeqCst);
        if failing > 0 {
            self.knobs.failing_reads.store(failing - 1, Ordering::SeqCst);
            return Err(StoreError::TaskPanicked { task: "test read" });
        }
        self.inner.read(redaction).await
    }

    async fn running_turns(&self) -> Result<HashSet<TurnId>, StoreError> {
        self.inner.running_turns().await
    }

    async fn apply(&self, page: SearchPage) -> Result<SearchApplied, StoreError> {
        self.job();
        self.inner.apply(page).await
    }

    async fn reset(&self, redaction: u32) -> Result<(), StoreError> {
        self.inner.reset(redaction).await
    }

    async fn merge(&self, pages: u32) -> Result<bool, StoreError> {
        self.job();
        self.inner.merge(pages).await
    }

    fn subscribe(&self) -> broadcast::Receiver<Committed> {
        match &self.broadcast {
            Some(sender) => sender.subscribe(),
            None => self.inner.subscribe(),
        }
    }
}

/// An indexer over an in-memory store, run by a test.
struct Harness {
    store: Store,
    clock: TestClock,
    knobs: Arc<Knobs>,
    status: watch::Receiver<SearchIndexStatus>,
    stop: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl Harness {
    async fn new() -> Self {
        let clock = TestClock::new();
        let store = Store::open_in_memory(clock.shared()).await.unwrap();
        let (_, status) = status_channel();
        Harness {
            store,
            clock,
            knobs: Arc::default(),
            status,
            stop: CancellationToken::new(),
            task: None,
        }
    }

    fn writer(&self) -> &WriterHandle {
        self.store.writer()
    }

    fn readers(&self) -> &Readers {
        self.store.readers()
    }

    fn test_store(&self, broadcast: Option<broadcast::Sender<Committed>>) -> TestStore {
        TestStore {
            inner: StoreIndex {
                writer: self.writer().clone(),
                reader: self.store.index_reader().clone(),
            },
            clock: self.clock.clone(),
            knobs: Arc::clone(&self.knobs),
            broadcast,
        }
    }

    /// Starts the indexer with `rules` over the store, or over a store with the
    /// broadcast `broadcast`.
    fn start_with(&mut self, rules: Rules, broadcast: Option<broadcast::Sender<Committed>>) {
        let (sender, status) = status_channel();
        self.status = status;
        let indexer = Indexer::new(
            self.test_store(broadcast),
            self.clock.shared(),
            rules,
            sender,
            self.stop.clone(),
        );
        self.task = Some(tokio::spawn(indexer.run()));
    }

    fn start(&mut self) {
        self.start_with(Arc::new(index_text), None);
    }

    async fn append(&self, batch: Batch) -> Seq {
        self.writer().append(batch).await.unwrap().events().last().unwrap().seq
    }

    /// Waits until `done` holds for the status, ending each sleep of the indexer on the
    /// way by moving the clock to its end. Returns the status and the sleeps it ended.
    async fn run_until(
        &mut self,
        done: impl Fn(&SearchIndexStatus) -> bool,
    ) -> (SearchIndexStatus, Vec<Duration>) {
        let mut slept = Vec::new();
        loop {
            let clock = self.clock.clone();
            let status = *self
                .status
                .wait_for(|status| {
                    done(status) || status.sleeping_until.is_some_and(|until| until > clock.now())
                })
                .await
                .unwrap();
            if done(&status) {
                return (status, slept);
            }
            if let Some(until) = status.sleeping_until {
                let now = self.clock.now();
                slept.push(Duration::try_from(until.duration_since(now)).unwrap());
                self.clock.set(until);
            }
        }
    }

    /// Waits until the index has every event and no merge is due.
    async fn run_to_end(&mut self) -> (SearchIndexStatus, Vec<Duration>) {
        let target = newest(self.readers()).await;
        self.run_until(|status| status.indexed_seq >= target && status.idle).await
    }

    async fn stop(mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            task.await.unwrap();
        }
        self.store.close().await;
    }
}

#[tokio::test]
async fn backfill_indexes_old_conversations() {
    let mut harness = Harness::new().await;
    let mut log = Log::new();
    for n in 0..3 {
        let conversation = log.conversation();
        harness
            .append(log.turn(conversation, true, [&format!("prompt {n}"), "the answer", "ls -la"]))
            .await;
    }
    harness.start();
    let (status, slept) = harness.run_to_end().await;
    assert_eq!(status.indexed_seq, newest(harness.readers()).await);
    assert_eq!(status.pages, 1);
    assert!(slept.is_empty(), "{slept:?}");
    let docs = docs(harness.readers()).await;
    let texts: Vec<&str> = docs.iter().map(|(_, _, text)| text.as_str()).collect();
    assert_eq!(texts.iter().filter(|text| text.starts_with("prompt ")).count(), 3);
    assert_eq!(texts.iter().filter(|text| **text == "ls -la").count(), 3);
    harness.stop().await;
}

/// After the round at its start, a caught-up indexer waits for the broadcast only.
#[tokio::test]
async fn caught_up_indexer_holds_no_timer() {
    let mut harness = Harness::new().await;
    let mut log = Log::new();
    let conversation = log.conversation();
    harness.append(log.turn(conversation, true, ["hello", "hi", "ls"])).await;
    harness.start();
    harness.run_to_end().await;
    assert_eq!(harness.clock.pending_sleeps(), 0);
    assert!(harness.clock.requested_sleeps().is_empty());
    // A batch that ends no turn wakes nothing.
    let (_, queued) = log.prompt(conversation, "later", false);
    harness.append(queued).await;
    let target = newest(harness.readers()).await;
    assert!(harness.status.borrow().indexed_seq < target);
    assert_eq!(harness.clock.pending_sleeps(), 0);
    harness.stop().await;
}

#[tokio::test]
async fn a_turn_end_waits_the_debounce_then_indexes() {
    let mut harness = Harness::new().await;
    harness.start();
    harness.run_to_end().await;
    let mut log = Log::new();
    let conversation = log.conversation();
    let seq = harness.append(log.turn(conversation, true, ["zebracorn here", "ok", "ls"])).await;
    let (status, slept) = harness.run_until(|status| status.indexed_seq >= seq).await;
    assert_eq!(slept, vec![DEBOUNCE]);
    assert_eq!(status.pages, 1);
    assert!(docs(harness.readers()).await.iter().any(|(_, _, text)| text == "zebracorn here"));
    harness.stop().await;
}

#[tokio::test]
async fn panicking_row_is_skipped() {
    let mut harness = Harness::new().await;
    let mut log = Log::new();
    let conversation = log.conversation();
    harness.append(log.turn(conversation, true, ["first prompt", "boom", "ls"])).await;
    harness.append(log.turn(conversation, false, ["second prompt", "fine", "pwd"])).await;
    let rules: Rules = Arc::new(|kind, text| {
        assert!(text != "boom", "a test rule that fails");
        index_text(kind, text)
    });
    harness.start_with(rules, None);
    let (status, _) = harness.run_to_end().await;
    assert_eq!(status.indexed_seq, newest(harness.readers()).await);
    let texts: Vec<String> = docs(harness.readers()).await.into_iter().map(|doc| doc.2).collect();
    assert_eq!(texts, ["first prompt", "ls", "second prompt", "fine", "pwd"]);
    harness.stop().await;
}

#[tokio::test]
async fn store_error_backs_off() {
    let mut harness = Harness::new().await;
    harness.knobs.failing_reads.store(3, Ordering::SeqCst);
    harness.start();
    let (status, slept) = harness.run_to_end().await;
    assert_eq!(slept, [1, 2, 4].map(Duration::from_secs));
    // Three failed reads, one that finds the new store, and one after its reset.
    assert_eq!(harness.knobs.reads.load(Ordering::SeqCst), 5);
    assert_eq!(status.sleeping_until, None);
    // A success ends the backoff: the next error waits 1 s again.
    harness.knobs.failing_reads.store(1, Ordering::SeqCst);
    let mut log = Log::new();
    let conversation = log.conversation();
    let seq = harness.append(log.turn(conversation, true, ["a", "b", "c"])).await;
    let (_, slept) = harness.run_until(|status| status.indexed_seq >= seq).await;
    assert_eq!(slept, [DEBOUNCE, Duration::from_secs(1)]);
    harness.stop().await;
}

/// The texts of `n` turns, each prompt unique, with no command.
async fn many_turns(harness: &Harness, log: &mut Log, turns: usize) {
    let conversation = log.conversation();
    for n in 0..turns {
        harness
            .append(log.turn(
                conversation,
                n == 0,
                [&format!("prompt number {n}"), "an answer", "ls"],
            ))
            .await;
    }
}

#[tokio::test]
async fn backfill_pauses_four_times_the_job() {
    let mut harness = Harness::new().await;
    let mut log = Log::new();
    // 3 docs a turn and 32 docs a page: 4 pages.
    many_turns(&harness, &mut log, 40).await;
    harness.knobs.job_millis.store(7, Ordering::SeqCst);
    harness.start();
    let (status, slept) = harness.run_to_end().await;
    assert_eq!(status.pages, 4);
    assert!(status.merges >= 1);
    // A pause after each page that left more, and before each merge, of 4 times 7 ms.
    assert!(slept.len() >= 4, "{slept:?}");
    assert!(slept.iter().all(|wait| *wait == Duration::from_millis(28)), "{slept:?}");
    // Without a measured job, the pause is the shortest one.
    harness.stop().await;
    let mut harness = Harness::new().await;
    many_turns(&harness, &mut Log::new(), 40).await;
    harness.start();
    let (_, slept) = harness.run_to_end().await;
    assert!(slept.iter().all(|wait| *wait == MIN_PAUSE), "{slept:?}");
    harness.stop().await;
}

#[tokio::test]
async fn backfill_slows_while_a_turn_runs() {
    let mut harness = Harness::new().await;
    let mut log = Log::new();
    harness.knobs.job_millis.store(2, Ordering::SeqCst);
    harness.start();
    harness.run_to_end().await;
    // A turn starts and stays running; then a lot of old text arrives at once (a
    // restart's catch-up has the same shape).
    let conversation = log.conversation();
    let (turn, queued) = log.prompt(conversation, "a long turn", true);
    harness.append(queued).await;
    harness.append(Log::start(turn)).await;
    many_turns(&harness, &mut log, 40).await;
    let target = newest(harness.readers()).await;
    let (_, slept) = harness
        .run_until(|status| status.indexed_seq >= target && status.sleeping_until.is_none())
        .await;
    // The debounce, then a pause of a second less the job before each later page.
    assert_eq!(slept[0], DEBOUNCE);
    let pauses = &slept[1..];
    assert!(pauses.len() >= 3, "{slept:?}");
    for pause in pauses.iter().take(3) {
        assert_eq!(*pause, TURN_PAGE_INTERVAL - Duration::from_millis(2), "{slept:?}");
    }
    // The turn ends: the merges after the backfill end too.
    harness.append(Log::end(turn)).await;
    let (status, _) = harness.run_to_end().await;
    assert!(!status.merging);
    harness.stop().await;
}

#[tokio::test]
async fn merges_run_after_the_backfill() {
    let mut harness = Harness::new().await;
    let mut log = Log::new();
    many_turns(&harness, &mut log, 40).await;
    harness.start();
    let (status, _) = harness.run_until(|status| status.merging).await;
    assert!(status.pages >= 2);
    let (status, _) = harness.run_to_end().await;
    assert!(status.merges >= 1);
    assert!(!status.merging);
    // One live round merges nothing.
    let conversation = log.conversation();
    let merges = status.merges;
    let seq = harness.append(log.turn(conversation, true, ["x", "y", "z"])).await;
    let (status, _) = harness.run_until(|status| status.indexed_seq >= seq).await;
    assert_eq!(status.merges, merges);
    assert!(!status.merging);
    harness.stop().await;
}

#[tokio::test]
async fn lagged_broadcast_loses_nothing() {
    let mut harness = Harness::new().await;
    let (small, _) = broadcast::channel(1);
    harness.start_with(Arc::new(index_text), Some(small.clone()));
    harness.run_to_end().await;
    let mut real = harness.writer().subscribe();
    let mut log = Log::new();
    let conversation = log.conversation();
    // Three turns' batches reach a broadcast of one at once: the indexer lags.
    let mut batches = Vec::new();
    for n in 0..3 {
        harness.append(log.turn(conversation, n == 0, [&format!("lag {n}"), "a", "b"])).await;
        batches.push(real.recv().await.unwrap());
    }
    for batch in batches {
        small.send(batch).unwrap();
    }
    let (_, slept) = harness.run_to_end().await;
    assert_eq!(slept, vec![DEBOUNCE]);
    let texts: Vec<String> = docs(harness.readers()).await.into_iter().map(|doc| doc.2).collect();
    for n in 0..3 {
        assert!(texts.contains(&format!("lag {n}")), "{texts:?}");
    }
    harness.stop().await;
}

#[tokio::test]
async fn redaction_change_reindexes() {
    let mut harness = Harness::new().await;
    let mut log = Log::new();
    let conversation = log.conversation();
    let seq = harness.append(log.turn(conversation, true, ["kept text", "a", "b"])).await;
    // An index that an older redaction (version 0) wrote, with a text that the
    // current rules would never write.
    let applied = harness
        .writer()
        .search_apply(SearchPage {
            after: Seq::ZERO,
            through: seq,
            redaction: 0,
            docs: vec![efr_store::search::SourceRow {
                seq,
                conversation_id: conversation,
                kind: DocKind::Answer,
                text: "an old text".to_owned(),
            }],
            drops: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(applied, SearchApplied::Applied);
    harness.start();
    harness.run_to_end().await;
    let texts: Vec<String> = docs(harness.readers()).await.into_iter().map(|doc| doc.2).collect();
    assert_eq!(texts, ["kept text", "a", "b"]);
    harness.stop().await;
}

#[tokio::test]
async fn withdrawn_prompt_leaves_the_index() {
    let mut harness = Harness::new().await;
    harness.start();
    harness.run_to_end().await;
    let mut log = Log::new();
    let conversation = log.conversation();
    let (first, queued) = log.prompt(conversation, "the running prompt", true);
    harness.append(queued).await;
    harness.append(Log::start(first)).await;
    let (second, queued) = log.prompt(conversation, "a waiting prompt", false);
    harness.append(queued).await;
    harness.append(Log::end(first)).await;
    harness.run_to_end().await;
    assert_eq!(texts(harness.readers()).await, ["the running prompt", "a waiting prompt"]);
    harness
        .append(Batch::new().event(
            conversation,
            Event::PromptWithdrawn { turn_id: second.turn, origin: Origin::Shell },
        ))
        .await;
    harness.run_to_end().await;
    assert_eq!(texts(harness.readers()).await, ["the running prompt"]);
    harness.stop().await;
}

#[tokio::test]
async fn withdrawn_steer_leaves_the_index() {
    let mut harness = Harness::new().await;
    harness.start();
    harness.run_to_end().await;
    let mut log = Log::new();
    let conversation = log.conversation();
    let (turn, queued) = log.prompt(conversation, "the prompt", true);
    harness.append(queued).await;
    harness.append(Log::start(turn)).await;
    let kept = harness
        .append(Batch::new().event(
            conversation,
            Event::TurnSteered { turn_id: turn.turn, text: "a kept steer".to_owned() },
        ))
        .await;
    let taken = harness
        .append(Batch::new().event(
            conversation,
            Event::TurnSteered { turn_id: turn.turn, text: "a taken steer".to_owned() },
        ))
        .await;
    harness
        .append(Batch::new().event(
            conversation,
            Event::SteeringWithdrawn {
                turn_id: turn.turn,
                steers: vec![taken],
                origin: Origin::Shell,
            },
        ))
        .await;
    harness.append(Log::end(turn)).await;
    harness.run_to_end().await;
    let docs = docs(harness.readers()).await;
    assert!(
        docs.iter().any(|(seq, kind, text)| Seq::new(*seq) == kept
            && *kind == 1
            && text == "a kept steer")
    );
    assert!(!docs.iter().any(|(_, _, text)| text == "a taken steer"), "{docs:?}");
    harness.stop().await;
}

/// efrd's stop in the middle of a page: the indexer ends at the next row and writes
/// none of the page.
#[tokio::test]
async fn indexer_stops_in_the_middle_of_a_page() {
    let mut harness = Harness::new().await;
    let mut log = Log::new();
    let conversation = log.conversation();
    harness.append(log.turn(conversation, true, ["one", "two", "three"])).await;
    let stop = harness.stop.clone();
    let rules: Rules = Arc::new(move |kind, text| {
        if text == "two" {
            stop.cancel();
        }
        index_text(kind, text)
    });
    harness.start_with(rules, None);
    let task = harness.task.take().unwrap();
    task.await.unwrap();
    assert_eq!(harness.status.borrow().pages, 0);
    assert!(docs(harness.readers()).await.is_empty());
    harness.stop().await;
}

/// efrd's stop while the indexer pauses between pages ends it at once.
#[tokio::test]
async fn indexer_stops_while_it_pauses() {
    let mut harness = Harness::new().await;
    many_turns(&harness, &mut Log::new(), 40).await;
    harness.start();
    let clock = harness.clock.clone();
    let _ = *harness
        .status
        .wait_for(|status| status.sleeping_until.is_some_and(|until| until > clock.now()))
        .await
        .unwrap();
    harness.stop.cancel();
    harness.task.take().unwrap().await.unwrap();
    assert_eq!(harness.status.borrow().pages, 1);
    harness.stop().await;
}
