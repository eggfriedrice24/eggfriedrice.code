//! `device.enroll`: a phone sends the token of a pairing QR code, its name and its
//! public key, through the tailnet listener and before any proof.
//!
//! The order keeps the token single use and the terminal in charge:
//!
//! 1. Refuse a remote connection that has no whois facts of its peer. The Unix socket
//!    never gets here: the dispatch refuses it with `forbidden` (`RemoteOnly`).
//! 2. Check the key (65 bytes, a point on the P-256 curve), then the token, which
//!    counts a wrong one (`pairing.rs`). A key of a device that is not revoked is a
//!    `conflict` before the token is spent, so the phone can make a new key and try
//!    again.
//! 3. Spend the token and show the phone's facts at the terminal, then wait for its
//!    answer. Only a yes writes the row; the answer then has the device's id and
//!    scopes. A no or an expiry is `forbidden`.

use efr_protocol::{DeviceEnroll, DeviceEnrollResult};
use efr_store::devices::DevicePublicKey;
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::error::PairingProblem;
use crate::pairing::{self, Answer};
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: DeviceEnroll,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let Some(peer) = pairing::enroll_peer(context) else {
        return Err(refused(PairingProblem::NoAdmission));
    };
    let key = DevicePublicKey::from_x963(params.public_key.as_bytes())
        .ok()
        .filter(|key| crate::remote::prove::is_p256_point(key.as_bytes()))
        .ok_or_else(|| refused(PairingProblem::BadKey))?;
    let pairing_id = state.pairings.check(&params.token).map_err(refused)?;
    let holder = state
        .readers
        .with(move |conn| {
            let devices = efr_store::devices::list(conn)?;
            Ok(devices
                .into_iter()
                .find(|device| device.is_active() && device.public_key == key)
                .map(|device| device.device_id))
        })
        .await?;
    if let Some(device_id) = holder {
        return Err(refused(PairingProblem::KeyInUse { device_id }));
    }
    let wait = state
        .pairings
        .enroll(pairing_id, &params.token, &params.name, key, peer)
        .map_err(refused)?;
    tracing::info!(%pairing_id, "a phone enrolled; the terminal answers");
    match wait.outcome().await {
        Answer::Paired { device_id, scopes } => {
            responder.item(&DeviceEnrollResult { device_id, scopes }).await?;
            Ok(())
        }
        Answer::Ended(reason) => Err(refused(PairingProblem::Ended { reason })),
        Answer::Failed => Err(refused(PairingProblem::NotStored)),
    }
}

fn refused(problem: PairingProblem) -> DaemonError {
    DaemonError::Pairing { problem }
}

#[cfg(test)]
mod tests;
