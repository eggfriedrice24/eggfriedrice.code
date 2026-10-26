use efr_protocol::{ApprovalDecision, Event, Origin, Usage};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::outbox::NewOutboxItem;
use crate::receipts::NewReceipt;
use crate::search::{SearchApplied, SearchPage};
use crate::testing::{self, TestClock};
use crate::turn_messages::NewTurnMessages;
use crate::{Batch, WriterHandle, conversations, events, search};

/// The conversation that goes, and the one that stays.
const GONE: u64 = 1;
const KEPT: u64 = 2;

fn id(n: u64) -> ConversationId {
    testing::conversation(n)
}

/// Turn numbers per conversation, so the ids never meet.
fn turn(conversation: u64, n: u64) -> u64 {
    conversation * 10 + n
}

fn answer(turn: u64, text: &str) -> Event {
    Event::AssistantMessageCompleted {
        turn_id: testing::turn(turn),
        index: 0,
        text: text.to_owned(),
    }
}

fn shell_call(turn: u64, call: u64, command: &str) -> Event {
    Event::ToolCallStarted {
        turn_id: testing::turn(turn),
        call_id: testing::call(call),
        tool: "shell".to_owned(),
        input: json!({ "command": command }),
        freeform: false,
        manual_input: false,
        launch: None,
    }
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

fn job_started(conversation: u64, turn: u64) -> Event {
    let job = efr_protocol::JobInfo {
        id: efr_protocol::JobId::nth(1),
        conversation_id: id(conversation),
        turn_id: testing::turn(turn),
        call_id: testing::call(conversation * 100 + 90),
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
        cgroup: None,
    };
    Event::JobStarted {
        turn_id: job.turn_id,
        call_id: job.call_id,
        job,
        launcher_pid: 4343,
        launcher_start: 99,
    }
}

fn undo_id(n: u64) -> UndoId {
    format!("00000007-0000-7000-8000-{n:012x}").parse().unwrap()
}

/// A whole conversation `n`: a turn with a prompt, an answer, a shell call, an approval,
/// a shell with a recording, a job, a compaction, saved messages, a receipt, an outbox
/// row and an undo, so each table that keeps something of a conversation has a row.
async fn fill(writer: &WriterHandle, n: u64, words: &str) {
    let conversation = id(n);
    let first = turn(n, 1);
    let call = n * 100 + 1;
    let batch = Batch::new()
        .event(conversation, testing::created(Some(&format!("pts-{n}"))))
        .event(conversation, testing::queued(first, &format!("{words} prompt")))
        .receipt(NewReceipt::accepted(testing::command(first), "prompt.send", json!({})))
        .enqueue(NewOutboxItem::replay_safe(
            "notify.tty",
            json!({ "conversation_id": conversation.to_string() }),
        ));
    writer.append(batch).await.unwrap();
    let events = vec![
        testing::started(first, "/etc/nixos"),
        Event::ShellStarted { pty_id: testing::pty(n), cwd: "/etc/nixos".into(), pid: Some(42) },
        shell_call(first, call, &format!("{words} command")),
        Event::ApprovalRequested {
            turn_id: testing::turn(first),
            call_id: testing::call(call),
            summary: "rm".to_owned(),
            diff_preview: None,
            interactive: false,
            exit: None,
            expires_in_secs: None,
            local_only: false,
        },
        Event::ApprovalResolved {
            turn_id: testing::turn(first),
            call_id: testing::call(call),
            decision: ApprovalDecision::Allow,
            origin: Origin::Shell,
        },
        answer(first, &format!("{words} answer")),
        job_started(n, first),
        completed(first),
    ];
    let mut batch = Batch::new();
    for event in events {
        batch = batch.event(conversation, event);
    }
    let messages = NewTurnMessages::new(
        conversation,
        testing::turn(first),
        "openai-subscription",
        "gpt-5.5",
        vec![json!({ "role": "user", "content": [{ "kind": "text", "text": words }] })],
    );
    writer.append(batch.turn_messages(messages)).await.unwrap();
    let undo = Event::UndoApplied {
        conversation_id: conversation,
        undone_turn_id: testing::turn(first),
        undo_id: undo_id(n),
        redo: false,
        origin: Origin::Cli,
        restored: 1,
        trashed: 0,
        deleted: 0,
        conflicts: 0,
        not_covered: 0,
        refs: Vec::new(),
        trash: None,
        stopped: false,
    };
    let compacted = testing::compacted(n, first, None, false);
    writer
        .append(Batch::new().event(conversation, undo).event(conversation, compacted))
        .await
        .unwrap();
    let pty = testing::pty(n).to_string();
    testing::on_writer(writer, move |conn| {
        conn.execute(
            "INSERT INTO recording_segments (pty_id, start_seq, path, bytes, started_at) \
             VALUES (?1, 0, ?1 || '/0.rec', 10, 0)",
            [pty],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// Rounds of an indexer that keeps each text as it is, until the index has every event.
async fn index_all(writer: &WriterHandle) {
    loop {
        let current = testing::on_writer(writer, search::state).await.unwrap();
        if current.indexed_seq == current.newest_seq {
            return;
        }
        let after = current.indexed_seq;
        let page =
            testing::on_writer(writer, move |conn| search::pending(conn, after)).await.unwrap();
        let page = SearchPage {
            after,
            through: page.through,
            redaction: current.redaction,
            docs: page.rows,
            drops: page.drops,
        };
        assert_eq!(writer.search_apply(page).await.unwrap(), SearchApplied::Applied);
    }
}

/// The rows of every table that keep something of conversation `n`, by table.
async fn rows_of(writer: &WriterHandle, n: u64) -> Vec<(&'static str, i64)> {
    let conversation = id(n).to_string();
    let pty = testing::pty(n).to_string();
    testing::on_writer(writer, move |conn| {
        let count = |sql: &str, value: &str| -> Result<i64, StoreError> {
            Ok(conn.query_row(sql, [value], |row| row.get(0))?)
        };
        let by_id = [
            ("events", "SELECT count(*) FROM events WHERE conversation_id = ?1"),
            ("conversations", "SELECT count(*) FROM conversations WHERE id = ?1"),
            ("turns", "SELECT count(*) FROM turns WHERE conversation_id = ?1"),
            ("approvals", "SELECT count(*) FROM approvals WHERE conversation_id = ?1"),
            ("shells", "SELECT count(*) FROM shells WHERE conversation_id = ?1"),
            ("compactions", "SELECT count(*) FROM compactions WHERE conversation_id = ?1"),
            ("jobs", "SELECT count(*) FROM jobs WHERE conversation_id = ?1"),
            ("turn_messages", "SELECT count(*) FROM turn_messages WHERE conversation_id = ?1"),
            ("search_docs", "SELECT count(*) FROM search_docs WHERE conversation_id = ?1"),
            ("search_titles", "SELECT count(*) FROM search_titles WHERE conversation_id = ?1"),
            (
                "outbox",
                "SELECT count(*) FROM outbox WHERE json_extract(payload, '$.conversation_id') = ?1",
            ),
        ];
        let mut found = Vec::new();
        for (table, sql) in by_id {
            found.push((table, count(sql, &conversation)?));
        }
        found.push((
            "receipts",
            conn.query_row(
                "SELECT count(*) FROM receipts WHERE command_id = ?1",
                [testing::command(turn(n, 1)).to_string()],
                |row| row.get(0),
            )?,
        ));
        found.push((
            "recording_segments",
            count("SELECT count(*) FROM recording_segments WHERE pty_id = ?1", &pty)?,
        ));
        Ok(found)
    })
    .await
    .unwrap()
}

/// The seqs that the full text index finds for `query`.
async fn found(writer: &WriterHandle, query: &'static str) -> Vec<i64> {
    testing::on_writer(writer, move |conn| {
        let mut stmt =
            conn.prepare("SELECT rowid FROM search_fts WHERE search_fts MATCH ?1 ORDER BY rowid")?;
        let rows = stmt.query_map([query], |row| row.get(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
    .unwrap()
}

/// Records the tombstone of conversation `n`.
async fn delete(writer: &WriterHandle, n: u64) -> Seq {
    let batch = Batch::new().event(id(n), Event::ConversationDeleted { origin: Origin::Cli });
    writer.append(batch).await.unwrap().last_seq()
}

/// Steps of `limit` events until the rows of conversation `n` are gone; the number of
/// steps.
async fn steps(writer: &WriterHandle, n: u64, limit: u32) -> usize {
    let mut taken = 0;
    loop {
        taken += 1;
        let at = sql::truncate_to_micros(testing::start());
        let step = testing::on_writer(writer, move |conn| {
            let tx = conn.unchecked_transaction()?;
            let step = step(&tx, id(n), limit, at)?;
            tx.commit()?;
            Ok(step)
        })
        .await
        .unwrap();
        match step {
            DeleteStep::Removed(removed) => assert!(removed <= limit),
            DeleteStep::RowsDone => return taken,
            DeleteStep::NotDeleted => panic!("conversation {n} has no tombstone"),
        }
    }
}

fn nothing() -> Vec<(&'static str, i64)> {
    [
        "events",
        "conversations",
        "turns",
        "approvals",
        "shells",
        "compactions",
        "jobs",
        "turn_messages",
        "search_docs",
        "search_titles",
        "outbox",
        "receipts",
        "recording_segments",
    ]
    .into_iter()
    .map(|table| (table, 0))
    .collect()
}

#[tokio::test]
async fn the_tombstone_hides_the_conversation_and_the_steps_remove_every_row() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    fill(&writer, GONE, "rsync").await;
    fill(&writer, KEPT, "docker").await;
    index_all(&writer).await;
    let kept_before = rows_of(&writer, KEPT).await;
    assert!(kept_before.iter().all(|(_, count)| *count > 0), "{kept_before:?}");
    assert!(!found(&writer, "rsync").await.is_empty());

    let seq = delete(&writer, GONE).await;

    // The projections forget it in the batch of the tombstone.
    let summary = testing::on_writer(&writer, |conn| conversations::get(conn, id(GONE))).await;
    assert_eq!(summary.unwrap(), None);
    let tombstone = testing::on_writer(&writer, |conn| get(conn, id(GONE))).await.unwrap().unwrap();
    assert_eq!(
        (tombstone.seq, tombstone.ptys.clone(), tombstone.undos.clone(), tombstone.rows_done),
        (seq, vec![testing::pty(GONE)], vec![undo_id(GONE)], false)
    );
    let open = testing::on_writer(&writer, pending).await.unwrap();
    assert_eq!(open, vec![tombstone]);

    // Pages of 5 events: the 12 events before the tombstone take 3 steps, and one more
    // removes the rest.
    assert_eq!(steps(&writer, GONE, 5).await, 4);

    let mut left = rows_of(&writer, GONE).await;
    // NOTE: the tombstone event stays, so the numbers of the log only grow.
    assert_eq!(left.remove(0), ("events", 1));
    let mut expected = nothing();
    expected.remove(0);
    assert_eq!(left, expected);
    assert_eq!(rows_of(&writer, KEPT).await, kept_before);
    assert_eq!(found(&writer, "rsync").await, Vec::<i64>::new());
    assert!(!found(&writer, "docker").await.is_empty());
    let newest = testing::on_writer(&writer, events::last_seq).await.unwrap();
    assert_eq!(newest, seq);
    testing::on_writer(&writer, |conn| {
        conn.execute("INSERT INTO search_fts (search_fts) VALUES ('integrity-check')", [])?;
        Ok(())
    })
    .await
    .unwrap();

    // The writer's own step finds nothing more, and the delete can end.
    assert_eq!(writer.delete_step(id(GONE)).await.unwrap(), DeleteStep::RowsDone);
    assert!(writer.delete_done(id(GONE)).await.unwrap());
    assert_eq!(testing::on_writer(&writer, pending).await.unwrap(), Vec::new());
    assert_eq!(writer.delete_step(id(KEPT)).await.unwrap(), DeleteStep::NotDeleted);
}

#[tokio::test]
async fn a_delete_is_not_done_before_its_rows_are_gone() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    fill(&writer, GONE, "rsync").await;
    delete(&writer, GONE).await;
    assert!(!writer.delete_done(id(GONE)).await.unwrap());
    assert!(matches!(writer.delete_step(id(GONE)).await.unwrap(), DeleteStep::Removed(12)));
    assert_eq!(writer.delete_step(id(GONE)).await.unwrap(), DeleteStep::RowsDone);
    assert!(writer.delete_done(id(GONE)).await.unwrap());
}

#[tokio::test]
async fn the_store_refuses_every_event_of_a_deleted_conversation() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    fill(&writer, GONE, "rsync").await;
    delete(&writer, GONE).await;

    let late = Batch::new().event(id(GONE), testing::queued(turn(GONE, 2), "again"));
    let error = writer.append(late).await.unwrap_err();
    assert!(matches!(error, StoreError::UnknownConversation { .. }), "{error:?}");
    let twice = Batch::new().event(id(GONE), Event::ConversationDeleted { origin: Origin::Cli });
    let error = writer.append(twice).await.unwrap_err();
    assert!(matches!(error, StoreError::UnknownConversation { .. }), "{error:?}");
    let again = Batch::new().event(id(GONE), testing::created(Some("pts-1")));
    let error = writer.append(again).await.unwrap_err();
    assert!(matches!(error, StoreError::ConversationExists { .. }), "{error:?}");
    let unknown = Batch::new().event(id(9), Event::ConversationDeleted { origin: Origin::Cli });
    let error = writer.append(unknown).await.unwrap_err();
    assert!(matches!(error, StoreError::UnknownConversation { .. }), "{error:?}");
}

#[tokio::test]
async fn a_rebuild_gives_no_rows_of_a_deleted_conversation() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    fill(&writer, GONE, "rsync").await;
    fill(&writer, KEPT, "docker").await;
    delete(&writer, GONE).await;
    let tables = ["conversations", "turns", "approvals", "shells", "compactions", "jobs"];
    let dump = |writer: &WriterHandle| {
        let writer = writer.clone();
        async move {
            testing::on_writer(&writer, move |conn| {
                Ok(tables.iter().map(|table| testing::dump(conn, table)).collect::<Vec<_>>())
            })
            .await
            .unwrap()
        }
    };
    let before = dump(&writer).await;

    // While the events of the delete are still in the log, and after they went.
    writer.rebuild_projections().await.unwrap();
    assert_eq!(dump(&writer).await, before);
    steps(&writer, GONE, 5).await;
    writer.rebuild_projections().await.unwrap();
    assert_eq!(dump(&writer).await, before);
}

#[tokio::test]
async fn a_page_of_the_indexer_read_before_the_delete_writes_no_doc_of_it() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    fill(&writer, GONE, "rsync").await;
    let current = testing::on_writer(&writer, search::state).await.unwrap();
    let page =
        testing::on_writer(&writer, move |conn| search::pending(conn, Seq::ZERO)).await.unwrap();
    assert!(!page.rows.is_empty());

    delete(&writer, GONE).await;
    let page = SearchPage {
        after: Seq::ZERO,
        through: page.through,
        redaction: current.redaction,
        docs: page.rows,
        drops: page.drops,
    };
    assert_eq!(writer.search_apply(page).await.unwrap(), SearchApplied::Applied);

    assert_eq!(found(&writer, "rsync").await, Vec::<i64>::new());
    assert_eq!(rows_of(&writer, GONE).await[8], ("search_docs", 0));
}
