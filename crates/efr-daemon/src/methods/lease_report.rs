//! `lease.report`: what a client shows, replacing its previous report.
//!
//! The daemon keeps the lease until it expires or the connection closes, and uses it
//! to tell whether a terminal shows a conversation (no notice then) and whether a
//! PTY is watched (never closed as idle then).

use efr_protocol::{LeaseReport, LeaseReportResult};
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::connections::{LEASE_TTL, Lease};
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: LeaseReport,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let now = state.clock.now();
    let expires_at = now.checked_add(LEASE_TTL).unwrap_or(now);
    state.connections.report_lease(
        context.conn_id(),
        Lease {
            conversations: params.conversations,
            ptys: params.ptys,
            visible: params.visible,
            expires_at,
        },
    );
    let ttl_secs = u32::try_from(LEASE_TTL.as_secs()).unwrap_or(u32::MAX);
    responder.item(&LeaseReportResult { ttl_secs }).await?;
    Ok(())
}
