use efr_protocol::{Origin, ShellContext, TurnSettings};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::testing::{self, TestClock, on_writer};
use crate::{Batch, WriterHandle};

fn login(provider: &str) -> Event {
    Event::LoginCompleted { provider: provider.to_owned() }
}

fn prompt(turn: u64, text: &str) -> Event {
    Event::PromptQueued {
        turn_id: testing::turn(turn),
        command_id: testing::command(turn),
        text: text.to_owned(),
        origin: Origin::Shell,
        context: Some(ShellContext::new("/etc/nixos")),
        settings: TurnSettings::default(),
        steers: Vec::new(),
    }
}

async fn insert_raw(writer: &WriterHandle, kind: &str, payload: &str) {
    let (kind, payload) = (kind.to_owned(), payload.to_owned());
    on_writer(writer, move |conn| {
        conn.execute(
            "INSERT INTO events (seq, kind, payload, created_at) VALUES (1, ?1, ?2, 0)",
            params![kind, payload],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn events_read_back_exactly_as_they_were_committed() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    let committed = writer
        .append(
            Batch::new()
                .event(id, testing::created(Some("pts-3")))
                .event(id, prompt(1, "why is the disk full"))
                .global_event(login("openai")),
        )
        .await
        .unwrap();

    let read = on_writer(&writer, |conn| read_after(conn, Seq::ZERO, 10)).await.unwrap();

    assert_eq!(read, committed.events());
    assert_eq!(read[2].conversation_id, None);
}

#[tokio::test]
async fn the_indexed_columns_repeat_the_event() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer
        .append(Batch::new().event(id, testing::created(None)).event(id, prompt(1, "hi")))
        .await
        .unwrap();

    let rows = on_writer(&writer, |conn| {
        let mut stmt =
            conn.prepare("SELECT conversation_id, turn_id, kind FROM events ORDER BY seq")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<(Option<String>, Option<String>, String)>, _>>()?;
        Ok(rows)
    })
    .await
    .unwrap();

    assert_eq!(
        rows,
        [
            (Some(id.to_string()), None, "conversation_created".to_owned()),
            (Some(id.to_string()), Some(testing::turn(1).to_string()), "prompt_queued".to_owned()),
        ]
    );
}

#[tokio::test]
async fn read_after_starts_after_the_given_seq_and_stops_at_the_limit() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let mut batch = Batch::new();
    for n in 1..=5 {
        batch = batch.global_event(login(&format!("p{n}")));
    }
    writer.append(batch).await.unwrap();

    let read = on_writer(&writer, |conn| read_after(conn, Seq::new(2), 2)).await.unwrap();

    let seqs: Vec<u64> = read.iter().map(|envelope| envelope.seq.get()).collect();
    assert_eq!(seqs, [3, 4]);
    assert_eq!(read[0].event, login("p3"));
}

#[tokio::test]
async fn an_unknown_kind_reads_back_as_unknown() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    insert_raw(&writer, "future_thing", r#"{"kind":"future_thing","x":1}"#).await;

    let read = on_writer(&writer, |conn| read_after(conn, Seq::ZERO, 10)).await.unwrap();

    let serde_json::Value::Object(payload) = json!({ "x": 1 }) else { unreachable!() };
    assert_eq!(read[0].event, Event::Unknown { kind: "future_thing".to_owned(), payload });
}

#[tokio::test]
async fn a_known_kind_with_a_malformed_body_is_an_error() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    insert_raw(&writer, "turn_started", r#"{"kind":"turn_started","turn_id":"nope"}"#).await;

    let error = on_writer(&writer, |conn| read_after(conn, Seq::ZERO, 10)).await.unwrap_err();

    assert!(
        matches!(error, StoreError::DecodeEvent { seq, .. } if seq == Seq::new(1)),
        "{error:?}"
    );
}

#[tokio::test]
async fn conversation_reads_see_one_conversation_and_page_both_ways() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let (one, two) = (testing::conversation(1), testing::conversation(2));
    writer
        .append(
            Batch::new()
                .event(one, testing::created(None))
                .event(two, testing::created(None))
                .event(one, prompt(1, "a"))
                .event(two, prompt(2, "b"))
                .event(one, prompt(3, "c")),
        )
        .await
        .unwrap();
    let seqs = |events: Vec<EventEnvelope>| -> Vec<u64> {
        events.iter().map(|envelope| envelope.seq.get()).collect()
    };

    let after = on_writer(&writer, move |conn| read_conversation_after(conn, one, Seq::new(1), 10));
    assert_eq!(seqs(after.await.unwrap()), [3, 5]);

    let newest = on_writer(&writer, move |conn| read_conversation_before(conn, one, None, 2));
    assert_eq!(seqs(newest.await.unwrap()), [3, 5]);

    let older =
        on_writer(&writer, move |conn| read_conversation_before(conn, one, Some(Seq::new(3)), 2));
    assert_eq!(seqs(older.await.unwrap()), [1]);
}

#[tokio::test]
async fn the_turn_history_leaves_out_tool_output_updates() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let one = testing::conversation(1);
    let output = |bytes: u64| Event::ToolCallOutputUpdated {
        turn_id: testing::turn(1),
        call_id: testing::call(9),
        tail: "building".to_owned(),
        bytes,
    };
    writer
        .append(
            Batch::new()
                .event(one, testing::created(None))
                .event(one, prompt(1, "a"))
                .event(one, output(10))
                .event(one, output(20))
                .event(one, prompt(2, "b")),
        )
        .await
        .unwrap();

    let page = on_writer(&writer, move |conn| read_turn_history(conn, one, 2)).await.unwrap();

    let seqs: Vec<u64> = page.iter().map(|envelope| envelope.seq.get()).collect();
    assert_eq!(seqs, [2, 5], "the limit counts only the events kept");
}

#[tokio::test]
async fn a_turn_range_holds_the_events_of_one_conversation_without_tool_output_updates() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let (one, two) = (testing::conversation(1), testing::conversation(2));
    let output = Event::ToolCallOutputUpdated {
        turn_id: testing::turn(1),
        call_id: testing::call(9),
        tail: "building".to_owned(),
        bytes: 10,
    };
    writer
        .append(
            Batch::new()
                .event(one, testing::created(None))
                .event(two, testing::created(None))
                .event(one, prompt(1, "a"))
                .event(two, prompt(3, "c"))
                .event(one, output)
                .event(one, prompt(2, "b")),
        )
        .await
        .unwrap();

    let range =
        on_writer(&writer, move |conn| read_turn_range(conn, one, Seq::new(2), Seq::new(6)))
            .await
            .unwrap();

    let seqs: Vec<u64> = range.iter().map(|envelope| envelope.seq.get()).collect();
    assert_eq!(seqs, [3, 6], "the other conversation and the output stay out");
}

#[tokio::test]
async fn last_seq_is_zero_for_an_empty_log_and_the_newest_seq_after() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    assert_eq!(on_writer(&writer, last_seq).await.unwrap(), Seq::ZERO);

    writer.append(Batch::new().global_event(login("a")).global_event(login("b"))).await.unwrap();

    assert_eq!(on_writer(&writer, last_seq).await.unwrap(), Seq::new(2));
}

#[tokio::test]
async fn a_bound_beyond_the_largest_sqlite_integer_reads_nothing() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.append(Batch::new().global_event(login("a"))).await.unwrap();

    let read = on_writer(&writer, |conn| read_after(conn, Seq::new(u64::MAX), 10)).await.unwrap();

    assert_eq!(read, []);
}
