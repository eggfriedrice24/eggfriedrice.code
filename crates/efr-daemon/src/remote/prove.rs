//! `device.prove`: a remote device proves that it holds the key of its row.
//!
//! The daemon builds the message that the device signed (`ProveMessage`: the domain
//! tag, the hello's challenge, the daemon id, the protocol version, the device, the
//! peer node of this connection's whois and efrd's tailnet) and checks the DER
//! signature with aws-lc-rs (`ECDSA_P256_SHA256_ASN1`), which also refuses a key that is
//! not a point of the curve. Then the row: not revoked, the same user and tailnet as
//! this connection. A row of another node means that the phone has a new Tailscale node
//! (a reinstall): the device must pair again, and the answer says so.
//!
//! Every refusal but that one has the same message, so a peer learns nothing about
//! which devices exist. Each refusal counts; the third closes the connection. The
//! connection joins the table of live devices before the row is read, so a revoke can
//! never fall between the read and the proof; a refusal takes it out again.

use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};
use efr_protocol::{
    DeviceId, DeviceProve, DeviceProveResult, PROTOCOL_VERSION, ProveMessage, ScopeName,
};
use efr_store::StoreError;
use efr_store::devices::Device;
use efr_transport::{ConnectionContext, Responder};

use super::admission::{Admission, Proved};
use crate::DaemonError;
use crate::methods::PHONE_SCOPES;
use crate::state::State;

/// The failed proofs after which the daemon closes the connection.
pub(crate) const MAX_FAILURES: u32 = 3;

/// The pause that a phone waits before it tries again when the store is busy.
const RETRY_AFTER_SECS: u32 = 2;

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: DeviceProve,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let admission =
        super::admission(context).ok_or(DaemonError::RemoteOnly { method: "device.prove" })?;
    if admission.proved().is_some() {
        return Err(DaemonError::AlreadyProved);
    }
    let live = state.remote.live();
    let (device_id, key) = (params.device_id, admission.key());
    // NOTE: joined before the read of the row, so a revoke after the read closes this
    // connection. A refusal takes out only its own entry: a proof of the same
    // connection that runs at the same time keeps its own.
    live.add(device_id, key, admission.close_token().clone());
    let proved = match prove(state, admission, &params).await {
        Ok(proved) => proved,
        Err(error) => {
            live.remove(device_id, key);
            if counts(&error) && admission.fail() >= MAX_FAILURES {
                tracing::warn!(
                    node = admission.peer().node_stable_id,
                    "a remote peer failed its proof too often; closing it"
                );
                admission.close_token().cancel();
            }
            return Err(error);
        }
    };
    let result = DeviceProveResult { scopes: proved.scopes.clone() };
    if !admission.prove(proved) {
        live.remove(device_id, key);
        return Err(DaemonError::AlreadyProved);
    }
    tracing::info!(%device_id, node = admission.peer().node_stable_id, "a device proved its key");
    if let Err(error) = state.writer.device_touch(device_id).await {
        tracing::warn!(%device_id, error = %error, "the device's last time could not be kept");
    }
    responder.item(&result).await?;
    Ok(())
}

/// True for an error that counts as a failed proof: not a busy store.
fn counts(error: &DaemonError) -> bool {
    !matches!(error, DaemonError::RemoteStarting { .. })
}

/// Checks the proof of `params` on the connection of `admission`.
async fn prove(
    state: &State,
    admission: &Admission,
    params: &DeviceProve,
) -> Result<Proved, DaemonError> {
    let device_id = params.device_id;
    let (challenge, hello_device) = admission.challenge().ok_or(DaemonError::ProofRefused)?;
    if hello_device.is_some_and(|hello_device| hello_device != device_id) {
        return Err(DaemonError::InvalidParams {
            reason: "device_id differs from the device that the hello named",
        });
    }
    let row =
        state.readers.with(move |conn| efr_store::devices::get(conn, device_id)).await.map_err(
            |error| match error {
                StoreError::Busy { .. } | StoreError::WriterStopped => {
                    DaemonError::RemoteStarting { retry_after_secs: RETRY_AFTER_SECS }
                }
                other => DaemonError::from(other),
            },
        )?;
    let row = row.ok_or(DaemonError::ProofRefused)?;
    let message = ProveMessage {
        challenge: &challenge,
        daemon_id: state.daemon_id,
        protocol: PROTOCOL_VERSION,
        device_id,
        peer_node: &admission.peer().node_stable_id,
        daemon_tailnet: admission.tailnet(),
    };
    check(&row, admission, &message.to_bytes(), params.signature.as_bytes())?;
    Ok(Proved { device_id, scopes: scopes(&row.scopes) })
}

/// Checks `signature` of `message` with the key of `row`, then the row against the
/// connection of `admission`.
pub(crate) fn check(
    row: &Device,
    admission: &Admission,
    message: &[u8],
    signature: &[u8],
) -> Result<(), DaemonError> {
    verify(row.public_key.as_bytes(), message, signature)?;
    if !row.is_active() {
        return Err(DaemonError::ProofRefused);
    }
    if row.user_id != admission.peer().user_id || row.tailnet_stable_id != admission.tailnet() {
        return Err(DaemonError::ProofRefused);
    }
    if row.node_stable_id != admission.peer().node_stable_id {
        return Err(DaemonError::RepairNeeded { device_id: row.device_id });
    }
    Ok(())
}

/// Checks the DER signature `signature` of `message` with the P-256 key `public_key`
/// (65 bytes, uncompressed).
pub(crate) fn verify(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), DaemonError> {
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, public_key)
        .verify(message, signature)
        .map_err(|_| DaemonError::ProofRefused)
}

/// The scopes of a proved connection: those of the device that a phone may hold at
/// all, so never `admin`, and never `terminal` in this version.
pub(crate) fn scopes(device: &[ScopeName]) -> Vec<ScopeName> {
    PHONE_SCOPES.iter().copied().filter(|scope| device.contains(scope)).collect()
}

/// The device that proved its key on the connection of `context`, if any.
pub(crate) fn proved_device(context: &ConnectionContext) -> Option<DeviceId> {
    super::admission(context).and_then(Admission::proved).map(|proved| proved.device_id)
}

#[cfg(test)]
mod tests;
