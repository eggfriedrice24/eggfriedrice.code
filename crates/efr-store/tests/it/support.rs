//! The seeded corpus in a store file, the indexer's rounds with stand-in text rules,
//! and the numbers of a bench.
//!
//! A corpus file is built once per size, seed, schema version and generator version,
//! below cargo's target temp dir (never `/tmp`), and each test copies it. Two shapes:
//! `Log` holds the events only, for a backfill; `Indexed` is the index of production:
//! the backfill pages over all but the newest tenth of the conversations, the merges
//! after the backfill, then one round of the indexer after each turn of the rest.

use std::fs;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use efr_protocol::{ConversationId, Event};
use efr_store::search::{self, DocKind, SearchApplied, SearchPage, SourceRow};
use efr_store::{Batch, Migrations, Store, StoreConfig};
use efr_test_support::{CORPUS_VERSION, CorpusConversation, CorpusSize, SearchCorpus, TestClock};
use tempfile::TempDir;

/// The seed of every corpus of the benches.
pub(crate) const SEED: u64 = 42;

/// What a corpus file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    /// The events only.
    Log,
    /// The events and the index, built as efrd builds it.
    Indexed,
}

/// The dir of the corpus files: below cargo's target temp dir, which `cargo clean`
/// removes.
fn cache_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("search-corpus")
}

/// A new temp dir below the target dir, for a copy of a corpus.
pub(crate) fn temp_dir() -> TempDir {
    let dir = cache_dir();
    fs::create_dir_all(&dir).unwrap();
    tempfile::tempdir_in(dir).unwrap()
}

/// The text rules of efrd's indexer as far as the size of the index goes: no heredoc
/// body in a command, and the cut of each kind at a line end. efrd's redaction and its
/// control character strip change the size little, and the store cannot reach them.
pub(crate) fn index_text(kind: DocKind, text: &str) -> Option<String> {
    let cap = match kind {
        DocKind::Prompt | DocKind::Steer => 32 * 1024,
        DocKind::Answer => 16 * 1024,
        DocKind::Command => 2 * 1024,
    };
    let text = if kind == DocKind::Command { without_heredoc(text) } else { text.to_owned() };
    let text = cut(&text, cap);
    (!text.trim().is_empty()).then(|| text.to_owned())
}

/// The corpus writes each heredoc as `<<'EOF'` to a line `EOF`.
fn without_heredoc(command: &str) -> String {
    let mut kept = Vec::new();
    let mut body = false;
    for line in command.lines() {
        if body {
            body = line != "EOF";
            continue;
        }
        kept.push(line);
        body = line.contains("<<'EOF'");
    }
    kept.join("\n")
}

/// `text` cut to at most `cap` bytes at the last line end before the cap, or at a
/// character boundary when the first line is longer.
fn cut(text: &str, cap: usize) -> &str {
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    match text[..end].rfind('\n') {
        Some(line_end) if line_end > 0 => &text[..line_end],
        _ => &text[..end],
    }
}

/// A store on the database file `path`, with no backups and a test clock.
pub(crate) async fn open(path: &Path, clock: &TestClock) -> Store {
    let mut config = StoreConfig::in_data_dir(path.parent().unwrap());
    config.path = path.to_path_buf();
    config.backups = None;
    Store::open(config, clock.shared()).await.unwrap()
}

/// Appends each turn of `conversation` as one batch, a minute apart; after each turn
/// when `rounds`, one round of the indexer.
pub(crate) async fn append(
    store: &Store,
    clock: &TestClock,
    conversation: CorpusConversation,
    rounds: bool,
) {
    for turn in conversation.turns {
        append_turn(store, clock, conversation.id, turn).await;
        if rounds {
            round(store).await;
        }
    }
}

/// Appends the events of one turn as one batch, a minute after the one before.
pub(crate) async fn append_turn(
    store: &Store,
    clock: &TestClock,
    id: ConversationId,
    turn: Vec<Event>,
) {
    let mut batch = Batch::new();
    for event in turn {
        batch = batch.event(id, event);
    }
    clock.advance(Duration::from_secs(60));
    store.writer().append(batch).await.unwrap();
}

/// What one round of the indexer did and cost.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Round {
    /// The time of the writer job `search_apply`, as its caller waits for it.
    pub(crate) apply: Duration,
    /// The time of the read `search::pending`.
    pub(crate) read: Duration,
    /// The docs that the round wrote.
    pub(crate) docs: usize,
    /// True when the index had every event before the round.
    pub(crate) idle: bool,
}

/// One round of an indexer with the stand-in text rules.
pub(crate) async fn round(store: &Store) -> Round {
    let started = Instant::now();
    let (current, pending) = store
        .readers()
        .with(|conn| {
            let current = search::state(conn)?;
            let pending = search::pending(conn, current.indexed_seq)?;
            Ok((current, pending))
        })
        .await
        .unwrap();
    let read = started.elapsed();
    if current.indexed_seq == current.newest_seq {
        return Round { apply: Duration::ZERO, read, docs: 0, idle: true };
    }
    let docs: Vec<SourceRow> = pending
        .rows
        .into_iter()
        .filter_map(|row| {
            let text = index_text(row.kind, &row.text)?;
            Some(SourceRow { text, ..row })
        })
        .collect();
    let count = docs.len();
    let page = SearchPage {
        after: current.indexed_seq,
        through: pending.through,
        redaction: current.redaction,
        docs,
        drops: pending.drops,
    };
    let started = Instant::now();
    let applied = store.writer().search_apply(page).await.unwrap();
    let apply = started.elapsed();
    assert_eq!(applied, SearchApplied::Applied);
    Round { apply, read, docs: count, idle: false }
}

/// Rounds until the index has every event; the rounds that wrote.
pub(crate) async fn backfill(store: &Store) -> Vec<Round> {
    let mut rounds = Vec::new();
    loop {
        let round = round(store).await;
        if round.idle {
            return rounds;
        }
        rounds.push(round);
    }
}

/// Merges until a merge does no work; the time of each merge that worked.
pub(crate) async fn merge_all(store: &Store) -> Vec<Duration> {
    let mut times = Vec::new();
    loop {
        let started = Instant::now();
        let worked = store.writer().search_merge(64).await.unwrap();
        if !worked {
            return times;
        }
        times.push(started.elapsed());
    }
}

/// A hash of the SQL of every migration, so a cached corpus of a schema whose newest
/// file changed before its release is built again. The std hasher may change with the
/// toolchain; that only builds a corpus again.
fn schema_hash() -> u64 {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/migrations");
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

/// The corpus file of `size` in `shape`, built when it is missing.
pub(crate) async fn corpus_file(size: CorpusSize, shape: Shape) -> PathBuf {
    let name = format!(
        "{}-seed{SEED}-schema{}-{:016x}-gen{CORPUS_VERSION}-{}.sqlite",
        size.name(),
        Migrations::new().latest(),
        schema_hash(),
        match shape {
            Shape::Log => "log",
            Shape::Indexed => "indexed",
        }
    );
    let target = cache_dir().join(name);
    if target.exists() {
        return target;
    }
    // NOTE: two tests may build the same file at once; each builds in a dir of its
    // own and renames the finished file into place, so a reader never sees half.
    let dir = temp_dir();
    let path = dir.path().join("efr.sqlite");
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let conversations = size.conversations() as usize;
    let backfilled = match shape {
        Shape::Log => conversations,
        Shape::Indexed => conversations - conversations / 10,
    };
    let mut corpus = SearchCorpus::new(size, SEED);
    for conversation in corpus.by_ref().take(backfilled) {
        append(&store, &clock, conversation, false).await;
    }
    if shape == Shape::Indexed {
        backfill(&store).await;
        merge_all(&store).await;
        for conversation in corpus {
            append(&store, &clock, conversation, true).await;
        }
    }
    store.close().await;
    fs::rename(&path, &target).unwrap();
    target
}

/// A copy of the corpus file in a new temp dir: the dir and the database file in it.
pub(crate) async fn corpus_copy(size: CorpusSize, shape: Shape) -> (TempDir, PathBuf) {
    let source = corpus_file(size, shape).await;
    let dir = temp_dir();
    let path = dir.path().join("efr.sqlite");
    fs::copy(source, &path).unwrap();
    // The disk writes the copy now, so its writeback does not land in the timed writes.
    fs::File::open(&path).unwrap().sync_all().unwrap();
    (dir, path)
}

/// The p50, p95 and max of `times`.
pub(crate) fn percentiles(times: &[Duration]) -> (Duration, Duration, Duration) {
    if times.is_empty() {
        return (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    }
    let mut sorted = times.to_vec();
    sorted.sort();
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.95), sorted[sorted.len() - 1])
}

/// One line of a table: `label  p50 p95 max  (n)` in milliseconds, then the five
/// longest times.
pub(crate) fn line(label: &str, times: &[Duration]) -> String {
    let (p50, p95, max) = percentiles(times);
    let ms = |time: Duration| time.as_secs_f64() * 1000.0;
    let mut longest = times.to_vec();
    longest.sort_by(|a, b| b.cmp(a));
    let longest: Vec<String> =
        longest.iter().take(5).map(|time| format!("{:.1}", ms(*time))).collect();
    format!(
        "{label:<28} p50 {:>7.3} ms  p95 {:>7.3} ms  max {:>7.3} ms  ({} samples; longest {})",
        ms(p50),
        ms(p95),
        ms(max),
        times.len(),
        longest.join(" ")
    )
}

/// Writes the lines of a bench to stderr, after the load average of the machine: the
/// numbers of a busy machine are not valid.
#[expect(clippy::print_stderr, reason = "a bench reports what it measured")]
pub(crate) fn report(bench: &str, lines: &[String]) {
    let load = fs::read_to_string("/proc/loadavg").unwrap_or_default();
    eprintln!(
        "{bench}: load average {}",
        load.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
    );
    for line in lines {
        eprintln!("{bench}: {line}");
    }
}

/// The frames of the write-ahead log of `path` that no checkpoint copied yet, read
/// from the wal-index header of the `-shm` file as the store reads it.
pub(crate) fn wal_frames_waiting(path: &Path) -> u32 {
    let mut shm = path.as_os_str().to_owned();
    shm.push("-shm");
    let header = fs::read(shm).unwrap();
    let word = |at: usize| u32::from_ne_bytes(header[at..at + 4].try_into().unwrap());
    assert_eq!(word(0), 3_007_000, "a wal-index header");
    word(16).saturating_sub(word(96))
}
