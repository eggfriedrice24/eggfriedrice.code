//! The remote listener end to end: a daemon in this process, a fake tailscaled, and a
//! phone on a real loopback socket. The test binds 127.0.0.1 in place of the tailnet
//! address; the fake whois decides each peer, as tailscaled's does.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _};
use efr_config::RemoteSettings;
use efr_protocol::{
    AdminStatus, Base64Bytes, Capabilities, ClientFrame, DeviceId, DeviceProve, DeviceProveResult,
    ErrorBody, ErrorCode, Hello, HelloResult, LeaseReport, LeaseReportResult, Method, Origin,
    PROTOCOL_VERSION, ProjectsList, ProveContext, ProveMessage, RequestId, ScopeName, ServerFrame,
};
use efr_stdx::time::{Clock as _, SystemClock};
use efr_store::devices::{DevicePeer, DevicePublicKey, NewDevice};
use efr_test_support::{TestClock, TestDirs, WAIT_LIMIT, Wait};
use fastwebsockets::{Frame, OpCode, Payload, Role, WebSocket};
use pretty_assertions::assert_eq;
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::discovery::DaemonInfo;
use crate::methods::Methods;
use crate::state::State;
use crate::testing::{self, RawClient};
use crate::{DaemonError, Settings};
use efr_test_support::tailscaled::{self as fake, FakeTailscaled};

/// A daemon with its remote listener on 127.0.0.1 and a fake tailscaled.
struct Harness {
    fake: FakeTailscaled,
    state: Arc<State>,
    shutdown: CancellationToken,
    served: JoinHandle<Result<(), DaemonError>>,
    remote: JoinHandle<()>,
    _dirs: TestDirs,
}

impl Harness {
    async fn start() -> Harness {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = crate::start(Settings::default(), testing::deps(&dirs, &clock)).await.unwrap();
        let state = daemon.state();
        let socket = daemon.socket_path().to_path_buf();
        let shutdown = CancellationToken::new();
        let served = tokio::spawn(daemon.serve(shutdown.clone()));
        let fake = FakeTailscaled::start().unwrap();
        let mut settings = RemoteSettings::default();
        settings.enabled = true;
        settings.tailscaled_socket = fake.socket().to_path_buf();
        let info = DaemonInfo {
            pid: state.pid,
            socket,
            protocol: PROTOCOL_VERSION,
            daemon_id: state.daemon_id,
            tailnet_endpoint: None,
        };
        let remote = tokio::spawn(super::serve(
            Arc::clone(&state),
            Arc::new(Methods::new(Arc::clone(&state))),
            settings,
            info,
            Some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
            shutdown.child_token(),
        ));
        let harness = Harness { fake, state, shutdown, served, remote, _dirs: dirs };
        harness.addr().await;
        harness
    }

    /// Where the listener listens, once it does.
    async fn addr(&self) -> SocketAddrV4 {
        let endpoint = Wait::new("the remote listener to listen")
            .until_some(|| self.state.remote.endpoint.borrow().clone())
            .await
            .unwrap();
        SocketAddrV4::new(endpoint.host, endpoint.port)
    }

    /// Adds a device with `key`, paired from the phone's node.
    async fn pair(&self, key: &EcdsaKeyPair) -> DeviceId {
        let device_id = DeviceId::from_uuid(uuid::Uuid::now_v7());
        let peer = DevicePeer {
            node_stable_id: fake::PHONE_NODE.to_owned(),
            user_id: fake::USER,
            tailnet_stable_id: fake::TAILNET.to_owned(),
        };
        let public = DevicePublicKey::from_x963(key.public_key().as_ref()).unwrap();
        let scopes = [ScopeName::Read, ScopeName::Operate, ScopeName::Approve];
        let new = NewDevice::new(device_id, "phone", public, "SHA256:test", peer, &scopes).unwrap();
        self.state.writer.device_insert(new).await.unwrap();
        device_id
    }

    async fn stop(self) {
        let Harness { fake, state, shutdown, served, remote, _dirs } = self;
        shutdown.cancel();
        remote.await.unwrap();
        // NOTE: the drain waits for the last handle of the state.
        drop(state);
        served.await.unwrap().unwrap();
        drop(fake);
    }
}

fn key() -> EcdsaKeyPair {
    EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).unwrap()
}

/// The signature of the proof of `device_id` after `hello`.
fn proof(key: &EcdsaKeyPair, hello: &HelloResult, device_id: DeviceId) -> Base64Bytes {
    let prove = hello.prove.as_ref().expect("a remote hello has prove facts");
    let message = ProveMessage {
        challenge: &hello.challenge,
        daemon_id: hello.daemon_id,
        protocol: hello.protocol,
        device_id,
        peer_node: &prove.peer_node,
        daemon_tailnet: &prove.daemon_tailnet,
    };
    let signature = key.sign(&SystemRandom::new(), &message.to_bytes()).unwrap();
    Base64Bytes::new(signature.as_ref().to_vec())
}

/// The phone: a WebSocket client on a TCP stream.
struct Phone {
    ws: WebSocket<TcpStream>,
    next_id: u64,
}

/// Opens a TCP stream to `addr` and sends the opening handshake with `origin`. The
/// status line of the answer, or an empty string when the stream closed first.
async fn handshake(addr: SocketAddrV4, origin: Option<&str>) -> (TcpStream, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let origin = origin.map(|origin| format!("Origin: {origin}\r\n")).unwrap_or_default();
    let head = format!(
        "GET / HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{origin}\r\n"
    );
    // NOTE: a refused peer may be closed before the head is written.
    let _ = stream.write_all(head.as_bytes()).await;
    let mut answer = Vec::new();
    let mut byte = [0; 1];
    let read = async {
        while !answer.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte).await {
                Ok(1) => answer.push(byte[0]),
                _ => break,
            }
        }
    };
    SystemClock.timeout(WAIT_LIMIT, read).await.expect("the listener answered nothing");
    let status = String::from_utf8_lossy(&answer).lines().next().unwrap_or_default().to_owned();
    (stream, status)
}

impl Phone {
    async fn open(addr: SocketAddrV4) -> Phone {
        let (stream, status) = handshake(addr, Some(&format!("http://{addr}"))).await;
        assert_eq!(status, "HTTP/1.1 101 Switching Protocols");
        let mut ws = WebSocket::after_handshake(stream, Role::Client);
        ws.set_auto_close(true);
        ws.set_auto_pong(true);
        Phone { ws, next_id: 0 }
    }

    async fn send(&mut self, method: Method) -> RequestId {
        self.next_id += 1;
        let id = RequestId::new(self.next_id);
        let text = serde_json::to_vec(&ClientFrame::Request { id, method }).unwrap();
        self.ws.write_frame(Frame::text(Payload::Owned(text))).await.unwrap();
        id
    }

    /// The next frame from the daemon; `None` once the connection closed.
    async fn frame(&mut self) -> Option<ServerFrame> {
        let read = SystemClock.timeout(WAIT_LIMIT, self.ws.read_frame()).await;
        let frame = read.expect("the daemon sent nothing").ok()?;
        match frame.opcode {
            OpCode::Text => Some(ServerFrame::from_json(&frame.payload).unwrap()),
            OpCode::Close => None,
            other => panic!("unexpected frame {other:?}"),
        }
    }

    async fn call<T: DeserializeOwned>(&mut self, method: Method) -> Result<T, ErrorBody> {
        let id = self.send(method).await;
        match self.frame().await.expect("the daemon closed the connection") {
            ServerFrame::Item { id: got, item } => {
                assert_eq!(got, id);
                let end = self.frame().await;
                assert!(matches!(end, Some(ServerFrame::End { id: end }) if end == id));
                Ok(serde_json::from_value(item).unwrap())
            }
            ServerFrame::Error(frame) => Err(frame.error),
            other => panic!("unexpected frame {other:?}"),
        }
    }

    async fn hello(&mut self, device_id: Option<DeviceId>) -> HelloResult {
        let hello = Method::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            origin: Origin::Shell,
            client: Some("efr phone tests".to_owned()),
            capabilities: Capabilities::default(),
            tty: None,
            pid: None,
            device_id,
        });
        self.call(hello).await.unwrap()
    }

    async fn prove(
        &mut self,
        device_id: DeviceId,
        signature: Base64Bytes,
    ) -> Result<DeviceProveResult, ErrorBody> {
        self.call(Method::DeviceProve(DeviceProve { device_id, signature })).await
    }

    /// Reads until the daemon closes the connection.
    async fn closed(&mut self) {
        while self.frame().await.is_some() {}
    }
}

/// A phone whose device proved its key.
async fn proved(harness: &Harness, key: &EcdsaKeyPair, device_id: DeviceId) -> Phone {
    let mut phone = Phone::open(harness.addr().await).await;
    let hello = phone.hello(Some(device_id)).await;
    phone.prove(device_id, proof(key, &hello, device_id)).await.unwrap();
    phone
}

#[tokio::test]
async fn a_paired_phone_proves_its_key_and_then_holds_its_scopes() {
    let harness = Harness::start().await;
    let addr = harness.addr().await;
    let written = std::fs::read(harness.state.dirs.daemon_json_path()).unwrap();
    let info: DaemonInfo = serde_json::from_slice(&written).unwrap();
    assert_eq!(info.tailnet_endpoint, Some(addr.to_string()), "daemon.json names the endpoint");

    let key = key();
    let device_id = harness.pair(&key).await;
    let mut phone = Phone::open(addr).await;
    let hello = phone.hello(Some(device_id)).await;
    assert_eq!(
        hello.prove,
        Some(ProveContext {
            peer_node: fake::PHONE_NODE.to_owned(),
            daemon_node: fake::SELF_NODE.to_owned(),
            daemon_tailnet: fake::TAILNET.to_owned(),
        })
    );
    assert_eq!(hello.capabilities.device_prove, Some(true));
    assert_eq!(hello.capabilities.admin, Some(false));

    let early = phone.call::<serde_json::Value>(Method::ProjectsList(ProjectsList {})).await;
    assert_eq!(early.unwrap_err().code, ErrorCode::Unauthorized, "nothing before the proof");

    let result = phone.prove(device_id, proof(&key, &hello, device_id)).await.unwrap();
    assert_eq!(result.scopes, [ScopeName::Read, ScopeName::Operate, ScopeName::Approve]);
    assert!(phone.call::<serde_json::Value>(Method::ProjectsList(ProjectsList {})).await.is_ok());
    // NOTE: the app's liveness probe: JS never sees a pong, so it sends a lease.
    let lease = LeaseReport { conversations: Vec::new(), ptys: Vec::new(), visible: true };
    let leased: LeaseReportResult = phone.call(Method::LeaseReport(lease)).await.unwrap();
    assert_eq!(leased.ttl_secs, 60);
    let admin = phone.call::<serde_json::Value>(Method::AdminStatus(AdminStatus {})).await;
    assert_eq!(admin.unwrap_err().code, ErrorCode::Forbidden, "never admin");
    let again = phone.prove(device_id, proof(&key, &hello, device_id)).await;
    assert_eq!(again.unwrap_err().code, ErrorCode::Conflict, "one proof per connection");

    let row = harness
        .state
        .readers
        .with(move |conn| efr_store::devices::get(conn, device_id))
        .await
        .unwrap()
        .unwrap();
    assert!(row.last_seen_at.is_some(), "the proof marks the device as seen");
    harness.stop().await;
}

#[tokio::test]
async fn three_failed_proofs_close_the_socket() {
    let harness = Harness::start().await;
    let key = key();
    let device_id = harness.pair(&key).await;
    let mut phone = Phone::open(harness.addr().await).await;
    let hello = phone.hello(None).await;
    let wrong = proof(&self::key(), &hello, device_id);
    for _ in 0..2 {
        let refused = phone.prove(device_id, wrong.clone()).await.unwrap_err();
        assert_eq!(refused.code, ErrorCode::Unauthorized);
        assert_eq!(refused.message, "the device could not prove its key");
    }
    phone.send(Method::DeviceProve(DeviceProve { device_id, signature: wrong })).await;
    phone.closed().await;
    harness.stop().await;
}

#[tokio::test]
async fn a_signature_for_another_connection_proves_nothing() {
    let harness = Harness::start().await;
    let key = key();
    let device_id = harness.pair(&key).await;
    let addr = harness.addr().await;
    let mut first = Phone::open(addr).await;
    let first_hello = first.hello(None).await;
    let signature = proof(&key, &first_hello, device_id);

    let mut second = Phone::open(addr).await;
    let second_hello = second.hello(None).await;
    assert_ne!(first_hello.challenge, second_hello.challenge, "each connection has its own");
    let replayed = second.prove(device_id, signature.clone()).await;
    assert_eq!(replayed.unwrap_err().code, ErrorCode::Unauthorized);
    assert!(first.prove(device_id, signature).await.is_ok(), "on its own connection it proves");

    let unknown = DeviceId::from_uuid(uuid::Uuid::now_v7());
    let refused = second.prove(unknown, proof(&key, &second_hello, unknown)).await.unwrap_err();
    assert_eq!(refused.message, "the device could not prove its key", "no hint that it is unknown");
    harness.stop().await;
}

#[tokio::test]
async fn a_revoke_closes_the_live_socket_and_refuses_the_next_proof() {
    let harness = Harness::start().await;
    let key = key();
    let device_id = harness.pair(&key).await;
    let mut phone = proved(&harness, &key, device_id).await;

    harness.state.writer.device_revoke(device_id).await.unwrap();
    phone.closed().await;

    let mut next = Phone::open(harness.addr().await).await;
    let hello = next.hello(None).await;
    let refused = next.prove(device_id, proof(&key, &hello, device_id)).await;
    assert_eq!(refused.unwrap_err().code, ErrorCode::Unauthorized);
    harness.stop().await;
}

#[tokio::test]
async fn a_phone_on_a_new_tailscale_node_must_pair_again() {
    let harness = Harness::start().await;
    let key = key();
    let device_id = harness.pair(&key).await;
    harness.fake.set_whois(Some(fake::whois("nNEWxxCNTRL", fake::USER, &[])));
    let mut phone = Phone::open(harness.addr().await).await;
    let hello = phone.hello(None).await;
    let refused = phone.prove(device_id, proof(&key, &hello, device_id)).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::Unauthorized);
    assert_eq!(refused.data.as_ref().and_then(|data| data.get("re_pair")), Some(&json!(true)));
    harness.stop().await;
}

#[tokio::test]
async fn the_self_node_a_tagged_node_another_user_and_an_unknown_peer_get_no_byte() {
    let harness = Harness::start().await;
    let addr = harness.addr().await;
    let refused = [
        Some(fake::whois(fake::SELF_NODE, fake::USER, &[])),
        Some(fake::whois("nTAGGEDCNTRL", fake::USER, &["tag:ci"])),
        Some(fake::whois(fake::PHONE_NODE, fake::USER + 1, &[])),
        None,
    ];
    for whois in refused {
        harness.fake.set_whois(whois.clone());
        let (_, status) = handshake(addr, None).await;
        assert_eq!(status, "", "{whois:?} gets no answer");
    }
    harness.fake.set_whois(Some(fake::whois(fake::PHONE_NODE, fake::USER, &[])));
    let (_, status) = handshake(addr, None).await;
    assert_eq!(status, "HTTP/1.1 101 Switching Protocols", "no Origin is the app's too");
    harness.stop().await;
}

#[tokio::test]
async fn daemon_json_names_the_endpoint_only_once_pairing_can_use_it() {
    let harness = Harness::start().await;
    let path = harness.state.dirs.daemon_json_path();
    Wait::new("daemon.json to name the endpoint")
        .until(|| {
            std::fs::read(&path).ok().is_some_and(|text| {
                serde_json::from_slice::<serde_json::Value>(&text)
                    .is_ok_and(|info| info["tailnet_endpoint"].is_string())
            })
        })
        .await
        .unwrap();
    assert!(
        harness.state.remote.endpoint.borrow().is_some(),
        "a client that read daemon.json can start a pairing"
    );
    harness.stop().await;
}

#[tokio::test]
async fn only_a_missing_or_the_self_origin_may_open_a_socket() {
    let harness = Harness::start().await;
    let addr = harness.addr().await;
    for origin in ["http://evil.example", "null", "https://127.0.0.1", "http://localhost"] {
        let (_, status) = handshake(addr, Some(origin)).await;
        assert!(status.starts_with("HTTP/1.1 4"), "{origin}: {status}");
    }
    let (_, status) = handshake(addr, Some(&format!("http://{addr}"))).await;
    assert_eq!(status, "HTTP/1.1 101 Switching Protocols");
    let named = format!("http://desk.example.ts.net:{}", addr.port());
    let (_, status) = handshake(addr, Some(&named)).await;
    assert_eq!(status, "HTTP/1.1 101 Switching Protocols", "the MagicDNS name of the node");
    harness.stop().await;
}

#[tokio::test]
async fn the_unix_socket_refuses_the_proof() {
    let harness = Harness::start().await;
    let socket = harness.state.dirs.checked_socket_path().unwrap();
    let (mut client, hello) = RawClient::hello(&socket, None).await;
    assert_eq!(hello.prove, None);
    assert_eq!(hello.capabilities.device_prove, None);
    let device_id = DeviceId::from_uuid(uuid::Uuid::now_v7());
    let signature = Base64Bytes::new(vec![0x30, 0x00]);
    let refused = client
        .call::<serde_json::Value>(Method::DeviceProve(DeviceProve { device_id, signature }))
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Forbidden);
    harness.stop().await;
}

#[tokio::test]
async fn a_tailscaled_that_is_gone_closes_the_listener_and_the_phones() {
    let harness = Harness::start().await;
    let key = key();
    let device_id = harness.pair(&key).await;
    let mut phone = proved(&harness, &key, device_id).await;

    harness.fake.set_status(None);
    harness.fake.end_watches();
    phone.closed().await;
    Wait::new("the listener to stop")
        .until(|| harness.state.remote.endpoint.borrow().is_none())
        .await
        .unwrap();
    let written = std::fs::read(harness.state.dirs.daemon_json_path()).unwrap();
    let info: DaemonInfo = serde_json::from_slice(&written).unwrap();
    assert_eq!(info.tailnet_endpoint, None);
    assert_eq!(harness.state.remote.live().count(device_id), 0);
    harness.stop().await;
}

#[tokio::test]
async fn another_tailnet_rebinds_and_closes_the_phones_of_the_old_one() {
    let harness = Harness::start().await;
    let key = key();
    let device_id = harness.pair(&key).await;
    let mut phone = proved(&harness, &key, device_id).await;
    let before = harness.addr().await;

    harness.fake.set_status(Some(fake::status(fake::SELF_IP, "T9999CNTRL")));
    harness.fake.push(&json!({ "State": 6 }));
    phone.closed().await;
    let after = Wait::new("a new listener")
        .until_some(|| {
            let endpoint = harness.state.remote.endpoint.borrow().clone()?;
            (endpoint.port != before.port()).then_some(endpoint)
        })
        .await
        .unwrap();

    // The device was paired on the old tailnet, so it cannot prove itself on the new one.
    let mut next = Phone::open(SocketAddrV4::new(after.host, after.port)).await;
    let hello = next.hello(None).await;
    let refused = next.prove(device_id, proof(&key, &hello, device_id)).await;
    assert_eq!(refused.unwrap_err().code, ErrorCode::Unauthorized);
    harness.stop().await;
}

#[test]
fn a_remote_error_carries_its_retry_hint() {
    let frame = efr_protocol::ErrorFrame::from(DaemonError::RemoteStarting { retry_after_secs: 2 });
    assert_eq!(frame.error.code, ErrorCode::Busy);
    assert_eq!(frame.error.data, Some(json!({ "starting": true, "retry_after_secs": 2 })));
}
