//! The attachments of a conversation as the actor and the turns see them: the files of
//! a prompt and the images of a tool, which the daemon stores (ADR 0026).

use std::fmt;
use std::path::Path;

use async_trait::async_trait;
use efr_protocol::{Attachment, AttachmentId, ConversationId, UploadId};
use efr_provider::EncodedBytes;

/// The store of the attachments, which the daemon implements: its blob dir, the copies
/// in `$SCRATCH/attachments` and the base64 text that a request sends.
///
/// [`ConversationDeps`](crate::ConversationDeps) is one value for every conversation,
/// so each call names its conversation.
#[async_trait]
pub trait Attachments: Send + Sync + fmt::Debug {
    /// Takes the finished upload `upload` as the attachment `id`: its bytes go into the
    /// blob dir and its copy into `scratch`/attachments. `None` when the upload is gone
    /// or a step failed; the prompt is then refused, and the upload stays.
    async fn bind(
        &self,
        conversation_id: ConversationId,
        id: AttachmentId,
        upload: UploadId,
        scratch: &Path,
    ) -> Option<Attachment>;

    /// Keeps the image `bytes` of the file `path` that a tool read, as the attachment
    /// `id`. `None` when the daemon keeps no image.
    async fn keep(
        &self,
        conversation_id: ConversationId,
        id: AttachmentId,
        path: &Path,
        bytes: Vec<u8>,
    ) -> Option<Attachment>;

    /// The base64 text of the bytes that a model gets of `attachment`. `None` when it
    /// has no blob: it then goes as its label with the path.
    async fn load(
        &self,
        conversation_id: ConversationId,
        attachment: &Attachment,
    ) -> Option<EncodedBytes>;
}

/// The store of a daemon that takes no attachments: no upload binds, no image is kept
/// and no bytes load.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoAttachments;

#[async_trait]
impl Attachments for NoAttachments {
    async fn bind(
        &self,
        _conversation_id: ConversationId,
        _id: AttachmentId,
        _upload: UploadId,
        _scratch: &Path,
    ) -> Option<Attachment> {
        None
    }

    async fn keep(
        &self,
        _conversation_id: ConversationId,
        _id: AttachmentId,
        _path: &Path,
        _bytes: Vec<u8>,
    ) -> Option<Attachment> {
        None
    }

    async fn load(
        &self,
        _conversation_id: ConversationId,
        _attachment: &Attachment,
    ) -> Option<EncodedBytes> {
        None
    }
}
