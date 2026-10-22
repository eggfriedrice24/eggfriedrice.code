use efr_protocol::{Event, Origin, Usage};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::testing::{self, TestClock};
use crate::{Batch, Migrations, Readers, StoreWriter, WriterHandle, db};

fn conversation() -> ConversationId {
    testing::conversation(1)
}

fn seqs(values: &[u64]) -> Vec<Seq> {
    values.iter().copied().map(Seq::new).collect()
}

fn steered(turn: u64, text: &str) -> Event {
    Event::TurnSteered { turn_id: testing::turn(turn), text: text.to_owned() }
}

fn answer(turn: u64, index: u32, text: &str) -> Event {
    Event::AssistantMessageCompleted { turn_id: testing::turn(turn), index, text: text.to_owned() }
}

fn call(turn: u64, call: u64, tool: &str, input: serde_json::Value, freeform: bool) -> Event {
    Event::ToolCallStarted {
        turn_id: testing::turn(turn),
        call_id: testing::call(call),
        tool: tool.to_owned(),
        input,
        freeform,
        manual_input: false,
        launch: None,
    }
}

fn shell(turn: u64, n: u64, command: &str) -> Event {
    call(turn, n, "shell", json!({ "command": command }), false)
}

fn completed(turn: u64) -> Event {
    Event::TurnCompleted {
        turn_id: testing::turn(turn),
        usage: Some(Usage::new(10, 2)),
        changes: None,
        context: None,
        stop: None,
        undo: None,
    }
}

fn job_started(turn: u64) -> Event {
    let job = efr_protocol::JobInfo {
        id: efr_protocol::JobId::nth(1),
        conversation_id: conversation(),
        turn_id: testing::turn(turn),
        call_id: testing::call(90),
        command: "journalctl -f".to_owned(),
        launch: efr_protocol::Launch::contained(),
        state: efr_protocol::JobState::Running,
        started_at: sql::truncate_to_micros(testing::start()),
        ends_by: sql::truncate_to_micros(testing::start()),
        ended_at: None,
        exit_code: None,
        stopped_by: None,
        output: "/d/scratch/x/jobs/j1.out".into(),
        output_bytes: 0,
        watch: None,
        wakes: 0,
        forwards: Vec::new(),
        moved_from: None,
        waits_input: false,
    };
    Event::JobStarted {
        turn_id: job.turn_id,
        call_id: job.call_id,
        job,
        launcher_pid: 4343,
        launcher_start: 99,
    }
}

/// A writer over a new in-memory store.
fn writer() -> (WriterHandle, StoreWriter) {
    testing::memory_writer(TestClock::new())
}

/// Appends the events to the conversation in one batch, and makes the conversation
/// first when it is new.
async fn append(writer: &WriterHandle, events: Vec<Event>) {
    let mut batch = Batch::new();
    for event in events {
        batch = batch.event(conversation(), event);
    }
    writer.append(batch).await.unwrap();
}

async fn create(writer: &WriterHandle) {
    append(writer, vec![testing::created(Some("pts-1"))]).await;
}

async fn read<T: Send + 'static>(
    writer: &WriterHandle,
    f: impl FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
) -> T {
    testing::on_writer(writer, f).await.unwrap()
}

async fn pending_after(writer: &WriterHandle, after: u64) -> Pending {
    read(writer, move |conn| pending(conn, Seq::new(after))).await
}

/// One round of an indexer whose text rules keep the text as it is.
async fn round(writer: &WriterHandle) -> SearchApplied {
    let current = read(writer, state).await;
    let page = read(writer, move |conn| pending(conn, current.indexed_seq)).await;
    let page = SearchPage {
        after: current.indexed_seq,
        through: page.through,
        redaction: current.redaction,
        docs: page.rows,
        drops: page.drops,
    };
    writer.search_apply(page).await.unwrap()
}

/// Rounds until the index has every event.
async fn index_all(writer: &WriterHandle) {
    loop {
        let current = read(writer, state).await;
        if current.indexed_seq == current.newest_seq {
            return;
        }
        assert_eq!(round(writer).await, SearchApplied::Applied);
    }
}

/// The docs of the index as (seq, kind, text).
async fn docs(writer: &WriterHandle) -> Vec<(u64, i64, String)> {
    read(writer, |conn| {
        let mut stmt = conn.prepare("SELECT seq, kind, text FROM search_docs ORDER BY seq")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}

/// The seqs that the full text index finds for `query`.
async fn found(writer: &WriterHandle, query: &str) -> Vec<u64> {
    let query = query.to_owned();
    read(writer, move |conn| {
        let mut stmt =
            conn.prepare("SELECT rowid FROM search_fts WHERE search_fts MATCH ?1 ORDER BY rowid")?;
        let rows = stmt
            .query_map([query], |row| Ok(row.get::<_, i64>(0)? as u64))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}

/// The titles of the index as (conversation, title, first seq).
async fn titles(writer: &WriterHandle) -> Vec<(String, String, u64)> {
    read(writer, |conn| {
        let mut stmt =
            conn.prepare("SELECT conversation_id, title, first_seq FROM search_titles ORDER BY 1")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? as u64)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}

fn title(text: &str, first_seq: u64) -> (String, String, u64) {
    (conversation().to_string(), text.to_owned(), first_seq)
}

fn integrity_check(conn: &Connection) -> Result<(), StoreError> {
    conn.execute("INSERT INTO search_fts (search_fts) VALUES ('integrity-check')", [])?;
    Ok(())
}

#[tokio::test]
async fn pending_reads_the_four_kinds_and_skips_others() {
    let (writer, _thread) = writer();
    create(&writer).await;
    append(
        &writer,
        vec![
            testing::queued(1, "rsync to the laptop"),
            testing::started(1, "/etc/nixos"),
            Event::AssistantMessageUpdated {
                turn_id: testing::turn(1),
                index: 0,
                offset: 0,
                delta: "a delta".to_owned(),
            },
            answer(1, 0, "I will run rsync."),
            shell(1, 1, "rsync -a ~/src laptop:"),
            // A file tool, a freeform input and a command that is not text give no doc.
            call(1, 2, "read_file", json!({ "path": "/etc/hosts" }), false),
            call(1, 3, "shell", json!("rsync as freeform text"), true),
            call(1, 4, "shell", json!({ "command": 42 }), false),
            steered(1, "use --delete too"),
            job_started(1),
            completed(1),
        ],
    )
    .await;

    let page = pending_after(&writer, 0).await;

    let id = conversation();
    assert_eq!(
        page.rows,
        [
            SourceRow {
                seq: Seq::new(2),
                conversation_id: id,
                kind: DocKind::Prompt,
                text: "rsync to the laptop".to_owned(),
            },
            SourceRow {
                seq: Seq::new(5),
                conversation_id: id,
                kind: DocKind::Answer,
                text: "I will run rsync.".to_owned(),
            },
            SourceRow {
                seq: Seq::new(6),
                conversation_id: id,
                kind: DocKind::Command,
                text: "rsync -a ~/src laptop:".to_owned(),
            },
            SourceRow {
                seq: Seq::new(10),
                conversation_id: id,
                kind: DocKind::Steer,
                text: "use --delete too".to_owned(),
            },
        ]
    );
    assert_eq!(page.drops, []);
    assert_eq!(page.through, Seq::new(12));
}

#[tokio::test]
async fn pending_never_passes_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    let mut conn = db::open(&path).unwrap();
    Migrations::new().migrate(&mut conn, None).unwrap();
    let (writer, _thread) =
        StoreWriter::spawn(conn, TestClock::new(), crate::DEFAULT_BROADCAST_CAPACITY).unwrap();
    create(&writer).await;
    append(&writer, vec![testing::queued(1, "first")]).await;

    let reader = db::open_read_only(&path).unwrap();
    let tx = reader.unchecked_transaction().unwrap();
    // The first read starts the snapshot at seq 2.
    assert_eq!(events::last_seq(&tx).unwrap(), Seq::new(2));
    append(&writer, vec![testing::queued(2, "second")]).await;

    let page = pending(&tx, Seq::ZERO).unwrap();

    assert_eq!(page.through, Seq::new(2));
    assert_eq!(page.rows.len(), 1);
    drop(tx);
    let page = pending(&reader, Seq::new(2)).unwrap();
    assert_eq!((page.rows.len(), page.through), (1, Seq::new(3)));
}

#[tokio::test]
async fn pending_page_stops_at_rows_bytes_and_scan() {
    let (writer, _thread) = writer();
    create(&writer).await;

    // Rows: one more prompt than a page holds.
    let prompts = (1..=u64::from(PAGE_ROWS) + 1).map(|n| testing::queued(n, "a prompt")).collect();
    append(&writer, prompts).await;
    let page = pending_after(&writer, 0).await;
    assert_eq!(page.rows.len(), PAGE_ROWS as usize);
    assert_eq!(page.through, page.rows.last().unwrap().seq);
    let last = read(&writer, events::last_seq).await;

    // Bytes: the page ends after the row that reaches the bound.
    let long = "x".repeat(PAGE_BYTES / 2);
    let start = last.get();
    append(&writer, (0..3).map(|n| testing::queued(1000 + n, &long)).collect()).await;
    let page = pending_after(&writer, start).await;
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.through, Seq::new(start + 2));

    // Scan: a long run of events with no text ends a page at its last event.
    let start = read(&writer, events::last_seq).await.get();
    let deltas: Vec<Event> = (0..u64::from(PAGE_SCAN) + 10)
        .map(|n| Event::AssistantMessageUpdated {
            turn_id: testing::turn(1),
            index: 0,
            offset: n,
            delta: "d".to_owned(),
        })
        .collect();
    append(&writer, deltas).await;
    append(&writer, vec![testing::queued(2000, "after the run")]).await;
    let page = pending_after(&writer, start).await;
    assert_eq!(page.rows, []);
    assert_eq!(page.through, Seq::new(start + u64::from(PAGE_SCAN)));
    let page = pending_after(&writer, page.through.get()).await;
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.through, read(&writer, events::last_seq).await);
}

#[tokio::test]
async fn pending_cuts_text_in_sql() {
    let (writer, _thread) = writer();
    create(&writer).await;
    let huge = "é".repeat(1024 * 1024);
    append(&writer, vec![testing::queued(1, &huge)]).await;

    let page = pending_after(&writer, 0).await;

    assert_eq!(page.rows[0].text.chars().count(), SOURCE_CHARS as usize);
    assert!(page.rows[0].text.chars().all(|c| c == 'é'));
}

#[tokio::test]
async fn withdrawn_prompt_is_dropped() {
    let (writer, _thread) = writer();
    create(&writer).await;
    append(&writer, vec![testing::queued(1, "keep this"), testing::queued(2, "take zebra back")])
        .await;
    index_all(&writer).await;
    assert_eq!(found(&writer, "zebra").await, [3]);

    append(
        &writer,
        vec![Event::PromptWithdrawn { turn_id: testing::turn(2), origin: Origin::Shell }],
    )
    .await;
    assert_eq!(pending_after(&writer, 3).await.drops, seqs(&[3]));
    index_all(&writer).await;

    assert_eq!(found(&writer, "zebra").await, Vec::<u64>::new());
    assert_eq!(docs(&writer).await, [(2, 0, "keep this".to_owned())]);
    read(&writer, integrity_check).await;
}

#[tokio::test]
async fn withdrawn_steers_are_dropped() {
    let (writer, _thread) = writer();
    create(&writer).await;
    append(
        &writer,
        vec![
            testing::queued(1, "start"),
            testing::started(1, "/etc/nixos"),
            steered(1, "zebra one"),
            steered(1, "zebra two"),
            steered(1, "kept steer"),
        ],
    )
    .await;
    // The steers and their withdrawal in one page.
    append(
        &writer,
        vec![Event::SteeringWithdrawn {
            turn_id: testing::turn(1),
            steers: seqs(&[4, 5]),
            origin: Origin::Shell,
        }],
    )
    .await;

    index_all(&writer).await;

    assert_eq!(found(&writer, "zebra").await, Vec::<u64>::new());
    assert_eq!(found(&writer, "kept").await, [6]);
    read(&writer, integrity_check).await;
}

#[tokio::test]
async fn resent_steers_are_dropped() {
    let (writer, _thread) = writer();
    create(&writer).await;
    append(
        &writer,
        vec![testing::queued(1, "start"), testing::started(1, "/etc/nixos"), steered(1, "zebra")],
    )
    .await;
    index_all(&writer).await;
    assert_eq!(found(&writer, "zebra").await, [4]);

    let Event::PromptQueued { turn_id, command_id, origin, context, settings, .. } =
        testing::queued(2, "")
    else {
        unreachable!("queued makes a prompt_queued");
    };
    let resent = Event::PromptQueued {
        turn_id,
        command_id,
        text: "zebra again".to_owned(),
        origin,
        context,
        settings,
        steers: seqs(&[4]),
    };
    append(&writer, vec![resent]).await;
    index_all(&writer).await;

    assert_eq!(found(&writer, "zebra").await, [5]);
}

#[tokio::test]
async fn first_prompt_names_the_conversation() {
    let (writer, _thread) = writer();
    create(&writer).await;
    append(
        &writer,
        vec![
            testing::queued(1, "  \n\t\n"),
            answer(1, 0, "an answer is no title"),
            testing::queued(2, "\n  docker compose up  \nmore"),
            testing::queued(3, "a later prompt"),
        ],
    )
    .await;

    index_all(&writer).await;

    assert_eq!(titles(&writer).await, [title("docker compose up", 4)]);
}

#[tokio::test]
async fn withdrawn_first_prompt_leaves_the_title() {
    let (writer, _thread) = writer();
    create(&writer).await;
    append(&writer, vec![testing::queued(1, "secret plan"), testing::queued(2, "second plan")])
        .await;
    index_all(&writer).await;
    assert_eq!(titles(&writer).await, [title("secret plan", 2)]);

    append(
        &writer,
        vec![Event::PromptWithdrawn { turn_id: testing::turn(1), origin: Origin::Shell }],
    )
    .await;
    index_all(&writer).await;
    assert_eq!(titles(&writer).await, [title("second plan", 3)]);

    append(
        &writer,
        vec![Event::PromptWithdrawn { turn_id: testing::turn(2), origin: Origin::Shell }],
    )
    .await;
    index_all(&writer).await;
    assert_eq!(titles(&writer).await, []);
}

#[tokio::test]
async fn stale_page_writes_nothing() {
    let (writer, _thread) = writer();
    create(&writer).await;
    append(&writer, vec![testing::queued(1, "rsync")]).await;
    let doc = SourceRow {
        seq: Seq::new(2),
        conversation_id: conversation(),
        kind: DocKind::Prompt,
        text: "rsync".to_owned(),
    };
    let page = SearchPage {
        after: Seq::ZERO,
        through: Seq::new(2),
        redaction: 0,
        docs: vec![doc],
        drops: Vec::new(),
    };

    let wrong_after = SearchPage { after: Seq::new(1), ..page.clone() };
    assert_eq!(writer.search_apply(wrong_after).await.unwrap(), SearchApplied::Stale);
    let wrong_redaction = SearchPage { redaction: 1, ..page.clone() };
    assert_eq!(writer.search_apply(wrong_redaction).await.unwrap(), SearchApplied::Stale);
    assert_eq!(docs(&writer).await, []);
    assert_eq!(read(&writer, state).await.indexed_seq, Seq::ZERO);

    assert_eq!(writer.search_apply(page.clone()).await.unwrap(), SearchApplied::Applied);
    // The same page again is stale: the cursor moved.
    assert_eq!(writer.search_apply(page).await.unwrap(), SearchApplied::Stale);
    assert_eq!(docs(&writer).await.len(), 1);
}

#[tokio::test]
async fn reset_clears_every_search_table() {
    let (writer, _thread) = writer();
    create(&writer).await;
    append(&writer, vec![testing::queued(1, "rsync laptop"), shell(1, 1, "rsync -a a b")]).await;
    index_all(&writer).await;
    assert_eq!(docs(&writer).await.len(), 2);

    writer.search_reset(7).await.unwrap();

    assert_eq!(docs(&writer).await, []);
    assert_eq!(titles(&writer).await, []);
    assert_eq!(found(&writer, "rsync").await, Vec::<u64>::new());
    let current = read(&writer, state).await;
    assert_eq!((current.indexed_seq, current.redaction), (Seq::ZERO, 7));
    read(&writer, integrity_check).await;
    // The backfill fills it again with the new version.
    index_all(&writer).await;
    assert_eq!(found(&writer, "rsync").await, [2, 3]);
    assert_eq!(read(&writer, state).await.redaction, 7);
}

#[tokio::test]
async fn delete_of_a_missing_seq_is_no_error() {
    let (writer, _thread) = writer();
    create(&writer).await;
    let page = SearchPage {
        after: Seq::ZERO,
        through: Seq::new(1),
        redaction: 0,
        docs: Vec::new(),
        drops: seqs(&[1, 99]),
    };

    assert_eq!(writer.search_apply(page).await.unwrap(), SearchApplied::Applied);
    assert_eq!(read(&writer, state).await.indexed_seq, Seq::new(1));
}

#[tokio::test]
async fn merge_reports_its_work() {
    let (writer, _thread) = writer();
    create(&writer).await;
    assert!(!writer.search_merge(64).await.unwrap(), "an empty index has nothing to merge");
    // Each round makes a segment of its own.
    for turn in 1..=6 {
        append(&writer, vec![testing::queued(turn, &format!("prompt number {turn}"))]).await;
        index_all(&writer).await;
    }

    let mut merges = 0;
    while writer.search_merge(64).await.unwrap() {
        merges += 1;
        assert!(merges < 100, "the merges do not end");
    }

    assert!(merges >= 1);
    assert!(!writer.search_merge(64).await.unwrap());
    assert_eq!(found(&writer, "number").await, [2, 3, 4, 5, 6, 7]);
    read(&writer, integrity_check).await;
}

#[tokio::test]
async fn turn_end_batch_writes_no_search_row() {
    let (writer, _thread) = writer();
    create(&writer).await;
    let before = read(&writer, |conn| Ok(search_tables(conn))).await;

    append(
        &writer,
        vec![
            testing::queued(1, "rsync to the laptop"),
            testing::started(1, "/etc/nixos"),
            answer(1, 0, "done"),
            shell(1, 1, "rsync -a a b"),
            completed(1),
        ],
    )
    .await;

    assert_eq!(read(&writer, |conn| Ok(search_tables(conn))).await, before);
    assert_eq!(read(&writer, state).await.indexed_seq, Seq::ZERO);
}

/// Every row of every search table, the shadow tables of the index included.
fn search_tables(conn: &Connection) -> Vec<Vec<String>> {
    [
        "search_docs",
        "search_titles",
        "search_state",
        "search_fts_data",
        "search_fts_idx",
        "search_fts_docsize",
    ]
    .iter()
    .map(|table| testing::dump(conn, table))
    .collect()
}

#[test]
fn doc_kinds_keep_their_codes() {
    for (kind, code) in
        [(DocKind::Prompt, 0), (DocKind::Steer, 1), (DocKind::Answer, 2), (DocKind::Command, 3)]
    {
        assert_eq!(kind.code(), code);
        assert_eq!(DocKind::from_code(code), Some(kind));
    }
    assert_eq!(DocKind::from_code(4), None);
}

/// One step of the proptest: a page of new docs and drops, or a merge.
#[derive(Debug, Clone)]
enum Step {
    Page { docs: Vec<(u8, u8, String)>, drops: Vec<u16> },
    Merge(u32),
}

fn step() -> impl Strategy<Value = Step> {
    let word = prop::sample::select(vec!["rsync", "laptop", "docker", "src/a", "zebra", "é"]);
    let text = prop::collection::vec(word, 1..6).prop_map(|words| words.join(" "));
    let doc = (0u8..3, 0u8..4, text);
    prop_oneof![
        3 => (prop::collection::vec(doc, 0..8), prop::collection::vec(any::<u16>(), 0..4))
            .prop_map(|(docs, drops)| Step::Page { docs, drops }),
        1 => (1u32..16).prop_map(Step::Merge),
    ]
}

/// The docs of the index, as FTS5 counts them.
fn fts_docs(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM search_fts_docsize", [], |row| row.get(0)).unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn fts_and_docs_stay_in_step(steps in prop::collection::vec(step(), 1..24)) {
        let mut conn = db::open_in_memory().unwrap();
        Migrations::new().migrate(&mut conn, None).unwrap();
        let mut cursor = 0u64;
        let mut written: Vec<u64> = Vec::new();
        for step in steps {
            let tx = conn.transaction().unwrap();
            match step {
                Step::Page { docs, drops } => {
                    let after = cursor;
                    let docs: Vec<SourceRow> = docs
                        .into_iter()
                        .map(|(conversation, kind, text)| {
                            cursor += 1;
                            written.push(cursor);
                            SourceRow {
                                seq: Seq::new(cursor),
                                conversation_id: testing::conversation(u64::from(conversation)),
                                kind: DocKind::from_code(i64::from(kind)).unwrap(),
                                text,
                            }
                        })
                        .collect();
                    // A drop names a doc that was written, or one that never was.
                    let drops = drops
                        .into_iter()
                        .map(|pick| match written.len() {
                            0 => Seq::new(u64::from(pick)),
                            len => Seq::new(written[usize::from(pick) % len]),
                        })
                        .collect();
                    cursor += 1;
                    let page = SearchPage {
                        after: Seq::new(after),
                        through: Seq::new(cursor),
                        redaction: 0,
                        docs,
                        drops,
                    };
                    prop_assert_eq!(apply(&tx, &page).unwrap(), SearchApplied::Applied);
                }
                Step::Merge(pages) => {
                    merge(&tx, pages).unwrap();
                }
            }
            tx.commit().unwrap();
            integrity_check(&conn).unwrap();
            let rows: i64 =
                conn.query_row("SELECT COUNT(*) FROM search_docs", [], |row| row.get(0)).unwrap();
            prop_assert_eq!(fts_docs(&conn), rows);
        }
    }
}

#[tokio::test]
async fn readers_see_the_index_through_the_pool() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    let mut conn = db::open(&path).unwrap();
    Migrations::new().migrate(&mut conn, None).unwrap();
    let (writer, _thread) = StoreWriter::spawn(conn, TestClock::new(), 16).unwrap();
    create(&writer).await;
    append(&writer, vec![testing::queued(1, "rsync")]).await;
    let readers = Readers::open(&path, 1);

    let page = readers.with(|conn| pending(conn, Seq::ZERO)).await.unwrap();

    assert_eq!(page.rows.len(), 1);
}
