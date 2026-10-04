use std::future::{Future, ready};
use std::sync::Arc;

use efr_protocol::{
    AdminStatus, ClientFrame, ConversationId, ConversationSubscribe, ConversationsList,
    ConversationsListResult, ErrorBody, ErrorCode, Hello, HelloResult, Method, Origin,
    PROTOCOL_VERSION, RequestId, Seq, ServerFrame,
};
use efr_stdx::time::Clock;
use efr_transport::{ConnectionContext, Dispatcher, Offer, Request, UnixListener, subscription};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::DuplexStream;
use tokio::sync::oneshot;

use super::{Client, ConnectOptions, STREAM_QUEUE_FRAMES};
use crate::ClientError;
use crate::testing::{FakeServer, InstantClock, StoppedClock, hello_result};

fn options(clock: Arc<dyn Clock>) -> ConnectOptions {
    ConnectOptions::new(Origin::Cli, clock).with_client("efr-test").with_tty("/dev/pts/3")
}

/// A client that completed hello with a scripted daemon on the other end of a pipe.
async fn connected() -> (Client, FakeServer<DuplexStream>) {
    let (client_side, server_side) = tokio::io::duplex(1 << 20);
    let mut server = FakeServer::new(server_side);
    let (client, _) = tokio::join!(
        Client::handshake(client_side, options(Arc::new(StoppedClock))),
        server.answer_hello()
    );
    (client.unwrap(), server)
}

fn list() -> Method {
    Method::ConversationsList(ConversationsList { cursor: None, limit: Some(10) })
}

fn subscribe() -> Method {
    Method::ConversationSubscribe(ConversationSubscribe {
        conversation_id: "019a9b1c-3d00-7a10-8b20-000000000001".parse::<ConversationId>().unwrap(),
        after_seq: None,
    })
}

fn empty_list() -> ConversationsListResult {
    ConversationsListResult { conversations: Vec::new(), next_cursor: None }
}

#[tokio::test]
async fn connect_says_hello_with_this_protocol_and_keeps_the_answer() {
    let (client_side, server_side) = tokio::io::duplex(1 << 16);
    let mut server = FakeServer::new(server_side);
    let (client, hello) = tokio::join!(
        Client::handshake(client_side, options(Arc::new(StoppedClock))),
        server.answer_hello()
    );
    assert_eq!(hello.protocol, PROTOCOL_VERSION);
    assert_eq!(hello.origin, Origin::Cli);
    assert_eq!(hello.client.as_deref(), Some("efr-test"));
    assert_eq!(hello.tty.as_deref(), Some("/dev/pts/3"));
    assert_eq!(hello.pid, Some(std::process::id()));
    assert_eq!(client.unwrap().hello(), &hello_result(PROTOCOL_VERSION));
}

#[tokio::test]
async fn a_refused_hello_with_a_mismatch_reports_both_versions() {
    let (client_side, server_side) = tokio::io::duplex(1 << 16);
    let mut server = FakeServer::new(server_side);
    let script = async {
        let (id, _) = server.request().await;
        let body = ErrorBody::protocol_mismatch(PROTOCOL_VERSION + 1, PROTOCOL_VERSION);
        server.send(ServerFrame::error(Some(id), body)).await;
    };
    let (client, ()) =
        tokio::join!(Client::handshake(client_side, options(Arc::new(StoppedClock))), script);
    assert!(matches!(
        client.unwrap_err(),
        ClientError::ProtocolMismatch { daemon, client }
            if daemon == PROTOCOL_VERSION + 1 && client == PROTOCOL_VERSION
    ));
}

#[tokio::test]
async fn a_daemon_that_answers_with_another_protocol_is_refused() {
    let (client_side, server_side) = tokio::io::duplex(1 << 16);
    let mut server = FakeServer::new(server_side);
    let (client, _) = tokio::join!(
        Client::handshake(client_side, options(Arc::new(StoppedClock))),
        server.answer_hello_as(PROTOCOL_VERSION + 1)
    );
    assert!(matches!(
        client.unwrap_err(),
        ClientError::ProtocolMismatch { daemon, .. } if daemon == PROTOCOL_VERSION + 1
    ));
}

#[tokio::test]
async fn another_refused_hello_is_a_server_error() {
    let (client_side, server_side) = tokio::io::duplex(1 << 16);
    let mut server = FakeServer::new(server_side);
    let script = async {
        let (id, _) = server.request().await;
        let body = ErrorBody::new(ErrorCode::Unauthorized, "unknown device");
        server.send(ServerFrame::error(Some(id), body)).await;
    };
    let (client, ()) =
        tokio::join!(Client::handshake(client_side, options(Arc::new(StoppedClock))), script);
    assert!(matches!(
        client.unwrap_err(),
        ClientError::Server { body } if body.code == ErrorCode::Unauthorized
    ));
}

#[tokio::test]
async fn a_hello_that_is_never_answered_times_out_on_the_clock() {
    let (client_side, _server_side) = tokio::io::duplex(1 << 16);
    let error = Client::handshake(client_side, options(Arc::new(InstantClock))).await.unwrap_err();
    assert!(matches!(error, ClientError::HelloTimedOut { .. }));
}

#[tokio::test]
async fn call_returns_the_typed_result() {
    let (client, mut server) = connected().await;
    let script = async {
        let (id, method) = server.request().await;
        assert_eq!(method, list());
        server.send(ServerFrame::item(id, &empty_list()).unwrap()).await;
        server.send(ServerFrame::end(id)).await;
    };
    let (result, ()) = tokio::join!(client.call::<ConversationsListResult>(list()), script);
    assert_eq!(result.unwrap(), empty_list());
}

#[tokio::test]
async fn call_reports_the_daemon_error() {
    let (client, mut server) = connected().await;
    let body = ErrorBody::new(ErrorCode::NotFound, "no such conversation");
    let script = async {
        let (id, _) = server.request().await;
        server.send(ServerFrame::error(Some(id), body.clone())).await;
    };
    let (result, ()) = tokio::join!(client.call::<ConversationsListResult>(list()), script);
    assert!(matches!(result.unwrap_err(), ClientError::Server { body: got } if got == body));
}

#[tokio::test]
async fn call_without_a_result_is_an_error() {
    let (client, mut server) = connected().await;
    let script = async {
        let (id, _) = server.request().await;
        server.send(ServerFrame::end(id)).await;
    };
    let (result, ()) = tokio::join!(client.call::<ConversationsListResult>(list()), script);
    assert!(matches!(
        result.unwrap_err(),
        ClientError::MissingResult { method: "conversations.list" }
    ));
}

#[tokio::test]
async fn call_with_a_result_of_another_shape_is_an_error() {
    let (client, mut server) = connected().await;
    let script = async {
        let (id, _) = server.request().await;
        server.send(ServerFrame::item(id, &json!({ "unexpected": true })).unwrap()).await;
        server.send(ServerFrame::end(id)).await;
    };
    let (result, ()) = tokio::join!(client.call::<ConversationsListResult>(list()), script);
    assert!(matches!(result.unwrap_err(), ClientError::DecodeItem { .. }));
}

#[tokio::test]
async fn call_refuses_a_streaming_method_without_sending_it() {
    let (client, mut server) = connected().await;
    let error = client.call::<serde_json::Value>(subscribe()).await.unwrap_err();
    assert!(matches!(error, ClientError::NotUnary { method: "conversation.subscribe" }));
    drop(client);
    assert_eq!(server.recv().await, None, "nothing was sent before the client closed");
}

#[tokio::test]
async fn concurrent_calls_are_matched_by_id() {
    let (client, mut server) = connected().await;
    let script = async {
        let (first, _) = server.request().await;
        let (second, _) = server.request().await;
        // Answer out of order.
        server.send(ServerFrame::item(second, &2).unwrap()).await;
        server.send(ServerFrame::end(second)).await;
        server.send(ServerFrame::item(first, &1).unwrap()).await;
        server.send(ServerFrame::end(first)).await;
    };
    let (one, two, ()) = tokio::join!(
        client.call::<u32>(list()),
        client.call::<u32>(Method::AdminStatus(AdminStatus {})),
        script
    );
    assert_eq!((one.unwrap(), two.unwrap()), (1, 2));
}

#[tokio::test]
async fn a_frame_for_an_unknown_request_is_ignored() {
    let (client, mut server) = connected().await;
    let script = async {
        let (id, _) = server.request().await;
        server.send(ServerFrame::item(RequestId::new(99), &0).unwrap()).await;
        server.send(ServerFrame::end(RequestId::new(99))).await;
        server.send(ServerFrame::item(id, &7).unwrap()).await;
        server.send(ServerFrame::end(id)).await;
    };
    let (result, ()) = tokio::join!(client.call::<u32>(list()), script);
    assert_eq!(result.unwrap(), 7);
}

#[tokio::test]
async fn a_malformed_frame_for_a_request_fails_only_that_request() {
    let (client, mut server) = connected().await;
    let script = async {
        let (id, _) = server.request().await;
        let payload = format!(r#"{{"id": {id}, "item": 1, "end": true}}"#);
        server.send_raw(payload.as_bytes()).await;
    };
    let (result, ()) = tokio::join!(client.call::<u32>(list()), script);
    assert!(matches!(result.unwrap_err(), ClientError::Protocol { .. }));
    assert!(!client.is_closed());
}

#[tokio::test]
async fn a_stream_yields_its_items_until_the_end() {
    let (client, mut server) = connected().await;
    let mut stream = client.stream::<u64>(subscribe()).await.unwrap();
    let (id, method) = server.request().await;
    assert_eq!(method, subscribe());
    assert_eq!(stream.id(), id);
    server.send(ServerFrame::Ack { id }).await;
    for n in 1..=3_u64 {
        server.send(ServerFrame::item(id, &n).unwrap()).await;
    }
    server.send(ServerFrame::end(id)).await;
    let items: Vec<u64> = (&mut stream).map(Result::unwrap).collect().await;
    assert_eq!(items, vec![1, 2, 3]);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn a_stream_ends_with_the_daemon_error() {
    let (client, mut server) = connected().await;
    let mut stream = client.stream::<u64>(subscribe()).await.unwrap();
    let (id, _) = server.request().await;
    server.send(ServerFrame::item(id, &41).unwrap()).await;
    server.send(ServerFrame::error(Some(id), ErrorBody::overflow(Seq::new(41)))).await;
    assert_eq!(stream.next().await.unwrap().unwrap(), 41);
    let Some(Err(ClientError::Server { body })) = stream.next().await else {
        panic!("the stream must end with the overflow error");
    };
    assert_eq!(body.last_seq(), Some(Seq::new(41)));
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn cancel_sends_a_cancel_frame_and_the_stream_ends_cancelled() {
    let (client, mut server) = connected().await;
    let mut stream = client.stream::<u64>(subscribe()).await.unwrap();
    let (id, _) = server.request().await;
    stream.cancel().await.unwrap();
    assert_eq!(server.recv().await, Some(ClientFrame::Cancel { id }));
    let cancelled = ErrorBody::new(ErrorCode::Cancelled, "the request was cancelled");
    server.send(ServerFrame::error(Some(id), cancelled)).await;
    assert!(matches!(
        stream.next().await,
        Some(Err(ClientError::Server { body })) if body.code == ErrorCode::Cancelled
    ));
}

#[tokio::test]
async fn client_cancel_names_any_request() {
    let (client, mut server) = connected().await;
    client.cancel(RequestId::new(12)).await.unwrap();
    assert_eq!(server.recv().await, Some(ClientFrame::Cancel { id: RequestId::new(12) }));
}

#[tokio::test]
async fn dropping_an_unfinished_stream_cancels_it() {
    let (client, mut server) = connected().await;
    let stream = client.stream::<u64>(subscribe()).await.unwrap();
    let (id, _) = server.request().await;
    drop(stream);
    assert_eq!(server.recv().await, Some(ClientFrame::Cancel { id }));
}

#[tokio::test]
async fn dropping_a_finished_stream_sends_nothing() {
    let (client, mut server) = connected().await;
    let mut stream = client.stream::<u64>(subscribe()).await.unwrap();
    let (id, _) = server.request().await;
    server.send(ServerFrame::end(id)).await;
    assert!(stream.next().await.is_none());
    drop(stream);
    drop(client);
    assert_eq!(server.recv().await, None, "the client closes without a cancel");
}

#[tokio::test]
async fn a_consumer_that_falls_behind_is_cancelled_locally() {
    let (client, mut server) = connected().await;
    let mut stream = client.stream::<u64>(subscribe()).await.unwrap();
    let (id, _) = server.request().await;
    let sent = u64::try_from(STREAM_QUEUE_FRAMES).unwrap() + 10;
    for n in 0..sent {
        server.send(ServerFrame::item(id, &n).unwrap()).await;
    }
    // The client cancels on the daemon as soon as its queue is full.
    assert_eq!(server.recv().await, Some(ClientFrame::Cancel { id }));
    let mut received = 0;
    let last = loop {
        match stream.next().await.unwrap() {
            Ok(_) => received += 1,
            Err(error) => break error,
        }
    };
    assert_eq!(received, STREAM_QUEUE_FRAMES - 1, "one slot stays free for the end");
    assert!(matches!(last, ClientError::StreamOverflow { id: got } if got == id));
}

#[tokio::test]
async fn an_error_without_an_id_fails_every_request() {
    let (client, mut server) = connected().await;
    let script = async {
        server.request().await;
        server.request().await;
        let body = ErrorBody::new(ErrorCode::Invalid, "the byte stream is out of step");
        server.send(ServerFrame::error(None, body)).await;
    };
    let (one, two, ()) = tokio::join!(
        client.call::<u32>(list()),
        client.call::<u32>(Method::AdminStatus(AdminStatus {})),
        script
    );
    for result in [one, two] {
        assert!(matches!(
            result.unwrap_err(),
            ClientError::Server { body } if body.code == ErrorCode::Invalid
        ));
    }
    assert!(client.is_closed());
    assert!(matches!(client.call::<u32>(list()).await.unwrap_err(), ClientError::Closed));
}

#[tokio::test]
async fn a_closed_connection_fails_requests_in_flight_and_later_ones() {
    let (client, mut server) = connected().await;
    let script = async {
        server.request().await;
        drop(server);
    };
    let (result, ()) = tokio::join!(client.call::<u32>(list()), script);
    assert!(matches!(result.unwrap_err(), ClientError::Closed));
    assert!(client.is_closed());
    assert!(matches!(client.call::<u32>(list()).await.unwrap_err(), ClientError::Closed));
}

#[tokio::test]
async fn dropping_the_client_closes_the_connection() {
    let (client, mut server) = connected().await;
    drop(client);
    assert_eq!(server.recv().await, None);
}

/// A daemon built from the real transport: lists answer with an empty page and
/// subscriptions forward three queued items.
struct TransportDaemon;

impl Dispatcher for TransportDaemon {
    fn hello(
        &self,
        _context: &ConnectionContext,
        _hello: &Hello,
    ) -> impl Future<Output = Result<HelloResult, ErrorBody>> + Send {
        ready(Ok(hello_result(PROTOCOL_VERSION)))
    }

    async fn dispatch(&self, request: Request) -> Result<(), ErrorBody> {
        let internal = |error: efr_transport::TransportError| {
            ErrorBody::new(ErrorCode::Internal, error.to_string())
        };
        let Request { method, responder, .. } = request;
        match method {
            Method::ConversationsList(_) => responder.item(&empty_list()).await.map_err(internal),
            Method::ConversationSubscribe(_) => {
                let (mut tx, rx) = subscription();
                for seq in 1..=3 {
                    assert_eq!(tx.offer(Seq::new(seq), seq), Offer::Queued);
                }
                drop(tx);
                responder.forward(rx).await.map_err(internal)
            }
            _ => Err(ErrorBody::new(ErrorCode::NotFound, "not served by this daemon")),
        }
    }
}

#[tokio::test]
async fn the_client_talks_to_the_real_transport_over_a_socket() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("efr").join("daemon.sock");
    let listener = UnixListener::bind(&socket).await.unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let server =
        tokio::spawn(listener.serve(Arc::new(TransportDaemon), Arc::new(StoppedClock), async {
            let _ = stopped.await;
        }));

    let client = Client::connect(&socket, options(Arc::new(StoppedClock))).await.unwrap();
    assert_eq!(client.hello().protocol, PROTOCOL_VERSION);
    let page: ConversationsListResult = client.call(list()).await.unwrap();
    assert_eq!(page, empty_list());
    let items: Vec<u64> =
        client.stream::<u64>(subscribe()).await.unwrap().map(Result::unwrap).collect().await;
    assert_eq!(items, vec![1, 2, 3]);
    let refused = client.call::<u32>(Method::AdminStatus(AdminStatus {})).await.unwrap_err();
    assert!(matches!(refused, ClientError::Server { body } if body.code == ErrorCode::NotFound));

    stop.send(()).unwrap();
    server.await.unwrap();
    assert!(matches!(client.call::<u32>(list()).await.unwrap_err(), ClientError::Closed));
}

#[tokio::test]
async fn connecting_to_a_missing_socket_says_no_daemon_is_running() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("daemon.sock");
    let error = Client::connect(&socket, options(Arc::new(StoppedClock))).await.unwrap_err();
    assert!(matches!(error, ClientError::DaemonNotRunning { .. }));
}
