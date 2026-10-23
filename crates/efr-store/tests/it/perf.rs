//! The `bench_perf_*` benches of the store on the perf corpus: the read of a
//! conversation's user messages (the plan's W1-B) and the latency of one append as the
//! log grows. Ignored; `just bench-perf` runs them in release. Each bench writes its
//! log in parts and measures after each part, so one build serves the whole ladder.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use efr_protocol::{CallId, CommandId, ConversationId, Event, Origin, Scope, TurnId, TurnSettings};
use efr_store::{Batch, Store, WAL_AUTOCHECKPOINT, user_messages};
use efr_test_support::bench::{self, Report};
use efr_test_support::{PerfCorpus, PerfShape, TestClock, open_store_file};
use tempfile::TempDir;

use crate::support::wal_frames_waiting;

/// The seed of the corpus.
const SEED: u64 = 42;

/// The log sizes of the user messages bench, in events.
const USER_LADDER: [u64; 3] = [10_000, 100_000, 1_000_000];

/// The log sizes of the append bench, in events.
const APPEND_LADDER: [u64; 3] = [1_000, 100_000, 1_000_000];

/// 20 conversations interleaved by turn, with turns enough for 1,000,000 events.
const SHAPE: PerfShape = PerfShape { conversations: 20, turns: 1_100, compacted_through: None };

/// The query of `user_messages::of_conversation`, for its plan.
// NOTE: a copy of the SQL in `src/user_messages.rs`; keep the two the same.
const USER_QUERY: &str = "SELECT seq, conversation_id, created_at, payload FROM events \
     WHERE conversation_id = ?1 AND kind IN (?2, ?3, ?4, ?5, ?6) ORDER BY seq";

/// A new dir below cargo's target temp dir, never `/tmp`.
fn temp_dir() -> TempDir {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("perf-store");
    std::fs::create_dir_all(&dir).unwrap();
    tempfile::tempdir_in(dir).unwrap()
}

/// A log that grows in parts: the corpus of [`SHAPE`] in a fresh store file.
struct Log {
    _dir: TempDir,
    path: PathBuf,
    clock: TestClock,
    store: Store,
    corpus: PerfCorpus,
    events: u64,
}

impl Log {
    async fn new() -> Log {
        let dir = temp_dir();
        let path = dir.path().join("efr.sqlite");
        let clock = TestClock::new();
        let store = open_store_file(&path, &clock).await.unwrap();
        Log { _dir: dir, path, clock, store, corpus: PerfCorpus::of_shape(SHAPE, SEED), events: 0 }
    }

    /// Writes whole rounds of turns (one turn of each conversation) until the log holds
    /// at least `events` events; the time it took.
    async fn grow_to(&mut self, events: u64) -> Duration {
        let started = Instant::now();
        while self.events < events {
            let written = self
                .corpus
                .write_turns(self.store.writer(), &self.clock, SHAPE.conversations.into())
                .await
                .unwrap();
            assert!(written.turns > 0, "the corpus ended before {events} events");
            self.events = written.last_seq.get();
        }
        started.elapsed()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_user_messages() {
    let mut report = Report::new("bench_perf_user_messages");
    let mut log = Log::new().await;
    let id = *log.corpus.conversation_ids().last().unwrap();
    let readers = log.store.readers().clone();
    let mut points = Vec::new();
    for target in USER_LADDER {
        let build = log.grow_to(target).await;
        let label = format!("{}k events", target / 1_000);
        report.time(&format!("{label}: build of the log"), &[build]);
        report.count(&format!("{label}: events in the log"), log.events);
        let read = || readers.with(move |conn| user_messages::of_conversation(conn, id));
        let found = read().await.unwrap().len();
        let samples = bench::run_async(2, 20, || async {
            read().await.unwrap();
        })
        .await;
        let stats = report.time(&format!("{label}: of_conversation"), &samples);
        report.count(&format!("{label}: user events of the conversation"), found as u64);
        points.push((log.events, stats.p50));
        if stats.p50 > Duration::from_secs(10) {
            report.count("stopped_at_n", log.events);
            break;
        }
    }
    report.slope("of_conversation over the events in the log", &points);
    let plan = readers
        .with(move |conn| {
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {USER_QUERY}"))?;
            let rows = stmt
                .query_map(
                    rusqlite::params![
                        id.to_string(),
                        "prompt_queued",
                        "turn_steered",
                        "prompt_withdrawn",
                        "steering_withdrawn",
                        "terminal_attached"
                    ],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(3)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap();
    // NOTE: the report has no text lines, so each line of the plan is a label, with
    // the id of its node as the count.
    for (node, detail) in &plan {
        report.count(&format!("query plan: {detail}"), u64::try_from(*node).unwrap_or(0));
    }
    drop(readers);
    log.store.close().await;
    report.finish();
}

/// A conversation of its own with one running turn and a running call, so a probe
/// batch of one output update lands in a real turn.
async fn probe(store: &Store, clock: &TestClock) -> (ConversationId, TurnId, CallId) {
    let id: ConversationId = "019a0000-0000-7000-8000-00000000c0de".parse().unwrap();
    let turn_id: TurnId = "019a0000-0000-7000-8000-00000000c0df".parse().unwrap();
    let command_id: CommandId = "019a0000-0000-7000-8000-00000000c0e0".parse().unwrap();
    let call_id: CallId = "019a0000-0000-7000-8000-00000000c0e1".parse().unwrap();
    clock.advance(Duration::from_secs(1));
    let batch = Batch::new()
        .event(id, Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None })
        .event(
            id,
            Event::PromptQueued {
                turn_id,
                command_id,
                text: "probe".to_owned(),
                origin: Origin::Shell,
                context: None,
                settings: TurnSettings::default(),
                steers: Vec::new(),
            },
        )
        .event(
            id,
            Event::TurnStarted {
                turn_id,
                cwd: PathBuf::from("/srv"),
                scope: Scope::Path(PathBuf::from("/srv")),
                settings: None,
            },
        )
        .event(
            id,
            Event::ToolCallStarted {
                turn_id,
                call_id,
                tool: "shell".to_owned(),
                input: serde_json::json!({ "command": "make" }),
                freeform: false,
                manual_input: false,
                launch: None,
            },
        );
    store.writer().append(batch).await.unwrap();
    (id, turn_id, call_id)
}

/// The batch of one output update of about 300 bytes, as the coalescer writes one.
fn update(id: ConversationId, turn_id: TurnId, call_id: CallId, n: u64) -> Batch {
    let tail =
        format!("{n:>8} compiling crate{} v0.1.0 (/srv/src/crate{})\n", n % 40, n % 40).repeat(5);
    Batch::new().event(id, Event::ToolCallOutputUpdated { turn_id, call_id, tail, bytes: n * 300 })
}

/// The time of each of `count` appends of one update.
async fn time_updates(
    store: &Store,
    clock: &TestClock,
    probe: (ConversationId, TurnId, CallId),
    from: u64,
    count: u64,
) -> Vec<Duration> {
    let (id, turn_id, call_id) = probe;
    let mut times = Vec::new();
    for n in from..from + count {
        clock.advance(Duration::from_millis(200));
        let batch = update(id, turn_id, call_id, n);
        let started = Instant::now();
        store.writer().append(batch).await.unwrap();
        times.push(started.elapsed());
    }
    times
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_store_append() {
    let mut report = Report::new("bench_perf_store_append");
    let mut log = Log::new().await;
    let probe = probe(&log.store, &log.clock).await;
    let mut n = 0;
    let mut updates = Vec::new();
    let mut turns = Vec::new();
    for target in APPEND_LADDER {
        let build = log.grow_to(target).await;
        let label = format!("{}k events", target / 1_000);
        report.time(&format!("{label}: build of the log"), &[build]);
        report.count(&format!("{label}: events in the log"), log.events);
        let times = time_updates(&log.store, &log.clock, probe, n, 200).await;
        n += 200;
        let stats = report.time(&format!("{label}: append of one output update"), &times);
        updates.push((log.events, stats.p50));
        // A turn's end: 48 events and the turn's messages in one batch.
        let mut times = Vec::new();
        for turn in log.corpus.by_ref().take(20) {
            log.clock.advance(Duration::from_secs(10));
            let batch = turn.batch();
            let started = Instant::now();
            let committed = log.store.writer().append(batch).await.unwrap();
            times.push(started.elapsed());
            log.events = committed.last_seq().get();
        }
        let stats = report.time(&format!("{label}: append of one turn's batch"), &times);
        turns.push((log.events, stats.p50));
    }
    report.slope("one update over the events in the log", &updates);
    report.slope("one turn's batch over the events in the log", &turns);

    // Near the passive checkpoint of the writer: updates until the write-ahead log
    // holds 90% of the frames that start one, then the appends that follow, apart by
    // whether the commit ran the checkpoint (the frames waiting went down).
    let near = WAL_AUTOCHECKPOINT * 9 / 10;
    let mut filled = 0;
    while wal_frames_waiting(&log.path) < near && filled < 100_000 {
        time_updates(&log.store, &log.clock, probe, n, 1).await;
        n += 1;
        filled += 1;
    }
    report.count("wal near the checkpoint: updates to fill it", filled);
    let (mut plain, mut checkpointed) = (Vec::new(), Vec::new());
    for _ in 0..300 {
        let before = wal_frames_waiting(&log.path);
        let time = time_updates(&log.store, &log.clock, probe, n, 1).await;
        n += 1;
        let after = wal_frames_waiting(&log.path);
        if after < before { &mut checkpointed } else { &mut plain }.extend(time);
    }
    let all: Vec<Duration> = plain.iter().chain(&checkpointed).copied().collect();
    report.time("wal near the checkpoint: one update", &all);
    report.time("wal near the checkpoint:   without a checkpoint", &plain);
    report.time("wal near the checkpoint:   with a checkpoint", &checkpointed);
    log.store.close().await;
    report.finish();
}
