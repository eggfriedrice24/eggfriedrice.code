//! `attachment.discard`: drop an upload that no prompt will name.

use efr_protocol::AttachmentDiscard;
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    _state: &State,
    _context: &ConnectionContext,
    _params: AttachmentDiscard,
    _responder: &Responder,
) -> Result<(), DaemonError> {
    Err(DaemonError::InvalidParams { reason: "this efrd takes no attachments" })
}
