//! `attachment.put`: one chunk of the bytes of a file that a prompt will name.

use efr_protocol::AttachmentPut;
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    _state: &State,
    _context: &ConnectionContext,
    _params: AttachmentPut,
    _responder: &Responder,
) -> Result<(), DaemonError> {
    Err(DaemonError::InvalidParams { reason: "this efrd takes no attachments" })
}
