//! `hello`, the scopes it grants and the single-instance lock, over a `TestDaemon`.

use efr_daemon::DaemonError;
use efr_protocol::framing::{self, Decoder};
use efr_protocol::{
    AdminStatus, AdminStatusResult, Capabilities, ClientFrame, ConversationsList,
    ConversationsListResult, ErrorCode, Hello, Method, Origin, PROTOCOL_VERSION, PtyAttach, PtyId,
    RequestId, ServerFrame,
};
use efr_test_daemon::{ClientError, TestDaemon, TestDaemonError};
use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;

fn refusal<T: std::fmt::Debug>(result: Result<T, ClientError>) -> ErrorCode {
    match result {
        Err(ClientError::Server { body }) => body.code,
        other => panic!("expected a refusal from the daemon, got {other:?}"),
    }
}

#[tokio::test]
async fn hello_names_the_daemon_its_directories_and_a_fresh_challenge() {
    let daemon = TestDaemon::start().await.unwrap();
    let first = daemon.client().await.unwrap();
    let second = daemon.client().await.unwrap();

    let hello = first.hello();
    assert_eq!(hello.protocol, PROTOCOL_VERSION);
    assert_eq!(hello.daemon_id, daemon.daemon_id());
    assert_eq!(hello.capabilities.admin, Some(true));
    assert_eq!(hello.capabilities.screen_snapshots, Some(true));
    let data = daemon.dirs().dirs().data();
    assert_eq!(hello.paths.data_dir, data);
    assert_eq!(hello.paths.scratch_root, data.join("scratch"));
    assert!(!hello.challenge.is_empty());
    assert_ne!(hello.challenge, second.hello().challenge, "every hello gets its own challenge");

    drop((first, second));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn admin_status_reports_the_test_daemon() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();

    let status: AdminStatusResult =
        client.call(Method::AdminStatus(AdminStatus::default())).await.unwrap();

    assert_eq!(status.daemon_id, daemon.daemon_id());
    assert_eq!(status.protocol, PROTOCOL_VERSION);
    assert_eq!(status.screen_backend, "vt100");
    assert_eq!(status.conversations, 0);
    assert_eq!(status.shells, 0);
    let providers: Vec<(&str, bool)> = status
        .providers
        .iter()
        .map(|provider| (provider.provider.as_str(), provider.logged_in))
        .collect();
    assert_eq!(
        providers,
        [("openai-subscription", false), ("openai-api", false), ("anthropic-api", false)]
    );

    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_phone_connection_reads_but_has_no_admin_or_terminal_scope() {
    let daemon = TestDaemon::start().await.unwrap();
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();
    assert_eq!(phone.hello().capabilities.admin, Some(false));

    let list: ConversationsListResult =
        phone.call(Method::ConversationsList(ConversationsList::default())).await.unwrap();
    assert!(list.conversations.is_empty());
    let status = phone.call::<AdminStatusResult>(Method::AdminStatus(AdminStatus::default())).await;
    assert_eq!(refusal(status), ErrorCode::Forbidden);
    let attach = Method::PtyAttach(PtyAttach {
        pty_id: PtyId::from_uuid(uuid::Uuid::from_u128(1)),
        since_seq: None,
        scrollback_rows: None,
    });
    let mut stream = phone.stream::<serde_json::Value>(attach).await.unwrap();
    let ended = futures::StreamExt::next(&mut stream).await.unwrap();
    assert_eq!(refusal(ended), ErrorCode::Forbidden);

    drop((stream, phone));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_client_of_another_protocol_version_is_refused_at_hello() {
    let daemon = TestDaemon::start().await.unwrap();
    let mut socket = UnixStream::connect(daemon.socket_path()).await.unwrap();
    let hello = Method::Hello(Hello {
        protocol: PROTOCOL_VERSION + 1,
        origin: Origin::Cli,
        client: Some("from the future".to_owned()),
        capabilities: Capabilities::default(),
        tty: None,
        pid: None,
        device_id: None,
    });
    let id = RequestId::new(1);
    let frame = framing::encode(&ClientFrame::Request { id, method: hello }).unwrap();
    socket.write_all(&frame).await.unwrap();

    let mut decoder = Decoder::new();
    let mut buffer = vec![0; 64 * 1024];
    let answer = loop {
        let read = socket.read(&mut buffer).await.unwrap();
        assert!(read > 0, "the daemon closed the connection without an answer");
        if let Some(payload) = decoder.push(&buffer[..read]).unwrap().into_iter().next() {
            break ServerFrame::from_json(&payload).unwrap();
        }
    };

    let ServerFrame::Error(error) = answer else { panic!("expected an error, got {answer:?}") };
    assert_eq!(error.id, Some(id));
    assert_eq!(error.error.code, ErrorCode::ProtocolMismatch);
    drop(socket);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_second_daemon_on_the_same_tree_is_refused_by_the_lock() {
    let daemon = TestDaemon::start().await.unwrap();

    let second = TestDaemon::builder().dirs(daemon.dirs().clone()).start().await;

    assert!(
        matches!(
            second,
            Err(TestDaemonError::Daemon { source: DaemonError::AlreadyRunning { .. } })
        ),
        "{second:?}"
    );
    daemon.stop().await.unwrap();
}
