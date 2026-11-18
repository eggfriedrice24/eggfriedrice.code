//! `admin.push_test`: a test notice to each phone that has push on, for
//! `efr push test`. The answer comes after the receipts, in about 14 seconds
//! (`push/check.rs`).

use efr_protocol::{AdminPushTest, AdminPushTestResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: AdminPushTest,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let devices = crate::push::check(state).await?;
    responder.item(&AdminPushTestResult { devices }).await?;
    Ok(())
}
