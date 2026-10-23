//! `bench_perf_daemon_memory` and `bench_perf_daemon_growth`: the resident memory of
//! the daemon. The daemon of a `TestDaemon` runs in this test's process, so the numbers
//! are those of this process (`VmRSS` and `VmHWM` from `/proc/self/status`): they hold
//! the test's client and its runtime too, which a bench starts before the first line,
//! so the differences between the lines are the daemon's.
//!
//! Each turn reads a file with `read_file` and answers. The memory bench reads a file of
//! 1 MiB, which `read_file` cuts to its output limit of 32 KiB, so the history of each
//! turn holds 32 KiB of tool output; the model's context window makes the longer
//! conversations compact. The growth bench reads a file of 4 KiB.
//!
//! A conversation actor that is idle for 10 minutes drops the turns that it keeps in
//! memory (`efr_conversation`, the idle trim). The memory bench counts the dropped turns
//! from the actor's debug event ([`Trims`]) and fails when the idle period drops fewer
//! than [`TRIM_BUDGET`] of the turns of the load. It does not set a budget on the
//! resident set: glibc keeps most of the freed heap in its arenas, so `VmRSS` falls
//! by a few MiB only. The growth bench moves the clock past the idle trim after each
//! batch, so the slope is the memory that a conversation keeps after its trim (the
//! next batches use the freed heap again), and fails when it passes [`GROWTH_BUDGET`].
//!
//! The memory bench also counts the saved messages that each turn's start decodes, from
//! the history's debug event ([`Trims`] counts it too). The actor's cache holds every
//! earlier turn of the load, so a start decodes none of them; it fails when the mean
//! passes [`DECODE_BUDGET`].

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use efr_provider::Provider;
use efr_stdx::time::{Clock as _, SystemClock};
use efr_test_daemon::TestDaemon;
use efr_test_support::TestClock;
use efr_test_support::bench::{self, Memory, Report};
use serde_json::json;
use tracing::field::{Field, Visit};
use tracing::subscriber::Interest;
use tracing::{Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};

use super::{Follower, PerfModel, Plan, builder, prompt, run_turn, tree};

/// The idle period that the memory bench moves the clock by: past the idle trim of the
/// conversation actors (10 minutes), the idle collector of the hidden shells (60
/// minutes) and the debounce of the indexer.
const IDLE: Duration = Duration::from_secs(3 * 3600);

/// The idle period that the growth bench moves the clock by after each batch: past the
/// idle trim of the conversation actors (10 minutes).
const TRIM: Duration = Duration::from_secs(11 * 60);

/// The part of the turns of the load that the idle period must drop from the actors'
/// memory, in percent. Each actor drops every turn that it keeps, so the fixed code
/// drops all of them; before the idle trim it dropped none.
const TRIM_BUDGET: u64 = 90;

/// The most resident memory that one conversation of the growth bench may keep after
/// its idle trim, in bytes (46 KiB, about 0.045 MiB). Without the trim the growth was
/// 0.06 to 0.076 MiB for each conversation; with it, 0.024 to 0.029 MiB.
const GROWTH_BUDGET: u64 = 46 * 1024;

/// The most bytes of saved message JSON that a turn's start of the memory bench may
/// decode on average (64 KiB). The actor's cache holds every earlier turn, so the fixed
/// code decodes none; before, each start decoded every saved turn of its conversation,
/// about 0.8 MiB on average.
const DECODE_BUDGET: u64 = 64 * 1024;

/// The longest real time that a bench waits for the trims after it moved the clock.
const TRIM_WAIT: Duration = Duration::from_secs(10);

/// The real time that the bench waits after it moved the clock, so the timers run.
const SETTLE: Duration = Duration::from_secs(3);

/// The memory of this process now.
fn memory() -> Memory {
    Memory::of_self().expect("/proc/self/status")
}

/// The trims of the conversation actors of this process: how many there were, and the
/// turns that they dropped; and the snapshot reads: how many there were, and the bytes
/// of saved messages that they decoded.
#[derive(Debug, Default)]
struct Trims {
    trims: AtomicU64,
    turns: AtomicU64,
    reads: AtomicU64,
    decoded: AtomicU64,
}

/// The target of the actor's debug event of a trim, which names `turns`.
const TRIM_TARGET: &str = "efr_conversation::actor";

/// The target of the history's debug event of a snapshot read, which names `held` and
/// `bytes`.
const READ_TARGET: &str = "efr_conversation::history";

/// True for the actor's debug event of a trim.
fn is_trim(metadata: &Metadata<'_>) -> bool {
    metadata.is_event()
        && metadata.target() == TRIM_TARGET
        && metadata.fields().field("turns").is_some()
}

/// True for the history's debug event of a snapshot read.
fn is_read(metadata: &Metadata<'_>) -> bool {
    metadata.is_event()
        && metadata.target() == READ_TARGET
        && metadata.fields().field("held").is_some()
}

/// A layer that only counts the trims and the snapshot reads; every other span and
/// event is off, so the benches measure no logging.
struct TrimLayer(&'static Trims);

impl<S: Subscriber> Layer<S> for TrimLayer {
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        if is_trim(metadata) || is_read(metadata) { Interest::always() } else { Interest::never() }
    }

    fn enabled(&self, metadata: &Metadata<'_>, _: Context<'_, S>) -> bool {
        is_trim(metadata) || is_read(metadata)
    }

    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if is_read(event.metadata()) {
            let mut bytes = U64Field("bytes", 0);
            event.record(&mut bytes);
            self.0.reads.fetch_add(1, Ordering::SeqCst);
            self.0.decoded.fetch_add(bytes.1, Ordering::SeqCst);
            return;
        }
        let mut turns = U64Field("turns", 0);
        event.record(&mut turns);
        self.0.trims.fetch_add(1, Ordering::SeqCst);
        self.0.turns.fetch_add(turns.1, Ordering::SeqCst);
    }
}

/// The field of an event that the first value names.
struct U64Field(&'static str, u64);

impl Visit for U64Field {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == self.0 {
            self.1 = value;
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

/// The counts of the trims of this process, from the first call on. The layer is the
/// global subscriber: the actors run on the runtime's workers. nextest runs each bench
/// in a process of its own; under cargo test a subscriber that another test set first
/// stays, the counts stay zero, and the budget fails.
fn trims() -> &'static Trims {
    static TRIMS: OnceLock<&'static Trims> = OnceLock::new();
    TRIMS.get_or_init(|| {
        let trims: &'static Trims = Box::leak(Box::default());
        let subscriber = tracing_subscriber::registry().with(TrimLayer(trims));
        let _ = tracing::subscriber::set_global_default(subscriber);
        trims
    })
}

impl Trims {
    fn trims(&self) -> u64 {
        self.trims.load(Ordering::SeqCst)
    }

    fn turns(&self) -> u64 {
        self.turns.load(Ordering::SeqCst)
    }

    fn reads(&self) -> u64 {
        self.reads.load(Ordering::SeqCst)
    }

    fn decoded(&self) -> u64 {
        self.decoded.load(Ordering::SeqCst)
    }
}

/// Moves `clock` by `by`, then by [`TRIM`] again until the actors trimmed `count` times
/// in all: an actor whose turn ended just before a move starts its idle period after
/// it. False when that takes more than [`TRIM_WAIT`] of real time.
async fn idle_trim(clock: &TestClock, by: Duration, trims: &Trims, count: u64) -> bool {
    let deadline = Instant::now() + TRIM_WAIT;
    clock.advance(by);
    loop {
        let round = Instant::now() + Duration::from_millis(500);
        while trims.trims() < count && Instant::now() < round {
            SystemClock.sleep(Duration::from_millis(5)).await;
        }
        if trims.trims() >= count {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        clock.advance(TRIM);
    }
}

/// `turns` turns in the conversation of the terminal `tty`, each one read through
/// `client`; stops early at `deadline`. Gives the turns that ran.
async fn turns(
    daemon: &TestDaemon,
    tty: &str,
    cwd: &Path,
    turns: usize,
    deadline: Instant,
) -> usize {
    let client = daemon.client_for_tty(tty).await.unwrap();
    for n in 0..turns {
        if Instant::now() > deadline {
            return n;
        }
        let run = run_turn(&client, prompt(tty, cwd, "read the file", None)).await;
        assert_eq!(run.end_kind(), "turn_completed", "{:#?}", run.events);
    }
    turns
}

/// A daemon whose model reads `file` of `bytes` in each turn, on a new tree.
async fn reading_daemon(bytes: usize) -> (TestDaemon, TestClock, Follower, std::path::PathBuf) {
    let dirs = tree();
    let cwd = dirs.create_dir("home/project").unwrap();
    std::fs::write(
        cwd.join("file.txt"),
        "a line of the file that the model reads\n".repeat(bytes / 40),
    )
    .unwrap();
    let clock = TestClock::new();
    let follower = Follower::start(&clock);
    let model =
        PerfModel::new(Plan::call("read_file", json!({ "path": "file.txt" }), "I read it."));
    let daemon = Box::pin(
        builder(&dirs, &clock)
            .custom_provider(Arc::clone(&model) as Arc<dyn Provider>)
            .config(|config| config.shell.idle_minutes = 60)
            .start(),
    )
    .await
    .unwrap();
    (daemon, clock, follower, cwd)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_daemon_memory() {
    const CONVERSATIONS: usize = 20;
    const TURNS: usize = 50;
    let mut report = Report::new("bench_perf_daemon_memory");
    let trims = trims();
    let (daemon, clock, _follower, cwd) = reading_daemon(1024 * 1024).await;
    // NOTE: one turn first, so the client's side and the daemon's first-use costs are
    // in the line after the start.
    let first = turns(&daemon, "/dev/pts/perf-memory-first", &cwd, 1, Instant::now() + IDLE).await;
    assert_eq!(first, 1);
    bench::reset_peak();
    let (reads_before, decoded_before) = (trims.reads(), trims.decoded());
    let start = memory();
    report.bytes("after start: rss", start.rss);
    let begin = Instant::now();
    let deadline = begin + Duration::from_secs(90);
    let ran = futures::future::join_all((0..CONVERSATIONS).map(|n| {
        let tty = format!("/dev/pts/perf-memory-{n}");
        let cwd = cwd.clone();
        let daemon = &daemon;
        async move { turns(daemon, &tty, &cwd, TURNS, deadline).await }
    }))
    .await;
    let done: usize = ran.iter().sum();
    report.count("turns of the load", done as u64);
    let reads = trims.reads() - reads_before;
    let decoded = trims.decoded() - decoded_before;
    let per_start = decoded / reads.max(1);
    report.count("snapshot reads of the load", reads);
    report.bytes("saved messages decoded per turn start (mean)", per_start);
    if done < CONVERSATIONS * TURNS {
        report.count("stopped_at_n (turns, 90 s)", done as u64);
    }
    report.time("load", &[begin.elapsed()]);
    let loaded = memory();
    report.bytes("after the load: rss", loaded.rss);
    report.bytes("after the load: peak (hwm)", loaded.peak);
    report.bytes("the load added: rss", loaded.rss.saturating_sub(start.rss));
    let before = trims.turns();
    idle_trim(&clock, IDLE, trims, CONVERSATIONS as u64 + 1).await;
    SystemClock.sleep(SETTLE).await;
    let idle = memory();
    report.bytes("after the idle period: rss", idle.rss);
    report.bytes("the idle period freed: rss", loaded.rss.saturating_sub(idle.rss));
    let dropped = trims.turns() - before;
    report.count("the idle period dropped: cached turns", dropped);
    daemon.stop().await.unwrap();
    report.finish();
    assert!(
        dropped * 100 >= done as u64 * TRIM_BUDGET,
        "the idle period dropped {dropped} cached turns of {done}, under {TRIM_BUDGET}%"
    );
    assert!(reads >= done as u64, "{reads} snapshot reads for {done} turns");
    assert!(
        per_start <= DECODE_BUDGET,
        "a turn's start decoded {per_start} bytes of saved messages, over {DECODE_BUDGET}"
    );
}

/// The least squares slope of `points`, in bytes per conversation.
fn bytes_per(points: &[(u64, u64)]) -> u64 {
    let count = points.len() as f64;
    let mean_x = points.iter().map(|(x, _)| *x as f64).sum::<f64>() / count;
    let mean_y = points.iter().map(|(_, y)| *y as f64).sum::<f64>() / count;
    let covariance: f64 =
        points.iter().map(|(x, y)| (*x as f64 - mean_x) * (*y as f64 - mean_y)).sum();
    let variance: f64 = points.iter().map(|(x, _)| (*x as f64 - mean_x).powi(2)).sum();
    if variance == 0.0 { 0 } else { (covariance / variance).max(0.0) as u64 }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_daemon_growth() {
    const CONVERSATIONS: usize = 300;
    const BATCH: usize = 50;
    const TURNS: usize = 5;
    /// The conversations that run at one time.
    const AT_ONCE: usize = 10;
    let mut report = Report::new("bench_perf_daemon_growth");
    let trims = trims();
    let (daemon, clock, _follower, cwd) = reading_daemon(4 * 1024).await;
    let first = turns(&daemon, "/dev/pts/perf-growth-first", &cwd, 1, Instant::now() + IDLE).await;
    assert_eq!(first, 1);
    let start = memory();
    report.bytes("after start: rss", start.rss);
    let mut points = vec![(0, start.rss)];
    let mut times = Vec::new();
    let mut opened = 0;
    while opened < CONVERSATIONS {
        let begin = Instant::now();
        let deadline = begin + Duration::from_secs(60);
        for chunk in (opened..opened + BATCH).collect::<Vec<_>>().chunks(AT_ONCE) {
            futures::future::join_all(chunk.iter().map(|n| {
                let tty = format!("/dev/pts/perf-growth-{n}");
                let cwd = cwd.clone();
                let daemon = &daemon;
                async move { turns(daemon, &tty, &cwd, TURNS, deadline).await }
            }))
            .await;
        }
        opened += BATCH;
        let time = begin.elapsed();
        times.push(time);
        // NOTE: every actor of the batches so far, and the one of the first turn, trims
        // once; an actor that trimmed before keeps nothing to trim.
        let trimmed = idle_trim(&clock, TRIM, trims, opened as u64 + 1).await;
        assert!(trimmed, "{} trims after {opened} conversations", trims.trims());
        let now = memory();
        points.push((opened as u64, now.rss));
        if [50, 150, 300].contains(&opened) {
            report.bytes(&format!("after {opened} conversations: rss"), now.rss);
        }
        if time > Duration::from_secs(60) {
            report.count("stopped_at_n (conversations)", opened as u64);
            break;
        }
    }
    report.time(&format!("{BATCH} conversations x {TURNS} turns"), &times);
    let growth = bytes_per(&points[1..]);
    report.bytes("rss growth per conversation (fit)", growth);
    report.count("peak (hwm) in MiB", memory().peak / (1024 * 1024));
    clock.advance(IDLE);
    SystemClock.sleep(SETTLE).await;
    report.bytes("after the idle period: rss", memory().rss);
    daemon.stop().await.unwrap();
    report.finish();
    assert!(
        growth <= GROWTH_BUDGET,
        "each conversation keeps {growth} bytes after its idle trim, over {GROWTH_BUDGET}"
    );
}
