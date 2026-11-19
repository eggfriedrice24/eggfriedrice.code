//! `device.push_set`: the push token of the phone app of the proof, or none.
//!
//! The method runs only on the tailnet listener and after the proof (`methods.rs`), so
//! it acts on the device of the proof and takes no device id. A token of the wrong form
//! is `invalid`; it goes into a JSON string of a push, so its text cannot change the
//! request. No log holds it.

use efr_protocol::{DevicePushSet, DevicePushSetResult, is_expo_push_token};
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::state::State;

const METHOD: &str = "device.push_set";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: DevicePushSet,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let Some(device_id) = crate::remote::prove::proved_device(context) else {
        return Err(DaemonError::ProofNeeded { method: METHOD });
    };
    if params.token.as_ref().is_some_and(|token| !is_expo_push_token(token.expose_secret())) {
        return Err(DaemonError::InvalidParams { reason: "the token is not an Expo push token" });
    }
    let on = params.token.is_some();
    // NOTE: a revoke between the proof and this write leaves no token: the store sets
    // none for a revoked device, and the revoke closes this connection.
    state.writer.device_push_set(device_id, params.token).await?;
    tracing::info!(%device_id, on, "a device set its push");
    responder.item(&DevicePushSetResult {}).await?;
    Ok(())
}
