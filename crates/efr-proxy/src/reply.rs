//! The answers that the proxy writes itself: `200 Connection established`, and the
//! refusals with a one-line body that names the reason.

use efr_protocol::BlockReason;
use tokio::io::AsyncWriteExt as _;
use tokio::net::UnixStream;

/// The answer to a `CONNECT` that the proxy carries.
pub(crate) const ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection established\r\n\r\n";

/// A status of a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    BadRequest,
    Forbidden,
    MethodNotAllowed,
    RequestTimeout,
    BadGateway,
    Unavailable,
}

impl Status {
    fn line(self) -> &'static str {
        match self {
            Status::BadRequest => "400 Bad Request",
            Status::Forbidden => "403 Forbidden",
            Status::MethodNotAllowed => "405 Method Not Allowed",
            Status::RequestTimeout => "408 Request Timeout",
            Status::BadGateway => "502 Bad Gateway",
            Status::Unavailable => "503 Service Unavailable",
        }
    }
}

/// A whole answer: the status, `Connection: close` and the body
/// `efr-proxy: <text>`.
pub(crate) fn response(status: Status, text: &str) -> Vec<u8> {
    let body = format!("efr-proxy: {text}\n");
    format!(
        "HTTP/1.1 {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        status.line(),
        body.len()
    )
    .into_bytes()
}

/// Writes an answer and closes the client's side for writes. A client that went away
/// gets nothing.
pub(crate) async fn send(client: &mut UnixStream, status: Status, text: &str) {
    if client.write_all(&response(status, text)).await.is_ok() {
        let _ = client.shutdown().await;
    }
}

/// The text of a refusal, for the body of its answer.
pub(crate) fn text(reason: BlockReason) -> &'static str {
    match reason {
        BlockReason::NotAllowed => "not_allowed: the host is not on the allow list",
        BlockReason::PrivateAddress => {
            "private_address: the host resolves to a private or local address"
        }
        BlockReason::SniMismatch => "sni_mismatch: the TLS server name differs from the host",
        BlockReason::UploadLimit => "upload_limit: the call sent its whole upload budget",
        BlockReason::Method => {
            "method: only GET and HEAD, only to a host with plain HTTP, and no tunnel to port 80"
        }
        BlockReason::IpLiteral => "ip_literal: an IP address that no entry lists",
        BlockReason::NotTls => "not_tls: the tunnel did not start with a TLS ClientHello",
        BlockReason::BadRequest => "bad_request: the request head is malformed",
        BlockReason::Limit => "limit: too many connections",
        BlockReason::Denied => "denied: efr denies this host for this command",
        _ => "refused",
    }
}
