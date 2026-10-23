//! The bench of a turn's start on the perf corpus: `Snapshot::read` and `window()`
//! (the plan's W1-A and W1-B). Ignored; `just bench-perf` runs it in release.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use efr_protocol::TurnId;
use efr_provider::ProviderId;
use efr_store::{events, turn_messages, user_messages};
use efr_test_support::bench::{self, Report};
use efr_test_support::{
    PERF_MODEL, PERF_PROVIDER, PerfCorpus, PerfSize, TestClock, open_store_file,
};

use crate::history::{CachedTurn, HistoryLimits, ModelKey, Snapshot, older_ranges, summary_cut};

/// The seed of the corpus.
const SEED: u64 = 42;

/// A size whose read takes longer than this ends the ladder.
const STOP_AFTER: Duration = Duration::from_secs(10);

/// The budget of `read_turn_range` calls in one read of any size. A read skips the
/// turns that the summary covers, and every turn after the cut saved its messages, so
/// it makes none; before the skip, L made 1,873.
const MAX_RANGE_CALLS: usize = 4;

/// The budget of saved message JSON that one read decodes, in bytes, when the actor's
/// cache holds every saved turn. The read decodes only the turns that the cache lacks,
/// so it decodes none; before, it decoded each saved turn at each turn's start.
const MAX_HELD_BYTES: u64 = 0;

/// The dir of the corpus files: cargo's target temp dir. A unit test gets no
/// `CARGO_TARGET_TMPDIR`, so the dir comes from the test binary, which cargo puts at
/// `<target>/<profile>/deps/<binary>`.
fn perf_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.ancestors().nth(3).unwrap().join("tmp/perf-corpus")
}

/// The runs of a size: fewer for the large ones, so the bench stays within about two
/// minutes.
fn runs(size: PerfSize) -> usize {
    match size {
        PerfSize::S | PerfSize::M => 20,
        PerfSize::L | PerfSize::Xl => 5,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_snapshot_read() {
    let mut report = Report::new("bench_perf_snapshot_read");
    let limits = HistoryLimits::default();
    let key = ModelKey::new(ProviderId::new(PERF_PROVIDER).unwrap(), PERF_MODEL);
    let cache = HashMap::new();
    let mut reads = Vec::new();
    let mut windows = Vec::new();
    let mut held_reads = Vec::new();
    let mut over_budget = Vec::new();
    for size in PerfSize::ALL {
        let name = size.name();
        let shape = size.shape();
        let started = Instant::now();
        let path = PerfCorpus::file(shape, SEED, &perf_dir()).await.unwrap();
        report
            .time(&format!("{name}: corpus file (built once, then cached)"), &[started.elapsed()]);
        let clock = TestClock::new();
        let store = open_store_file(&path, &clock).await.unwrap();
        let readers = store.readers().clone();
        // NOTE: the last conversation of XL has every other conversation's events
        // between its own, as a user's newest conversation does.
        let id = *PerfCorpus::of_shape(shape, SEED).conversation_ids().last().unwrap();
        let events_in_log = readers.with(events::last_seq).await.unwrap().get();
        report.count(&format!("{name}: events in the log"), events_in_log);

        let snapshot = Snapshot::read(&readers, id, limits, &cache).await.unwrap();
        let cut = summary_cut(&snapshot.compactions);
        let ranges = older_ranges(&snapshot.turns, &snapshot.saved, &snapshot.page, cut);
        let read = bench::run_async(1, runs(size), || async {
            black_box(Snapshot::read(&readers, id, limits, &cache).await.unwrap());
        })
        .await;
        let stats = report.time(&format!("{name}: Snapshot::read"), &read);
        report.count(&format!("{name}: read_turn_range calls per read"), ranges.len() as u64);
        if ranges.len() > MAX_RANGE_CALLS {
            over_budget.push(format!("{name}: {} read_turn_range calls", ranges.len()));
        }
        report.count(&format!("{name}: turns"), snapshot.turns.len() as u64);
        report.count(&format!("{name}: turns with saved messages"), snapshot.saved.len() as u64);
        report.count(&format!("{name}: events in the page"), snapshot.page.len() as u64);
        report.count(&format!("{name}: events of older turns"), snapshot.older.len() as u64);
        report.count(&format!("{name}: user events"), snapshot.user_events.len() as u64);
        reads.push((shape.turns.into(), stats.p50));

        // A turn's start with the actor's cache: it holds every saved turn, as after
        // the actor ran them, so the read decodes none of them.
        let full: HashMap<TurnId, Arc<CachedTurn>> =
            snapshot.saved.iter().map(|(turn_id, turn)| (*turn_id, Arc::clone(turn))).collect();
        let decoded = |held: HashSet<TurnId>| {
            let readers = readers.clone();
            async move {
                readers
                    .with(move |conn| turn_messages::of_conversation_except(conn, id, &held))
                    .await
                    .unwrap()
                    .bytes
            }
        };
        let cold = decoded(HashSet::new()).await;
        let warm = decoded(full.keys().copied().collect()).await;
        report.bytes(&format!("{name}: saved messages decoded per read, empty cache"), cold);
        report.bytes(&format!("{name}: saved messages decoded per read, full cache"), warm);
        if warm > MAX_HELD_BYTES {
            over_budget.push(format!("{name}: {warm} bytes decoded with the full cache"));
        }
        let held = bench::run_async(1, runs(size), || async {
            black_box(Snapshot::read(&readers, id, limits, &full).await.unwrap());
        })
        .await;
        let held_stats = report.time(&format!("{name}: Snapshot::read, full cache"), &held);
        held_reads.push((shape.turns.into(), held_stats.p50));

        // The parts of the read on their own: the range queries of the turns that saved
        // no messages, and the user messages of the whole log.
        let only_ranges = ranges.clone();
        let range_reads = bench::run_async(0, runs(size).min(5), || {
            let ranges = only_ranges.clone();
            let readers = readers.clone();
            async move {
                let read = readers
                    .with(move |conn| {
                        let mut older = BTreeMap::new();
                        for (from, through) in ranges {
                            let events = events::read_turn_range(conn, id, from, through)?;
                            older.extend(
                                events.into_iter().map(|envelope| (envelope.seq, envelope)),
                            );
                        }
                        Ok(older.len())
                    })
                    .await;
                black_box(read.unwrap());
            }
        })
        .await;
        report.time(&format!("{name}:   of it the read_turn_range calls"), &range_reads);
        let user_reads = bench::run_async(0, runs(size).min(5), || {
            let readers = readers.clone();
            async move {
                let read = readers.with(move |conn| user_messages::of_conversation(conn, id)).await;
                black_box(read.unwrap());
            }
        })
        .await;
        report.time(&format!("{name}:   of it user_messages::of_conversation"), &user_reads);

        let mut placed = 0;
        let window = bench::run(1, runs(size), || {
            let window = snapshot.window(None, &cache, &key, limits, Some("fresh"));
            placed = window.placed.len();
            black_box(window);
        });
        let window_stats = report.time(&format!("{name}: window()"), &window);
        report.count(&format!("{name}: messages in the window"), placed as u64);
        windows.push((shape.turns.into(), window_stats.p50));
        drop(readers);
        store.close().await;
        if stats.p50 > STOP_AFTER {
            report.count("stopped_at_turns", shape.turns.into());
            break;
        }
    }
    // NOTE: S, M and L grow the turns of one conversation; XL grows the log around it.
    report.slope("Snapshot::read over the turns, s to l", &reads[..reads.len().min(3)]);
    report.slope("window() over the turns, s to l", &windows[..windows.len().min(3)]);
    report.slope(
        "Snapshot::read with the full cache over the turns, s to l",
        &held_reads[..held_reads.len().min(3)],
    );
    report.finish();
    assert!(
        over_budget.is_empty(),
        "over {MAX_RANGE_CALLS} range calls or {MAX_HELD_BYTES} bytes per read: {over_budget:?}"
    );
}
