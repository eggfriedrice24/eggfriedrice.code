//! `FakePhone`: the phone app's side of efrd's remote listener, as a test plays it.
//!
//! A phone opens a TCP stream to the listener, sends the opening handshake of a
//! WebSocket with an `Origin` of its choice, and then speaks the protocol in text
//! frames, as the app does. Its [`PhoneKey`] is a software P-256 key in place of the
//! Secure Enclave key of the app: the same 65-byte X9.63 public key and the same DER
//! signature over [`ProveMessage`].
//!
//! Every read has a real time limit ([`efr_test_support::WAIT_LIMIT`]), so a daemon
//! that never answers fails the test with a message and does not hang it. The frames
//! of other requests that arrive while the phone waits for one request are kept, in
//! order, for a later read: a subscription and a call can run on one connection.

use std::collections::VecDeque;
use std::net::SocketAddrV4;

use aws_lc_rs::digest::{SHA256, digest};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _};
use efr_protocol::{
    Base64Bytes, Capabilities, ClientFrame, DeviceId, DeviceProve, DeviceProveResult, Hello,
    HelloResult, Method, Origin, PROTOCOL_VERSION, ProveMessage, RequestId, ServerFrame,
};
use efr_test_support::Wait;
use fastwebsockets::{Frame, OpCode, Payload, Role, WebSocket};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use crate::TestDaemonError;

/// The bytes of a key fingerprint after `SHA256:`, as `efr device add` shows it.
const FINGERPRINT_BYTES: usize = 8;

/// The `client` of a phone's hello.
const CLIENT: &str = "efr-test-daemon phone";

/// A phone's device key: a software P-256 key.
#[derive(Debug)]
pub struct PhoneKey {
    pair: EcdsaKeyPair,
}

impl PhoneKey {
    /// A new random key.
    ///
    /// # Errors
    ///
    /// [`TestDaemonError::PhoneKey`] when aws-lc-rs cannot make one.
    pub fn generate() -> Result<PhoneKey, TestDaemonError> {
        let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING)
            .map_err(|source| TestDaemonError::PhoneKey { source })?;
        Ok(PhoneKey { pair })
    }

    /// The public key as `device.enroll` sends it: the 65-byte X9.63 point
    /// `04 || X || Y`.
    pub fn public_key(&self) -> Base64Bytes {
        Base64Bytes::new(self.pair.public_key().as_ref().to_vec())
    }

    /// The fingerprint that the terminal shows for this key: `SHA256:` and the first
    /// 8 bytes of the SHA-256 of the public key, in hex.
    pub fn fingerprint(&self) -> String {
        let hash = digest(&SHA256, self.pair.public_key().as_ref());
        let hex: String =
            hash.as_ref()[..FINGERPRINT_BYTES].iter().map(|byte| format!("{byte:02x}")).collect();
        format!("SHA256:{hex}")
    }

    /// The DER signature of `device.prove` for `device_id` after `hello`: over the
    /// challenge, efrd's id, the protocol, the device and the prove facts of the hello.
    ///
    /// # Errors
    ///
    /// [`TestDaemonError::NoProveFacts`] when the hello has no prove facts (a hello
    /// over the Unix socket), and [`TestDaemonError::PhoneKey`] when the signature
    /// fails.
    pub fn sign(
        &self,
        hello: &HelloResult,
        device_id: DeviceId,
    ) -> Result<Base64Bytes, TestDaemonError> {
        let prove = hello.prove.as_ref().ok_or(TestDaemonError::NoProveFacts)?;
        let message = ProveMessage {
            challenge: &hello.challenge,
            daemon_id: hello.daemon_id,
            protocol: hello.protocol,
            device_id,
            peer_node: &prove.peer_node,
            daemon_tailnet: &prove.daemon_tailnet,
        };
        let signature = self
            .pair
            .sign(&SystemRandom::new(), &message.to_bytes())
            .map_err(|source| TestDaemonError::PhoneKey { source })?;
        Ok(Base64Bytes::new(signature.as_ref().to_vec()))
    }
}

/// A phone on the remote listener.
pub struct FakePhone {
    ws: WebSocket<TcpStream>,
    next_id: u64,
    /// Frames of other requests that came while the phone waited for one.
    held: VecDeque<ServerFrame>,
}

impl std::fmt::Debug for FakePhone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakePhone")
            .field("next_id", &self.next_id)
            .field("held", &self.held.len())
            .finish_non_exhaustive()
    }
}

/// Opens a TCP stream to `addr`, sends the opening handshake with `origin` (none when
/// `None`) and reads the head of the answer. Returns the stream and the status line of
/// the answer, which is empty when the listener closed the stream with no byte, as it
/// does for a peer that it refuses.
async fn open(
    addr: SocketAddrV4,
    origin: Option<&str>,
) -> Result<(TcpStream, String), TestDaemonError> {
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|source| TestDaemonError::Io { what: "the phone's connect", source })?;
    let origin = origin.map(|origin| format!("Origin: {origin}\r\n")).unwrap_or_default();
    let head = format!(
        "GET / HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{origin}\r\n"
    );
    // NOTE: a refused peer may be closed before the head is written; the read below
    // then sees the end of the stream.
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
    Wait::new("the answer to the phone's handshake").until_ready(read).await?;
    let status = String::from_utf8_lossy(&answer).lines().next().unwrap_or_default().to_owned();
    Ok((stream, status))
}

impl FakePhone {
    /// The status line of the listener's answer to a handshake with `origin`; empty
    /// when the listener refused the peer with no byte. The stream is closed after.
    ///
    /// # Errors
    ///
    /// [`TestDaemonError::Io`] when the connect fails, and
    /// [`TestDaemonError::Support`] when no answer comes within the limit.
    pub async fn handshake(
        addr: SocketAddrV4,
        origin: Option<&str>,
    ) -> Result<String, TestDaemonError> {
        Ok(open(addr, origin).await?.1)
    }

    /// Connects to the listener at `addr` with the origin of the app's web view of
    /// it, `http://<addr>`.
    ///
    /// # Errors
    ///
    /// [`TestDaemonError::PhoneRefused`] when the listener does not switch protocols,
    /// and the errors of [`handshake`](Self::handshake).
    pub async fn connect(addr: SocketAddrV4) -> Result<FakePhone, TestDaemonError> {
        FakePhone::connect_with_origin(addr, Some(&format!("http://{addr}"))).await
    }

    /// Connects to the listener at `addr` with `origin`; `None` sends no `Origin`, as
    /// the native app does.
    ///
    /// # Errors
    ///
    /// The errors of [`connect`](Self::connect).
    pub async fn connect_with_origin(
        addr: SocketAddrV4,
        origin: Option<&str>,
    ) -> Result<FakePhone, TestDaemonError> {
        let (stream, status) = open(addr, origin).await?;
        if status != "HTTP/1.1 101 Switching Protocols" {
            return Err(TestDaemonError::PhoneRefused { status });
        }
        let mut ws = WebSocket::after_handshake(stream, Role::Client);
        ws.set_auto_close(true);
        ws.set_auto_pong(true);
        Ok(FakePhone { ws, next_id: 0, held: VecDeque::new() })
    }

    /// Sends a request with `method` and returns its id; the answer is read with
    /// [`result`](Self::result) or [`next_item`](Self::next_item).
    ///
    /// # Errors
    ///
    /// [`TestDaemonError::Json`] and [`TestDaemonError::PhoneSocket`].
    pub async fn send(&mut self, method: Method) -> Result<RequestId, TestDaemonError> {
        self.next_id += 1;
        let id = RequestId::new(self.next_id);
        let text = serde_json::to_vec(&ClientFrame::Request { id, method })
            .map_err(|source| TestDaemonError::Json { what: "the phone's request", source })?;
        self.ws
            .write_frame(Frame::text(Payload::Owned(text)))
            .await
            .map_err(|source| TestDaemonError::PhoneSocket { source })?;
        Ok(id)
    }

    /// Sends a request with `method` and reads its one item.
    ///
    /// # Errors
    ///
    /// [`TestDaemonError::PhoneServer`] when the daemon answers with an error, with
    /// its code and message; the errors of [`send`](Self::send) and
    /// [`result`](Self::result).
    pub async fn call<T: DeserializeOwned>(
        &mut self,
        method: Method,
    ) -> Result<T, TestDaemonError> {
        let id = self.send(method).await?;
        self.result(id).await
    }

    /// The one item of the request `id`, and its end.
    ///
    /// # Errors
    ///
    /// [`TestDaemonError::PhoneServer`] for an error of the daemon,
    /// [`TestDaemonError::PhoneClosed`] when the connection closes first, and
    /// [`TestDaemonError::StreamEnded`] when the request ends with no item.
    pub async fn result<T: DeserializeOwned>(
        &mut self,
        id: RequestId,
    ) -> Result<T, TestDaemonError> {
        let item = self.next_item(id).await?.ok_or(TestDaemonError::StreamEnded)?;
        match self.next_item::<serde_json::Value>(id).await? {
            None => Ok(item),
            Some(_) => Err(TestDaemonError::PhoneUnexpected { what: "a second item" }),
        }
    }

    /// The next item of the request `id`, such as a subscription; `None` at its end.
    ///
    /// # Errors
    ///
    /// [`TestDaemonError::PhoneServer`] for an error of the daemon,
    /// [`TestDaemonError::PhoneClosed`] when the connection closes first, and
    /// [`TestDaemonError::Json`] for an item that is not a `T`.
    pub async fn next_item<T: DeserializeOwned>(
        &mut self,
        id: RequestId,
    ) -> Result<Option<T>, TestDaemonError> {
        match self.frame_of(id).await? {
            ServerFrame::Item { item, .. } => serde_json::from_value(item)
                .map(Some)
                .map_err(|source| TestDaemonError::Json { what: "an item for the phone", source }),
            ServerFrame::End { .. } => Ok(None),
            ServerFrame::Error(frame) => Err(TestDaemonError::PhoneServer { body: frame.error }),
            _ => Err(TestDaemonError::PhoneUnexpected { what: "an ack" }),
        }
    }

    /// Says hello as the app does, with `device_id` when the phone was paired.
    ///
    /// # Errors
    ///
    /// The errors of [`call`](Self::call).
    pub async fn hello(
        &mut self,
        device_id: Option<DeviceId>,
    ) -> Result<HelloResult, TestDaemonError> {
        let hello = Hello {
            protocol: PROTOCOL_VERSION,
            origin: Origin::Phone,
            client: Some(CLIENT.to_owned()),
            capabilities: Capabilities::default(),
            tty: None,
            pid: None,
            device_id,
        };
        self.call(Method::Hello(hello)).await
    }

    /// Proves that the phone holds `key` of `device_id`, with the challenge of `hello`.
    ///
    /// # Errors
    ///
    /// The errors of [`PhoneKey::sign`] and [`call`](Self::call).
    pub async fn prove(
        &mut self,
        key: &PhoneKey,
        hello: &HelloResult,
        device_id: DeviceId,
    ) -> Result<DeviceProveResult, TestDaemonError> {
        let signature = key.sign(hello, device_id)?;
        self.call(Method::DeviceProve(DeviceProve { device_id, signature })).await
    }

    /// Reads until the daemon closes the connection, and drops what comes before.
    ///
    /// # Errors
    ///
    /// [`TestDaemonError::Support`] when the connection is still open at the limit.
    pub async fn closed(&mut self) -> Result<(), TestDaemonError> {
        self.held.clear();
        let ws = &mut self.ws;
        let read = async {
            loop {
                match ws.read_frame().await {
                    Ok(frame) if frame.opcode == OpCode::Close => return,
                    Ok(_) => {}
                    // NOTE: a stream that ends without a close frame is closed too.
                    Err(_) => return,
                }
            }
        };
        Wait::new("the daemon to close the phone's connection").until_ready(read).await?;
        Ok(())
    }

    /// The next frame of the request `id`: a held one, or one from the socket. The
    /// frames of other requests that come first are held.
    async fn frame_of(&mut self, id: RequestId) -> Result<ServerFrame, TestDaemonError> {
        if let Some(at) = self.held.iter().position(|frame| frame.request_id() == Some(id)) {
            if let Some(frame) = self.held.remove(at) {
                return Ok(frame);
            }
        }
        loop {
            let frame = self.read().await?;
            match frame.request_id() {
                Some(of) if of == id => return Ok(frame),
                // NOTE: an error without an id is about the connection, so every request
                // gets it.
                None => return Ok(frame),
                Some(_) => self.held.push_back(frame),
            }
        }
    }

    /// One frame from the socket.
    async fn read(&mut self) -> Result<ServerFrame, TestDaemonError> {
        let ws = &mut self.ws;
        let frame = Wait::new("a frame from the daemon")
            .until_ready(ws.read_frame())
            .await?
            .map_err(|source| TestDaemonError::PhoneSocket { source })?;
        match frame.opcode {
            OpCode::Text => ServerFrame::from_json(&frame.payload)
                .map_err(|source| TestDaemonError::PhoneFrame { source }),
            OpCode::Close => Err(TestDaemonError::PhoneClosed),
            _ => Err(TestDaemonError::PhoneUnexpected { what: "a frame that is not text" }),
        }
    }
}

#[cfg(test)]
mod tests;
