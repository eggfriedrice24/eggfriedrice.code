//! `device.push_set`: the push token of the phone app of the proof, or none.

use efr_protocol::DevicePushSet;
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    _state: &State,
    _context: &ConnectionContext,
    _params: DevicePushSet,
    _responder: &Responder,
) -> Result<(), DaemonError> {
    Err(DaemonError::InvalidParams { reason: "this efrd sends no push" })
}
