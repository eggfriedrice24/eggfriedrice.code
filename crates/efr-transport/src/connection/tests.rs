use std::future::pending;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use efr_protocol::framing::MAX_FRAME_LEN;
use efr_protocol::{
    ErrorBody, ErrorCode, Method, Origin, PROTOCOL_VERSION, RequestId, Seq, ServerFrame,
};
use efr_stdx::time::Clock;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::DuplexStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{ConnectionParams, RequestTable, run};
use crate::subscriptions::{Offer, subscription};
use crate::testing::{
    Closed, FakeDispatcher, InstantClock, StoppedClock, TestClient, cancel_frame, drop_signal,
    hello_frame, internal, list_frame, subscribe_frame,
};
use crate::{ConnId, ConnectionContext, PeerCred, Request, TransportError};

const PEER: PeerCred = PeerCred::new(1000, Some(4242));

struct Running {
    client: TestClient<DuplexStream>,
    task: JoinHandle<()>,
    shutdown: CancellationToken,
}

fn start(dispatcher: Arc<FakeDispatcher>) -> Running {
    start_with(dispatcher, Arc::new(StoppedClock), 1 << 16)
}

/// A connection over an in-memory pipe that holds at most `pipe_bytes` unread bytes.
fn start_with(
    dispatcher: Arc<FakeDispatcher>,
    clock: Arc<dyn Clock>,
    pipe_bytes: usize,
) -> Running {
    let (client, server) = tokio::io::duplex(pipe_bytes);
    let shutdown = CancellationToken::new();
    let params = ConnectionParams {
        conn_id: ConnId::new(7),
        peer: PEER,
        dispatcher,
        clock,
        shutdown: shutdown.clone(),
    };
    let task = tokio::spawn(run(server, params));
    Running { client: TestClient::new(client), task, shutdown }
}

fn id(value: u64) -> RequestId {
    RequestId::new(value)
}

fn error_code(frame: Option<ServerFrame>) -> (Option<RequestId>, ErrorCode) {
    match frame {
        Some(ServerFrame::Error(frame)) => (frame.id, frame.error.code),
        other => panic!("expected an error frame, got {other:?}"),
    }
}

/// Answers a unary request with `{"ok": true}`.
fn answering() -> Arc<FakeDispatcher> {
    FakeDispatcher::new(|request: Request| async move {
        request.responder.item(&json!({ "ok": true })).await.map_err(internal)
    })
}

/// Answers unary requests with `{"ok": true}`. A streaming request reports that it
/// started, holds a drop signal and never ends on its own.
fn holding_streams() -> (Arc<FakeDispatcher>, mpsc::UnboundedReceiver<()>, DroppedStreams) {
    let (started_tx, started) = mpsc::unbounded_channel();
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let signals = Arc::clone(&dropped);
    let dispatcher = FakeDispatcher::new(move |request: Request| {
        let started = started_tx.clone();
        let signals = Arc::clone(&signals);
        async move {
            if !request.method.is_stream() {
                return request.responder.item(&json!({ "ok": true })).await.map_err(internal);
            }
            let (signal, dropped) = drop_signal();
            signals.lock().unwrap().push(dropped);
            let _signal = signal;
            started.send(()).unwrap();
            pending::<()>().await;
            Ok(())
        }
    });
    (dispatcher, started, DroppedStreams(dropped))
}

/// The drop signals of every streaming request that `holding_streams` started.
struct DroppedStreams(Arc<Mutex<Vec<tokio::sync::oneshot::Receiver<()>>>>);

impl DroppedStreams {
    /// Waits until the `n`th streaming handler (counting from 0) was dropped.
    async fn wait(&self, n: usize) {
        let receiver =
            std::mem::replace(&mut self.0.lock().unwrap()[n], tokio::sync::oneshot::channel().1);
        receiver.await.unwrap();
    }
}

#[tokio::test]
async fn hello_answers_with_the_transport_protocol_and_builds_the_context() {
    let dispatcher = answering();
    let mut running = start(Arc::clone(&dispatcher));
    let result = running.client.hello().await;
    assert_eq!(result.protocol, PROTOCOL_VERSION);
    assert_eq!(result.version, "0.1.0");
    assert_eq!(
        dispatcher.contexts(),
        vec![ConnectionContext::new(ConnId::new(7), Origin::Cli, PEER)]
    );
}

#[tokio::test]
async fn a_method_before_hello_is_unauthorized_and_does_not_run() {
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    let dispatcher = FakeDispatcher::new(move |request: Request| {
        flag.store(true, Ordering::SeqCst);
        async move { request.responder.item(&1).await.map_err(internal) }
    });
    let mut running = start(dispatcher);
    running.client.send(&list_frame(2)).await;
    assert_eq!(error_code(running.client.recv().await), (Some(id(2)), ErrorCode::Unauthorized));
    running.client.hello().await;
    assert!(!ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_protocol_mismatch_is_refused_before_anything_runs_and_closes_the_connection() {
    let dispatcher = answering();
    let mut running = start(Arc::clone(&dispatcher));
    running.client.send(&hello_frame(1, PROTOCOL_VERSION + 1)).await;
    let Some(ServerFrame::Error(frame)) = running.client.recv().await else {
        panic!("a mismatched hello must be refused");
    };
    assert_eq!(frame.id, Some(id(1)));
    assert_eq!(frame.error, ErrorBody::protocol_mismatch(PROTOCOL_VERSION, PROTOCOL_VERSION + 1));
    assert_eq!(running.client.recv().await, None);
    running.task.await.unwrap();
    assert!(dispatcher.contexts().is_empty(), "the dispatcher must not see the hello");
}

#[tokio::test]
async fn a_second_hello_is_a_conflict() {
    let mut running = start(answering());
    running.client.hello().await;
    running.client.send(&hello_frame(2, PROTOCOL_VERSION)).await;
    assert_eq!(error_code(running.client.recv().await), (Some(id(2)), ErrorCode::Conflict));
}

#[tokio::test]
async fn a_refused_hello_leaves_the_connection_waiting_for_hello() {
    let refusal = ErrorBody::new(ErrorCode::Unauthorized, "unknown device");
    let mut running = start(FakeDispatcher::refusing_hello(refusal.clone()));
    running.client.send(&hello_frame(1, PROTOCOL_VERSION)).await;
    assert_eq!(running.client.recv().await, Some(ServerFrame::error(Some(id(1)), refusal)));
    running.client.send(&list_frame(2)).await;
    assert_eq!(error_code(running.client.recv().await), (Some(id(2)), ErrorCode::Unauthorized));
}

#[tokio::test]
async fn a_unary_request_gets_one_item_and_an_end() {
    let mut running = start(answering());
    running.client.hello().await;
    running.client.send(&list_frame(2)).await;
    assert_eq!(
        running.client.recv().await,
        Some(ServerFrame::Item { id: id(2), item: json!({ "ok": true }) })
    );
    assert_eq!(running.client.recv().await, Some(ServerFrame::end(id(2))));
}

#[tokio::test]
async fn the_request_carries_the_connection_context() {
    let dispatcher = FakeDispatcher::new(|request: Request| async move {
        let context = &request.context;
        let seen = json!({
            "conn_id": context.conn_id().get(),
            "surface": context.surface(),
            "uid": context.uid(),
            "pid": context.pid(),
            "id": request.id,
        });
        request.responder.item(&seen).await.map_err(internal)
    });
    let mut running = start(dispatcher);
    running.client.hello().await;
    running.client.send(&list_frame(4)).await;
    let expected = json!({ "conn_id": 7, "surface": "cli", "uid": 1000, "pid": 4242, "id": 4 });
    assert_eq!(running.client.recv().await, Some(ServerFrame::Item { id: id(4), item: expected }));
}

#[tokio::test]
async fn a_handler_error_ends_the_request_with_it() {
    let failure = ErrorBody::new(ErrorCode::NotFound, "no such conversation");
    let returned = failure.clone();
    let dispatcher = FakeDispatcher::new(move |_request: Request| {
        let returned = returned.clone();
        async move { Err(returned) }
    });
    let mut running = start(dispatcher);
    running.client.hello().await;
    running.client.send(&list_frame(2)).await;
    assert_eq!(running.client.recv().await, Some(ServerFrame::error(Some(id(2)), failure)));
}

#[tokio::test]
async fn a_unary_handler_without_a_result_is_answered_with_internal() {
    let mut running = start(FakeDispatcher::new(|_request: Request| async { Ok(()) }));
    running.client.hello().await;
    running.client.send(&list_frame(2)).await;
    assert_eq!(error_code(running.client.recv().await), (Some(id(2)), ErrorCode::Internal));
}

#[tokio::test]
async fn a_panicking_handler_is_answered_with_internal_and_the_connection_goes_on() {
    let dispatcher = FakeDispatcher::new(|request: Request| async move {
        if matches!(request.method, Method::ConversationsList(_)) && request.id.get() == 2 {
            panic!("a handler bug");
        }
        request.responder.item(&1).await.map_err(internal)
    });
    let mut running = start(dispatcher);
    running.client.hello().await;
    running.client.send(&list_frame(2)).await;
    assert_eq!(error_code(running.client.recv().await), (Some(id(2)), ErrorCode::Internal));
    running.client.send(&list_frame(3)).await;
    assert_eq!(running.client.recv().await, Some(ServerFrame::item(id(3), &1).unwrap()));
    assert_eq!(running.client.recv().await, Some(ServerFrame::end(id(3))));
}

/// A dispatcher whose streaming handler moves its responder into a task of its own and
/// returns at once. The task sends one item when `release` fires and reports whether the
/// transport refused it because the request had ended.
fn leaking_responders(
    panic_after_spawning: bool,
) -> (Arc<FakeDispatcher>, tokio::sync::oneshot::Sender<()>, mpsc::UnboundedReceiver<bool>) {
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release = Arc::new(Mutex::new(Some(release_rx)));
    let (refused_tx, refused) = mpsc::unbounded_channel();
    let dispatcher = FakeDispatcher::new(move |request: Request| {
        let release = release.lock().unwrap().take();
        let refused = refused_tx.clone();
        async move {
            if !request.method.is_stream() {
                return request.responder.item(&json!({ "ok": true })).await.map_err(internal);
            }
            let (id, responder, release) = (request.id, request.responder, release.unwrap());
            tokio::spawn(async move {
                release.await.unwrap();
                let sent = responder.item(&"late").await;
                let ended =
                    matches!(sent, Err(TransportError::RequestEnded { id: ended }) if ended == id);
                refused.send(ended).unwrap();
            });
            if panic_after_spawning {
                panic!("a handler bug");
            }
            Ok(())
        }
    });
    (dispatcher, release_tx, refused)
}

#[tokio::test]
async fn an_item_from_a_task_the_handler_spawned_never_follows_the_end_frame() {
    let (dispatcher, release, mut refused) = leaking_responders(false);
    let mut running = start(dispatcher);
    running.client.hello().await;
    running.client.send(&subscribe_frame(5)).await;
    assert_eq!(running.client.recv().await, Some(ServerFrame::end(id(5))));
    release.send(()).unwrap();
    assert!(refused.recv().await.unwrap(), "the late item must be refused as ended");
    running.client.send(&list_frame(6)).await;
    assert_eq!(
        running.client.recv().await,
        Some(ServerFrame::Item { id: id(6), item: json!({ "ok": true }) }),
        "nothing for request 5 arrives after its end frame"
    );
    assert_eq!(running.client.recv().await, Some(ServerFrame::end(id(6))));
}

#[tokio::test]
async fn an_item_from_a_task_of_a_panicked_handler_never_follows_its_error() {
    let (dispatcher, release, mut refused) = leaking_responders(true);
    let mut running = start(dispatcher);
    running.client.hello().await;
    running.client.send(&subscribe_frame(5)).await;
    assert_eq!(error_code(running.client.recv().await), (Some(id(5)), ErrorCode::Internal));
    release.send(()).unwrap();
    assert!(refused.recv().await.unwrap(), "the late item must be refused as ended");
    running.client.send(&list_frame(6)).await;
    assert_eq!(
        running.client.recv().await,
        Some(ServerFrame::Item { id: id(6), item: json!({ "ok": true }) })
    );
}

#[tokio::test]
async fn cancel_drops_the_handler_and_frees_the_id() {
    let (dispatcher, mut started, dropped) = holding_streams();
    let mut running = start(dispatcher);
    running.client.hello().await;
    running.client.send(&subscribe_frame(5)).await;
    started.recv().await.unwrap();
    running.client.send(&cancel_frame(5)).await;
    assert_eq!(error_code(running.client.recv().await), (Some(id(5)), ErrorCode::Cancelled));
    dropped.wait(0).await;

    running.client.send(&list_frame(5)).await;
    assert_eq!(
        running.client.recv().await,
        Some(ServerFrame::Item { id: id(5), item: json!({ "ok": true }) })
    );
    assert_eq!(running.client.recv().await, Some(ServerFrame::end(id(5))));
}

#[tokio::test]
async fn a_cancel_for_a_request_not_in_flight_is_ignored() {
    let mut running = start(answering());
    running.client.hello().await;
    running.client.send(&cancel_frame(42)).await;
    running.client.send(&list_frame(2)).await;
    assert_eq!(
        running.client.recv().await,
        Some(ServerFrame::Item { id: id(2), item: json!({ "ok": true }) })
    );
}

#[tokio::test]
async fn a_duplicate_id_is_refused_and_the_first_request_goes_on() {
    let (dispatcher, mut started, _dropped) = holding_streams();
    let mut running = start(dispatcher);
    running.client.hello().await;
    running.client.send(&subscribe_frame(5)).await;
    started.recv().await.unwrap();
    running.client.send(&list_frame(5)).await;
    assert_eq!(error_code(running.client.recv().await), (Some(id(5)), ErrorCode::Invalid));
    // Requests run side by side: another id is answered while 5 is still open.
    running.client.send(&list_frame(6)).await;
    assert_eq!(
        running.client.recv().await,
        Some(ServerFrame::Item { id: id(6), item: json!({ "ok": true }) })
    );
    assert_eq!(running.client.recv().await, Some(ServerFrame::end(id(6))));
    running.client.send(&cancel_frame(5)).await;
    assert_eq!(error_code(running.client.recv().await), (Some(id(5)), ErrorCode::Cancelled));
}

#[tokio::test]
async fn an_invalid_frame_with_a_readable_id_is_answered_and_the_connection_goes_on() {
    let mut running = start(answering());
    running.client.hello().await;
    let payload = br#"{"id": 9, "method": "no.such_method", "params": {}}"#;
    let mut bytes = u32::try_from(payload.len()).unwrap().to_be_bytes().to_vec();
    bytes.extend_from_slice(payload);
    running.client.send_raw(&bytes).await;
    assert_eq!(error_code(running.client.recv().await), (Some(id(9)), ErrorCode::Invalid));
    running.client.send(&list_frame(2)).await;
    assert_eq!(
        running.client.recv().await,
        Some(ServerFrame::Item { id: id(2), item: json!({ "ok": true }) })
    );
}

#[tokio::test]
async fn an_invalid_frame_without_an_id_is_answered_and_closes_the_connection() {
    let mut running = start(answering());
    running.client.hello().await;
    running.client.send_raw(&[0, 0, 0, 8, b'n', b'o', b't', b' ', b'j', b's', b'o', b'n']).await;
    assert_eq!(error_code(running.client.recv().await), (None, ErrorCode::Invalid));
    assert_eq!(running.client.recv().await, None);
    running.task.await.unwrap();
}

#[tokio::test]
async fn an_oversized_prefix_is_answered_and_closes_the_connection() {
    let mut running = start(answering());
    running.client.send_raw(&u32::try_from(MAX_FRAME_LEN + 1).unwrap().to_be_bytes()).await;
    assert_eq!(error_code(running.client.recv().await), (None, ErrorCode::Invalid));
    assert_eq!(running.client.recv().await, None);
    running.task.await.unwrap();
}

#[tokio::test]
async fn closing_the_client_side_cancels_every_request_in_flight() {
    let (dispatcher, mut started, dropped) = holding_streams();
    let mut running = start(dispatcher);
    running.client.hello().await;
    running.client.send(&subscribe_frame(5)).await;
    running.client.send(&subscribe_frame(6)).await;
    started.recv().await.unwrap();
    started.recv().await.unwrap();
    running.client.close_write().await;
    dropped.wait(0).await;
    dropped.wait(1).await;
    // The end frames are best effort on a closing connection; the stream then ends.
    while let Some(frame) = running.client.recv().await {
        assert_eq!(error_code(Some(frame)).1, ErrorCode::Cancelled);
    }
    running.task.await.unwrap();
}

fn context() -> ConnectionContext {
    ConnectionContext::new(ConnId::new(7), Origin::Cli, PEER)
}

#[tokio::test]
async fn the_dispatcher_hears_of_the_close_once_after_every_handler_is_gone() {
    let (dispatcher, mut started, _dropped) = holding_streams();
    let mut running = start(Arc::clone(&dispatcher));
    running.client.hello().await;
    running.client.send(&subscribe_frame(5)).await;
    running.client.send(&subscribe_frame(6)).await;
    started.recv().await.unwrap();
    started.recv().await.unwrap();
    assert_eq!(dispatcher.closed_calls(), []);

    running.client.close_write().await;
    running.task.await.unwrap();

    assert_eq!(dispatcher.closed_calls(), [Closed { context: context(), live_handlers: 0 }]);
}

#[tokio::test]
async fn shutdown_reports_the_close_too() {
    let (dispatcher, mut started, _dropped) = holding_streams();
    let mut running = start(Arc::clone(&dispatcher));
    running.client.hello().await;
    running.client.send(&subscribe_frame(5)).await;
    started.recv().await.unwrap();

    running.shutdown.cancel();
    running.task.await.unwrap();

    assert_eq!(dispatcher.closed_calls(), [Closed { context: context(), live_handlers: 0 }]);
}

#[tokio::test]
async fn a_connection_without_an_accepted_hello_is_never_reported() {
    let silent = answering();
    let mut running = start(Arc::clone(&silent));
    running.client.close_write().await;
    running.task.await.unwrap();

    let refusing = FakeDispatcher::refusing_hello(ErrorBody::new(ErrorCode::Forbidden, "no"));
    let mut refused = start(Arc::clone(&refusing));
    refused.client.send(&hello_frame(1, PROTOCOL_VERSION)).await;
    assert_eq!(error_code(refused.client.recv().await).1, ErrorCode::Forbidden);
    refused.client.close_write().await;
    refused.task.await.unwrap();

    let mismatched = answering();
    let mut mismatch = start(Arc::clone(&mismatched));
    mismatch.client.send(&hello_frame(1, PROTOCOL_VERSION + 1)).await;
    mismatch.task.await.unwrap();

    assert_eq!(silent.closed_calls(), []);
    assert_eq!(refusing.closed_calls(), []);
    assert_eq!(refusing.contexts().len(), 1, "the hello reached the dispatcher");
    assert_eq!(mismatched.closed_calls(), []);
}

#[tokio::test]
async fn shutdown_cancels_requests_and_closes_the_connection() {
    let (dispatcher, mut started, dropped) = holding_streams();
    let mut running = start(dispatcher);
    running.client.hello().await;
    running.client.send(&subscribe_frame(5)).await;
    started.recv().await.unwrap();
    running.shutdown.cancel();
    dropped.wait(0).await;
    running.task.await.unwrap();
    while let Some(frame) = running.client.recv().await {
        assert_eq!(error_code(Some(frame)).1, ErrorCode::Cancelled);
    }
}

#[tokio::test]
async fn an_overflowing_subscription_ends_with_overflow_and_last_seq() {
    let dispatcher = FakeDispatcher::new(|request: Request| async move {
        let (mut tx, mut rx) = subscription();
        rx.skip_through(Seq::new(100));
        for seq in 101..=164 {
            assert_eq!(tx.offer(Seq::new(seq), seq), Offer::Queued);
        }
        assert_eq!(tx.offer(Seq::new(165), 165), Offer::Overflowed);
        // The handler ignores the overflow; the transport still reports it.
        let _ = request.responder.forward(rx).await;
        Ok(())
    });
    let mut running = start(dispatcher);
    running.client.hello().await;
    running.client.send(&subscribe_frame(8)).await;
    for seq in 101..=164_u64 {
        assert_eq!(running.client.recv().await, Some(ServerFrame::item(id(8), &seq).unwrap()));
    }
    assert_eq!(
        running.client.recv().await,
        Some(ServerFrame::error(Some(id(8)), ErrorBody::overflow(Seq::new(164))))
    );
}

#[tokio::test]
async fn a_closing_connection_gives_up_on_a_peer_that_stops_reading() {
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let dispatcher = FakeDispatcher::new(move |request: Request| {
        let started = started_tx.clone();
        async move {
            let chunk = "x".repeat(1000);
            for n in 0.. {
                request.responder.item(&chunk).await.map_err(internal)?;
                if n == 10 {
                    started.send(()).unwrap();
                }
            }
            Ok(())
        }
    });
    // The pipe holds far less than the queued frames, and the client never reads again.
    let mut running = start_with(dispatcher, Arc::new(InstantClock), 1024);
    running.client.hello().await;
    running.client.send(&subscribe_frame(5)).await;
    started.recv().await.unwrap();
    running.client.close_write().await;
    // With a clock that never fires this would wait forever on the full pipe.
    running.task.await.unwrap();
}

#[test]
fn the_request_table_refuses_an_id_in_flight_until_it_is_removed() {
    let table = RequestTable::default();
    let first = CancellationToken::new();
    assert!(table.insert(id(1), first.clone()));
    assert!(!table.insert(id(1), CancellationToken::new()));
    assert_eq!(table.len(), 1);
    assert!(table.cancel(id(1)));
    assert!(first.is_cancelled());
    assert!(!table.cancel(id(2)));
    table.remove(id(1));
    assert_eq!(table.len(), 0);
    assert!(table.insert(id(1), CancellationToken::new()));
}
