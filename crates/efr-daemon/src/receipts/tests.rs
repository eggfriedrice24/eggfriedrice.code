use efr_protocol::{CommandId, ErrorBody, ErrorCode};
use efr_store::Batch;
use efr_store::receipts::{NewReceipt, Receipt};
use efr_test_support::{TestClock, TestStore};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::DaemonError;
use crate::receipts::{is_final, replay};

fn command(n: u128) -> CommandId {
    CommandId::from_uuid(uuid::Uuid::from_u128(n))
}

async fn stored(receipt: NewReceipt) -> Receipt {
    let store = TestStore::open(TestClock::new().shared()).await.unwrap();
    let command_id = receipt.command_id();
    store.writer().append(Batch::new().receipt(receipt)).await.unwrap();
    let found = store
        .readers()
        .with(move |conn| efr_store::receipts::lookup(conn, command_id))
        .await
        .unwrap()
        .unwrap();
    store.close().await;
    found
}

#[tokio::test]
async fn an_accepted_receipt_answers_with_the_stored_result() {
    let receipt = stored(NewReceipt::accepted(
        command(1),
        "turn.steer",
        json!({"turn_id": "019a9b1c-3d00-7a10-8b20-000000000001"}),
    ))
    .await;

    let answer = replay("turn.steer", receipt).unwrap();

    assert_eq!(answer, json!({"turn_id": "019a9b1c-3d00-7a10-8b20-000000000001"}));
}

#[tokio::test]
async fn a_rejected_receipt_answers_with_the_stored_error() {
    let body = ErrorBody::new(ErrorCode::Invalid, "the request is invalid: the prompt is empty");
    let receipt = stored(NewReceipt::rejected(command(2), "prompt.send", body.clone())).await;

    let answer = replay("prompt.send", receipt);

    assert!(matches!(answer, Err(DaemonError::Rejected { body: stored }) if stored == body));
}

#[tokio::test]
async fn a_command_id_reused_for_another_method_is_a_conflict() {
    let receipt = stored(NewReceipt::accepted(command(3), "turn.steer", json!({}))).await;

    let answer = replay("turn.interrupt", receipt);

    match answer {
        Err(error @ DaemonError::CommandReused { .. }) => {
            assert_eq!(error.code(), ErrorCode::Conflict);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn only_refusals_a_retry_cannot_change_are_kept() {
    assert!(is_final(ErrorCode::Invalid));
    assert!(is_final(ErrorCode::NotFound));
    assert!(is_final(ErrorCode::Conflict));
    assert!(!is_final(ErrorCode::Busy));
    assert!(!is_final(ErrorCode::Internal));
    assert!(!is_final(ErrorCode::Cancelled));
}
