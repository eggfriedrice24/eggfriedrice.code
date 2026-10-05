//! `admin.config_reload`: read `config.toml` again now, for `efr config reload`.
//!
//! The answer is the outcome: applied, or the file's error with the old settings kept,
//! and the keys that wait for a restart. A file with an error is not a failed request;
//! the error is the result.

use efr_protocol::AdminConfigReload;
use efr_transport::Responder;

use crate::DaemonError;
use crate::reload;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: AdminConfigReload,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let result = reload::reload(state, "admin.config_reload").await?;
    responder.item(&result).await?;
    Ok(())
}
