#![allow(
    clippy::single_range_in_vec_init,
    reason = "ranges of code points are the values under test"
)]

use std::sync::Arc;

use efr_protocol::{Event, Usage};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::search::{DocKind, SearchApplied, SearchPage, SourceRow, pending};
use crate::testing::{self, TestClock};
use crate::{Batch, Migrations, Store, StoreWriter, WriterHandle, conversations, db};

/// An in-memory store whose conversations are a minute apart.
struct Fixture {
    writer: WriterHandle,
    _thread: StoreWriter,
    clock: Arc<TestClock>,
    turns: u64,
}

impl Fixture {
    fn new() -> Self {
        let clock = TestClock::new();
        let (writer, thread) = testing::memory_writer(Arc::clone(&clock));
        Fixture { writer, _thread: thread, clock, turns: 0 }
    }

    /// Appends one turn to conversation `n` (created with its first turn): a prompt,
    /// the answers and one `shell` command per entry of `commands`.
    async fn turn(&mut self, n: u64, prompt: &str, answers: &[&str], commands: &[&str]) {
        self.turns += 1;
        let turn = self.turns;
        let id = testing::conversation(n);
        let known = self.read(move |conn| conversations::get(conn, id)).await.is_some();
        let mut events = Vec::new();
        if !known {
            events.push(testing::created(Some("pts-1")));
        }
        events.push(testing::queued(turn, prompt));
        events.push(testing::started(turn, "/etc/nixos"));
        for (index, answer) in answers.iter().enumerate() {
            events.push(Event::AssistantMessageCompleted {
                turn_id: testing::turn(turn),
                index: index as u32,
                text: (*answer).to_owned(),
            });
        }
        for (index, command) in commands.iter().enumerate() {
            events.push(Event::ToolCallStarted {
                turn_id: testing::turn(turn),
                call_id: testing::call(turn * 100 + index as u64),
                tool: "shell".to_owned(),
                input: json!({ "command": command }),
                freeform: false,
                manual_input: false,
                launch: None,
            });
        }
        events.push(Event::TurnCompleted {
            turn_id: testing::turn(turn),
            usage: Some(Usage::new(1, 1)),
            changes: None,
            context: None,
            stop: None,
            undo: None,
        });
        let mut batch = Batch::new();
        for event in events {
            batch = batch.event(id, event);
        }
        self.clock.advance(std::time::Duration::from_secs(60));
        self.writer.append(batch).await.unwrap();
    }

    /// Indexes every event, with text rules that keep the text as it is.
    async fn index(&self) {
        loop {
            let current = self.read(state).await;
            if current.indexed_seq == current.newest_seq {
                return;
            }
            let page = self.read(move |conn| pending(conn, current.indexed_seq)).await;
            let page = SearchPage {
                after: current.indexed_seq,
                through: page.through,
                redaction: current.redaction,
                docs: page.rows,
                drops: page.drops,
            };
            assert_eq!(self.writer.search_apply(page).await.unwrap(), SearchApplied::Applied);
        }
    }

    async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    ) -> T {
        testing::on_writer(&self.writer, f).await.unwrap()
    }

    async fn search(&self, query: &str) -> Found {
        let query = query.to_owned();
        self.read(move |conn| search(conn, &query, 50, &SearchStop::new())).await
    }

    async fn search_in(&self, n: u64, query: &str) -> FoundIn {
        let query = query.to_owned();
        self.read(move |conn| {
            search_in(conn, testing::conversation(n), &query, 20, &SearchStop::new())
        })
        .await
    }
}

fn ids(found: &Found) -> Vec<ConversationId> {
    found.rows.iter().map(|row| row.conversation_id).collect()
}

fn conv(n: u64) -> ConversationId {
    testing::conversation(n)
}

/// The text of each range of a line.
fn marked(line: &SearchLine) -> Vec<String> {
    line.ranges
        .iter()
        .map(|range| {
            line.text
                .chars()
                .skip(range.start as usize)
                .take((range.end - range.start) as usize)
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn query_finds_prompts_answers_and_commands() {
    let mut fx = Fixture::new();
    fx.turn(1, "sync the photos", &["the laptop holds them"], &["rsync -a ~/pics nas:"]).await;
    fx.turn(2, "fix the build", &["done"], &["cargo build"]).await;
    fx.index().await;

    for (query, kind) in [
        ("photos", SearchKind::Prompt),
        ("laptop", SearchKind::Answer),
        ("rsync", SearchKind::Command),
    ] {
        let found = fx.search(query).await;
        assert_eq!(ids(&found), vec![conv(1)], "query {query}");
        let hit = found.rows[0].hit.as_ref().unwrap();
        assert_eq!(hit.kind, kind, "query {query}");
        assert_eq!(hit.turn_id, Some(testing::turn(1)));
        assert_eq!(found.partial, None);
    }
    assert_eq!(fx.search("cargo build").await.rows[0].title.as_deref(), Some("fix the build"));
    assert!(fx.search("absentword").await.rows.is_empty());
    assert!(fx.search("").await.rows.is_empty(), "the empty query is the recent list");
}

#[tokio::test]
async fn hit_is_one_document() {
    let mut fx = Fixture::new();
    fx.turn(1, "rsync the files", &["to the laptop"], &[]).await;
    fx.turn(2, "rsync to the laptop", &[], &[]).await;
    fx.index().await;
    let found = fx.search("rsync laptop").await;
    assert_eq!(ids(&found), vec![conv(2)], "two words in two docs do not match");
}

#[tokio::test]
async fn title_only_match() {
    let mut fx = Fixture::new();
    fx.turn(1, "docker compose for the media server", &["ok"], &[]).await;
    fx.index().await;
    // `dcms` is no word of any doc, but a subsequence of the title.
    let found = fx.search("dcms").await;
    assert_eq!(ids(&found), vec![conv(1)]);
    let row = &found.rows[0];
    assert!(row.hit.is_none());
    assert_eq!(row.title.as_deref(), Some("docker compose for the media server"));
    assert!(!row.title_ranges.is_empty());
    assert!(fx.search("dcmz").await.rows.is_empty(), "every character must match");
}

#[tokio::test]
async fn match_in_title_and_text_ranks_first() {
    let mut fx = Fixture::new();
    fx.turn(1, "docker compose", &["fine"], &[]).await;
    fx.turn(2, "something else entirely", &["fine"], &[]).await;
    fx.turn(2, "docker compose", &["fine"], &[]).await;
    fx.index().await;
    let found = fx.search("docker compose").await;
    assert_eq!(ids(&found), vec![conv(1), conv(2)], "the title match wins over recency");
}

#[tokio::test]
async fn near_tie_goes_to_the_newer() {
    let mut fx = Fixture::new();
    fx.turn(1, "alpha one", &["zebra crossing"], &[]).await;
    fx.turn(2, "beta two", &["zebra crossing"], &[]).await;
    fx.index().await;
    assert_eq!(ids(&fx.search("zebra").await), vec![conv(2), conv(1)]);
    fx.turn(1, "more", &[], &[]).await;
    fx.index().await;
    assert_eq!(ids(&fx.search("zebra").await), vec![conv(1), conv(2)], "a new turn makes it newer");
}

#[tokio::test]
async fn snippet_ranges_point_at_the_words() {
    let mut fx = Fixture::new();
    fx.turn(1, "hello", &["first line\nthen rsync -a src/app to the Laptop now\nlast"], &[]).await;
    fx.index().await;
    let found = fx.search("src/app lap").await;
    let hit = found.rows[0].hit.as_ref().unwrap();
    assert_eq!(hit.lines.len(), 1);
    assert_eq!(hit.lines[0].text, "then rsync -a src/app to the Laptop now");
    assert_eq!(marked(&hit.lines[0]), vec!["src/app", "Laptop"]);
}

#[tokio::test]
async fn list_runs_no_fts_statement_per_row() {
    // The snippet of each row comes from its doc's text through the snippet module,
    // not from FTS5's `highlight()`, so the list runs the one capped FTS5 statement.
    let mut fx = Fixture::new();
    for n in 1..=5 {
        fx.turn(n, &format!("title {n}"), &[&format!("the word kestrel in answer {n}")], &[]).await;
    }
    fx.index().await;
    let found = fx.search("kestrel").await;
    assert_eq!(found.rows.len(), 5);
    for row in &found.rows {
        let hit = row.hit.as_ref().unwrap();
        let seq = sql::seq(hit.seq);
        let text: String = fx
            .read(move |conn| {
                Ok(conn.query_row("SELECT text FROM search_docs WHERE seq = ?1", [seq], |row| {
                    row.get(0)
                })?)
            })
            .await;
        let needle = Needle::new(&["kestrel".to_owned()], true);
        let (line, ranges) = snippet::snippet(&text, &[needle]);
        assert_eq!(hit.lines, vec![SearchLine { text: line, ranges: text_ranges(&ranges) }]);
    }
}

#[tokio::test]
async fn row_cap_gives_partial() {
    let fx = Fixture::new();
    fx.writer.append(Batch::new().event(conv(1), testing::created(Some("pts-1")))).await.unwrap();
    let rows = ROW_CAP as u64 + 1;
    let docs = (0..rows)
        .map(|n| SourceRow {
            seq: Seq::new(n + 2),
            conversation_id: conv(1),
            kind: DocKind::Answer,
            text: "plover".to_owned(),
        })
        .collect();
    let page = SearchPage {
        after: Seq::ZERO,
        through: Seq::new(rows + 1),
        redaction: 0,
        docs,
        drops: Vec::new(),
    };
    assert_eq!(fx.writer.search_apply(page).await.unwrap(), SearchApplied::Applied);
    let found = fx.search("plover").await;
    assert_eq!(found.partial, Some(SearchPartial::RowCap));
    assert_eq!(found.rows_scanned, ROW_CAP);
    assert_eq!(found.conversations_scored, 1);
    assert_eq!(ids(&found), vec![conv(1)]);
}

#[tokio::test]
async fn soft_stop_ends_the_row_loop() {
    let mut fx = Fixture::new();
    for n in 1..=5 {
        fx.turn(n, &format!("plover {n}"), &[], &[]).await;
    }
    fx.index().await;
    let found = fx
        .read(|conn| {
            let stop = SearchStop::new();
            let inner = stop.clone();
            let stop = stop.on_rows(2, Arc::new(move |_| inner.soft()));
            search(conn, "plover", 50, &stop)
        })
        .await;
    assert_eq!(found.partial, Some(SearchPartial::TimeCap));
    assert_eq!(found.rows_scanned, 2);
    assert_eq!(found.titles_scored, 0, "the stop also skips the titles");
    assert_eq!(ids(&found), vec![conv(5), conv(4)], "the rows seen, ranked");
}

#[tokio::test]
async fn preview_lines_and_context() {
    let mut fx = Fixture::new();
    let answer: Vec<String> = (0..20)
        .map(|i| if i == 10 { "the ibis line".to_owned() } else { format!("line {i}") })
        .collect();
    fx.turn(1, "birds", &[&answer.join("\n")], &["echo ibis"]).await;
    fx.index().await;
    let found = fx.search_in(1, "ibis").await;
    assert_eq!(found.title.as_ref().map(|title| title.text.as_str()), Some("birds"));
    assert_eq!(found.hits.len(), 2);
    let command = &found.hits[0];
    assert_eq!(command.kind, SearchKind::Command, "newest first");
    assert_eq!(marked(&command.lines[0]), vec!["ibis"]);
    let lines: Vec<&str> = found.hits[1].lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(lines, vec!["line 9", "the ibis line", "line 11"]);
    assert_eq!(marked(&found.hits[1].lines[1]), vec!["ibis"]);
    assert!(fx.search_in(1, "absentword").await.hits.is_empty());
    assert!(fx.search_in(2, "ibis").await.hits.is_empty(), "another conversation has none");
}

#[tokio::test]
async fn preview_title_has_its_ranges() {
    let mut fx = Fixture::new();
    fx.turn(1, "rsync to the laptop", &[], &[]).await;
    fx.index().await;
    let found = fx.search_in(1, "lap").await;
    assert_eq!(marked(found.title.as_ref().unwrap()), vec!["lap"]);
}

#[tokio::test]
async fn empty_query_preview() {
    let mut fx = Fixture::new();
    let long: Vec<String> = (0..30).map(|i| format!("answer line {i}")).collect();
    fx.turn(1, "prompt one", &["old answer"], &[]).await;
    fx.turn(1, "prompt two", &[], &[]).await;
    fx.turn(1, "prompt three", &[], &[]).await;
    fx.turn(1, "prompt four", &[&long.join("\n")], &["ls"]).await;
    fx.index().await;
    let found = fx.search_in(1, "  ").await;
    let shown: Vec<(SearchKind, &str, usize)> = found
        .hits
        .iter()
        .map(|hit| (hit.kind, hit.lines[0].text.as_str(), hit.lines.len()))
        .collect();
    assert_eq!(
        shown,
        vec![
            (SearchKind::Answer, "answer line 0", snippet::PREVIEW_LINES),
            (SearchKind::Prompt, "prompt four", 1),
            (SearchKind::Prompt, "prompt three", 1),
            (SearchKind::Prompt, "prompt two", 1),
        ]
    );
    assert!(found.hits.iter().all(|hit| hit.lines.iter().all(|line| line.ranges.is_empty())));
}

#[tokio::test]
async fn short_ids_are_unique() {
    let fx = Fixture::new();
    let texts = [
        "019a9b1c-0000-7000-8000-000000000001",
        "019a9b1c-0000-7000-8000-000000000002",
        "019a9b1c-1000-7000-8000-000000000001",
        "019a9b1d-0000-7000-8000-000000000001",
        "019a9b2f-0000-7000-8000-000000000001",
    ];
    let ids: Vec<ConversationId> = texts.iter().map(|text| text.parse().unwrap()).collect();
    for id in &ids {
        fx.writer.append(Batch::new().event(*id, testing::created(None))).await.unwrap();
    }
    let short = fx.read(move |conn| conversations::short_ids(conn, &ids)).await;
    assert_eq!(
        short,
        vec![
            "019a9b1c-0000-7000-8000-000000000001",
            "019a9b1c-0000-7000-8000-000000000002",
            "019a9b1c-1",
            "019a9b1d",
            "019a9b2f",
        ]
    );
}

#[tokio::test]
async fn rows_and_titles_keep_the_order_of_the_ids() {
    let mut fx = Fixture::new();
    fx.turn(1, "first title", &[], &[]).await;
    fx.turn(2, "second title", &[], &[]).await;
    fx.index().await;
    fx.turn(3, "not indexed yet", &[], &[]).await;
    let wanted = vec![conv(2), conv(9), conv(1), conv(3)];
    let asked = wanted.clone();
    let rows = fx.read(move |conn| conversations::rows(conn, &asked)).await;
    let found: Vec<ConversationId> = rows.iter().map(|row| row.summary.id).collect();
    assert_eq!(found, vec![conv(2), conv(1), conv(3)], "an unknown id has no row");
    assert_eq!(rows[0].turns, 1);
    let titles = fx.read(move |conn| titles(conn, &wanted)).await;
    assert_eq!(
        titles,
        vec![Some("second title".to_owned()), None, Some("first title".to_owned()), None]
    );
}

#[tokio::test]
async fn index_percent_while_backfilling() {
    let fx = Fixture::new();
    assert_eq!(fx.read(index_percent).await, None, "an empty log is complete");
    let mut batch = Batch::new();
    for _ in 0..=LIVE_LAG {
        batch = batch.global_event(Event::LoginCompleted { provider: "a".to_owned() });
    }
    fx.writer.append(batch).await.unwrap();
    assert_eq!(fx.read(index_percent).await, Some(0));
    fx.index().await;
    assert_eq!(fx.read(index_percent).await, None);
}

#[tokio::test]
async fn query_limits_are_errors() {
    assert!(matches!(
        check_query(&"a".repeat(query::MAX_CHARS + 1)),
        Err(StoreError::QueryTooLong { limit: query::MAX_CHARS, .. })
    ));
    assert!(matches!(
        check_query(&vec!["w"; query::MAX_TERMS + 1].join(" ")),
        Err(StoreError::QueryTooManyTerms { limit: query::MAX_TERMS, .. })
    ));
    assert!(check_query("rsync laptop").is_ok());
}

/// The query plan of `sql`, one line per step.
fn plan(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let count = stmt.parameter_count();
    let params: Vec<rusqlite::types::Value> =
        (0..count).map(|_| rusqlite::types::Value::Text("x".to_owned())).collect();
    stmt.query_map(rusqlite::params_from_iter(params), |row| row.get::<_, String>(3))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn migrated() -> Connection {
    let mut conn = db::open_in_memory().unwrap();
    Migrations::new().migrate(&mut conn, None).unwrap();
    conn
}

#[test]
fn rows_are_newest_first_without_a_sort() {
    let conn = migrated();
    let fts = plan(&conn, FTS_SQL);
    assert!(fts.iter().all(|step| !step.contains("TEMP B-TREE")), "{fts:?}");
    assert_eq!(fts.len(), 1, "one scan of the full text index: {fts:?}");
    assert!(fts[0].contains("VIRTUAL TABLE"), "{fts:?}");
    let titles = plan(&conn, TITLES_SQL);
    assert!(titles.iter().all(|step| !step.contains("TEMP B-TREE")), "{titles:?}");
    assert!(titles.iter().any(|step| step.contains("conversations_updated")), "{titles:?}");
}

#[test]
fn preview_is_one_statement() {
    let conn = migrated();
    let preview = plan(&conn, PREVIEW_SQL);
    assert_eq!(preview.len(), 1, "{preview:?}");
    assert!(!preview[0].contains("TEMP B-TREE"), "{preview:?}");
    // FTS5's index string: `M` for the match, `<` and `>` for the bounds of the rowid,
    // `D` for the descending order.
    let index = preview[0].rsplit(':').next().unwrap_or_default();
    assert!(
        index.contains('<') && index.contains('>'),
        "the rowid range goes to FTS5: {preview:?}"
    );
}

#[tokio::test]
async fn a_long_prefix_expands_to_the_words_of_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let mut config = crate::StoreConfig::in_data_dir(dir.path());
    config.backups = None;
    let store = Store::open(config, clock.clone()).await.unwrap();
    let id = conv(1);
    let mut batch = Batch::new().event(id, testing::created(Some("pts-1")));
    for (n, text) in
        ["laptop", "Laptops here", "lapis", "lap", "a laptopish word"].iter().enumerate()
    {
        batch = batch.event(id, testing::queued(n as u64 + 1, text));
    }
    store.writer().append(batch).await.unwrap();
    let current = store.index_reader().with(state).await.unwrap();
    let page =
        store.index_reader().with(move |conn| pending(conn, current.indexed_seq)).await.unwrap();
    let page = SearchPage {
        after: Seq::ZERO,
        through: page.through,
        redaction: 0,
        docs: page.rows,
        drops: page.drops,
    };
    store.writer().search_apply(page).await.unwrap();
    let lanes = store.search_lanes().clone();
    let (expanded, found, plain) = lanes
        .with(&SearchStop::new(), std::time::Duration::from_millis(200), |conn| {
            let parsed = query::parse("lapt").unwrap();
            let expanded = expression(conn, &parsed)?.unwrap();
            let found = search(conn, "lapt", 50, &SearchStop::new())?;
            let mut stmt = conn.prepare(
                "SELECT rowid FROM search_fts WHERE search_fts MATCH ?1 ORDER BY rowid DESC",
            )?;
            let mut rowids = |expression: &str| -> Result<Vec<i64>, StoreError> {
                Ok(stmt.query_map([expression], |row| row.get(0))?.collect::<Result<_, _>>()?)
            };
            let plain = (rowids(&expanded.text)?, rowids("\"lapt\"*")?);
            Ok(((expanded.text, expanded.long_prefix), found, plain))
        })
        .await
        .unwrap();
    assert_eq!(expanded, ("(\"laptop\" OR \"laptopish\" OR \"laptops\")".to_owned(), false));
    assert_eq!(plain.0, plain.1, "the expansion finds the rows of the prefix");
    assert_eq!(plain.0.len(), 3);
    assert!(found.bm25, "an expanded prefix ranks with bm25()");
    assert_eq!(ids(&found), vec![id]);
    let words = lanes
        .with(&SearchStop::new(), std::time::Duration::from_millis(200), |conn| {
            prefix_words(conn, "zzzz")
        })
        .await
        .unwrap();
    assert_eq!(words, Some(vec![]));
    drop(lanes);
    store.close().await;
}

#[tokio::test]
async fn a_long_prefix_with_many_words_stays_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = crate::StoreConfig::in_data_dir(dir.path());
    config.backups = None;
    let store = Store::open(config, TestClock::new()).await.unwrap();
    let id = conv(1);
    let words: Vec<String> = (0..=EXPAND_WORDS).map(|n| format!("wordy{n:03}")).collect();
    let batch = Batch::new()
        .event(id, testing::created(Some("pts-1")))
        .event(id, testing::queued(1, &words.join(" ")));
    store.writer().append(batch).await.unwrap();
    let current = store.index_reader().with(state).await.unwrap();
    let page =
        store.index_reader().with(move |conn| pending(conn, current.indexed_seq)).await.unwrap();
    let page = SearchPage {
        after: Seq::ZERO,
        through: page.through,
        redaction: 0,
        docs: page.rows,
        drops: page.drops,
    };
    store.writer().search_apply(page).await.unwrap();
    let (expanded, found) = store
        .search_lanes()
        .with(&SearchStop::new(), std::time::Duration::from_millis(200), |conn| {
            let expanded = expression(conn, &query::parse("wordy").unwrap())?.unwrap();
            Ok((
                (expanded.text, expanded.long_prefix),
                search(conn, "wordy", 50, &SearchStop::new())?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(expanded, ("\"wordy\"*".to_owned(), true));
    assert!(!found.bm25, "a long prefix ranks without bm25()");
    assert_eq!(ids(&found), vec![id]);
    store.close().await;
}

#[tokio::test]
async fn an_in_memory_store_searches_through_its_lanes() {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock).await.unwrap();
    let id = conv(1);
    let batch = Batch::new()
        .event(id, testing::created(Some("pts-1")))
        .event(id, testing::queued(1, "find the heron"));
    store.writer().append(batch).await.unwrap();
    let current = store.index_reader().with(state).await.unwrap();
    let page =
        store.index_reader().with(move |conn| pending(conn, current.indexed_seq)).await.unwrap();
    let page = SearchPage {
        after: current.indexed_seq,
        through: page.through,
        redaction: 0,
        docs: page.rows,
        drops: page.drops,
    };
    store.writer().search_apply(page).await.unwrap();
    let found = store
        .search_lanes()
        .with(&SearchStop::new(), std::time::Duration::from_millis(200), |conn| {
            search(conn, "heron", 50, &SearchStop::new())
        })
        .await
        .unwrap();
    assert_eq!(ids(&found), vec![id]);
    store.close().await;
}

proptest! {
    #[test]
    fn parsed_queries_are_valid_fts5(query in "\\PC{0,60}") {
        let conn = db::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5 (text, tokenize = 'unicode61 remove_diacritics 2', prefix = '3');
             INSERT INTO t (text) VALUES ('some text to match');",
        )
        .unwrap();
        if let Ok(parsed) = query::parse(&query)
            && let Some(expression) = parsed.fts
        {
            let result: Result<i64, _> =
                conn.query_row("SELECT count(*) FROM t WHERE t MATCH ?1", [&expression], |row| row.get(0));
            prop_assert!(result.is_ok(), "{expression:?}: {result:?}");
        }
    }

    #[test]
    fn quote_and_symbol_queries_are_valid_fts5(query in "[a-z\"*()^:+\\- ]{0,30}") {
        let conn = db::open_in_memory().unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE t USING fts5 (text, prefix = '3');").unwrap();
        if let Ok(parsed) = query::parse(&query)
            && let Some(expression) = parsed.fts
        {
            let result: Result<i64, _> =
                conn.query_row("SELECT count(*) FROM t WHERE t MATCH ?1", [&expression], |row| row.get(0));
            prop_assert!(result.is_ok(), "{expression:?}: {result:?}");
        }
    }
}
