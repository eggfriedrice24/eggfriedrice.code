//! The attachments of the conversations (ADR 0026): the blob dir with the bytes that
//! efrd sends to a model, the intake that checks each file, the open uploads of
//! `attachment.put`, and the cache of the base64 text of the blobs.

use std::sync::Arc;

use efr_config::Settings;
use efr_conversation::{Attachments, NoAttachments};
use efr_protocol::AttachmentLimits;
use efr_stdx::paths::Dirs;

mod cache;
mod intake;
mod store;
pub(crate) mod uploads;

/// The store of the attachments that the conversations use, in the data dir of `dirs`.
pub(crate) fn store(_dirs: &Dirs) -> Arc<dyn Attachments> {
    Arc::new(NoAttachments)
}

/// The limits of the capability `attachments` under `settings`, or `None` when this
/// efrd takes no attachments.
pub(crate) fn limits(_settings: &Settings) -> Option<AttachmentLimits> {
    None
}
