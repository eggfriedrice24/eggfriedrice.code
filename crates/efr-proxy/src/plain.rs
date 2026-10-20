//! A plain HTTP request (DP8): `GET` and `HEAD` only, only to an http host, one request
//! per connection. The proxy reads no client byte after the head and sends a head that
//! it writes itself; the answer comes back as the upstream sends it.

use efr_permissions::HostVerdict;
use efr_permissions::network::is_ip_literal;
use efr_protocol::BlockReason;
use tokio::io::AsyncWriteExt as _;
use tokio::net::UnixStream;

use crate::call::Call;
use crate::head::{Method, Request, upstream_head};
use crate::reply::{self, Status};
use crate::tunnel::pump_down;
use crate::upstream::{self, Failure};

/// Serves one plain request. `after` counts the client bytes after the head in the
/// first reads: any such byte is a pipelined request, which the proxy refuses.
pub(crate) async fn run(call: &Call, mut client: UnixStream, request: &Request, after: usize) {
    let Request::Plain { method, host, port, .. } = request else {
        return;
    };
    let (host, port) = (host.as_str(), *port);
    let verdict = call.verdict(host, port);
    let refusal = if !matches!(method, Method::Get | Method::Head) {
        Some((Status::MethodNotAllowed, BlockReason::Method))
    } else if after > 0 {
        Some((Status::BadRequest, BlockReason::BadRequest))
    } else if is_ip_literal(host) && !matches!(verdict, HostVerdict::Allowed { .. }) {
        Some((Status::Forbidden, BlockReason::IpLiteral))
    } else if call.spent() {
        Some((Status::Forbidden, BlockReason::UploadLimit))
    } else {
        match verdict {
            HostVerdict::Allowed { http: true, .. } => None,
            HostVerdict::Allowed { http: false, .. } => {
                Some((Status::Forbidden, BlockReason::Method))
            }
            // A host that the list allows only for TLS has no plain HTTP.
            HostVerdict::NotListed
                if matches!(call.verdict(host, 443), HostVerdict::Allowed { .. }) =>
            {
                Some((Status::Forbidden, BlockReason::Method))
            }
            HostVerdict::Denied => Some((Status::Forbidden, BlockReason::Denied)),
            HostVerdict::NotListed => Some((Status::Forbidden, BlockReason::NotAllowed)),
        }
    };
    if let Some((status, reason)) = refusal {
        call.refuse(host, port, reason);
        return reply::send(&mut client, status, reply::text(reason)).await;
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
    let head = upstream_head(request);
    if !call.reserve_all(head.len()) {
        call.refuse(host, port, BlockReason::UploadLimit);
        let text = reply::text(BlockReason::UploadLimit);
        return reply::send(&mut client, Status::Forbidden, text).await;
    }
    if upstream.write_all(&head).await.is_err() {
        return;
    }
    call.sent(head.len());
    call.opened();
    let (mut upstream_read, _) = upstream.split();
    // NOTE: the answer's end is the upstream's close (`Connection: close`); an error on
    // either side only ends the request.
    let _ = pump_down(call, &mut upstream_read, &mut client).await;
}
