use efr_protocol::{ApprovalDecision, Origin};
use pretty_assertions::assert_eq;

use super::*;
use crate::testing::{self, TestClock, on_writer};
use crate::{Batch, WriterHandle};

fn requested(turn: u64, call: u64, summary: &str) -> Event {
    Event::ApprovalRequested {
        turn_id: testing::turn(turn),
        call_id: testing::call(call),
        summary: summary.to_owned(),
        diff_preview: Some("-a\n+b\n".to_owned()),
    }
}

fn resolved(turn: u64, call: u64) -> Event {
    Event::ApprovalResolved {
        turn_id: testing::turn(turn),
        call_id: testing::call(call),
        decision: ApprovalDecision::Deny,
        origin: Origin::Phone,
    }
}

async fn writer_with_turn(id: ConversationId) -> (WriterHandle, crate::StoreWriter) {
    let (writer, thread) = testing::memory_writer(TestClock::new());
    writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "x"))
                .event(id, testing::started(1, "/")),
        )
        .await
        .unwrap();
    (writer, thread)
}

#[tokio::test]
async fn a_requested_approval_is_pending_until_it_is_resolved() {
    let id = testing::conversation(1);
    let (writer, _thread) = writer_with_turn(id).await;

    let committed = writer.append(Batch::new().event(id, requested(1, 1, "rm -rf /tmp/x"))).await;
    let pending_now = on_writer(&writer, |conn| pending(conn, None)).await.unwrap();
    writer.append(Batch::new().event(id, resolved(1, 1))).await.unwrap();
    let pending_after = on_writer(&writer, |conn| pending(conn, None)).await.unwrap();

    let envelope = committed.unwrap().events()[0].clone();
    assert_eq!(
        pending_now,
        [PendingApproval {
            conversation_id: id,
            turn_id: testing::turn(1),
            call_id: testing::call(1),
            summary: "rm -rf /tmp/x".to_owned(),
            diff_preview: Some("-a\n+b\n".to_owned()),
            requested_seq: envelope.seq,
            requested_at: envelope.at,
        }]
    );
    assert_eq!(pending_after, []);
}

#[tokio::test]
async fn an_answer_is_stored_under_its_wire_names() {
    let id = testing::conversation(1);
    let (writer, _thread) = writer_with_turn(id).await;
    writer
        .append(Batch::new().event(id, requested(1, 1, "x")).event(id, resolved(1, 1)))
        .await
        .unwrap();

    let stored = on_writer(&writer, |conn| {
        Ok(conn.query_row("SELECT status, decision, resolved_by FROM approvals", [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?)
    })
    .await
    .unwrap();

    assert_eq!(stored, ("resolved".to_owned(), "deny".to_owned(), "phone".to_owned()));
}

#[tokio::test]
async fn an_expired_approval_is_no_longer_pending() {
    let id = testing::conversation(1);
    let (writer, _thread) = writer_with_turn(id).await;
    writer.append(Batch::new().event(id, requested(1, 1, "x"))).await.unwrap();

    writer
        .append(Batch::new().event(
            id,
            Event::ApprovalExpired { turn_id: testing::turn(1), call_id: testing::call(1) },
        ))
        .await
        .unwrap();

    assert_eq!(on_writer(&writer, |conn| pending(conn, None)).await.unwrap(), []);
    let call = testing::call(1);
    assert_eq!(on_writer(&writer, move |conn| pending_call(conn, call)).await.unwrap(), None);
}

#[tokio::test]
async fn pending_lists_one_conversation_or_all_oldest_first() {
    let (one, two) = (testing::conversation(1), testing::conversation(2));
    let (writer, _thread) = writer_with_turn(one).await;
    writer
        .append(
            Batch::new()
                .event(two, testing::created(None))
                .event(two, testing::queued(2, "y"))
                .event(two, testing::started(2, "/"))
                .event(two, requested(2, 2, "second"))
                .event(one, requested(1, 1, "third")),
        )
        .await
        .unwrap();

    let all = on_writer(&writer, |conn| pending(conn, None)).await.unwrap();
    let only_one = on_writer(&writer, move |conn| pending(conn, Some(one))).await.unwrap();

    let calls = |list: &[PendingApproval]| list.iter().map(|p| p.call_id).collect::<Vec<_>>();
    assert_eq!(calls(&all), [testing::call(2), testing::call(1)]);
    assert_eq!(calls(&only_one), [testing::call(1)]);
}

#[tokio::test]
async fn pending_call_finds_a_pending_call_only() {
    let id = testing::conversation(1);
    let (writer, _thread) = writer_with_turn(id).await;
    writer.append(Batch::new().event(id, requested(1, 1, "x"))).await.unwrap();
    let (one, two) = (testing::call(1), testing::call(2));

    let found = on_writer(&writer, move |conn| pending_call(conn, one)).await.unwrap();
    let missing = on_writer(&writer, move |conn| pending_call(conn, two)).await.unwrap();

    assert_eq!(found.map(|p| p.call_id), Some(one));
    assert_eq!(missing, None);
}

#[tokio::test]
async fn asking_again_makes_an_answered_call_pending_again() {
    let id = testing::conversation(1);
    let (writer, _thread) = writer_with_turn(id).await;
    writer
        .append(
            Batch::new()
                .event(id, requested(1, 1, "first ask"))
                .event(id, resolved(1, 1))
                .event(id, requested(1, 1, "second ask")),
        )
        .await
        .unwrap();

    let pending = on_writer(&writer, |conn| pending(conn, None)).await.unwrap();

    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].summary, "second ask");
}
