//! The pairing methods end to end: `efr device add`'s side on the Unix socket, and the
//! phone's side through the WebSocket entry with the admission value that the tailnet
//! listener gives a connection. The WebSocket runs over an in-memory pipe with a raw
//! client that writes its frames by hand.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use efr_protocol::{
    Base64Bytes, Capabilities, ClientFrame, DaemonPaths, DeviceEnroll, DeviceEnrollResult,
    DeviceId, DeviceInfo, DeviceList, DeviceListResult, DevicePairConfirm, DevicePairConfirmResult,
    DevicePairStart, DevicePairStartItem, DeviceRevoke, DeviceRevokeResult, ErrorBody, ErrorCode,
    Hello, HelloResult, Method, Origin, PROTOCOL_VERSION, PairEnd, PairingQr, RequestId, ScopeName,
    SecretText, ServerFrame,
};
use efr_test_support::{TestClock, TestDirs, Wait};
use efr_transport::{RemotePeer, WebSocketOptions};
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, DuplexStream};
use tokio_util::sync::CancellationToken;

use crate::Settings;
use crate::methods::Methods;
use crate::pairing::{EnrollPeer, PAIRING_TTL};
use crate::remote::RemoteEndpoint;
use crate::state::State;
use crate::testing::{RawClient, deps};

const MASK: [u8; 4] = [0x5a, 0x01, 0xc3, 0x7e];

fn endpoint() -> RemoteEndpoint {
    RemoteEndpoint {
        host: Ipv4Addr::new(100, 64, 0, 1),
        port: 7419,
        fqdn: Some("box.tail1234.ts.net".to_owned()),
    }
}

fn peer() -> EnrollPeer {
    EnrollPeer {
        node_stable_id: "nPHONE1CNTRL".to_owned(),
        node_name: "annas-iphone.tail1234.ts.net.".to_owned(),
        user_id: 42,
        user_login: "anna@example.com".to_owned(),
        tailnet_stable_id: "TtAiLnEt1".to_owned(),
        os: None,
    }
}

fn public_key(fill: u8) -> Base64Bytes {
    let mut key = vec![fill; 65];
    key[0] = 0x04;
    Base64Bytes::new(key)
}

/// A daemon on the Unix socket, with its state for the phone's side.
struct Daemon {
    socket: std::path::PathBuf,
    state: Arc<State>,
    clock: TestClock,
    shutdown: CancellationToken,
    _dirs: TestDirs,
}

impl Daemon {
    /// A daemon whose `[remote]` is on, with a listener at [`endpoint`] when
    /// `listening`.
    async fn start(remote: bool, listening: bool) -> Daemon {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let mut config = Settings::default();
        config.remote.enabled = remote;
        // NOTE: a socket that does not exist, so the listener never asks the real
        // tailscaled and never binds; the test sets the endpoint itself.
        config.remote.tailscaled_socket = dirs.root().join("no-tailscaled.sock");
        let daemon = crate::start(config, deps(&dirs, &clock)).await.unwrap();
        let state = daemon.state();
        if listening {
            state.remote.endpoint.send_replace(Some(endpoint()));
        }
        let socket = daemon.socket_path().to_path_buf();
        let shutdown = CancellationToken::new();
        tokio::spawn(daemon.serve(shutdown.clone()));
        Daemon { socket, state, clock, shutdown, _dirs: dirs }
    }

    async fn terminal(&self) -> RawClient {
        RawClient::hello_as(&self.socket, None, Origin::Cli).await.0
    }

    /// The terminal's answer to `pairing_id`, on a connection of its own, so its frames
    /// never mix with the stream's.
    async fn answer(
        &self,
        pairing_id: efr_protocol::PairingId,
        accept: bool,
    ) -> DevicePairConfirmResult {
        let confirm = Method::DevicePairConfirm(DevicePairConfirm { pairing_id, accept });
        self.terminal().await.call(confirm).await.unwrap()
    }

    /// A phone connection through the WebSocket entry, admitted with `peer`, after its
    /// hello.
    async fn phone(&self, peer: EnrollPeer) -> Phone {
        let mut phone = self.upgraded(peer).await;
        phone.hello(None).await;
        phone
    }

    /// A phone connection through the WebSocket entry, admitted with `peer`, before its
    /// hello.
    async fn upgraded(&self, peer: EnrollPeer) -> Phone {
        let (client, server) = tokio::io::duplex(1 << 16);
        let addr: SocketAddr = "100.64.0.2:51000".parse().unwrap();
        let remote = RemotePeer::new(addr, Arc::new(crate::remote::test_admission(&peer)));
        let methods = Arc::new(Methods::new(Arc::clone(&self.state)));
        let clock = self.clock.shared();
        let shutdown = self.shutdown.child_token();
        tokio::spawn(async move {
            let _ = efr_transport::serve_websocket(
                server,
                remote,
                &WebSocketOptions::new(),
                methods,
                clock,
                shutdown,
            )
            .await;
        });
        let mut phone = Phone { stream: client, next_id: 0 };
        phone.upgrade().await;
        phone
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// The phone's end of the pipe: a raw WebSocket client.
struct Phone {
    stream: DuplexStream,
    next_id: u64,
}

impl Phone {
    async fn upgrade(&mut self) {
        let head = "GET / HTTP/1.1\r\nHost: 100.64.0.1:7419\r\nUpgrade: websocket\r\n\
                    Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                    Sec-WebSocket-Version: 13\r\n\r\n";
        self.stream.write_all(head.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            response.push(self.stream.read_u8().await.unwrap());
        }
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 101"), "{response}");
    }

    async fn hello(&mut self, tty: Option<&str>) -> HelloResult {
        let hello = Method::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            origin: Origin::Phone,
            client: Some("efr-daemon tests".to_owned()),
            capabilities: Capabilities::default(),
            tty: tty.map(str::to_owned),
            pid: None,
            device_id: None,
        });
        let id = self.send(hello).await;
        let ServerFrame::Item { id: got, item } = self.recv().await else {
            panic!("hello has a result");
        };
        assert_eq!(got, id);
        assert_eq!(self.recv().await, ServerFrame::end(id));
        serde_json::from_value(item).unwrap()
    }

    async fn send(&mut self, method: Method) -> RequestId {
        self.next_id += 1;
        let id = RequestId::new(self.next_id);
        let payload = serde_json::to_vec(&ClientFrame::Request { id, method }).unwrap();
        let mut frame = vec![0x81];
        match payload.len() {
            len if len < 126 => frame.push(0x80 | u8::try_from(len).unwrap()),
            len => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&u16::try_from(len).unwrap().to_be_bytes());
            }
        }
        frame.extend_from_slice(&MASK);
        frame.extend(payload.iter().enumerate().map(|(n, byte)| byte ^ MASK[n % 4]));
        self.stream.write_all(&frame).await.unwrap();
        id
    }

    /// The next text message as a server frame; pings in between are skipped.
    async fn recv(&mut self) -> ServerFrame {
        loop {
            let mut head = [0_u8; 2];
            self.stream.read_exact(&mut head).await.unwrap();
            let len = match head[1] & 0x7f {
                126 => usize::from(self.stream.read_u16().await.unwrap()),
                127 => usize::try_from(self.stream.read_u64().await.unwrap()).unwrap(),
                len => usize::from(len),
            };
            let mut payload = vec![0; len];
            self.stream.read_exact(&mut payload).await.unwrap();
            match head[0] & 0x0f {
                0x1 => return ServerFrame::from_json(&payload).unwrap(),
                0x9 => {}
                opcode => panic!("unexpected frame of opcode {opcode}"),
            }
        }
    }

    /// The answer of a unary request.
    async fn answer(&mut self, id: RequestId) -> Result<serde_json::Value, ErrorBody> {
        match self.recv().await {
            ServerFrame::Item { id: got, item } => {
                assert_eq!(got, id);
                assert_eq!(self.recv().await, ServerFrame::end(id));
                Ok(item)
            }
            ServerFrame::Error(frame) => {
                assert_eq!(frame.id, Some(id));
                Err(frame.error)
            }
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

fn enroll(token: &SecretText, name: &str, key: Base64Bytes) -> Method {
    Method::DeviceEnroll(DeviceEnroll {
        token: token.clone(),
        name: name.to_owned(),
        public_key: key,
    })
}

/// Starts `device.pair_start` and returns its id and the `started` item's QR code.
async fn pair_start(terminal: &mut RawClient) -> (RequestId, efr_protocol::PairingId, PairingQr) {
    let id = terminal.send(Method::DevicePairStart(DevicePairStart::default())).await;
    let item = terminal.next(id).await.unwrap().unwrap();
    let DevicePairStartItem::Started { pairing_id, qr, .. } = serde_json::from_value(item).unwrap()
    else {
        panic!("the first item is started");
    };
    (id, pairing_id, qr)
}

async fn item(terminal: &mut RawClient, id: RequestId) -> DevicePairStartItem {
    serde_json::from_value(terminal.next(id).await.unwrap().unwrap()).unwrap()
}

#[tokio::test]
async fn a_phone_pairs_through_the_listener_after_a_yes_at_the_terminal() {
    let daemon = Daemon::start(true, true).await;
    let mut terminal = daemon.terminal().await;
    let mut phone = daemon.phone(peer()).await;

    let (stream, pairing_id, qr) = pair_start(&mut terminal).await;
    assert_eq!(qr.daemon_id, daemon.state.daemon_id);
    assert_eq!((qr.host, qr.port), (Ipv4Addr::new(100, 64, 0, 1), 7419));
    assert_eq!(qr.fqdn.as_deref(), Some("box.tail1234.ts.net"));
    assert!(qr.url().expose_secret().starts_with("efr://pair?v=1&d="));

    let enrolled = phone.send(enroll(&qr.token, "Anna's iPhone", public_key(7))).await;
    let DevicePairStartItem::Pending { device_name, node_name, user_login, os, .. } =
        item(&mut terminal, stream).await
    else {
        panic!("a phone that sent the token is pending");
    };
    assert_eq!(
        (device_name.as_str(), node_name.as_str(), user_login.as_str(), os),
        ("Anna's iPhone", "annas-iphone.tail1234.ts.net", "anna@example.com", None)
    );
    let confirmed = daemon.answer(pairing_id, true).await;
    let device_id = confirmed.device_id.unwrap();
    assert_eq!(
        item(&mut terminal, stream).await,
        DevicePairStartItem::Paired { pairing_id, device_id }
    );
    assert_eq!(terminal.next(stream).await, Ok(None));

    let result: DeviceEnrollResult =
        serde_json::from_value(phone.answer(enrolled).await.unwrap()).unwrap();
    assert_eq!(result.device_id, device_id);
    assert_eq!(result.scopes, vec![ScopeName::Read, ScopeName::Operate, ScopeName::Approve]);

    let listed: DeviceListResult =
        terminal.call(Method::DeviceList(DeviceList::default())).await.unwrap();
    let [device]: [DeviceInfo; 1] = listed.devices.try_into().unwrap();
    assert_eq!((device.device_id, device.name.as_str()), (device_id, "Anna's iPhone"));
    assert_eq!(device.node, "nPHONE1CNTRL");
    assert!(device.key_fingerprint.starts_with("SHA256:"), "{}", device.key_fingerprint);
    assert_eq!((device.last_seen_at, device.revoked_at), (None, None));
}

#[tokio::test]
async fn a_no_at_the_terminal_is_forbidden_for_the_phone() {
    let daemon = Daemon::start(true, true).await;
    let mut terminal = daemon.terminal().await;
    let mut phone = daemon.phone(peer()).await;
    let (stream, pairing_id, qr) = pair_start(&mut terminal).await;

    let enrolled = phone.send(enroll(&qr.token, "iPhone", public_key(7))).await;
    assert!(matches!(item(&mut terminal, stream).await, DevicePairStartItem::Pending { .. }));
    let answered = daemon.answer(pairing_id, false).await;

    assert_eq!(answered.device_id, None);
    assert_eq!(
        item(&mut terminal, stream).await,
        DevicePairStartItem::Ended { pairing_id, reason: PairEnd::Refused }
    );
    assert_eq!(terminal.next(stream).await, Ok(None));
    let refused = phone.answer(enrolled).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(refused.message, "the terminal refused the device");
    let listed: DeviceListResult =
        terminal.call(Method::DeviceList(DeviceList::default())).await.unwrap();
    assert!(listed.devices.is_empty());
}

#[tokio::test]
async fn a_wrong_token_is_unauthorized_and_the_pairing_stays_open() {
    let daemon = Daemon::start(true, true).await;
    let mut terminal = daemon.terminal().await;
    let mut phone = daemon.phone(peer()).await;
    let (_stream, _pairing_id, qr) = pair_start(&mut terminal).await;

    let wrong = SecretText::new("AAAAAAAAAAAAAAAAAAAAAAAA");
    let id = phone.send(enroll(&wrong, "iPhone", public_key(7))).await;
    let refused = phone.answer(id).await.unwrap_err();

    assert_eq!(refused.code, ErrorCode::Unauthorized);
    assert!(daemon.state.pairings.check(&qr.token).is_ok(), "the pairing waits on");
}

#[tokio::test]
async fn a_key_that_is_not_a_point_of_65_bytes_is_invalid() {
    let daemon = Daemon::start(true, true).await;
    let mut terminal = daemon.terminal().await;
    let mut phone = daemon.phone(peer()).await;
    let (_stream, _pairing_id, qr) = pair_start(&mut terminal).await;

    let short = phone.send(enroll(&qr.token, "iPhone", Base64Bytes::new(vec![4; 33]))).await;
    let refused = phone.answer(short).await.unwrap_err();

    assert_eq!(refused.code, ErrorCode::Invalid);
    assert!(daemon.state.pairings.check(&qr.token).is_ok(), "the token is not spent");
}

#[tokio::test]
async fn a_key_of_a_paired_device_is_a_conflict_until_its_revoke() {
    let daemon = Daemon::start(true, true).await;
    let mut terminal = daemon.terminal().await;
    let mut phone = daemon.phone(peer()).await;
    let device_id = pair(&daemon, &mut terminal, &mut phone, public_key(9)).await;

    let (stream, _pairing_id, qr) = pair_start(&mut terminal).await;
    let again = phone.send(enroll(&qr.token, "iPhone", public_key(9))).await;
    let refused = phone.answer(again).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(refused.message, format!("the device {device_id} has this key; revoke it first"));

    let revoked: DeviceRevokeResult =
        terminal.call(Method::DeviceRevoke(DeviceRevoke { device_id })).await.unwrap();
    let _after = phone.send(enroll(&qr.token, "iPhone", public_key(9))).await;
    assert!(matches!(item(&mut terminal, stream).await, DevicePairStartItem::Pending { .. }));
    let listed: DeviceListResult =
        terminal.call(Method::DeviceList(DeviceList::default())).await.unwrap();
    assert_eq!(listed.devices[0].revoked_at, Some(revoked.revoked_at));
}

/// Pairs a device with `key` from start to end and returns its id.
async fn pair(
    daemon: &Daemon,
    terminal: &mut RawClient,
    phone: &mut Phone,
    key: Base64Bytes,
) -> DeviceId {
    let (stream, pairing_id, qr) = pair_start(terminal).await;
    let enrolled = phone.send(enroll(&qr.token, "iPhone", key)).await;
    assert!(matches!(item(terminal, stream).await, DevicePairStartItem::Pending { .. }));
    let _ = daemon.answer(pairing_id, true).await;
    assert!(matches!(item(terminal, stream).await, DevicePairStartItem::Paired { .. }));
    assert_eq!(terminal.next(stream).await, Ok(None));
    let result: DeviceEnrollResult =
        serde_json::from_value(phone.answer(enrolled).await.unwrap()).unwrap();
    result.device_id
}

#[tokio::test]
async fn a_revoke_keeps_the_row_and_its_first_time() {
    let daemon = Daemon::start(true, true).await;
    let mut terminal = daemon.terminal().await;
    let mut phone = daemon.phone(peer()).await;
    let device_id = pair(&daemon, &mut terminal, &mut phone, public_key(3)).await;
    let mut revocations = daemon.state.writer.subscribe_revocations();

    let first: DeviceRevokeResult =
        terminal.call(Method::DeviceRevoke(DeviceRevoke { device_id })).await.unwrap();
    let second: DeviceRevokeResult =
        terminal.call(Method::DeviceRevoke(DeviceRevoke { device_id })).await.unwrap();

    assert_eq!(second.revoked_at, first.revoked_at);
    assert_eq!(revocations.recv().await.unwrap(), device_id);
    assert!(revocations.try_recv().is_err(), "the second revoke announces nothing");
    let listed: DeviceListResult =
        terminal.call(Method::DeviceList(DeviceList::default())).await.unwrap();
    assert_eq!(listed.devices[0].revoked_at, Some(first.revoked_at));
}

#[tokio::test]
async fn a_revoke_of_an_unknown_device_is_not_found() {
    let daemon = Daemon::start(true, true).await;
    let mut terminal = daemon.terminal().await;
    let device_id: DeviceId = "019a9b1c-3d00-7a10-8b20-000000000009".parse().unwrap();

    let refused = terminal
        .call::<DeviceRevokeResult>(Method::DeviceRevoke(DeviceRevoke { device_id }))
        .await
        .unwrap_err();

    assert_eq!(refused.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn enroll_on_the_unix_socket_is_forbidden() {
    let daemon = Daemon::start(true, true).await;
    let mut terminal = daemon.terminal().await;
    let (_stream, _pairing_id, qr) = pair_start(&mut terminal).await;

    let mut local = daemon.terminal().await;
    let refused = local
        .call::<DeviceEnrollResult>(enroll(&qr.token, "iPhone", public_key(7)))
        .await
        .unwrap_err();

    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert!(daemon.state.pairings.check(&qr.token).is_ok(), "the token is not spent");
}

#[tokio::test]
async fn a_remote_hello_keeps_no_tty_and_gets_no_local_paths() {
    let daemon = Daemon::start(true, true).await;
    let mut phone = daemon.upgraded(peer()).await;
    let tty = "/dev/pts/7";

    let hello = phone.hello(Some(tty)).await;

    assert_eq!(hello.paths, DaemonPaths::default());
    assert!(hello.prove.is_some(), "the hello is the tailnet listener's");
    assert_eq!(daemon.state.connections.open_in(tty), 0, "the phone is no client on {tty}");
    let (_local, local) = RawClient::hello_as(&daemon.socket, Some(tty), Origin::Cli).await;
    assert_eq!(local.paths.data_dir, daemon.state.dirs.data());
    assert_eq!(daemon.state.connections.open_in(tty), 1);
}

#[tokio::test]
async fn a_phone_cannot_open_list_confirm_or_revoke() {
    let daemon = Daemon::start(true, true).await;
    let mut phone = daemon.phone(peer()).await;
    let device_id: DeviceId = "019a9b1c-3d00-7a10-8b20-000000000009".parse().unwrap();
    let pairing_id = "019a9b1c-3d00-7a10-8b20-00000000000a".parse().unwrap();

    for method in [
        Method::DevicePairStart(DevicePairStart::default()),
        Method::DevicePairConfirm(DevicePairConfirm { pairing_id, accept: true }),
        Method::DeviceList(DeviceList::default()),
        Method::DeviceRevoke(DeviceRevoke { device_id }),
    ] {
        let name = method.name();
        let id = phone.send(method).await;
        let refused = phone.answer(id).await.unwrap_err();
        // NOTE: before its proof a phone may only enroll and prove; `remote::tests`
        // checks that a proved phone gets `forbidden` for the admin methods.
        assert_eq!(refused.code, ErrorCode::Unauthorized, "{name}");
    }
}

#[tokio::test]
async fn pair_start_needs_the_tailnet_listener() {
    let off = Daemon::start(false, false).await;
    let mut terminal = off.terminal().await;
    let refused = terminal
        .call::<serde_json::Value>(Method::DevicePairStart(DevicePairStart::default()))
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(refused.data, Some(json!({ "remote": "off" })));

    let down = Daemon::start(true, false).await;
    let mut terminal = down.terminal().await;
    let refused = terminal
        .call::<serde_json::Value>(Method::DevicePairStart(DevicePairStart::default()))
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Busy);
    assert_eq!(refused.data, Some(json!({ "remote": "not_listening" })));
}

#[tokio::test]
async fn a_pairing_ends_as_expired_after_five_minutes() {
    let daemon = Daemon::start(true, true).await;
    let mut terminal = daemon.terminal().await;
    let (stream, pairing_id, qr) = pair_start(&mut terminal).await;

    Wait::new("the sleep of the pairing")
        .until(|| daemon.clock.requested_sleeps().contains(&PAIRING_TTL))
        .await
        .unwrap();
    daemon.clock.advance(PAIRING_TTL);

    assert_eq!(
        item(&mut terminal, stream).await,
        DevicePairStartItem::Ended { pairing_id, reason: PairEnd::Expired }
    );
    assert_eq!(terminal.next(stream).await, Ok(None));
    assert!(daemon.state.pairings.check(&qr.token).is_err());
}

#[tokio::test]
async fn a_cancel_of_pair_start_spends_the_token() {
    let daemon = Daemon::start(true, true).await;
    let mut terminal = daemon.terminal().await;
    let (stream, _pairing_id, qr) = pair_start(&mut terminal).await;
    let mut phone = daemon.phone(peer()).await;

    terminal.send_frame(&ClientFrame::Cancel { id: stream }).await;
    Wait::new("the end of the pairing")
        .until(|| daemon.state.pairings.open_count() == 0)
        .await
        .unwrap();

    let id = phone.send(enroll(&qr.token, "iPhone", public_key(7))).await;
    assert_eq!(phone.answer(id).await.unwrap_err().code, ErrorCode::Unauthorized);
}
