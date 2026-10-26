//! `device.revoke`: take a device's right to connect away, for `efr device revoke`.
//!
//! The store marks the row revoked and announces the id after the commit; the tailnet
//! listener closes that device's connections when the id arrives. A revoke of a
//! revoked device changes nothing and answers the time of the first one. An unknown
//! id is `not_found`.

use efr_protocol::{DeviceRevoke, DeviceRevokeResult};
use efr_store::devices::RevokeOutcome;
use efr_transport::Responder;

use crate::DaemonError;
use crate::error::PairingProblem;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    params: DeviceRevoke,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let device_id = params.device_id;
    let revoked_at = match state.writer.device_revoke(device_id).await? {
        RevokeOutcome::Revoked { at } => {
            tracing::info!(%device_id, "revoked a device");
            at
        }
        RevokeOutcome::AlreadyRevoked { at } => at,
        // NOTE: `unknown`, or an outcome of a later store that this efrd does not know.
        _ => {
            return Err(DaemonError::Pairing {
                problem: PairingProblem::UnknownDevice { device_id },
            });
        }
    };
    responder.item(&DeviceRevokeResult { revoked_at }).await?;
    Ok(())
}
