//! `bench_perf_subscribe_snapshot`: `conversation.subscribe` at the start (no
//! `after_seq`), until the client holds the snapshot. The start reads the newest 129
//! events and keeps the newest that fit in 128 events and 1 MiB. Each conversation has
//! more than 129 events, from real turns whose answers are 1 KB, 32 KB or 500 KB of
//! text (an answer gives two events of its size: its update and its completion). One
//! more case adds an answer of 1.5 MiB as the newest events of the 32 KB conversation.
//! The snapshot leaves that answer out, and its history cursor reaches it.
//!
//! The budgets: the start keeps the newest events in one pass from the newest back, so
//! it measures an event at most once and stops at the bound. Before, it removed the
//! oldest event and measured the rest again until they fit: 343 ms for the 500 KB
//! events and 108 ms with the 1.5 MiB answer.

use std::sync::Arc;
use std::time::{Duration, Instant};

use efr_protocol::{
    ConversationHistory, ConversationHistoryResult, ConversationId, ConversationSubscribe,
    ConversationSubscribeItem, Method, PageCursor,
};
use efr_provider::Provider;
use efr_test_daemon::Client;
use efr_test_support::TestClock;
use efr_test_support::bench::Report;
use futures::StreamExt as _;

use super::{Follower, PerfModel, Plan, builder, prompt, run_turn, tree};

/// The sizes of the answers.
const SIZES: [(&str, usize); 3] = [("1 KB", 1024), ("32 KB", 32 * 1024), ("500 KB", 500 * 1024)];

/// The events of each conversation, at least: more than the 129 that the start reads.
const EVENTS: usize = 140;

/// The measured subscriptions of each case, after [`WARMUP`] that are not.
const SAMPLES: usize = 15;

/// The subscriptions before the measured ones.
const WARMUP: usize = 2;

/// The most time the start of the 500 KB conversation may take (about 50 ms on a busy
/// machine, 343 ms before the one pass).
const BUDGET_500_KB: Duration = Duration::from_millis(150);

/// The most time the start with the newest answer of 1.5 MiB may take (about 6 ms on
/// a busy machine, 108 ms before the one pass).
const BUDGET_NEWEST: Duration = Duration::from_millis(36);

/// An answer of `bytes` of text.
fn answer(bytes: usize) -> String {
    let words = "the build passed and the tests passed, so the change is ready. ";
    words.repeat(bytes / words.len() + 1)[..bytes].to_owned()
}

/// What the subscriptions of one case measured.
#[derive(Debug)]
struct Start {
    times: Vec<Duration>,
    events: u64,
    bytes: u64,
    cursor: Option<PageCursor>,
}

/// Subscribes at the start of `conversation_id` `WARMUP + SAMPLES` times.
async fn starts(client: &Client, conversation_id: ConversationId) -> Start {
    let mut start = Start { times: Vec::new(), events: 0, bytes: 0, cursor: None };
    for n in 0..WARMUP + SAMPLES {
        let params = ConversationSubscribe {
            conversation_id,
            after_seq: None,
            answers_input: false,
            drafts: false,
        };
        let begin = Instant::now();
        let mut stream = client.stream(Method::ConversationSubscribe(params)).await.unwrap();
        let first = stream.next().await.unwrap().unwrap();
        let time = begin.elapsed();
        let ConversationSubscribeItem::Snapshot(snapshot) = first else {
            panic!("a subscription without after_seq starts with a snapshot: {first:?}");
        };
        drop(stream);
        if n < WARMUP {
            continue;
        }
        start.times.push(time);
        start.events = snapshot.events.len() as u64;
        start.bytes = serde_json::to_string(&snapshot.events).unwrap().len() as u64;
        start.cursor = snapshot.history_cursor;
    }
    start
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_subscribe_snapshot() {
    let dirs = tree();
    let cwd = dirs.create_dir("home/project").unwrap();
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let model = PerfModel::new(Plan::answer("ok"));
    let daemon = Box::pin(
        builder(&dirs, &clock).custom_provider(Arc::clone(&model) as Arc<dyn Provider>).start(),
    )
    .await
    .unwrap();
    let mut report = Report::new("bench_perf_subscribe_snapshot");
    let mut points = Vec::new();
    let mut middle = None;
    let mut p50_500_kb = Duration::ZERO;
    for (label, bytes) in SIZES {
        let tty = format!("/dev/pts/perf-subscribe-{bytes}");
        let client = daemon.client_for_tty(&tty).await.unwrap();
        model.set_plan(Plan::answer(answer(bytes)));
        let (mut events, mut turns, mut conversation) = (0, 0, None);
        while events < EVENTS {
            let run = run_turn(&client, prompt(&tty, &cwd, "report", None)).await;
            assert_eq!(run.end_kind(), "turn_completed");
            // NOTE: the subscription of a turn starts after its `prompt_queued`.
            events += run.events.len() + 1;
            turns += 1;
            conversation = Some(run.sent.conversation_id);
        }
        let conversation = conversation.unwrap();
        let start = starts(&client, conversation).await;
        let stats = report.time(&format!("{label} events: subscribe->snapshot"), &start.times);
        report.count(&format!("{label} events: turns"), turns);
        report.count(&format!("{label} events: events in the snapshot"), start.events);
        report.bytes(&format!("{label} events: snapshot events"), start.bytes);
        points.push((bytes as u64, stats.p50));
        if bytes == 32 * 1024 {
            middle = Some((client, tty, conversation));
        }
        if bytes == 500 * 1024 {
            p50_500_kb = stats.p50;
        }
    }
    report.slope("subscribe over event size", &points);
    let (client, tty, conversation) = middle.unwrap();
    model.set_plan(Plan::answer(answer(1536 * 1024)));
    let run = run_turn(&client, prompt(&tty, &cwd, "report", None)).await;
    assert_eq!(run.end_kind(), "turn_completed");
    let start = starts(&client, conversation).await;
    let newest = report.time("newest 1.5 MiB: subscribe->snapshot", &start.times);
    report.count("newest 1.5 MiB: events in the snapshot", start.events);
    report.bytes("newest 1.5 MiB: snapshot events", start.bytes);
    // The answer that the snapshot left out is on the first page before its cursor.
    let page: ConversationHistoryResult = client
        .call(Method::ConversationHistory(ConversationHistory {
            conversation_id: conversation,
            cursor: start.cursor,
            limit: Some(4),
        }))
        .await
        .unwrap();
    let largest = page
        .events
        .iter()
        .map(|envelope| serde_json::to_vec(envelope).unwrap().len())
        .max()
        .unwrap_or(0);
    drop(client);
    daemon.stop().await.unwrap();
    report.finish();
    assert!(largest > 1536 * 1024, "the cursor reaches the answer: {largest} bytes");
    assert!(p50_500_kb < BUDGET_500_KB, "500 KB events: {p50_500_kb:?}");
    assert!(newest.p50 < BUDGET_NEWEST, "newest 1.5 MiB: {:?}", newest.p50);
}
