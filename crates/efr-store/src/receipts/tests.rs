use efr_protocol::{ErrorCode, Event};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::testing::{self, TestClock, on_writer};
use crate::{Batch, WriterHandle, events};

async fn stored(writer: &WriterHandle, command_id: CommandId) -> Option<Receipt> {
    on_writer(writer, move |conn| lookup(conn, command_id)).await.unwrap()
}

fn denied() -> ErrorBody {
    ErrorBody::new(ErrorCode::Forbidden, "writing /etc/shadow is never allowed")
        .with_data(json!({ "path": "/etc/shadow" }))
}

#[tokio::test]
async fn an_unknown_command_has_no_receipt() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    assert_eq!(stored(&writer, testing::command(1)).await, None);
}

#[tokio::test]
async fn an_accepted_receipt_keeps_the_result_and_the_last_event_seq() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    let result = json!({ "conversation_id": id.to_string(), "queued": false });

    let committed = writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "hello"))
                .receipt(NewReceipt::accepted(testing::command(1), "prompt.send", result.clone())),
        )
        .await
        .unwrap();

    assert_eq!(
        stored(&writer, testing::command(1)).await,
        Some(Receipt {
            command_id: testing::command(1),
            method: "prompt.send".to_owned(),
            outcome: ReceiptOutcome::Accepted { result },
            seq: Some(Seq::new(2)),
            created_at: committed.events()[0].at,
        })
    );
}

#[tokio::test]
async fn a_receipt_can_record_an_event_that_is_not_the_last_and_a_retry_gets_it() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer.append(Batch::new().event(id, testing::created(None))).await.unwrap();
    let receipt = || {
        NewReceipt::accepted(testing::command(1), "prompt.send", json!({ "queued": false }))
            .seq_of_event(0)
    };

    // `prompt_queued` is what the result reports; `turn_started` follows it.
    let committed = writer
        .append(
            Batch::new()
                .event(id, testing::queued(1, "hello"))
                .event(id, testing::started(1, "/"))
                .receipt(receipt()),
        )
        .await
        .unwrap();
    let retry = writer
        .append(Batch::new().event(id, testing::queued(2, "the retry")).receipt(receipt()))
        .await
        .unwrap_err();

    let queued_seq = committed.events()[0].seq;
    assert_eq!(queued_seq, Seq::new(2));
    assert_eq!(committed.last_seq(), Seq::new(3));
    assert_eq!(stored(&writer, testing::command(1)).await.unwrap().seq, Some(queued_seq));
    let StoreError::DuplicateCommand { receipt } = retry else { panic!("{retry:?}") };
    assert_eq!(receipt.seq, Some(queued_seq));
}

#[tokio::test]
async fn a_receipt_that_names_a_missing_event_fails_the_batch() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);

    let error = writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "a"))
                .receipt(
                    NewReceipt::accepted(testing::command(1), "prompt.send", json!({}))
                        .seq_of_event(2),
                ),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            StoreError::ReceiptEventMissing { command_id, index: 2, events: 2 }
                if command_id == testing::command(1)
        ),
        "{error:?}"
    );
    assert_eq!(stored(&writer, testing::command(1)).await, None);
    let log = on_writer(&writer, |conn| events::read_after(conn, Seq::ZERO, 10)).await.unwrap();
    assert_eq!(log, [], "the batch's events were not written");
    let next = writer.append(Batch::new().event(id, testing::created(None))).await.unwrap();
    assert_eq!(next.first_seq(), Some(Seq::new(1)), "no sequence number was used up");
}

#[tokio::test]
async fn a_receipt_without_events_has_no_seq() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());

    let committed = writer
        .append(Batch::new().receipt(NewReceipt::rejected(
            testing::command(1),
            "prompt.send",
            denied(),
        )))
        .await
        .unwrap();

    let receipt = stored(&writer, testing::command(1)).await.unwrap();
    assert_eq!(receipt.seq, None);
    assert_eq!(receipt.outcome, ReceiptOutcome::Rejected { error: denied() });
    assert_eq!(committed.events(), []);
}

#[tokio::test]
async fn a_duplicate_command_returns_the_stored_receipt_and_writes_nothing() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    let first = NewReceipt::accepted(testing::command(1), "prompt.send", json!({ "n": 1 }));
    writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "a"))
                .receipt(first),
        )
        .await
        .unwrap();

    let error =
        writer
            .append(Batch::new().event(id, testing::queued(2, "the retry")).receipt(
                NewReceipt::accepted(testing::command(1), "prompt.send", json!({ "n": 2 })),
            ))
            .await
            .unwrap_err();

    let StoreError::DuplicateCommand { receipt } = error else { panic!("{error:?}") };
    assert_eq!(receipt.outcome, ReceiptOutcome::Accepted { result: json!({ "n": 1 }) });
    let log = on_writer(&writer, |conn| events::read_after(conn, Seq::ZERO, 10)).await.unwrap();
    assert_eq!(log.len(), 2, "the retry's event was not written");
    let next = writer.append(Batch::new().event(id, testing::queued(3, "b"))).await.unwrap();
    assert_eq!(next.first_seq(), Some(Seq::new(3)));
}

#[tokio::test]
async fn a_rejected_command_stays_rejected() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer
        .append(Batch::new().receipt(NewReceipt::rejected(
            testing::command(1),
            "prompt.send",
            denied(),
        )))
        .await
        .unwrap();

    let error = writer
        .append(Batch::new().event(id, testing::created(None)).receipt(NewReceipt::accepted(
            testing::command(1),
            "prompt.send",
            json!({}),
        )))
        .await
        .unwrap_err();

    let StoreError::DuplicateCommand { receipt } = error else { panic!("{error:?}") };
    assert_eq!(receipt.outcome, ReceiptOutcome::Rejected { error: denied() });
    assert_eq!(
        stored(&writer, testing::command(1)).await.map(|receipt| receipt.outcome),
        Some(ReceiptOutcome::Rejected { error: denied() })
    );
}

#[tokio::test]
async fn one_command_id_twice_in_one_batch_is_a_duplicate() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());

    let error = writer
        .append(
            Batch::new()
                .global_event(Event::LoginCompleted { provider: "openai".to_owned() })
                .receipt(NewReceipt::accepted(testing::command(1), "admin.login_openai", json!(1)))
                .receipt(NewReceipt::accepted(testing::command(1), "admin.login_openai", json!(2))),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, StoreError::DuplicateCommand { .. }), "{error:?}");
    assert_eq!(stored(&writer, testing::command(1)).await, None);
}

#[tokio::test]
async fn outcomes_are_stored_by_name() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer
        .append(
            Batch::new()
                .receipt(NewReceipt::accepted(testing::command(1), "turn.steer", json!(null)))
                .receipt(NewReceipt::rejected(testing::command(2), "turn.steer", denied())),
        )
        .await
        .unwrap();

    let outcomes = on_writer(&writer, |conn| {
        let mut stmt = conn.prepare("SELECT outcome FROM receipts ORDER BY command_id")?;
        let rows = stmt.query_map([], |row| row.get(0))?.collect::<Result<Vec<String>, _>>()?;
        Ok(rows)
    })
    .await
    .unwrap();

    assert_eq!(outcomes, ["accepted", "rejected"]);
}
