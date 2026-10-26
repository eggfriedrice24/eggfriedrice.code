//! `device.pair_start`: open a pairing for a new device, for `efr device add`.
//!
//! The first item is `started` with the QR code: efrd's id, the tailnet listener's
//! address and port, its MagicDNS name and a one-time token. Then `pending` when a
//! phone sends the token, and `paired` or `ended` last. The pairing lives as long as
//! this request: a cancel or a client that goes away ends it (`pairing.rs`).
//!
//! The method needs the tailnet listener: with `[remote] enabled = false` it is a
//! `conflict`, and while the listener does not run (Tailscale is down) it is `busy`.

use efr_protocol::{DevicePairStart, DevicePairStartItem, PairEnd, PairingQr};
use efr_transport::Responder;
use jiff::Timestamp;

use crate::DaemonError;
use crate::error::PairingProblem;
use crate::pairing::Update;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    params: DevicePairStart,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let Some(endpoint) = state.pairings.endpoint() else {
        let problem = if state.settings.borrow().remote.enabled {
            PairingProblem::NotListening
        } else {
            PairingProblem::RemoteOff
        };
        return Err(DaemonError::Pairing { problem });
    };
    let mut opened = state.pairings.open(params.name).map_err(pairing)?;
    let pairing_id = opened.pairing_id;
    let qr = PairingQr {
        daemon_id: state.daemon_id,
        host: endpoint.host,
        port: endpoint.port,
        fqdn: endpoint.fqdn,
        token: opened.token.clone(),
    };
    let expires_at = opened.expires_at;
    responder.item(&DevicePairStartItem::Started { pairing_id, expires_at, qr }).await?;
    tracing::info!(%pairing_id, "opened a device pairing");
    loop {
        let deadline = opened.deadline();
        let update = tokio::select! {
            update = opened.next() => update,
            () = until(state, deadline) => {
                if opened.expire(state.clock.now()) {
                    Some(Update::Ended(PairEnd::Expired))
                } else {
                    continue;
                }
            }
        };
        let item = match update {
            Some(Update::Pending(facts)) => DevicePairStartItem::Pending {
                pairing_id,
                device_name: facts.device_name,
                node_name: facts.node_name,
                user_login: facts.user_login,
                key_fingerprint: facts.key_fingerprint,
                os: facts.os,
            },
            Some(Update::Paired { device_id }) => {
                responder.item(&DevicePairStartItem::Paired { pairing_id, device_id }).await?;
                return Ok(());
            }
            Some(Update::Ended(reason)) => {
                tracing::info!(%pairing_id, ?reason, "a device pairing ended");
                responder.item(&DevicePairStartItem::Ended { pairing_id, reason }).await?;
                return Ok(());
            }
            Some(Update::Failed) | None => return Err(pairing(PairingProblem::NotStored)),
        };
        responder.item(&item).await?;
    }
}

/// Waits until the pairing's `deadline`; forever without one, once the terminal
/// accepted, while the device is written and the last update comes.
async fn until(state: &State, deadline: Option<Timestamp>) {
    match deadline {
        Some(deadline) => {
            let left = deadline.duration_since(state.clock.now());
            let left = std::time::Duration::try_from(left).unwrap_or_default();
            state.clock.sleep(left).await;
        }
        None => std::future::pending().await,
    }
}

fn pairing(problem: PairingProblem) -> DaemonError {
    DaemonError::Pairing { problem }
}
