//! The search indexer over a whole efrd and over a store: the index lag of a turn, the
//! catch-up after a restart, the stop, the proptest of the secrets, and the benches of
//! the indexer (`just bench-search` runs the `bench_search_*` tests in release).
//!
//! These tests read the index tables themselves, through the store's readers. The
//! tests of the search methods (`search.rs`) find the same texts through
//! `conversations.search` and `conversation.search`, and repeat the proptest of the
//! secrets there.

use std::fs;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use efr_daemon::{SearchIndexStatus, follow_search_index, search_index_text};
use efr_protocol::{
    CallId, CommandId, ConversationId, Event, Origin, PromptSendResult, Scope, Seq, ShellContext,
    TurnId, TurnSettings,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_stdx::rng::Rng as _;
use efr_stdx::time::{Clock, SystemClock};
use efr_store::search::DocKind;
use efr_store::{Batch, Migrations, Readers, Store, StoreConfig};
use efr_test_daemon::{Replay, TTY, TestDaemon, events_until};
use efr_test_support::{CORPUS_VERSION, CorpusSize, SearchCorpus, TestClock, TestRng};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// The debounce of the indexer after a turn's end.
const DEBOUNCE: Duration = Duration::from_millis(500);

/// Every text of the index: the docs, then the titles.
async fn index_texts(readers: &Readers) -> Vec<String> {
    readers
        .with(|conn| {
            let mut texts = Vec::new();
            for sql in [
                "SELECT text FROM search_docs ORDER BY seq",
                "SELECT title FROM search_titles ORDER BY conversation_id",
            ] {
                let mut stmt = conn.prepare(sql)?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                for row in rows {
                    texts.push(row?);
                }
            }
            Ok(texts)
        })
        .await
        .unwrap()
}

/// The database file of a persistent test daemon.
fn database(daemon: &TestDaemon) -> PathBuf {
    daemon.dirs().dirs().data().join(efr_store::DATABASE_FILE)
}

#[tokio::test]
async fn turn_is_searchable_after_its_end() {
    let replay = Replay::run("tool_call_shell_ok").await.unwrap();
    let daemon = replay.daemon();
    let events = replay.events().await.unwrap();
    let first = events.first().unwrap().seq;
    let end = events
        .iter()
        .rev()
        .find(|envelope| matches!(envelope.event, Event::TurnCompleted { .. }))
        .unwrap()
        .seq;
    let clock = daemon.clock().clone();
    let mut status = daemon.search_index();
    let waiting = *status
        .wait_for(|status| status.sleeping_until.is_some() || status.indexed_seq >= end)
        .await
        .unwrap();
    // No doc before the turn's end and the debounce after it.
    assert!(waiting.indexed_seq < first, "{waiting:?}");
    let until = waiting.sleeping_until.unwrap();
    assert_eq!(Duration::try_from(until.duration_since(clock.now())).unwrap(), DEBOUNCE);
    clock.advance(DEBOUNCE - Duration::from_millis(1));
    assert!(daemon.search_index().borrow().indexed_seq < first);
    clock.advance(Duration::from_millis(1));
    let indexed = daemon.wait_indexed(end).await.unwrap();
    assert_eq!(indexed.pages, 1);
    replay.stop().await.unwrap();
}

/// A model that answers every request with the same words.
#[derive(Debug)]
struct OneAnswer {
    id: ProviderId,
}

#[async_trait]
impl Provider for OneAnswer {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let events = vec![
            Ok(ProviderEvent::TextDelta { text: "noted zebrafinch".to_owned() }),
            Ok(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None }),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

/// A turn of `text` on a new daemon; the daemon and the seq of the turn's end, with
/// the indexer in the debounce after it.
async fn one_turn(text: &str) -> (TestDaemon, Seq) {
    let provider = Arc::new(OneAnswer { id: ProviderId::new("test").unwrap() });
    let daemon =
        TestDaemon::builder().persistent().custom_provider(provider).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult = client.call(daemon.prompt(1, text, TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let events = events_until(&mut follow, |event| matches!(event, Event::TurnCompleted { .. }))
        .await
        .unwrap();
    let end = events.last().unwrap().seq;
    drop((follow, client));
    let mut status = daemon.search_index();
    let waiting = *status.wait_for(|status| status.sleeping_until.is_some()).await.unwrap();
    assert!(waiting.indexed_seq < end);
    (daemon, end)
}

/// A daemon that stops between a turn's commit and the round after it indexes the turn
/// at its next start, with no debounce.
#[tokio::test]
async fn catch_up_after_a_restart() {
    let (mut daemon, end) = one_turn("zebracorn after the restart").await;
    daemon.restart().await.unwrap();
    let indexed = daemon.wait_indexed(end).await.unwrap();
    assert_eq!(indexed.sleeping_until, None);
    let readers = Readers::open(database(&daemon), 1);
    let texts = index_texts(&readers).await;
    assert!(texts.contains(&"zebracorn after the restart".to_owned()), "{texts:?}");
    assert!(texts.contains(&"noted zebrafinch".to_owned()), "{texts:?}");
    daemon.stop().await.unwrap();
}

/// efrd's stop does not wait for the indexer's debounce or pause.
#[tokio::test]
async fn indexer_stops_with_the_daemon() {
    let (daemon, _) = one_turn("a turn before the stop").await;
    daemon.stop().await.unwrap();
}

/// The starts of keys that the redaction knows, each with the characters of its body
/// and the fewest of them (`preamble/secrets.rs`, `KEY_PREFIXES`).
const KEY_FORMS: &[(&str, &str, usize)] = &[
    ("sk-ant-", NAME, 16),
    ("sk-proj-", NAME, 16),
    ("github_pat_", NAME, 16),
    ("glpat-", NAME, 16),
    ("pypi-", NAME, 16),
    ("xoxb-", NAME, 16),
    ("xoxp-", NAME, 16),
    ("sk-", NAME, 16),
    ("ghp_", NAME, 16),
    ("gho_", NAME, 16),
    ("ghu_", NAME, 16),
    ("ghs_", NAME, 16),
    ("ghr_", NAME, 16),
    ("AKIA", UPPER, 16),
    ("AIza", NAME, 30),
    ("npm_", ALNUM, 30),
    ("hf_", ALNUM, 30),
];

const ALNUM: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const NAME: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const LOWER: &str = "abcdefghijklmnopqrstuvwxyz0123456789";

fn draw(rng: &TestRng, alphabet: &str, len: usize) -> String {
    let chars: Vec<char> = alphabet.chars().collect();
    (0..len).map(|_| chars[(rng.next_u64() % chars.len() as u64) as usize]).collect()
}

/// A text that holds one secret of form `form`, and the secret part that must never
/// be found. Forms past the keys are the config forms of the index pass.
pub(crate) fn secret_text(form: usize, seed: u64) -> (String, String) {
    let rng = TestRng::new(seed);
    if let Some((prefix, alphabet, min)) = KEY_FORMS.get(form) {
        // NOTE: a body that ends with `-` or `_` still ends at the blank after it.
        let body = draw(&rng, alphabet, min + (rng.next_u64() % 20) as usize);
        return (format!("my key is {prefix}{body} ok"), body);
    }
    let value = draw(&rng, LOWER, 12 + (rng.next_u64() % 20) as usize);
    let text = match form - KEY_FORMS.len() {
        0 => format!("config:\n  password: {value}\n  user: app"),
        1 => format!("[auth]\napi_key = \"{value}\"\nport = 5432"),
        2 => format!("{{\"user\": \"me\", \"client_secret\": \"{value}\"}}"),
        3 => format!("export GITHUB_TOKEN={value} && make"),
        _ => format!(
            "the key:\n-----BEGIN RSA PRIVATE KEY-----\n{value}\n{}\n-----END RSA PRIVATE KEY-----\ndone",
            draw(&rng, LOWER, 40)
        ),
    };
    (text, value)
}

/// Every form: the keys, then five config forms.
pub(crate) const FORMS: usize = KEY_FORMS.len() + 5;

/// An id of type `T` from a number.
fn id<T: std::str::FromStr>(n: u64) -> T
where
    T::Err: std::fmt::Debug,
{
    format!("0192f0c1-7a00-7000-8000-{n:012x}").parse().unwrap()
}

/// One turn whose prompt, steer, answer and command each hold `text`.
fn secret_turn(text: &str) -> Batch {
    let conversation: ConversationId = id(1);
    let turn_id: TurnId = id(2);
    let command_id: CommandId = id(3);
    let call_id: CallId = id(4);
    let events = vec![
        Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None },
        Event::PromptQueued {
            turn_id,
            command_id,
            text: text.to_owned(),
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
        Event::TurnSteered { turn_id, text: text.to_owned() },
        Event::AssistantMessageCompleted { turn_id, index: 0, text: text.to_owned() },
        Event::ToolCallStarted {
            turn_id,
            call_id,
            tool: "shell".to_owned(),
            input: json!({ "command": text }),
            freeform: false,
            manual_input: false,
            launch: None,
        },
        Event::TurnCompleted {
            turn_id,
            usage: None,
            context: None,
            changes: None,
            stop: None,
            undo: None,
        },
    ];
    events.into_iter().fold(Batch::new(), |batch, event| batch.event(conversation, event))
}

/// The index of a store that holds `batch`, built by the indexer.
async fn indexed(batch: Batch) -> Vec<String> {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.shared()).await.unwrap();
    store.writer().append(batch).await.unwrap();
    let newest = store.readers().with(efr_store::events::last_seq).await.unwrap();
    let (sender, mut status) = watch::channel(SearchIndexStatus::default());
    let stop = CancellationToken::new();
    let task = tokio::spawn(follow_search_index(
        store.writer().clone(),
        store.index_reader().clone(),
        clock.shared(),
        sender,
        stop.clone(),
    ));
    status.wait_for(|status| status.idle && status.indexed_seq >= newest).await.unwrap();
    let texts = index_texts(store.readers()).await;
    stop.cancel();
    task.await.unwrap();
    store.close().await;
    texts
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// No 8 characters of a secret that the index redaction recognises reach a doc or
    /// a title, in a prompt, a steer, an answer or a command.
    #[test]
    fn secret_is_never_found(form in 0..FORMS, seed in any::<u64>()) {
        let (text, secret) = secret_text(form, seed);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let texts = runtime.block_on(indexed(secret_turn(&text)));
        prop_assert_eq!(texts.len(), 5, "{:?}", texts);
        let chars: Vec<char> = secret.chars().collect();
        for window in chars.windows(8) {
            let part: String = window.iter().collect();
            for doc in &texts {
                prop_assert!(!doc.contains(&part), "{part:?} of {text:?} is in {doc:?}");
            }
        }
    }
}

// The benches. Each prints its numbers after the load average of the machine; the
// numbers of a busy machine are not valid.

/// The seed of every corpus of the benches, the store's too, so both share a file.
const SEED: u64 = 42;

/// The dir of the corpus files, below cargo's target temp dir, shared with the
/// benches of efr-store.
fn cache_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("search-corpus")
}

fn temp_dir() -> TempDir {
    fs::create_dir_all(cache_dir()).unwrap();
    tempfile::tempdir_in(cache_dir()).unwrap()
}

/// A hash of the SQL of every migration, as the benches of efr-store make it.
fn schema_hash() -> u64 {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../efr-store/src/migrations");
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    let mut hasher = DefaultHasher::new();
    for file in files {
        fs::read(file).unwrap().hash(&mut hasher);
    }
    hasher.finish()
}

/// A store on the database file `path`, with no backups.
async fn open(path: &Path, clock: &TestClock) -> Store {
    let mut config = StoreConfig::in_data_dir(path.parent().unwrap());
    config.path = path.to_path_buf();
    config.backups = None;
    Store::open(config, clock.shared()).await.unwrap()
}

/// Appends one turn as one batch, a minute after the one before; the time it took.
async fn append_turn(
    store: &Store,
    clock: &TestClock,
    id: ConversationId,
    turn: Vec<Event>,
) -> Duration {
    let batch = turn.into_iter().fold(Batch::new(), |batch, event| batch.event(id, event));
    clock.advance(Duration::from_secs(60));
    let started = Instant::now();
    store.writer().append(batch).await.unwrap();
    started.elapsed()
}

/// The corpus file of `size` with the events only (no index), built when it is
/// missing; the same file as the store's benches.
async fn corpus_file(size: CorpusSize) -> PathBuf {
    let name = format!(
        "{}-seed{SEED}-schema{}-{:016x}-gen{CORPUS_VERSION}-log.sqlite",
        size.name(),
        Migrations::new().latest(),
        schema_hash(),
    );
    let target = cache_dir().join(name);
    if target.exists() {
        return target;
    }
    let dir = temp_dir();
    let path = dir.path().join("efr.sqlite");
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    for conversation in SearchCorpus::new(size, SEED) {
        for turn in conversation.turns {
            append_turn(&store, &clock, conversation.id, turn).await;
        }
    }
    store.close().await;
    fs::rename(&path, &target).unwrap();
    target
}

/// A copy of the corpus file of `size` in a new temp dir.
async fn corpus_copy(size: CorpusSize) -> (TempDir, PathBuf) {
    let source = corpus_file(size).await;
    let dir = temp_dir();
    let path = dir.path().join("efr.sqlite");
    fs::copy(source, &path).unwrap();
    fs::File::open(&path).unwrap().sync_all().unwrap();
    (dir, path)
}

/// The p50, p95 and max of `times`.
fn percentiles(times: &[Duration]) -> (Duration, Duration, Duration) {
    if times.is_empty() {
        return (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    }
    let mut sorted = times.to_vec();
    sorted.sort();
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.95), sorted[sorted.len() - 1])
}

fn ms(time: Duration) -> f64 {
    time.as_secs_f64() * 1000.0
}

/// One line of a table: `label  p50 p95 max (n)` in milliseconds.
fn line(label: &str, times: &[Duration]) -> String {
    let (p50, p95, max) = percentiles(times);
    format!(
        "{label:<32} p50 {:>8.3} ms  p95 {:>8.3} ms  max {:>8.3} ms  ({} samples)",
        ms(p50),
        ms(p95),
        ms(max),
        times.len()
    )
}

#[expect(clippy::print_stderr, reason = "a bench reports what it measured")]
fn report(bench: &str, lines: &[String]) {
    let load = fs::read_to_string("/proc/loadavg").unwrap_or_default();
    eprintln!(
        "{bench}: load average {}",
        load.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
    );
    for line in lines {
        eprintln!("{bench}: {line}");
    }
}

/// The most characters of a text that leave the store (`SOURCE_CHARS`).
fn source_cut(text: &str) -> String {
    text.chars().take(efr_store::search::SOURCE_CHARS as usize).collect()
}

/// The docs of a turn of the corpus: each text with its kind.
fn turn_docs(turn: &[Event]) -> Vec<(DocKind, String)> {
    turn.iter()
        .filter_map(|event| match event {
            Event::PromptQueued { text, .. } => Some((DocKind::Prompt, source_cut(text))),
            Event::AssistantMessageCompleted { text, .. } => {
                Some((DocKind::Answer, source_cut(text)))
            }
            Event::ToolCallStarted { input, .. } => input
                .get("command")
                .and_then(|command| command.as_str())
                .map(|command| (DocKind::Command, source_cut(command))),
            _ => None,
        })
        .collect()
}

/// The text rules of one turn of the corpus shape (DQ39): p95 at most 1 ms.
#[test]
#[ignore = "a bench: `just bench-search` runs it in release"]
fn bench_search_redaction() {
    let mut times = Vec::new();
    let mut bytes = 0;
    'turns: for conversation in SearchCorpus::new(CorpusSize::L, SEED) {
        for turn in conversation.turns {
            let docs = turn_docs(&turn);
            bytes += docs.iter().map(|(_, text)| text.len()).sum::<usize>();
            let started = Instant::now();
            for (kind, text) in &docs {
                std::hint::black_box(search_index_text(*kind, text));
            }
            times.push(started.elapsed());
            if times.len() == 3_000 {
                break 'turns;
            }
        }
    }
    report(
        "bench_search_redaction",
        &[
            format!("{} turns, {:.1} MB of text", times.len(), bytes as f64 / 1e6),
            line("the text rules of a turn", &times),
        ],
    );
    let (_, p95, _) = percentiles(&times);
    assert!(p95 <= Duration::from_millis(1), "the p95 of a turn is {p95:?}");
}

/// The text rules of the worst docs (DQ39): max at most 5 ms each.
#[test]
#[ignore = "a bench: `just bench-search` runs it in release"]
fn bench_search_redaction_worst() {
    let fill = |unit: &str| source_cut(&unit.repeat(33 * 1024 / unit.len() + 1));
    let base64: String = {
        let rng = TestRng::new(SEED);
        draw(&rng, ALNUM, 34_000)
    };
    let worst = [
        ("xsk-sk- again and again", fill("xsk-sk-")),
        ("sk- again and again", fill("sk-")),
        ("a=1,b=2,...", fill("a=1,b=2,")),
        ("a=\" again and again", fill("a=\"")),
        ("TOKEN=\" again and again", fill("TOKEN=\"")),
        ("one long base64 word", source_cut(&base64)),
        ("eyJ again and again", fill("eyJ.")),
        (":// again and again", fill("a://")),
        ("-----BEGIN again and again", fill("-----BEGIN PRIVATE KEY")),
        ("\"a\": \" again and again", fill("\"a\": \"")),
        ("\"token\": \" again and again", fill("\"token\": \"")),
        ("password: on each line", fill("password: x\n")),
        ("-H again and again", fill("-H ")),
        ("a 1 MiB line cut in SQL", source_cut(&"lorem ipsum dolor ".repeat(60_000))),
        ("heredocs on each line", fill("cat <<A\n")),
    ];
    let mut lines = Vec::new();
    let mut slowest = (Duration::ZERO, "");
    for (name, text) in &worst {
        let mut times = Vec::new();
        for kind in [DocKind::Prompt, DocKind::Answer, DocKind::Command] {
            for _ in 0..5 {
                let started = Instant::now();
                std::hint::black_box(search_index_text(kind, text));
                times.push(started.elapsed());
            }
        }
        let (_, _, max) = percentiles(&times);
        if max > slowest.0 {
            slowest = (max, name);
        }
        lines.push(line(name, &times));
    }
    report("bench_search_redaction_worst", &lines);
    assert!(slowest.0 <= Duration::from_millis(5), "{} takes {:?}", slowest.1, slowest.0);
}

/// What one run of the indexer over a corpus file did.
struct Backfill {
    /// The wall time until the index had every event.
    pages_wall: Duration,
    /// The wall time until the merges ended too.
    wall: Duration,
    status: SearchIndexStatus,
}

/// Runs efrd's indexer with the system clock over `store`, a store file, until it has
/// every event and merged, or until `pages` pages when given.
async fn run_indexer(store: &Store, pages: Option<u64>) -> Backfill {
    let newest = store.readers().with(efr_store::events::last_seq).await.unwrap();
    let (sender, mut status) = watch::channel(SearchIndexStatus::default());
    let stop = CancellationToken::new();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let started = Instant::now();
    let task = tokio::spawn(follow_search_index(
        store.writer().clone(),
        store.index_reader().clone(),
        clock,
        sender,
        stop.clone(),
    ));
    let mut pages_wall = None;
    loop {
        let now = *status.borrow_and_update();
        if pages_wall.is_none() && now.indexed_seq >= newest {
            pages_wall = Some(started.elapsed());
        }
        if pages.is_some_and(|pages| now.pages >= pages) || (now.idle && now.indexed_seq >= newest)
        {
            break;
        }
        status.changed().await.unwrap();
    }
    let wall = started.elapsed();
    stop.cancel();
    task.await.unwrap();
    let status = *status.borrow();
    Backfill { pages_wall: pages_wall.unwrap_or(wall), wall, status }
}

fn backfill_lines(size: CorpusSize, run: &Backfill) -> Vec<String> {
    let status = &run.status;
    vec![
        format!(
            "corpus {}: {} pages in {:.2} s of wall time, {:.2} s of writer time (p50 page {:.3} ms)",
            size.name(),
            status.pages,
            run.pages_wall.as_secs_f64(),
            status.page_time.as_secs_f64(),
            ms(status.page_time) / status.pages.max(1) as f64,
        ),
        format!(
            "then {} merges in {:.2} s of wall time, {:.2} s of writer time",
            status.merges,
            (run.wall - run.pages_wall).as_secs_f64(),
            status.merge_time.as_secs_f64(),
        ),
        format!(
            "the writer's duty: {:.1}% of the wall time",
            100.0 * (status.page_time + status.merge_time).as_secs_f64() / run.wall.as_secs_f64()
        ),
    ]
}

/// The backfill of corpus S: at most 2 s of writer time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_backfill_s() {
    let (_dir, path) = corpus_copy(CorpusSize::S).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let run = run_indexer(&store, None).await;
    store.close().await;
    report("bench_search_backfill_s", &backfill_lines(CorpusSize::S, &run));
    let writer = run.status.page_time + run.status.merge_time;
    assert!(writer <= Duration::from_secs(2), "the writer time is {writer:?}");
}

/// The backfill of corpus L: at most 90 s of wall time at the duty cycle and 20 s of
/// writer time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_backfill_l() {
    let (_dir, path) = corpus_copy(CorpusSize::L).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let run = run_indexer(&store, None).await;
    store.close().await;
    report("bench_search_backfill_l", &backfill_lines(CorpusSize::L, &run));
    assert!(run.pages_wall <= Duration::from_secs(90), "the backfill takes {:?}", run.pages_wall);
    let writer = run.status.page_time + run.status.merge_time;
    assert!(writer <= Duration::from_secs(20), "the writer time is {writer:?}");
}

/// The frames of the write-ahead log of `path` that no checkpoint copied yet, from the
/// wal-index header of the `-shm` file.
fn wal_frames_waiting(path: &Path) -> u32 {
    let mut shm = path.as_os_str().to_owned();
    shm.push("-shm");
    let header = fs::read(shm).unwrap();
    let word = |at: usize| u32::from_ne_bytes(header[at..at + 4].try_into().unwrap());
    word(16).saturating_sub(word(96))
}

/// The appends of `count` turn batches of a second corpus, `gap` apart: the time of
/// each, split by whether a checkpoint of the write-ahead log ran between its start and
/// its end.
async fn timed_appends(
    store: &Store,
    clock: &TestClock,
    path: &Path,
    more: &mut SearchCorpus,
    count: usize,
) -> (Vec<Duration>, Vec<Duration>) {
    let (mut plain, mut checkpointed) = (Vec::new(), Vec::new());
    let gap = SystemClock;
    'turns: for conversation in more.by_ref() {
        for turn in conversation.turns {
            let before = wal_frames_waiting(path);
            let time = append_turn(store, clock, conversation.id, turn).await;
            let after = wal_frames_waiting(path);
            if after < before { &mut checkpointed } else { &mut plain }.push(time);
            if plain.len() + checkpointed.len() == count {
                break 'turns;
            }
            gap.sleep(Duration::from_millis(20)).await;
        }
    }
    (plain, checkpointed)
}

/// The added wait of a turn's commit while the backfill runs: p95 at most 2 ms and
/// max at most 10 ms over a run without the indexer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_commit_wait() {
    const TURNS: usize = 400;
    let (_dir, path) = corpus_copy(CorpusSize::L).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let mut more = SearchCorpus::new(CorpusSize::L, SEED + 7);
    let (alone, alone_checkpointed) = timed_appends(&store, &clock, &path, &mut more, TURNS).await;

    let (sender, status) = watch::channel(SearchIndexStatus::default());
    let stop = CancellationToken::new();
    let task = tokio::spawn(follow_search_index(
        store.writer().clone(),
        store.index_reader().clone(),
        Arc::new(SystemClock),
        sender,
        stop.clone(),
    ));
    let (beside, beside_checkpointed) =
        timed_appends(&store, &clock, &path, &mut more, TURNS).await;
    let during = *status.borrow();
    stop.cancel();
    task.await.unwrap();
    store.close().await;

    let all = |a: &[Duration], b: &[Duration]| a.iter().chain(b).copied().collect::<Vec<_>>();
    let (alone_all, beside_all) =
        (all(&alone, &alone_checkpointed), all(&beside, &beside_checkpointed));
    report(
        "bench_search_commit_wait",
        &[
            format!(
                "corpus l: {TURNS} turn batches without the indexer, then {TURNS} beside its backfill ({} pages and {} merges ran, the index at seq {})",
                during.pages, during.merges, during.indexed_seq
            ),
            line("append without the indexer", &alone_all),
            line("  without a checkpoint", &alone),
            line("  with a checkpoint", &alone_checkpointed),
            line("append beside the backfill", &beside_all),
            line("  without a checkpoint", &beside),
            line("  with a checkpoint", &beside_checkpointed),
            "a page or a merge of the indexer runs a checkpoint of the write-ahead log after its commit at 500 frames, a turn's batch at 1,000: a turn's batch counts it above when it ran one or waited for one".to_owned(),
        ],
    );
    let (_, alone_p95, _) = percentiles(&alone_all);
    let (_, beside_p95, _) = percentiles(&beside_all);
    let (_, _, alone_max) = percentiles(&alone);
    let (_, _, beside_max) = percentiles(&beside);
    assert!(
        beside_p95 <= alone_p95 + Duration::from_millis(2),
        "the p95 of a commit is {beside_p95:?} beside the backfill and {alone_p95:?} without it"
    );
    assert!(
        beside_max <= alone_max + Duration::from_millis(10),
        "the max of a commit is {beside_max:?} beside the backfill and {alone_max:?} without it"
    );
}

/// A field of `/proc/self/status` in KiB.
fn status_kib(field: &str) -> u64 {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap()
}

/// The memory of the indexer: at rest after the backfill of corpus S, at most 4 MiB
/// over a store that worked without it, and the peak at most 8 MiB over the rest.
/// The store first appends and reads turns with its readers, so the writer's page
/// cache and the threads of the runtime are warm, as in an efrd that runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_memory() {
    let (_dir, path) = corpus_copy(CorpusSize::S).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let mut more = SearchCorpus::new(CorpusSize::L, SEED + 11);
    for conversation in more.by_ref().take(20) {
        for turn in conversation.turns {
            append_turn(&store, &clock, conversation.id, turn).await;
        }
        let id = conversation.id;
        store.readers().with(move |conn| efr_store::conversations::turns(conn, id)).await.unwrap();
    }
    // NOTE: the corpus build and its copy are not the indexer's: the peak starts again
    // here.
    fs::write("/proc/self/clear_refs", "5").unwrap();
    let before = status_kib("VmRSS:");
    let newest = store.readers().with(efr_store::events::last_seq).await.unwrap();
    let (sender, mut status) = watch::channel(SearchIndexStatus::default());
    let stop = CancellationToken::new();
    let task = tokio::spawn(follow_search_index(
        store.writer().clone(),
        store.index_reader().clone(),
        Arc::new(SystemClock),
        sender,
        stop.clone(),
    ));
    let done =
        *status.wait_for(|status| status.idle && status.indexed_seq >= newest).await.unwrap();
    // At rest: the indexer waits for the broadcast with its connection open.
    let rest = status_kib("VmRSS:");
    let peak = status_kib("VmHWM:");
    stop.cancel();
    task.await.unwrap();
    store.close().await;
    let mib = |kib: u64| kib as f64 / 1024.0;
    report(
        "bench_search_memory",
        &[
            format!("corpus s: {} pages and {} merges", done.pages, done.merges),
            format!(
                "resident before {:.1} MiB, at rest {:.1} MiB, peak {:.1} MiB",
                mib(before),
                mib(rest),
                mib(peak)
            ),
            format!(
                "the indexer: {:+.1} MiB at rest, its peak {:.1} MiB over that",
                mib(rest) - mib(before),
                mib(peak.saturating_sub(rest))
            ),
        ],
    );
    assert!(
        rest.saturating_sub(before) <= 4 * 1024,
        "the rest grows by {} KiB",
        rest.saturating_sub(before)
    );
    assert!(
        peak.saturating_sub(rest) <= 8 * 1024,
        "the peak is {} KiB over the rest",
        peak.saturating_sub(rest)
    );
}
