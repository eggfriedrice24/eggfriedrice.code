//! The budgets of the search index in the store (the plan's DQ39): the disk of the
//! index per 1,000 turns (a normal test), and the writer time of a live round and of a
//! backfill page (benches: ignored, run in release by `just bench-search`, which fails
//! when a budget fails).

use std::time::Duration;

use efr_store::{SEARCH_CHECKPOINT_FRAMES, Store, WAL_AUTOCHECKPOINT};
use efr_test_support::{CorpusSize, SearchCorpus, TestClock};

use crate::support::{
    SEED, Shape, append_turn, backfill, corpus_copy, line, merge_all, open, percentiles, report,
    round, temp_dir, wal_frames_waiting,
};

/// The disk that the index may use per 1,000 turns of the corpus, the tail included.
const DISK_PER_1000_TURNS: i64 = 10_000_000;

/// The sizes of the tables and indexes whose name starts with `prefix`, by name.
fn sizes(conn: &rusqlite::Connection, prefix: &str) -> Vec<(String, i64)> {
    let mut stmt = conn
        .prepare(
            "SELECT name, SUM(pgsize) FROM dbstat WHERE name LIKE ?1 || '%' \
             GROUP BY name ORDER BY name",
        )
        .unwrap();
    stmt.query_map([prefix], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

async fn search_sizes(store: &Store) -> Vec<(String, i64)> {
    store.readers().with(|conn| Ok(sizes(conn, "search"))).await.unwrap()
}

fn total(sizes: &[(String, i64)]) -> i64 {
    sizes.iter().map(|(_, bytes)| bytes).sum()
}

fn mb(bytes: i64) -> f64 {
    bytes as f64 / 1_000_000.0
}

fn size_lines(label: &str, sizes: &[(String, i64)], turns: u32) -> Vec<String> {
    let mut lines: Vec<String> = sizes
        .iter()
        .map(|(name, bytes)| format!("{label}: {name:<24} {:>8.3} MB", mb(*bytes)))
        .collect();
    let all = total(sizes);
    lines.push(format!(
        "{label}: {:<24} {:>8.3} MB, {:.3} MB per 1,000 turns",
        "all",
        mb(all),
        mb(all) * 1000.0 / f64::from(turns)
    ));
    lines
}

#[tokio::test]
async fn index_disk_fits_its_budget() {
    let size = CorpusSize::S;
    let (_dir, path) = corpus_copy(size, Shape::Log).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;

    backfill(&store).await;
    let backfilled = search_sizes(&store).await;
    let merges = merge_all(&store).await;
    let merged = search_sizes(&store).await;
    let text: i64 = store
        .readers()
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT SUM(length(CAST(text AS BLOB))) FROM search_docs",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    store.close().await;

    // The prefix choices of DQ5, each built by FTS5's `rebuild` over the same docs.
    let conn = efr_store::db::open(&path).unwrap();
    let mut lines = vec![format!("the text of the docs: {:.3} MB", mb(text))];
    lines.extend(size_lines("after the backfill", &backfilled, size.turns()));
    lines.extend(size_lines(&format!("after {} merges", merges.len()), &merged, size.turns()));
    for (name, prefix) in
        [("fts_none", ""), ("fts_p3", "prefix = '3', "), ("fts_p23", "prefix = '2 3', ")]
    {
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE {name} USING fts5 (text, conversation_id UNINDEXED, \
             kind UNINDEXED, content = 'search_docs', content_rowid = 'seq', {prefix}\
             tokenize = 'unicode61 remove_diacritics 2');
             INSERT INTO {name} ({name}, rank) VALUES ('pgsz', 1000);
             INSERT INTO {name} ({name}) VALUES ('rebuild');"
        ))
        .unwrap();
        let fts = total(&sizes(&conn, name));
        lines.push(format!(
            "rebuilt {name:<8}: the full text index {:>8.3} MB, with the docs and titles {:.3} MB per 1,000 turns",
            mb(fts),
            mb(fts + total(&merged) - total(&sizes(&conn, "search_fts"))) * 1000.0 / f64::from(size.turns())
        ));
    }
    let updated = total(&sizes(&conn, "conversations_updated"));
    lines.push(format!("the index conversations_updated: {:.3} MB", mb(updated)));
    report("index_disk_fits_its_budget", &lines);

    let budget = DISK_PER_1000_TURNS * i64::from(size.turns()) / 1000;
    for (when, sizes) in [("after the backfill", &backfilled), ("after the merges", &merged)] {
        assert!(
            total(sizes) <= budget,
            "the index uses {:.3} MB {when}, more than {:.3} MB",
            mb(total(sizes)),
            mb(budget)
        );
    }
}

/// Appends turns of a corpus until the write-ahead log holds more than `frames` that
/// no checkpoint copied.
async fn append_until(
    store: &Store,
    clock: &TestClock,
    path: &std::path::Path,
    corpus: &mut SearchCorpus,
    frames: u32,
) {
    for conversation in corpus.by_ref() {
        for turn in conversation.turns {
            append_turn(store, clock, conversation.id, turn).await;
            if wal_frames_waiting(path) > frames {
                return;
            }
        }
    }
    panic!("the corpus ended first");
}

#[tokio::test]
async fn search_jobs_checkpoint_and_turn_commits_seldom_do() {
    let dir = temp_dir();
    let path = dir.path().join("efr.sqlite");
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let mut corpus = SearchCorpus::new(CorpusSize::S, SEED);

    // Turn commits run no checkpoint below the writer's size.
    append_until(&store, &clock, &path, &mut corpus, SEARCH_CHECKPOINT_FRAMES).await;
    let waiting = wal_frames_waiting(&path);
    assert!(waiting < WAL_AUTOCHECKPOINT, "{waiting} frames");
    // A page of the index commits with more than 500 frames in the log, and checkpoints.
    round(&store).await;
    assert_eq!(wal_frames_waiting(&path), 0);

    // A merge job that works does the same; the second page made a second segment.
    append_until(&store, &clock, &path, &mut corpus, SEARCH_CHECKPOINT_FRAMES).await;
    round(&store).await;
    append_until(&store, &clock, &path, &mut corpus, SEARCH_CHECKPOINT_FRAMES).await;
    assert!(store.writer().search_merge(64).await.unwrap());
    assert_eq!(wal_frames_waiting(&path), 0);

    // Below 500 frames, a search job leaves the log as it is.
    let conversation = corpus.next().unwrap();
    append_turn(&store, &clock, conversation.id, conversation.turns[0].clone()).await;
    let before = wal_frames_waiting(&path);
    assert!(before > 0 && before < SEARCH_CHECKPOINT_FRAMES, "{before} frames");
    round(&store).await;
    assert!(wal_frames_waiting(&path) > before, "no checkpoint");
    store.close().await;
}

// NOTE: the p95 of each bench counts the jobs without a checkpoint of the write-ahead
// log (the user's budgets of 2026-10-22). The max of a live round counts every job;
// the max of a backfill page counts the pages without a checkpoint only, because the
// flush of a checkpoint takes 10 to 150 ms on some disks, and the backfill runs once,
// in the background. A search job
// runs that checkpoint after its commit when the log holds 500 frames or more (DQ45),
// which takes it off the turns' commits; a store with no index pays it too, in a
// turn's commit at 1,000 frames. Each bench reports those jobs on a line of their own.

/// The writer time of one live round (one turn: 12 docs, about 4 KB).
const ROUND_P95: Duration = Duration::from_millis(2);
const ROUND_MAX: Duration = Duration::from_millis(10);

#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_apply_round() {
    let (_dir, path) = corpus_copy(CorpusSize::S, Shape::Indexed).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let mut more = SearchCorpus::new(CorpusSize::L, SEED + 1);
    let (mut applies, mut reads, mut plain, mut checkpointed) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut docs = 0;
    'rounds: for conversation in more.by_ref() {
        for turn in conversation.turns {
            append_turn(&store, &clock, conversation.id, turn).await;
            let before = wal_frames_waiting(&path);
            let round = round(&store).await;
            let after = wal_frames_waiting(&path);
            docs += round.docs;
            applies.push(round.apply);
            reads.push(round.read);
            if after < before { &mut checkpointed } else { &mut plain }.push(round.apply);
            if applies.len() == 1_000 {
                break 'rounds;
            }
        }
    }
    store.close().await;

    report(
        "bench_search_apply_round",
        &[
            format!("{} rounds on corpus s, {docs} docs", applies.len()),
            line("search_apply", &applies),
            line("  without a checkpoint", &plain),
            line("  with a checkpoint", &checkpointed),
            line("pending (the read)", &reads),
        ],
    );
    let (_, p95, _) = percentiles(&plain);
    let (_, _, max) = percentiles(&applies);
    assert!(p95 <= ROUND_P95, "the p95 of a round is {p95:?}, more than {ROUND_P95:?}");
    assert!(max <= ROUND_MAX, "the max of a round is {max:?}, more than {ROUND_MAX:?}");
}

/// The writer time of one backfill page.
const PAGE_P95: Duration = Duration::from_millis(4);
const PAGE_MAX: Duration = Duration::from_millis(10);

#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_backfill_page() {
    let (_dir, path) = corpus_copy(CorpusSize::L, Shape::Log).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;

    let (mut rounds, mut plain, mut checkpointed) = (Vec::new(), Vec::new(), Vec::new());
    loop {
        let before = wal_frames_waiting(&path);
        let round = round(&store).await;
        if round.idle {
            break;
        }
        let after = wal_frames_waiting(&path);
        if after < before { &mut checkpointed } else { &mut plain }.push(round.apply);
        rounds.push(round);
    }
    let merges = merge_all(&store).await;
    store.close().await;

    let applies: Vec<Duration> = rounds.iter().map(|round| round.apply).collect();
    let reads: Vec<Duration> = rounds.iter().map(|round| round.read).collect();
    let docs: usize = rounds.iter().map(|round| round.docs).sum();
    let sum = |times: &[Duration]| times.iter().sum::<Duration>().as_secs_f64();
    report(
        "bench_search_backfill_page",
        &[
            format!(
                "corpus l: {} pages, {docs} docs, {:.2} s of writer time, {:.2} s of reads",
                rounds.len(),
                sum(&applies),
                sum(&reads)
            ),
            line("search_apply (a page)", &applies),
            line("  without a checkpoint", &plain),
            line("  with a checkpoint", &checkpointed),
            line("pending (the read)", &reads),
            format!(
                "{} merges of about 64 pages each, {:.2} s of writer time",
                merges.len(),
                sum(&merges)
            ),
            line("search_merge", &merges),
        ],
    );
    let (_, p95, max) = percentiles(&plain);
    assert!(p95 <= PAGE_P95, "the p95 of a page is {p95:?}, more than {PAGE_P95:?}");
    assert!(max <= PAGE_MAX, "the max of a page is {max:?}, more than {PAGE_MAX:?}");
}

/// The writer time of `search_reset` on the index of corpus L and XL: one job that
/// empties the whole index, as efrd runs it at its start after a change of the
/// redaction. A report only: the plan sets no budget for it.
#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_reset() {
    let mut lines = Vec::new();
    for size in [CorpusSize::L, CorpusSize::Xl] {
        let (_dir, path) = corpus_copy(size, Shape::Indexed).await;
        let clock = TestClock::new();
        let store = open(&path, &clock).await;
        let started = std::time::Instant::now();
        store.writer().search_reset(u32::MAX).await.unwrap();
        let time = started.elapsed();
        store.close().await;
        lines.push(format!(
            "corpus {}: one search_reset job of {:.1} ms, its checkpoint included",
            size.name(),
            time.as_secs_f64() * 1000.0
        ));
    }
    report("bench_search_reset", &lines);
}

/// The writer time of one step of a delete with `secure-delete`, as of the other
/// writer jobs: a turn's commit waits for at most one step.
const DELETE_STEP_MAX: Duration = Duration::from_millis(10);

/// The writer time of a whole delete with `secure-delete`, for each KiB of the text of
/// its search docs, the checkpoints and the longer docs included. A step of 512 bytes
/// takes about 5 ms and at most 9 ms (`STEP_TEXT_BYTES`), so the budget is about twice
/// the rate of the slowest short step. efrd ends a delete after its answer, so this
/// time never reaches the user, but it holds the writer for other jobs between steps.
const DELETE_PER_KIB_MAX: Duration = Duration::from_millis(20);

/// The writer time of the delete of the conversation with the most docs on the index
/// of corpus L: each `delete_step` job (at most `STEP_DOCS` docs with at most
/// `STEP_TEXT_BYTES` of text, or one longer doc), with the FTS5 option `secure-delete`
/// of migration 12 and with it off, as before. It fails when a step with the option
/// takes more than `DELETE_STEP_MAX`, or the whole delete with the option takes more than
/// `DELETE_PER_KIB_MAX` for each KiB of text. As for a backfill page, the steps with a
/// checkpoint of the write-ahead log are on a line of their own and do not count, and
/// so are the steps of one longer doc, whose words go in one FTS5 `'delete'`.
#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_delete() {
    use efr_protocol::{ConversationId, Event, Origin};
    use efr_store::Batch;
    use efr_store::deletes::{DeleteStep, STEP_DOCS, STEP_TEXT_BYTES};

    let mut lines = Vec::new();
    let mut secure_max = Duration::ZERO;
    let (mut secure_all, mut secure_budget) = (Duration::ZERO, Duration::ZERO);
    for secure in [true, false] {
        let (_dir, path) = corpus_copy(CorpusSize::L, Shape::Indexed).await;
        let clock = TestClock::new();
        // NOTE: the corpus has the schema of this build, which sets the option.
        if !secure {
            let conn = efr_store::db::open(&path).unwrap();
            conn.execute(
                "INSERT INTO search_fts (search_fts, rank) VALUES ('secure-delete', 0)",
                [],
            )
            .unwrap();
        }
        let store = open(&path, &clock).await;
        let (id, docs): (String, i64) = store
            .readers()
            .with(|conn| {
                Ok(conn.query_row(
                    "SELECT conversation_id, COUNT(*) AS docs FROM search_docs \
                     GROUP BY conversation_id ORDER BY docs DESC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .await
            .unwrap();
        let id: ConversationId = id.parse().unwrap();
        let tombstone = Batch::new().event(id, Event::ConversationDeleted { origin: Origin::Cli });
        store.writer().append(tombstone).await.unwrap();
        let text_left = || {
            let id = id.to_string();
            store.readers().with(move |conn| {
                Ok(conn.query_row(
                    "SELECT COALESCE(SUM(length(CAST(text AS BLOB))), 0) FROM search_docs \
                     WHERE conversation_id = ?1",
                    [id],
                    |row| row.get::<_, i64>(0),
                )?)
            })
        };
        // The steps of at most `STEP_TEXT_BYTES` of text, without and with a
        // checkpoint, and the steps of one longer doc.
        let (mut times, mut plain, mut checkpointed, mut long) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut left = text_left().await.unwrap();
        let kib = u32::try_from((left + 1023) / 1024).unwrap();
        loop {
            let before = wal_frames_waiting(&path);
            let started = std::time::Instant::now();
            let step = store.writer().delete_step(id).await.unwrap();
            let time = started.elapsed();
            let after = wal_frames_waiting(&path);
            let now = text_left().await.unwrap();
            let text = usize::try_from(left - now).unwrap();
            left = now;
            times.push(time);
            if text > STEP_TEXT_BYTES {
                &mut long
            } else if after < before {
                &mut checkpointed
            } else {
                &mut plain
            }
            .push(time);
            if step == DeleteStep::RowsDone {
                break;
            }
        }
        let all: Duration = times.iter().sum();
        if secure {
            secure_max = percentiles(&plain).2;
            secure_all = all;
            secure_budget = DELETE_PER_KIB_MAX * kib;
        }
        let data: i64 = store
            .readers()
            .with(|conn| {
                Ok(conn.query_row(
                    "SELECT SUM(pgsize) FROM dbstat WHERE name = 'search_fts_data'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        store.close().await;
        let label = if secure { "secure-delete on" } else { "secure-delete off" };
        lines.push(format!(
            "{label}: {docs} docs ({kib} KiB of text) in {} steps of at most {STEP_DOCS} \
             docs and {STEP_TEXT_BYTES} bytes, {:.1} ms in all, search_fts_data {:.3} MB after",
            times.len(),
            all.as_secs_f64() * 1000.0,
            mb(data)
        ));
        lines.push(line("  delete_step (a step)", &times));
        lines.push(line("  without a checkpoint", &plain));
        lines.push(line("  with a checkpoint", &checkpointed));
        lines.push(line("  one longer doc", &long));
        lines.push(format!(
            "  the max of one step: {:.3} ms ({:.3} ms without a checkpoint or a longer doc)",
            percentiles(&times).2.as_secs_f64() * 1000.0,
            percentiles(&plain).2.as_secs_f64() * 1000.0
        ));
    }
    report("bench_search_delete", &lines);
    assert!(
        secure_max <= DELETE_STEP_MAX,
        "the max of a step with secure-delete is {secure_max:?}, more than {DELETE_STEP_MAX:?}"
    );
    assert!(
        secure_all <= secure_budget,
        "the whole delete with secure-delete took {secure_all:?}, more than {secure_budget:?}"
    );
}
