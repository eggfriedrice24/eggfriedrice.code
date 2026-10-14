//! A `CONNECT` in the order of DP6: the host form, the budget, the lists and the port
//! rule (no lookup before them), the upstream, `200`, the ClientHello with its server
//! name, then the bytes both ways under the call's budget (DP9).

use std::io;

use efr_permissions::HostVerdict;
use efr_permissions::network::is_ip_literal;
use efr_protocol::BlockReason;
use efr_stdx::time::{Clock as _, SystemClock};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpStream, UnixStream};

use crate::call::Call;
use crate::hello::{self, Hello};
use crate::reply::{self, Status};
use crate::upstream::{self, Failure};

/// The size of one copy of the splice.
const CHUNK: usize = 16 * 1024;

/// Serves a `CONNECT host:port`. `rest` holds the client bytes that came after the head
/// in the same read: the start of the ClientHello.
pub(crate) async fn run(call: &Call, mut client: UnixStream, host: &str, port: u16, rest: Vec<u8>) {
    let literal = is_ip_literal(host);
    let verdict = call.verdict(host, port);
    let allowed = matches!(verdict, HostVerdict::Allowed { .. });
    let refusal = if literal && !allowed {
        Some(BlockReason::IpLiteral)
    } else if call.spent() {
        Some(BlockReason::UploadLimit)
    } else if !allowed {
        Some(BlockReason::NotAllowed)
    } else if port == 80 {
        // Port 80 is plain GET and HEAD, never a tunnel.
        Some(BlockReason::Method)
    } else {
        None
    };
    if let Some(reason) = refusal {
        call.refuse(host, port, reason);
        return reply::send(&mut client, Status::Forbidden, reply::text(reason)).await;
    }
    let private = matches!(verdict, HostVerdict::Allowed { private: true, .. });
    let mut upstream = match upstream::open(call, host, port, private).await {
        Ok(upstream) => upstream,
        Err(Failure::Refused(reason)) => {
            call.refuse(host, port, reason);
            return reply::send(&mut client, Status::Forbidden, reply::text(reason)).await;
        }
        Err(Failure::Gateway(reason)) => {
            return reply::send(&mut client, Status::BadGateway, reason).await;
        }
    };
    if client.write_all(reply::ESTABLISHED).await.is_err() {
        return;
    }
    call.opened();
    let limits = call.deps.limits;
    let read = read_hello(&mut client, rest, limits.hello_bytes);
    let (bytes, parsed) = match SystemClock.timeout(limits.hello_timeout, read).await {
        Err(_) => return call.refuse(host, port, BlockReason::NotTls),
        Ok(Err(_)) => return,
        Ok(Ok(read)) => read,
    };
    let cut = match parsed {
        // The client closed in the middle of its hello.
        Hello::Partial => return,
        Hello::NotTls | Hello::NotHello | Hello::Done { ech: true, .. } => {
            Some(BlockReason::NotTls)
        }
        Hello::Done { sni: None, .. } if literal => None,
        Hello::Done { sni: None, .. } => Some(BlockReason::NotTls),
        Hello::Done { sni: Some(_), .. } if literal => Some(BlockReason::SniMismatch),
        Hello::Done { sni: Some(name), .. } => {
            let name = name.strip_suffix('.').unwrap_or(&name);
            (!name.eq_ignore_ascii_case(host)).then_some(BlockReason::SniMismatch)
        }
    };
    if let Some(reason) = cut {
        return call.refuse(host, port, reason);
    }
    let take = call.reserve(bytes.len());
    if take > 0 {
        if upstream.write_all(&bytes[..take]).await.is_err() {
            return;
        }
        call.sent(take);
    }
    if take < bytes.len() {
        return call.refuse(host, port, BlockReason::UploadLimit);
    }
    splice(call, &mut client, &mut upstream, host, port).await;
}

/// Reads the ClientHello after `bytes`, at most `cap` bytes in all. A hello that does
/// not fit is [`Hello::NotTls`]; a client that closes first gives [`Hello::Partial`].
async fn read_hello(
    client: &mut UnixStream,
    mut bytes: Vec<u8>,
    cap: usize,
) -> io::Result<(Vec<u8>, Hello)> {
    let mut chunk = vec![0; CHUNK];
    loop {
        let parsed = hello::parse(&bytes);
        if parsed != Hello::Partial {
            return Ok((bytes, parsed));
        }
        if bytes.len() >= cap {
            return Ok((bytes, Hello::NotTls));
        }
        let room = (cap - bytes.len()).min(chunk.len());
        let count = client.read(&mut chunk[..room]).await?;
        if count == 0 {
            return Ok((bytes, Hello::Partial));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

/// How the client's direction of a splice ended.
enum Up {
    /// The client closed its side.
    Closed,
    /// The budget is spent: the tunnel ends now.
    Budget,
}

/// Copies both ways until both sides closed, an error, or the budget.
async fn splice(
    call: &Call,
    client: &mut UnixStream,
    upstream: &mut TcpStream,
    host: &str,
    port: u16,
) {
    let (mut client_read, mut client_write) = client.split();
    let (mut upstream_read, mut upstream_write) = upstream.split();
    let up = pump_up(call, &mut client_read, &mut upstream_write);
    let down = pump_down(call, &mut upstream_read, &mut client_write);
    tokio::pin!(up, down);
    let (mut up_open, mut down_open) = (true, true);
    while up_open || down_open {
        tokio::select! {
            result = &mut up, if up_open => match result {
                Ok(Up::Closed) => up_open = false,
                Ok(Up::Budget) => return call.refuse(host, port, BlockReason::UploadLimit),
                Err(_) => return,
            },
            result = &mut down, if down_open => match result {
                Ok(()) => down_open = false,
                Err(_) => return,
            },
        }
    }
}

/// Client to upstream: at most the rest of the call's budget leaves.
async fn pump_up(
    call: &Call,
    from: &mut (impl AsyncRead + Unpin),
    to: &mut (impl AsyncWrite + Unpin),
) -> io::Result<Up> {
    let mut chunk = vec![0; CHUNK];
    loop {
        let count = from.read(&mut chunk).await?;
        if count == 0 {
            to.shutdown().await?;
            return Ok(Up::Closed);
        }
        let take = call.reserve(count);
        if take > 0 {
            to.write_all(&chunk[..take]).await?;
            call.sent(take);
        }
        if take < count {
            return Ok(Up::Budget);
        }
    }
}

/// Upstream to client, counted.
pub(crate) async fn pump_down(
    call: &Call,
    from: &mut (impl AsyncRead + Unpin),
    to: &mut (impl AsyncWrite + Unpin),
) -> io::Result<()> {
    let mut chunk = vec![0; CHUNK];
    loop {
        let count = from.read(&mut chunk).await?;
        if count == 0 {
            return to.shutdown().await;
        }
        to.write_all(&chunk[..count]).await?;
        call.received(count);
    }
}
