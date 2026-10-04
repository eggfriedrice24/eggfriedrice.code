use std::sync::{Arc, Mutex};

use efr_protocol::framing::Decoder;
use efr_protocol::{
    AdminStatus, ConversationId, ConversationSubscribe, ErrorBody, ErrorCode, Method, RequestId,
    Seq, ServerFrame,
};
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::{Responder, ResponseState, cancelled, terminal_frame};
use crate::TransportError;
use crate::codec::EncodedFrame;
use crate::subscriptions::{Offer, subscription};

const ID: RequestId = RequestId::new(3);

fn unary() -> Method {
    Method::AdminStatus(AdminStatus {})
}

fn stream() -> Method {
    Method::ConversationSubscribe(ConversationSubscribe {
        conversation_id: "019a9b1c-3d00-7a10-8b20-000000000001".parse::<ConversationId>().unwrap(),
        after_seq: None,
    })
}

fn responder(
    method: &Method,
    capacity: usize,
) -> (Responder, mpsc::Receiver<EncodedFrame>, Arc<Mutex<ResponseState>>) {
    let (tx, rx) = mpsc::channel(capacity);
    let state = Arc::new(Mutex::new(ResponseState::new(tx)));
    (Responder::new(ID, method, Arc::clone(&state)), rx, state)
}

fn decode(frame: &EncodedFrame) -> ServerFrame {
    let payloads = Decoder::new().push(frame.as_bytes()).unwrap();
    assert_eq!(payloads.len(), 1);
    ServerFrame::from_json(&payloads[0]).unwrap()
}

#[tokio::test]
async fn a_unary_method_sends_exactly_one_result() {
    let (responder, mut rx, state) = responder(&unary(), 4);
    responder.item(&serde_json::json!({ "ok": true })).await.unwrap();
    let error = responder.item(&serde_json::json!({ "ok": false })).await.unwrap_err();
    assert!(matches!(error, TransportError::ResultAlreadySent { method: "admin.status" }));
    assert_eq!(
        decode(&rx.recv().await.unwrap()),
        ServerFrame::Item { id: ID, item: serde_json::json!({ "ok": true }) }
    );
    assert!(rx.try_recv().is_err());
    assert_eq!(state.lock().unwrap().items, 1);
}

#[tokio::test]
async fn ack_and_forward_are_refused_on_a_unary_method() {
    let (responder, _rx, _state) = responder(&unary(), 4);
    assert!(matches!(
        responder.ack().await.unwrap_err(),
        TransportError::NotAStream { method: "admin.status" }
    ));
    let (_tx, sub) = subscription::<u32>();
    assert!(matches!(
        responder.forward(sub).await.unwrap_err(),
        TransportError::NotAStream { method: "admin.status" }
    ));
}

#[tokio::test]
async fn a_stream_sends_an_ack_and_many_items() {
    let (responder, mut rx, _state) = responder(&stream(), 4);
    responder.ack().await.unwrap();
    responder.item(&1).await.unwrap();
    responder.item(&2).await.unwrap();
    assert_eq!(decode(&rx.recv().await.unwrap()), ServerFrame::Ack { id: ID });
    assert_eq!(decode(&rx.recv().await.unwrap()), ServerFrame::item(ID, &1).unwrap());
    assert_eq!(decode(&rx.recv().await.unwrap()), ServerFrame::item(ID, &2).unwrap());
}

#[tokio::test]
async fn forward_sends_items_until_the_producer_finishes() {
    let (responder, mut rx, state) = responder(&stream(), 8);
    let (mut tx, sub) = subscription();
    assert_eq!(tx.offer(Seq::new(1), "a"), Offer::Queued);
    assert_eq!(tx.offer(Seq::new(2), "b"), Offer::Queued);
    drop(tx);
    responder.forward(sub).await.unwrap();
    assert_eq!(decode(&rx.recv().await.unwrap()), ServerFrame::item(ID, "a").unwrap());
    assert_eq!(decode(&rx.recv().await.unwrap()), ServerFrame::item(ID, "b").unwrap());
    assert_eq!(state.lock().unwrap().overflow, None);
}

#[tokio::test]
async fn forward_reports_an_overflow_after_the_queued_items() {
    let (responder, mut rx, state) = responder(&stream(), 128);
    let (mut tx, sub) = subscription();
    for seq in 1..=64 {
        assert_eq!(tx.offer(Seq::new(seq), seq), Offer::Queued);
    }
    assert_eq!(tx.offer(Seq::new(65), 65), Offer::Overflowed);
    let error = responder.forward(sub).await.unwrap_err();
    assert!(matches!(error, TransportError::Overflow { last_seq } if last_seq == Seq::new(64)));
    for seq in 1..=64_u64 {
        assert_eq!(decode(&rx.recv().await.unwrap()), ServerFrame::item(ID, &seq).unwrap());
    }
    assert!(rx.try_recv().is_err());
    assert_eq!(state.lock().unwrap().overflow, Some(Seq::new(64)));
}

#[tokio::test]
async fn sending_on_a_closed_connection_fails() {
    let (responder, rx, _state) = responder(&stream(), 1);
    drop(rx);
    assert!(matches!(responder.item(&1).await.unwrap_err(), TransportError::Closed));
}

#[tokio::test]
async fn nothing_is_queued_once_the_request_has_ended() {
    let (responder, mut rx, state) = responder(&stream(), 4);
    responder.item(&1).await.unwrap();
    state.lock().unwrap().end();
    assert!(matches!(responder.item(&2).await, Err(TransportError::RequestEnded { id: ID })));
    assert!(matches!(responder.ack().await, Err(TransportError::RequestEnded { id: ID })));
    assert_eq!(decode(&rx.recv().await.unwrap()), ServerFrame::item(ID, &1).unwrap());
    // The ended state let go of the sender, so a leaked responder keeps no writer alive.
    assert!(rx.recv().await.is_none());
}

#[tokio::test]
async fn an_item_waiting_for_queue_space_when_the_request_ends_is_dropped() {
    let (responder, mut rx, state) = responder(&stream(), 1);
    responder.item(&1).await.unwrap();
    let mut second = std::pin::pin!(responder.item(&2));
    assert!(futures::poll!(second.as_mut()).is_pending(), "the queue is full");
    state.lock().unwrap().end();
    assert_eq!(decode(&rx.recv().await.unwrap()), ServerFrame::item(ID, &1).unwrap());
    assert!(matches!(second.await, Err(TransportError::RequestEnded { id: ID })));
    assert!(rx.recv().await.is_none());
}

#[tokio::test]
async fn a_unary_result_lost_to_a_closed_connection_is_not_counted() {
    let (responder, rx, state) = responder(&unary(), 1);
    drop(rx);
    assert!(matches!(responder.item(&1).await, Err(TransportError::Closed)));
    assert_eq!(state.lock().unwrap().items, 0);
}

#[tokio::test]
async fn a_result_too_large_for_a_frame_fails_without_using_the_one_result() {
    let (responder, mut rx, _state) = responder(&unary(), 1);
    let huge = "x".repeat(efr_protocol::framing::MAX_FRAME_LEN);
    assert!(matches!(responder.item(&huge).await.unwrap_err(), TransportError::Protocol { .. }));
    responder.item("small").await.unwrap();
    assert_eq!(decode(&rx.recv().await.unwrap()), ServerFrame::item(ID, "small").unwrap());
}

#[test]
fn the_terminal_frame_table() {
    let state = |items, overflow| ResponseState { items, overflow, outbound: None };
    let failed = ErrorBody::new(ErrorCode::NotFound, "no such conversation");
    let no_result = ErrorBody::new(ErrorCode::Internal, "admin.status finished without a result");
    let overflow = ErrorBody::overflow(Seq::new(9));
    // (stream, outcome, items, overflow, expected)
    type Case = (bool, Result<(), ErrorBody>, u64, Option<Seq>, ServerFrame);
    let cases: Vec<Case> = vec![
        (false, Ok(()), 1, None, ServerFrame::end(ID)),
        (false, Ok(()), 0, None, ServerFrame::error(Some(ID), no_result)),
        (false, Err(failed.clone()), 0, None, ServerFrame::error(Some(ID), failed.clone())),
        (false, Err(cancelled()), 1, None, ServerFrame::error(Some(ID), cancelled())),
        (true, Ok(()), 0, None, ServerFrame::end(ID)),
        (true, Ok(()), 5, None, ServerFrame::end(ID)),
        (true, Err(failed.clone()), 5, None, ServerFrame::error(Some(ID), failed)),
        (true, Ok(()), 64, Some(Seq::new(9)), ServerFrame::error(Some(ID), overflow.clone())),
        (true, Err(cancelled()), 64, Some(Seq::new(9)), ServerFrame::error(Some(ID), overflow)),
    ];
    for (stream, outcome, items, overflow, expected) in cases {
        let method = if stream { "conversation.subscribe" } else { "admin.status" };
        let label = format!("stream {stream}, outcome {outcome:?}, items {items}");
        assert_eq!(
            terminal_frame(ID, method, stream, outcome, &state(items, overflow)),
            expected,
            "{label}"
        );
    }
}
