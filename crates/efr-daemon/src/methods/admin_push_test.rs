//! `admin.push_test`: a test notice to each phone that has push on, for
//! `efr push test`.

use efr_protocol::AdminPushTest;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    _state: &State,
    _params: AdminPushTest,
    _responder: &Responder,
) -> Result<(), DaemonError> {
    Err(DaemonError::InvalidParams { reason: "this efrd sends no push" })
}
