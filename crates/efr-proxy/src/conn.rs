//! One accepted connection: the request head, then the tunnel or the plain request.

use std::io;
use std::sync::Arc;

use efr_protocol::BlockReason;
use efr_stdx::time::{Clock as _, SystemClock};
use tokio::io::AsyncReadExt as _;
use tokio::net::UnixStream;
use tokio::sync::OwnedSemaphorePermit;

use crate::call::Call;
use crate::head::{self, Parsed, Request};
use crate::records::INVALID;
use crate::reply::{self, Status};
use crate::{plain, tunnel};

/// Serves one connection of a call. `_permit` holds the connection's place among the
/// call's open connections until the task ends.
pub(crate) async fn handle(call: Arc<Call>, mut client: UnixStream, _permit: OwnedSemaphorePermit) {
    let limits = call.deps.limits;
    let mut bytes = Vec::new();
    let read = read_head(&mut client, &mut bytes, limits.head_bytes);
    match SystemClock.timeout(limits.head_timeout, read).await {
        Err(_) => {
            reply::send(&mut client, Status::RequestTimeout, "the request head took too long").await
        }
        // The client went away before a whole head.
        Ok(Err(_) | Ok(None)) => {}
        Ok(Ok(Some(Parsed::Refused(reason)))) => {
            call.refuse(INVALID, 0, reason);
            reply::send(&mut client, Status::BadRequest, reply::text(reason)).await;
        }
        Ok(Ok(Some(Parsed::Done { value: Request::Connect { host, port }, used }))) => {
            let rest = bytes.split_off(used);
            tunnel::run(&call, client, &host, port, rest).await;
        }
        Ok(Ok(Some(Parsed::Done { value: request, used }))) => {
            let after = bytes.len() - used;
            plain::run(&call, client, &request, after).await;
        }
        Ok(Ok(Some(Parsed::Partial))) => {
            // NOTE: read_head gives no partial head; a refusal is the safe answer.
            call.refuse(INVALID, 0, BlockReason::BadRequest);
            reply::send(&mut client, Status::BadRequest, reply::text(BlockReason::BadRequest))
                .await;
        }
    }
}

/// Reads until the head is whole or refused, at most `limit` bytes. `None` when the
/// client closed first. Bytes after the head stay in `bytes`.
async fn read_head(
    client: &mut UnixStream,
    bytes: &mut Vec<u8>,
    limit: usize,
) -> io::Result<Option<Parsed<Request>>> {
    let mut chunk = [0; 4096];
    loop {
        let parsed = head::parse(bytes, limit);
        if parsed != Parsed::Partial {
            return Ok(Some(parsed));
        }
        // A partial head is shorter than the limit, so there is room for one byte.
        let room = limit.saturating_sub(bytes.len()).clamp(1, chunk.len());
        let count = client.read(&mut chunk[..room]).await?;
        if count == 0 {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}
