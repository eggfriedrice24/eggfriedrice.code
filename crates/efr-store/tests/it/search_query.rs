//! The search reads of the store on the seeded corpus: the Rust snippets against FTS5's
//! `highlight()`, the expected first row of a fixed list of queries, the interrupt
//! inside one FTS5 step (normal tests on corpus S), and the budgets of the plan's DQ39
//! for the reads (benches: ignored, run in release by `just bench-search`, which fails
//! when a budget fails).

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use efr_protocol::ConversationId;
use efr_stdx::rng::Rng as _;
use efr_store::search::{
    self, Found, SEARCH_LANES, SearchStop, fuzzy::Scorer, marks, query, snippet,
};
use efr_store::{Store, conversations};
use efr_test_support::{CorpusSize, SearchCorpus, TestClock, TestRng};
use rusqlite::Connection;

use crate::support::{SEED, Shape, corpus_copy, line, merge_all, open, percentiles, report};

/// The wait for a lane in the benches: none of them searches beside another one
/// unless it says so.
const WAIT: Duration = Duration::from_millis(200);

/// The 4 character (or `chars`) prefix that the most occurrences of the corpus share:
/// the sum of the Zipf weights of the words that start with it.
fn common_prefix(corpus: &SearchCorpus, chars: usize) -> String {
    let mut weights: HashMap<String, f64> = HashMap::new();
    for rank in 0..5_000 {
        let word = corpus.word(rank);
        if word.chars().count() >= chars {
            let prefix: String = word.chars().take(chars).collect();
            *weights.entry(prefix).or_default() += 1.0 / (rank + 1) as f64;
        }
    }
    let mut best: Vec<(String, f64)> = weights.into_iter().collect();
    best.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    best.into_iter().next().map(|(prefix, _)| prefix).unwrap_or_default()
}

/// One query of the bench list, with what kind it is.
#[derive(Debug, Clone)]
struct Probe {
    kind: &'static str,
    query: String,
}

/// The fixed list of 200 queries of the query benches: common and rare words, two and
/// three words, prefixes, phrases, `src/a`, `efr-c`, `a.b` and 16 words.
fn query_list(corpus: &SearchCorpus) -> Vec<Probe> {
    let rng = TestRng::new(SEED + 7);
    let pick = |from: usize, to: usize| {
        corpus.word(from + (rng.next_u64() % (to - from) as u64) as usize).to_owned()
    };
    let prefix4 = common_prefix(corpus, 4);
    let mut probes = Vec::with_capacity(200);
    for index in 0..200 {
        let (kind, query) = match index % 10 {
            0 => ("common", pick(0, 20)),
            1 => ("rare", pick(1_000, 5_000)),
            2 => ("two words", format!("{} {}", pick(0, 100), pick(100, 2_000))),
            3 => ("three words", format!("{} {} {}", pick(0, 50), pick(50, 500), pick(500, 5_000))),
            4 => ("prefix", {
                let word = pick(0, 200);
                word.chars().take(3 + (rng.next_u64() % 3) as usize).collect()
            }),
            5 => ("common prefix 4", format!("{} {prefix4}", pick(0, 200))),
            6 => ("phrase", format!("\"{} {}\"", pick(0, 100), pick(0, 100))),
            7 => (
                "symbols",
                ["src/a", "efr-c", "a.b", "etc/nixos", "foo_b"][index / 10 % 5].to_owned(),
            ),
            8 => ("16 words", (0..16).map(|_| pick(0, 300)).collect::<Vec<_>>().join(" ")),
            _ => ("word and blank", format!("{} ", pick(0, 1_000))),
        };
        probes.push(Probe { kind, query });
    }
    probes
}

/// Runs `query` on a lane and times it, from the call to the answer.
async fn timed(store: &Store, query: &str, limit: u32) -> (Duration, Found) {
    let query = query.to_owned();
    let started = Instant::now();
    let found = store
        .search_lanes()
        .with(&SearchStop::new(), WAIT, move |conn| {
            search::search(conn, &query, limit, &SearchStop::new())
        })
        .await
        .unwrap();
    (started.elapsed(), found)
}

/// The times of each query of the list, by kind and in all.
async fn query_lines(
    store: &Store,
    corpus: &SearchCorpus,
    label: &str,
) -> (Vec<String>, Vec<Duration>, HashMap<&'static str, Vec<Duration>>) {
    let probes = query_list(corpus);
    // One run to warm the page cache, as a person's second key would find it.
    for probe in &probes {
        timed(store, &probe.query, 50).await;
    }
    let mut all = Vec::new();
    let mut kinds: HashMap<&'static str, Vec<Duration>> = HashMap::new();
    let mut partial = 0;
    let mut scanned = 0_u64;
    let mut ranked: HashMap<&'static str, usize> = HashMap::new();
    for probe in &probes {
        let (time, found) = timed(store, &probe.query, 50).await;
        all.push(time);
        kinds.entry(probe.kind).or_default().push(time);
        partial += usize::from(found.partial.is_some());
        scanned += u64::from(found.rows_scanned);
        *ranked.entry(probe.kind).or_default() += usize::from(found.bm25);
    }
    let mut lines = vec![
        line(&format!("{label} all"), &all),
        format!(
            "{label}: {} queries, {partial} partial (row cap), {scanned} rows scanned, {} ranked with bm25()",
            probes.len(),
            ranked.values().sum::<usize>()
        ),
    ];
    let mut names: Vec<&&str> = kinds.keys().collect();
    names.sort();
    for name in names {
        let with = ranked.get(*name).copied().unwrap_or(0);
        let label = format!("{label}   {name} (bm25 {with}/{})", kinds[*name].len());
        lines.push(line(&label, &kinds[*name]));
    }
    (lines, all, kinds)
}

/// The query budgets of one corpus: the p95 of all, the max of all, and the p95 of the
/// rare words, when they have a budget.
struct QueryBudget {
    p95: Duration,
    max: Option<Duration>,
    rare: Option<Duration>,
}

async fn query_bench(size: CorpusSize, budget: QueryBudget) {
    let corpus = SearchCorpus::new(size, SEED);
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    for merged in [false, true] {
        let (_dir, path) = corpus_copy(size, Shape::Indexed).await;
        let clock = TestClock::new();
        let store = open(&path, &clock).await;
        if merged {
            merge_all(&store).await;
        }
        let label = format!("{}{}", size.name(), if merged { " merged" } else { "" });
        let (more, all, kinds) = query_lines(&store, &corpus, &label).await;
        lines.extend(more);
        let (_, p95, max) = percentiles(&all);
        if p95 > budget.p95 {
            failures.push(format!("{label}: the p95 is {p95:?}, more than {:?}", budget.p95));
        }
        if let Some(limit) = budget.max
            && max > limit
        {
            failures.push(format!("{label}: the max is {max:?}, more than {limit:?}"));
        }
        if let Some(limit) = budget.rare {
            let (_, _, rare) = percentiles(&kinds["rare"]);
            if rare > limit {
                failures.push(format!("{label}: a rare word takes {rare:?}, more than {limit:?}"));
            }
        }
        store.close().await;
    }
    report(&format!("bench_search_query_{}", size.name()), &lines);
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_query_s() {
    query_bench(
        CorpusSize::S,
        QueryBudget { p95: Duration::from_millis(30), max: None, rare: None },
    )
    .await;
}

#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_query_l() {
    let budget = QueryBudget {
        p95: Duration::from_millis(50),
        max: None,
        rare: Some(Duration::from_millis(30)),
    };
    query_bench(CorpusSize::L, budget).await;
}

#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_query_xl() {
    let budget = QueryBudget {
        p95: Duration::from_millis(60),
        max: Some(Duration::from_millis(200)),
        rare: None,
    };
    query_bench(CorpusSize::Xl, budget).await;
}

/// The time to the first row and to the 10,000th of `expression`, newest first, with
/// the columns that the search reads.
async fn first_rows(
    store: &Store,
    expression: String,
    bm25: bool,
) -> (Vec<Duration>, Vec<Duration>, usize) {
    store
        .readers()
        .with(move |conn| {
            let columns = if bm25 {
                "rowid, conversation_id, kind, bm25(search_fts)"
            } else {
                "rowid, conversation_id, kind"
            };
            let sql = format!(
                "SELECT {columns} FROM search_fts WHERE search_fts MATCH ?1 \
                 ORDER BY rowid DESC LIMIT 10000"
            );
            let (mut first, mut all, mut count) = (Vec::new(), Vec::new(), 0);
            for _ in 0..20 {
                let started = Instant::now();
                let mut stmt = conn.prepare(&sql)?;
                let mut rows = stmt.query([&expression])?;
                let mut n = 0;
                while let Some(row) = rows.next()? {
                    if n == 0 {
                        first.push(started.elapsed());
                    }
                    let _: i64 = row.get(0)?;
                    let _: String = row.get(1)?;
                    n += 1;
                }
                all.push(started.elapsed());
                count = n;
            }
            Ok((first, all, count))
        })
        .await
        .unwrap()
}

/// The time to the first row of the worst queries on XL.
const FIRST_ROW_MAX: Duration = Duration::from_millis(150);

#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_first_row() {
    let mut lines = Vec::new();
    let mut worst_xl = Duration::ZERO;
    for size in [CorpusSize::L, CorpusSize::Xl] {
        let corpus = SearchCorpus::new(size, SEED);
        let prefix4 = common_prefix(&corpus, 4);
        let prefix3 = common_prefix(&corpus, 3);
        let common = corpus.word(0).to_owned();
        for merged in [false, true] {
            let (_dir, path) = corpus_copy(size, Shape::Indexed).await;
            let clock = TestClock::new();
            let store = open(&path, &clock).await;
            if merged {
                merge_all(&store).await;
            }
            let label = format!("{}{}", size.name(), if merged { " merged" } else { "" });
            for (name, expression, bm25) in [
                (format!("\"{prefix4}\"* bm25"), format!("\"{prefix4}\"*"), true),
                (format!("\"{prefix4}\"*"), format!("\"{prefix4}\"*"), false),
                (format!("\"{prefix3}\"* bm25"), format!("\"{prefix3}\"*"), true),
                (format!("\"{common}\" bm25"), format!("\"{common}\""), true),
                (format!("\"{common}\""), format!("\"{common}\""), false),
            ] {
                let (first, all, count) = first_rows(&store, expression, bm25).await;
                let (_, _, max) = percentiles(&first);
                if bm25 && size == CorpusSize::Xl {
                    worst_xl = worst_xl.max(max);
                }
                lines.push(line(&format!("{label} first {name}"), &first));
                lines.push(line(&format!("{label} {count} rows {name}"), &all));
            }
            store.close().await;
        }
    }
    report("bench_search_first_row", &lines);
    assert!(
        worst_xl <= FIRST_ROW_MAX,
        "the first row of a worst query takes {worst_xl:?} on xl, more than {FIRST_ROW_MAX:?}"
    );
}

/// The empty query of the picker: the recent rows, their search titles, their short
/// ids and the state of the index.
#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_empty_query() {
    const P95: Duration = Duration::from_millis(10);
    let (_dir, path) = corpus_copy(CorpusSize::Xl, Shape::Indexed).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let route = store
        .readers()
        .with(|conn| Ok(conversations::recent(conn, None, 1)?.pop().map(|row| row.route)))
        .await
        .unwrap();
    let mut times = Vec::new();
    for run in 0..220 {
        let first = if run % 2 == 0 { route.clone() } else { None };
        let started = Instant::now();
        store
            .search_lanes()
            .with(&SearchStop::new(), WAIT, move |conn| {
                let rows = conversations::recent(conn, first.as_ref(), 50)?;
                let ids: Vec<ConversationId> = rows.iter().map(|row| row.summary.id).collect();
                let titles = search::titles(conn, &ids)?;
                let short = conversations::short_ids(conn, &ids)?;
                let percent = search::index_percent(conn)?;
                Ok((rows.len(), titles.len(), short.len(), percent))
            })
            .await
            .unwrap();
        if run >= 20 {
            times.push(started.elapsed());
        }
    }
    store.close().await;
    report(
        "bench_search_empty_query",
        &[line("xl: recent 50, titles, short ids, percent", &times)],
    );
    let (_, p95, _) = percentiles(&times);
    assert!(p95 <= P95, "the p95 of the empty query is {p95:?}, more than {P95:?}");
}

/// The preview: the hits of a query in one conversation.
#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_preview() {
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    for (size, budget) in
        [(CorpusSize::L, Duration::from_millis(30)), (CorpusSize::Xl, Duration::from_millis(40))]
    {
        let corpus = SearchCorpus::new(size, SEED);
        let probes = query_list(&corpus);
        let (_dir, path) = corpus_copy(size, Shape::Indexed).await;
        let clock = TestClock::new();
        let store = open(&path, &clock).await;
        // The conversations of the best rows of each query, as the picker shows them.
        let mut targets = Vec::new();
        for probe in &probes {
            let (_, found) = timed(&store, &probe.query, 3).await;
            for row in found.rows {
                targets.push((row.conversation_id, probe.query.clone()));
            }
        }
        let mut times = Vec::new();
        let mut hits = 0;
        for (id, query) in targets.iter().take(400) {
            let (id, query) = (*id, query.clone());
            let started = Instant::now();
            let found = store
                .search_lanes()
                .with(&SearchStop::new(), WAIT, move |conn| {
                    search::search_in(conn, id, &query, 20, &SearchStop::new())
                })
                .await
                .unwrap();
            times.push(started.elapsed());
            hits += found.hits.len();
        }
        store.close().await;
        lines
            .push(line(&format!("{}: {} previews, {hits} hits", size.name(), times.len()), &times));
        let (_, p95, _) = percentiles(&times);
        if p95 > budget {
            failures.push(format!("{}: the p95 is {p95:?}, more than {budget:?}", size.name()));
        }
    }
    report("bench_search_preview", &lines);
    assert!(failures.is_empty(), "{failures:?}");
}

/// The snippets of 50 rows: the docs by sequence number and the Rust snippet, with no
/// FTS5 statement.
#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_snippets() {
    const MAX: Duration = Duration::from_millis(5);
    let corpus = SearchCorpus::new(CorpusSize::L, SEED);
    let prefix4 = common_prefix(&corpus, 4);
    let (_dir, path) = corpus_copy(CorpusSize::L, Shape::Indexed).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let mut lines = Vec::new();
    let mut worst = Duration::ZERO;
    for word in [corpus.word(0).to_owned(), prefix4.clone(), corpus.word(3_000).to_owned()] {
        let prefix = word == prefix4;
        let times = store
            .readers()
            .with(move |conn| {
                // The 50 longest docs that match: the worst rows a list can show.
                let expression =
                    if prefix { format!("\"{word}\"*") } else { format!("\"{word}\"") };
                let mut stmt = conn.prepare(
                    "SELECT d.seq FROM search_fts AS f JOIN search_docs AS d ON d.seq = f.rowid \
                     WHERE search_fts MATCH ?1 ORDER BY length(d.text) DESC LIMIT 50",
                )?;
                let seqs: Vec<i64> =
                    stmt.query_map([expression], |row| row.get(0))?.collect::<Result<_, _>>()?;
                let needles = [snippet::Needle::new(std::slice::from_ref(&word), prefix)];
                let mut read = conn.prepare("SELECT text FROM search_docs WHERE seq = ?1")?;
                let mut times = Vec::new();
                for _ in 0..20 {
                    let started = Instant::now();
                    for seq in &seqs {
                        let text: String = read.query_row([seq], |row| row.get(0))?;
                        std::hint::black_box(snippet::snippet(&text, &needles));
                    }
                    times.push(started.elapsed());
                }
                Ok(times)
            })
            .await
            .unwrap();
        let (_, _, max) = percentiles(&times);
        worst = worst.max(max);
        lines.push(line(
            &format!(
                "l: 50 longest docs of {}",
                if prefix { "a common 4 character prefix" } else { "a word" }
            ),
            &times,
        ));
    }
    store.close().await;
    report("bench_search_snippets", &lines);
    assert!(worst <= MAX, "the snippets of 50 rows take {worst:?}, more than {MAX:?}");
}

/// The title scorer over the titles of the 10,000 newest conversations of XL.
#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_titles_10k() {
    const MAX: Duration = Duration::from_millis(5);
    let corpus = SearchCorpus::new(CorpusSize::Xl, SEED);
    let (_dir, path) = corpus_copy(CorpusSize::Xl, Shape::Indexed).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let titles: Vec<String> = store
        .readers()
        .with(|conn| {
            let mut stmt = conn.prepare("SELECT title FROM search_titles LIMIT 10000")?;
            let titles = stmt.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
            Ok(titles)
        })
        .await
        .unwrap();
    store.close().await;
    let probes = query_list(&corpus);
    let mut times = Vec::new();
    for probe in probes.iter().take(100) {
        let parsed = query::parse(&probe.query).unwrap();
        let scorer = Scorer::new(&parsed.words());
        let started = Instant::now();
        let matched = titles.iter().filter(|title| scorer.score(title).is_some()).count();
        std::hint::black_box(matched);
        times.push(started.elapsed());
    }
    report(
        "bench_search_titles_10k",
        &[line(&format!("{} titles, 100 queries of the list", titles.len()), &times)],
    );
    let (_, _, max) = percentiles(&times);
    assert!(max <= MAX, "the title scorer takes {max:?} over 10,000 titles, more than {MAX:?}");
}

/// What `SearchStop::hard` ends: a search between two rows, and one that a stop reaches
/// as it starts, before the merge of a prefix in FTS5. A stop inside that merge waits
/// for its end, which no interrupt reaches: at most the first row time of the prefix in
/// `bench_search_first_row`. No test can place a stop inside the merge without a timer.
#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_cancel() {
    const BETWEEN_ROWS: Duration = Duration::from_millis(10);
    let corpus = SearchCorpus::new(CorpusSize::Xl, SEED);
    let prefix4 = common_prefix(&corpus, 4);
    let common = corpus.word(0).to_owned();
    let (_dir, path) = corpus_copy(CorpusSize::Xl, Shape::Indexed).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let mut between = Vec::new();
    let mut inside = Vec::new();
    for _ in 0..20 {
        // Between rows: the hook says when the search reads rows.
        let (seen_tx, seen) = std_mpsc::channel();
        let stop = SearchStop::new().on_rows(
            100,
            Arc::new(move |rows| {
                if rows == 100 {
                    let _ = seen_tx.send(());
                }
            }),
        );
        let lanes = store.search_lanes().clone();
        let running = {
            let stop = stop.clone();
            let query = common.clone();
            tokio::spawn(async move {
                lanes
                    .with(&stop.clone(), WAIT, move |conn| search::search(conn, &query, 50, &stop))
                    .await
            })
        };
        tokio::task::spawn_blocking(move || seen.recv()).await.unwrap().unwrap();
        let started = Instant::now();
        stop.hard();
        let _ = running.await.unwrap();
        between.push(started.elapsed());

        // As the search starts: the row loop sees the stop before the prefix step.
        let (start_tx, start) = std_mpsc::channel();
        let stop = SearchStop::new();
        let lanes = store.search_lanes().clone();
        let running = {
            let stop = stop.clone();
            let query = format!("{common} {prefix4}");
            tokio::spawn(async move {
                lanes
                    .with(&stop.clone(), WAIT, move |conn| {
                        let _ = start_tx.send(());
                        search::search(conn, &query, 50, &stop)
                    })
                    .await
            })
        };
        tokio::task::spawn_blocking(move || start.recv()).await.unwrap().unwrap();
        let started = Instant::now();
        stop.hard();
        let _ = running.await.unwrap();
        inside.push(started.elapsed());
    }
    // The lane serves again after both kinds.
    let (_, found) = timed(&store, &common, 5).await;
    assert!(!found.rows.is_empty());
    store.close().await;
    report(
        "bench_search_cancel",
        &[
            line("xl: hard stop between rows, to the free lane", &between),
            line("xl: hard stop as a prefix search starts", &inside),
            "xl: a stop inside the prefix step waits for its end: see the first row of bench_search_first_row".to_owned(),
        ],
    );
    let (_, _, max) = percentiles(&between);
    assert!(max <= BETWEEN_ROWS, "a stop between rows takes {max:?}, more than {BETWEEN_ROWS:?}");
}

/// Runs the queries of `probes` on two lanes at once, half on each.
async fn run_on_two_lanes(store: &Store, probes: &[Probe]) {
    let half = probes.len() / 2;
    let mut sides = Vec::new();
    for side in [probes[..half].to_vec(), probes[half..].to_vec()] {
        let lanes = store.search_lanes().clone();
        sides.push(tokio::spawn(async move {
            for probe in side {
                let query = probe.query.clone();
                lanes
                    .with(&SearchStop::new(), Duration::from_secs(5), move |conn| {
                        search::search(conn, &query, 200, &SearchStop::new())
                    })
                    .await
                    .unwrap();
            }
        }));
    }
    for side in sides {
        side.await.unwrap();
    }
}

/// A number of KiB of `/proc/self/status`, such as `VmRSS`.
fn status_kib(name: &str) -> u64 {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        .unwrap()
}

/// The memory of the searches: the worst query list on two lanes at once, then at
/// rest. One test per corpus, so each runs in a process of its own and memory that an
/// earlier corpus left does not hide what this one uses.
#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_memory_l() {
    memory_bench(CorpusSize::L).await;
}

#[tokio::test]
#[ignore = "a bench: `just bench-search` runs it in release"]
async fn bench_search_memory_xl() {
    memory_bench(CorpusSize::Xl).await;
}

async fn memory_bench(size: CorpusSize) {
    // The user's budgets of 2026-10-22: most of the rest is heap that the allocator
    // keeps after a search, which the performance work after v0.0.4 looks at.
    const REST: u64 = 10 * 1024;
    const PEAK: u64 = 12 * 1024;
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    {
        let corpus = SearchCorpus::new(size, SEED);
        let probes = query_list(&corpus);
        let (_dir, path) = corpus_copy(size, Shape::Indexed).await;
        let clock = TestClock::new();
        let store = open(&path, &clock).await;
        // The corpus and the store are open; from here, the searches count.
        fs::write("/proc/self/clear_refs", "5").unwrap();
        let before = status_kib("VmRSS");
        // Two lanes open their connections with a search of a rare word.
        let mut warm = Vec::new();
        for _ in 0..2 {
            let lanes = store.search_lanes().clone();
            let query = corpus.word(4_000).to_owned();
            warm.push(tokio::spawn(async move {
                lanes
                    .with(&SearchStop::new(), Duration::from_secs(5), move |conn| {
                        search::search(conn, &query, 50, &SearchStop::new())
                    })
                    .await
                    .unwrap()
            }));
        }
        for search in warm {
            search.await.unwrap();
        }
        let connections = status_kib("VmRSS").saturating_sub(before);
        lines.push(format!(
            "{}: two lanes open and search one rare word: {connections} KiB",
            size.name()
        ));
        run_on_two_lanes(&store, &probes).await;
        let peak = status_kib("VmHWM").saturating_sub(before);
        let rest = status_kib("VmRSS").saturating_sub(before);
        // The list again: memory that a search kept would grow; the heap that the
        // allocator keeps for the threads does not.
        let mut again = Vec::new();
        for _ in 0..2 {
            run_on_two_lanes(&store, &probes).await;
            again.push(status_kib("VmRSS").saturating_sub(before).to_string());
        }
        store.close().await;
        lines.push(format!(
            "{}: at rest after the list twice more: {} KiB",
            size.name(),
            again.join(" and ")
        ));
        lines.push(format!(
            "{}: {} queries on {} of {SEARCH_LANES} lanes: peak {peak} KiB over the start, {rest} KiB more at rest",
            size.name(),
            probes.len(),
            2
        ));
        if peak > PEAK {
            failures.push(format!("{}: peak {peak} KiB, more than {PEAK} KiB", size.name()));
        }
        if rest > REST {
            failures.push(format!("{}: {rest} KiB at rest, more than {REST} KiB", size.name()));
        }
    }
    report(&format!("bench_search_memory_{}", size.name()), &lines);
    assert!(failures.is_empty(), "{failures:?}");
}

/// The ranges of the matches of `expression` in each doc, by FTS5's `highlight()`.
fn highlighted(
    conn: &Connection,
    expression: &str,
) -> Vec<(i64, String, Vec<std::ops::Range<u32>>)> {
    let mut stmt = conn
        .prepare(
            "SELECT rowid, highlight(search_fts, 0, char(2), char(3)) FROM search_fts \
             WHERE search_fts MATCH ?1 ORDER BY rowid DESC LIMIT 200",
        )
        .unwrap();
    stmt.query_map([expression], |row| {
        let marked: String = row.get(1)?;
        let (text, ranges) = marks::strip(&marked);
        Ok((row.get(0)?, text, ranges))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[tokio::test]
async fn rust_snippet_agrees_with_highlight() {
    let corpus = SearchCorpus::new(CorpusSize::S, SEED);
    let (_dir, path) = corpus_copy(CorpusSize::S, Shape::Indexed).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let mut words: Vec<(String, bool)> =
        (0..10).map(|rank| (corpus.word(rank * 37).to_owned(), false)).collect();
    words.push((common_prefix(&corpus, 3), true));
    words.push((common_prefix(&corpus, 4), true));
    let (compared, docs) = store
        .readers()
        .with(move |conn| {
            let mut compared = 0;
            let mut docs = 0;
            for (word, prefix) in &words {
                let expression =
                    if *prefix { format!("\"{word}\"*") } else { format!("\"{word}\"") };
                let needles = [snippet::Needle::new(std::slice::from_ref(word), *prefix)];
                for (seq, text, ranges) in highlighted(conn, &expression) {
                    let rust: Vec<std::ops::Range<u32>> = snippet::find(&text, &needles)
                        .into_iter()
                        .map(|(_, range)| range)
                        .collect();
                    assert_eq!(rust, ranges, "doc {seq} for {word:?}");
                    compared += ranges.len();
                    docs += 1;
                }
            }
            Ok((compared, docs))
        })
        .await
        .unwrap();
    store.close().await;
    assert!(docs > 500 && compared > 1_000, "{docs} docs, {compared} ranges");
}

/// The first row of a fixed list of queries on corpus S, from titles whose first words
/// are not among the 100 most common words of the vocabulary (a common word fits
/// many titles). The first words of a title find that conversation first, also with a
/// blank after them. The starts of its first two words, which match no whole word of a
/// doc (a match in the title only, which `MIN_TITLE_SCORE` must let through), find it
/// among the first three: a title with the same starts closer together scores higher,
/// as in fzf. The initials of its first four words find it among the rows: in made-up words they
/// fit many titles, so not always first.
#[tokio::test]
async fn queries_find_their_conversation_first() {
    let corpus = SearchCorpus::new(CorpusSize::S, SEED);
    let common: std::collections::HashSet<String> =
        (0..100).map(|rank| corpus.word(rank).to_owned()).collect();
    let (_dir, path) = corpus_copy(CorpusSize::S, Shape::Indexed).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let titles: Vec<(String, String)> = store
        .readers()
        .with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.conversation_id, t.title FROM search_titles AS t \
                 JOIN conversations AS c ON c.id = t.conversation_id ORDER BY c.last_seq LIMIT 100",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap();
    let mut misses = Vec::new();
    let mut asked = 0;
    for (id, title) in titles.iter().step_by(3) {
        let words: Vec<&str> = title
            .split_whitespace()
            .filter(|word| {
                word.len() > 4 && word.chars().all(char::is_alphabetic) && !common.contains(*word)
            })
            .take(4)
            .collect();
        if words.len() < 4 {
            continue;
        }
        let start = |word: &str| word.chars().take(4).collect::<String>();
        let initials: String = words.iter().filter_map(|word| word.chars().next()).collect();
        let queries = [
            (words[..3].join(" "), 1),
            (format!("{} ", words[..3].join(" ")), 1),
            (format!("{} {}", start(words[0]), start(words[1])), 3),
            (initials, 50),
        ];
        for (query, within) in queries {
            asked += 1;
            let (_, found) = timed(&store, &query, 50).await;
            let rank = found.rows.iter().position(|row| row.conversation_id.to_string() == *id);
            if !rank.is_some_and(|rank| rank < within) {
                misses.push(format!("{query:?} gave {id} the rank {rank:?} ({title:?})"));
            }
        }
    }
    store.close().await;
    assert!(asked >= 40, "{asked} queries");
    assert!(misses.is_empty(), "{} of {asked} missed: {misses:#?}", misses.len());
}

#[tokio::test]
async fn fts5_step_runs_past_the_interrupt() {
    let corpus = SearchCorpus::new(CorpusSize::S, SEED);
    let prefix4 = common_prefix(&corpus, 4);
    let (_dir, path) = corpus_copy(CorpusSize::S, Shape::Indexed).await;
    let clock = TestClock::new();
    let store = open(&path, &clock).await;
    let (start_tx, start) = std_mpsc::channel();
    let stop = SearchStop::new();
    let running = {
        let lanes = store.search_lanes().clone();
        let stop = stop.clone();
        let query = format!("{} {prefix4}", corpus.word(0));
        tokio::spawn(async move {
            lanes
                .with(&stop.clone(), WAIT, move |conn| {
                    let _ = start_tx.send(());
                    search::search(conn, &query, 50, &stop)
                })
                .await
        })
    };
    tokio::task::spawn_blocking(move || start.recv()).await.unwrap().unwrap();
    let started = Instant::now();
    stop.hard();
    let result = running.await.unwrap();
    report(
        "fts5_step_runs_past_the_interrupt",
        &[format!(
            "corpus s, {prefix4:?} after a common word: the lane was free {:.3} ms after the stop ({})",
            started.elapsed().as_secs_f64() * 1000.0,
            match &result {
                Ok(found) => format!("stopped with {} rows read", found.rows_scanned),
                Err(error) => format!("{error}"),
            }
        )],
    );
    // Whatever the step did, the lane serves the next search.
    let (_, found) = timed(&store, corpus.word(0), 5).await;
    assert!(!found.rows.is_empty());
    store.close().await;
}
