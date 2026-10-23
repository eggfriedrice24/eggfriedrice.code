//! The turns that the newest summary covers: a turn's start reads none of their events
//! and the window builds none of their messages (the plan's W1-A). The window, the
//! request that the model gets and the context breakdown stay byte for byte the same as
//! when every turn is read and rebuilt and the cut drops their messages.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{
    CallId, CommandId, Compaction, CompactionId, CompactionTrigger, ConversationCompact, ErrorBody,
    ErrorCode, Event, Origin, Scope, Seq, TurnId, TurnSettings,
};
use efr_provider::{Message, ProviderId, Request};
use efr_stdx::id::uuid_v7;
use efr_stdx::rng::Rng as _;
use efr_store::Batch;
use efr_store::conversations::TurnStatus;
use efr_store::turn_messages::NewTurnMessages;
use efr_test_support::{TestClock, TestRng, TestStore};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{completed, conversation, finished, key, started};
use crate::compaction::with_window;
use crate::context::breakdown;
use crate::history::{CachedTurn, Covered, HistoryLimits, ModelKey, Snapshot};
use crate::testing::{Recorder, compacting, compactions, request};

/// The page sizes and byte limits that each check reads with: a page that holds one
/// turn, a few turns, or all of them, and a byte limit that leaves out turns.
const LIMITS: [(u32, usize); 6] = [
    (3, usize::MAX),
    (10, usize::MAX),
    (40, usize::MAX),
    (4096, usize::MAX),
    (10, 700),
    (4096, 1500),
];

/// What one check found.
#[derive(Debug, Default)]
struct Checked {
    /// The checks in which the skip read fewer events than the full read.
    fewer_events: usize,
}

/// The window of `store`'s conversation with and without the skip, for each of
/// [`LIMITS`], each model key and an empty or full cache: the windows, the requests as
/// JSON and the context breakdowns must be equal. `current` is the turn being
/// assembled; `running` is a turn that the breakdown counts as ended, as
/// `conversation.context` does.
async fn assert_same(
    store: &TestStore,
    current: Option<TurnId>,
    running: Option<TurnId>,
    fresh: Option<&str>,
    checked: &mut Checked,
) {
    let base = request(Vec::new());
    for (max_events, max_bytes) in LIMITS {
        let limits = HistoryLimits::new(max_events, max_bytes);
        let read = |covered| Snapshot::read_with(store.readers(), conversation(), limits, covered);
        let (mut skipped, mut kept) =
            (read(Covered::Skip).await.expect("read"), read(Covered::Keep).await.expect("read"));
        assert_eq!(skipped.turns, kept.turns);
        assert_eq!(skipped.page, kept.page);
        assert_eq!(skipped.compactions, kept.compactions);
        assert!(kept.older.len() >= skipped.older.len());
        if kept.older.len() > skipped.older.len() {
            checked.fewer_events += 1;
        }
        for view in [&mut skipped, &mut kept] {
            for turn in &mut view.turns {
                if Some(turn.id) == running && turn.status == TurnStatus::Running {
                    turn.status = TurnStatus::Completed;
                }
            }
        }
        let numbers: HashMap<TurnId, u32> = kept
            .turns
            .iter()
            .filter(|turn| turn.started_at.is_some())
            .zip(1_u32..)
            .map(|(turn, number)| (turn.id, number))
            .collect();
        let full: HashMap<TurnId, Arc<CachedTurn>> =
            kept.saved.iter().map(|(id, turn)| (*id, Arc::new(turn.clone()))).collect();
        for cache in [HashMap::new(), full] {
            for model in [key("replay", "m"), key("other", "m")] {
                let window = |snapshot: &Snapshot, covered| {
                    snapshot.window_with(current, &cache, &model, limits, fresh, covered)
                };
                let (new, old) = (window(&skipped, Covered::Skip), window(&kept, Covered::Keep));
                let what = format!("{limits:?}, {model:?}, {} cached", cache.len());
                assert_eq!(new, old, "{what}");
                let json =
                    |window| serde_json::to_string(&with_window(&base, window)).expect("json");
                assert_eq!(json(&new), json(&old), "the request, {what}");
                assert_eq!(
                    breakdown::measure(&base, &new, &numbers),
                    breakdown::measure(&base, &old, &numbers),
                    "the breakdown, {what}"
                );
            }
        }
    }
}

/// A conversation written into a store batch by batch, with the sequence number of
/// each event, as the actor writes it.
struct Script {
    rng: TestRng,
    batches: Vec<Batch>,
    /// The sequence number of the next event; 1 is `conversation_created`.
    seq: u64,
    /// The turns, oldest first.
    turns: Vec<TurnId>,
    next_id: u64,
}

impl Script {
    fn new(seed: u64) -> Self {
        let created = Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None };
        Script {
            rng: TestRng::new(seed),
            batches: vec![Batch::new().event(conversation(), created)],
            seq: 2,
            turns: Vec::new(),
            next_id: 100,
        }
    }

    fn below(&self, n: u64) -> u64 {
        self.rng.next_u64() % n
    }

    fn chance(&self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn turn_id(&mut self) -> TurnId {
        let seed = self.id();
        TurnId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(seed)))
    }

    fn call_id(&mut self) -> CallId {
        let seed = self.id();
        CallId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(seed)))
    }

    /// Adds `event` to `events` and gives its sequence number.
    fn push(&mut self, events: &mut Vec<Event>, event: Event) -> Seq {
        events.push(event);
        self.seq += 1;
        Seq::new(self.seq - 1)
    }

    fn append(&mut self, events: Vec<Event>, saved: Option<NewTurnMessages>) {
        let batch = events.into_iter().fold(Batch::new(), |b, e| b.event(conversation(), e));
        self.batches.push(match saved {
            Some(saved) => batch.turn_messages(saved),
            None => batch,
        });
    }

    fn queued(&mut self, events: &mut Vec<Event>, t: TurnId, steers: Vec<Seq>) {
        let command_id = CommandId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(1000)));
        let queued = Event::PromptQueued {
            turn_id: t,
            command_id,
            text: format!("prompt {}", self.turns.len()),
            origin: Origin::Shell,
            context: None,
            settings: TurnSettings::default(),
            steers,
        };
        self.push(events, queued);
        let started = Event::TurnStarted {
            turn_id: t,
            cwd: PathBuf::from("/home/u"),
            scope: Scope::Machine,
            settings: None,
        };
        self.push(events, started);
    }

    /// A compaction through `through_turn`, run in `turn_id`.
    fn compaction(
        &mut self,
        turn_id: Option<TurnId>,
        through_turn: TurnId,
        through_message: Option<u32>,
        summary: bool,
    ) -> Event {
        let seed = self.id();
        Event::ConversationCompacted(Compaction {
            compaction_id: CompactionId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(seed))),
            turn_id,
            trigger: CompactionTrigger::Auto,
            focus: None,
            model: "m".to_owned(),
            window: 100_000,
            limit: 76_000,
            tokens_before: 80_000,
            tokens_after: 20_000,
            through_turn,
            through_message,
            kept_turns: 1,
            pruned_outputs: 0,
            pruned_tokens: 0,
            omitted_turns: u32::from(summary && self.chance(20)),
            omitted_messages: 0,
            fresh: summary.then(|| "the fresh block".to_owned()),
            summary: summary.then(|| format!("summary {seed}")),
            usage: None,
        })
    }

    /// A completed turn without saved messages, as after a compaction dropped them: a
    /// call with a long result, then an answer.
    fn plain_turn(&mut self, number: usize) -> TurnId {
        let t = self.turn_id();
        self.turns.push(t);
        let mut events = Vec::new();
        self.queued(&mut events, t, Vec::new());
        let c = self.call_id();
        self.push(&mut events, started(t, c, "shell"));
        self.push(&mut events, finished(t, c, &"x".repeat(400)));
        self.push(&mut events, completed(t, 0, &format!("answer {number}")));
        let end = Event::TurnCompleted {
            turn_id: t,
            usage: None,
            changes: None,
            context: None,
            stop: None,
            undo: None,
        };
        self.push(&mut events, end);
        self.append(events, None);
        t
    }

    /// A store with the conversation.
    async fn store(self) -> TestStore {
        let store = TestStore::open(TestClock::new().shared()).await.expect("store");
        for batch in self.batches {
            store.writer().append(batch).await.expect("append");
        }
        store
    }

    /// The cut of a random compaction: mostly a turn up to the newest, or a turn that
    /// the conversation does not have.
    fn cut(&mut self) -> (TurnId, Option<u32>) {
        let through = if self.turns.is_empty() || self.chance(10) {
            self.turn_id()
        } else {
            self.turns[self.below(self.turns.len() as u64) as usize]
        };
        let message = if self.chance(50) { None } else { Some(self.below(5) as u32) };
        (through, message)
    }
}

/// A random conversation of a few turns with random bodies and ends, saved messages
/// for some turns, steers that the model read and steers it never read, and
/// compactions: summaries with every kind of cut, prune-only ones older and newer than
/// the summary. Gives the store and the running turn, when one runs.
async fn random_conversation(seed: u64) -> (TestStore, Option<TurnId>) {
    let mut s = Script::new(seed);
    let count = 3 + s.below(8);
    let summary_after = s.below(count);
    let mut left: Vec<Seq> = Vec::new();
    for number in 0..count {
        let t = s.turn_id();
        s.turns.push(t);
        let mut events = Vec::new();
        let steers = std::mem::take(&mut left);
        s.queued(&mut events, t, steers);
        let mut index = 0;
        for _ in 0..s.below(5) {
            match s.below(7) {
                0 | 1 => {
                    let text = format!("answer {number}.{index}");
                    s.push(&mut events, completed(t, index, &text));
                    index += 1;
                }
                2 | 3 => {
                    let c = s.call_id();
                    s.push(&mut events, started(t, c, "shell"));
                    let output = if s.chance(50) { "x".repeat(400) } else { "ok".to_owned() };
                    s.push(&mut events, finished(t, c, &output));
                }
                4 => {
                    let c = s.call_id();
                    s.push(&mut events, started(t, c, "shell"));
                }
                5 => {
                    let steer = Event::TurnSteered { turn_id: t, text: format!("steer {number}") };
                    let seq = s.push(&mut events, steer);
                    if s.chance(50) {
                        s.push(
                            &mut events,
                            Event::SteeringDelivered { turn_id: t, steers: vec![seq] },
                        );
                    } else if s.chance(50) {
                        let withdrawn = Event::SteeringWithdrawn {
                            turn_id: t,
                            steers: vec![seq],
                            origin: Origin::Shell,
                        };
                        s.push(&mut events, withdrawn);
                    } else {
                        left.push(seq);
                    }
                }
                _ => {
                    let update = Event::AssistantMessageUpdated {
                        turn_id: t,
                        index,
                        offset: 0,
                        delta: format!("streamed {number}"),
                    };
                    s.push(&mut events, update);
                }
            }
        }
        // NOTE: an auto compaction runs inside a turn, a manual one between turns.
        let inside = s.chance(50);
        let mut summary = None;
        if number == summary_after {
            if s.chance(30) {
                let (through, message) = s.cut();
                let older = s.compaction(Some(t), through, message, false);
                s.push(&mut events, older);
            }
            let (through, message) = s.cut();
            summary = Some(s.compaction(inside.then_some(t), through, message, true));
        }
        if inside && let Some(compaction) = summary.take() {
            s.push(&mut events, compaction);
        }
        let end = match s.below(4) {
            0 => Event::TurnFailed {
                turn_id: t,
                error: ErrorBody::new(ErrorCode::Unauthorized, "no"),
                usage: None,
                context: None,
                changes: None,
                undo: None,
            },
            1 => Event::TurnInterrupted {
                turn_id: t,
                usage: None,
                context: None,
                changes: None,
                undo: None,
            },
            2 => Event::TurnCancelled { turn_id: t },
            _ => Event::TurnCompleted {
                turn_id: t,
                usage: None,
                changes: None,
                context: None,
                stop: None,
                undo: None,
            },
        };
        s.push(&mut events, end);
        let saved = s.chance(60).then(|| {
            let provider = if s.chance(70) { "replay" } else { "other" };
            let exact = [
                Message::user(format!("<live_state>prompt {number}</live_state>")),
                Message::assistant(format!("exact {number}"))
                    .with_provider_raw(json!([{ "id": format!("msg_{number}") }])),
            ];
            let exact =
                exact.iter().map(|message| serde_json::to_value(message).expect("json")).collect();
            NewTurnMessages::new(conversation(), t, provider, "m", exact)
        });
        s.append(events, saved);
        if let Some(compaction) = summary {
            s.append(vec![compaction], None);
        }
        if number > summary_after && s.chance(30) {
            let (through, message) = s.cut();
            let pruned = s.compaction(None, through, message, false);
            s.append(vec![pruned], None);
        }
    }
    let running = s.chance(30).then(|| {
        let t = s.turn_id();
        s.turns.push(t);
        let mut events = Vec::new();
        s.queued(&mut events, t, std::mem::take(&mut left));
        s.push(&mut events, completed(t, 0, "so far"));
        s.append(events, None);
        t
    });
    (s.store().await, running)
}

/// Random conversations, each checked as a turn starts and as the context breakdown
/// reads it: the skip changes nothing that the model or the user sees.
#[tokio::test]
async fn the_skip_changes_no_window_of_random_conversations() {
    let mut checked = Checked::default();
    for seed in 0..150 {
        let (store, running) = random_conversation(seed).await;
        assert_same(&store, running, None, Some("fresh"), &mut checked).await;
        assert_same(&store, None, running, None, &mut checked).await;
    }
    assert!(
        checked.fewer_events > 50,
        "the skip read fewer events in {} checks",
        checked.fewer_events
    );
}

/// Two turns without saved messages, as after a compaction dropped them, then the
/// compaction with a summary at `cut`, then a third turn.
async fn three_turns(cut: impl Fn(&[TurnId]) -> (TurnId, Option<u32>)) -> (TestStore, Vec<TurnId>) {
    let mut s = Script::new(7);
    for number in 0..3 {
        s.plain_turn(number);
        if number == 1 {
            let (through, message) = cut(&s.turns);
            let compaction = s.compaction(None, through, message, true);
            s.append(vec![compaction], None);
        }
    }
    let turns = s.turns.clone();
    (s.store().await, turns)
}

#[tokio::test]
async fn a_cut_inside_a_turn_keeps_the_rest_of_that_turn() {
    let (store, turns) = three_turns(|turns| (turns[1], Some(2))).await;
    let mut checked = Checked::default();

    assert_same(&store, None, None, Some("fresh"), &mut checked).await;

    assert!(checked.fewer_events > 0, "the first turn is never read");
    let limits = HistoryLimits::new(3, usize::MAX);
    let snapshot = Snapshot::read(store.readers(), conversation(), limits).await.expect("read");
    let read: Vec<TurnId> = snapshot.older.iter().filter_map(|e| e.event.turn_id()).collect();
    assert!(!read.contains(&turns[0]), "{read:?}");
    assert!(read.contains(&turns[1]), "the turn of the cut is read: {read:?}");
}

#[tokio::test]
async fn a_cut_after_a_whole_turn_skips_that_turn_too() {
    let (store, turns) = three_turns(|turns| (turns[1], None)).await;
    let mut checked = Checked::default();

    assert_same(&store, None, None, Some("fresh"), &mut checked).await;

    let limits = HistoryLimits::new(3, usize::MAX);
    let snapshot = Snapshot::read(store.readers(), conversation(), limits).await.expect("read");
    let read: Vec<TurnId> = snapshot.older.iter().filter_map(|e| e.event.turn_id()).collect();
    assert!(!read.contains(&turns[0]) && !read.contains(&turns[1]), "{read:?}");
    assert!(read.contains(&turns[2]), "the turn after the cut is read: {read:?}");
}

#[tokio::test]
async fn a_cut_at_a_turn_that_the_turns_do_not_know_skips_nothing() {
    let unknown = TurnId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(99_999)));
    let (store, turns) = three_turns(|_| (unknown, None)).await;
    let mut checked = Checked::default();

    assert_same(&store, None, None, Some("fresh"), &mut checked).await;

    assert_eq!(checked.fewer_events, 0, "every turn is read as before");
    let limits = HistoryLimits::new(3, usize::MAX);
    let snapshot = Snapshot::read(store.readers(), conversation(), limits).await.expect("read");
    let read: Vec<TurnId> = snapshot.older.iter().filter_map(|e| e.event.turn_id()).collect();
    assert!(turns.iter().all(|turn| read.contains(turn)), "{read:?}");
}

/// A prune-only compaction newer than the summary with a cut after it: the skip still
/// uses the summary's cut, never the newest compaction's.
#[tokio::test]
async fn a_newer_pruning_moves_no_skip() {
    let mut s = Script::new(8);
    for number in 0..4 {
        let t = s.plain_turn(number);
        if number == 1 {
            let through = s.turns[0];
            let compaction = s.compaction(None, through, None, true);
            s.append(vec![compaction], None);
        }
        if number == 3 {
            let pruned = s.compaction(None, t, Some(1), false);
            s.append(vec![pruned], None);
        }
    }
    let turns = s.turns.clone();
    let store = s.store().await;
    let mut checked = Checked::default();

    assert_same(&store, None, None, Some("fresh"), &mut checked).await;

    let limits = HistoryLimits::new(3, usize::MAX);
    let snapshot = Snapshot::read(store.readers(), conversation(), limits).await.expect("read");
    let read: Vec<TurnId> = snapshot.older.iter().filter_map(|e| e.event.turn_id()).collect();
    assert!(!read.contains(&turns[0]), "{read:?}");
    assert!(read.contains(&turns[1]) && read.contains(&turns[2]), "{read:?}");
    let window = snapshot.window(None, &HashMap::new(), &key("replay", "m"), limits, Some("fresh"));
    let stubbed = window.messages().iter().any(|message| {
        serde_json::to_string(message).expect("json").contains("efr removed this output")
    });
    assert!(stubbed, "the pruning still stubs the results before its cut");
}

/// The breakdown of `conversation.context` while a turn runs: the running turn counts
/// as ended, and the covered turns are left out of it as of the window.
#[tokio::test]
async fn the_context_breakdown_of_a_running_turn_stays_the_same() {
    let (store, turns) = three_turns(|turns| (turns[0], None)).await;
    let mut s = Script::new(9);
    s.next_id = 10_000;
    let t = s.turn_id();
    let mut events = Vec::new();
    s.queued(&mut events, t, Vec::new());
    events.push(completed(t, 0, "so far"));
    let batch = events.into_iter().fold(Batch::new(), |b, e| b.event(conversation(), e));
    store.writer().append(batch).await.expect("append");
    let mut checked = Checked::default();

    assert_same(&store, None, Some(t), Some("fresh"), &mut checked).await;

    assert!(checked.fewer_events > 0, "the first turn {} is never read", turns[0]);
}

/// The prompt of a turn that the [`Recorder`] answers with a read of `bytes` bytes.
fn read_prompt(home: &std::path::Path, bytes: usize) -> String {
    format!("read {}", home.join(format!("big-{bytes}")).display())
}

/// Real compactions of the actor: a manual one between turns (a cut after a whole
/// turn), an auto one inside a turn (a cut inside it), and a pruning newer than the
/// summary. After each turn the window is the same with and without the skip, and after
/// a restart the model gets the request of the window without the skip.
#[tokio::test]
async fn real_compactions_give_the_same_request_with_and_without_the_skip() {
    let setup = compacting();
    let home = setup.home().to_path_buf();
    let model = Recorder::new(3);
    let mut h = setup.start_model(model.clone()).await;
    let mut checked = Checked::default();
    let big = |title: &str| format!("{title}\n{}", "x".repeat(112_000));
    let prompts: Vec<String> = vec![
        "one".to_owned(),
        "two".to_owned(),
        big("three"),
        big("four"),
        "manual".to_owned(),
        "five".to_owned(),
        read_prompt(&home, 200_000),
        "six".to_owned(),
        read_prompt(&home, 80_000),
        read_prompt(&home, 80_000),
        read_prompt(&home, 80_000),
        "seven".to_owned(),
    ];
    for text in &prompts {
        if text == "manual" {
            let params = ConversationCompact {
                command_id: h.command_id(),
                conversation_id: h.conversation_id,
                focus: None,
            };
            h.handle.compact(params).await.expect("compacted");
        } else {
            let sent = h.prompt(text).await;
            let end = h.wait_end(sent.turn_id).await;
            assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
        }
        assert_same(&h.store, None, None, Some("fresh"), &mut checked).await;
    }
    let compacted = compactions(&h).await;
    let summaries: Vec<&Compaction> = compacted.iter().filter(|c| c.summary.is_some()).collect();
    assert!(summaries.iter().any(|c| c.through_message.is_none()), "{compacted:#?}");
    assert!(summaries.iter().any(|c| c.through_message.is_some()), "{compacted:#?}");
    let newest_summary = compacted.iter().rposition(|c| c.summary.is_some()).expect("a summary");
    assert!(
        compacted[newest_summary..].iter().any(|c| c.summary.is_none()),
        "a pruning newer than the summary: {compacted:#?}"
    );
    assert!(checked.fewer_events > 0, "the skip read fewer events");

    // NOTE: after a restart the cache is empty, so every turn comes from the store.
    let limits = h.settings.borrow().history.for_window(crate::testing::WINDOW);
    let kept = Snapshot::read_with(h.store.readers(), h.conversation_id, limits, Covered::Keep)
        .await
        .expect("read");
    let fresh = kept.summary().and_then(|compaction| compaction.fresh.clone());
    let key = ModelKey::new(ProviderId::new("replay").expect("id"), crate::testing::MODEL);
    let old =
        kept.window_with(None, &HashMap::new(), &key, limits, fresh.as_deref(), Covered::Keep);
    let expected = with_window(&request(Vec::new()), &old).messages;
    let mut h = h.restart_model(model.clone()).await;
    let sent = h.prompt("after the restart").await;
    h.wait_end(sent.turn_id).await;
    let requests: Vec<Request> = model.requests().into_iter().filter(|r| !r.side_call).collect();
    let after = requests.last().expect("the request after the restart");
    let sent = &after.messages[..after.messages.len() - 1];
    assert_eq!(
        serde_json::to_string(sent).expect("json"),
        serde_json::to_string(&expected).expect("json"),
        "the model gets the history of the window without the skip"
    );
    h.finish();
}
