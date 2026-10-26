//! `device.pair_confirm`: the terminal's answer about a phone that wants to pair.
//!
//! A no ends the pairing (`refused`) and answers without a device. A yes writes the
//! device row and answers with its id; the phone's `device.enroll` gets the same id.
//! A pairing that does not wait for an answer is `not_found`.

use efr_protocol::{DevicePairConfirm, DevicePairConfirmResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::pairing::Decision;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    params: DevicePairConfirm,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let decision = state
        .pairings
        .answer(params.pairing_id, params.accept)
        .map_err(|problem| DaemonError::Pairing { problem })?;
    let device_id = match decision {
        Decision::Refused => None,
        Decision::Accepted(accepted) => {
            // NOTE: in a task of its own, so a terminal that goes away now cannot stop
            // the write between the row and the phone's answer.
            let completed = tokio::spawn(accepted.complete(state.writer.clone()));
            let device_id = completed
                .await
                .map_err(|_| DaemonError::TaskPanicked { task: "device pairing" })??;
            Some(device_id)
        }
    };
    responder.item(&DevicePairConfirmResult { device_id }).await?;
    Ok(())
}
