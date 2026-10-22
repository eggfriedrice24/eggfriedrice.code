//! The budgets of the search index in the store (the plan's DQ39): the disk of the
//! index per 1,000 turns (a normal test), and the writer time of a live round and of a
//! backfill page (benches: ignored, run in release by `just bench-search`, which fails
//! when a budget fails).

use std::time::Duration;

use efr_store::Store;
use efr_test_support::{CorpusSize, SearchCorpus, TestClock};

use crate::support::{
    SEED, Shape, append_turn, backfill, corpus_copy, line, merge_all, open, percentiles, report,
    round, wal_frames_waiting,
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

// NOTE: the max of each bench counts the jobs without a checkpoint of the write-ahead
// log. SQLite runs that checkpoint in the commit that crosses 1,000 frames, whichever
// job it is, and it costs more than the budget (about 5 to 18 ms here); a store with no
// index pays it too, in a turn's commit. Each bench reports those jobs on a line of
// their own, and the p95 counts every job.

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
    let (_, p95, _) = percentiles(&applies);
    let (_, _, max) = percentiles(&plain);
    assert!(p95 <= ROUND_P95, "the p95 of a round is {p95:?}, more than {ROUND_P95:?}");
    assert!(max <= ROUND_MAX, "the max of a round is {max:?}, more than {ROUND_MAX:?}");
}

/// The writer time of one backfill page.
const PAGE_P95: Duration = Duration::from_millis(4);
const PAGE_MAX: Duration = Duration::from_millis(8);

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
    let (_, p95, _) = percentiles(&applies);
    let (_, _, max) = percentiles(&plain);
    assert!(p95 <= PAGE_P95, "the p95 of a page is {p95:?}, more than {PAGE_P95:?}");
    assert!(max <= PAGE_MAX, "the max of a page is {max:?}, more than {PAGE_MAX:?}");
}
