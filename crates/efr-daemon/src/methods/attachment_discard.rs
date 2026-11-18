//! `attachment.discard`: drop an upload that no prompt will name, such as the file of a
//! chip that the user removed. An unknown upload is no error; the upload of another
//! connection or device is forbidden.

use efr_protocol::{AttachmentDiscard, AttachmentDiscardResult};
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::attachments::uploads::Owner;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: AttachmentDiscard,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let owner = Owner::of(context);
    state.uploads.discard(owner, params.upload_id).map_err(|error| error.into_daemon())?;
    responder.item(&AttachmentDiscardResult::default()).await?;
    Ok(())
}
