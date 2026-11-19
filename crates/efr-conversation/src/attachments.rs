//! The attachments of a conversation as the actor and the turns see them: the files of
//! a prompt and the images of a tool, which the daemon stores (ADR 0026).
//!
//! The history holds only their metadata. A turn fills the bytes of each part that its
//! model reads as content into its window once, when it starts or when a tool result
//! brings a new image (`turn.rs`), and strips them from the messages that it keeps.
//! The rules of what goes as content are here ([`TurnAttachments`]):
//! the model's input kinds, the withheld set of the conversation, the budget of each
//! request and the recovery after a provider refused one.

use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use efr_protocol::{Attachment, AttachmentId, AttachmentKind, ConversationId, UploadId};
use efr_provider::{
    AttachmentPart, ContentBlock, EncodedBytes, InputKinds, Message, ProviderError,
};
use efr_store::attachments::Summary;

use crate::context::PRUNED_OUTPUT_STUB;

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

    /// Removes the files of `attachments`, which no request will read: those of a
    /// prompt that was withdrawn before it ran, or that was not recorded. Each blob that
    /// another attachment of the conversation names stays. A store that keeps no files
    /// has nothing to do.
    async fn forget(&self, _conversation_id: ConversationId, _attachments: &[Attachment]) {}
}

/// The numbers of the attachments of one conversation, which the queue step of a
/// prompt and the tool calls of its turns take, so no two attachments get one id.
#[derive(Debug, Default)]
pub(crate) struct AttachmentIds(AtomicU32);

impl AttachmentIds {
    /// Takes `last`, the highest number that the store has, as given.
    pub(crate) fn seen(&self, last: u32) {
        self.0.fetch_max(last, Ordering::Relaxed);
    }

    /// The id of the next attachment of the conversation.
    pub(crate) fn next(&self) -> AttachmentId {
        let last =
            self.0.update(Ordering::Relaxed, Ordering::Relaxed, |last| last.saturating_add(1));
        AttachmentId::nth(last.saturating_add(1))
    }
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

/// What one turn knows of the attachments of its conversation: whether it has any,
/// what its model reads, what goes as a label from now on, and what a good call of the
/// model carried already. The ids come from the [`AttachmentIds`] of the conversation.
#[derive(Debug, Default)]
pub(crate) struct TurnAttachments {
    /// The highest number of an attachment of the conversation so far.
    last: u32,
    /// What the turn's model reads as content besides text.
    inputs: InputKinds,
    /// The attachments that go as a label from now on (`attachments_withheld`).
    withheld: HashSet<AttachmentId>,
    /// The attachments that a model call of the turn's model sent as content and that
    /// it answered: a refused request withholds the others first.
    carried: HashSet<AttachmentId>,
}

impl TurnAttachments {
    /// The state of a turn whose model reads `inputs`, in a conversation of `summary`.
    pub(crate) fn new(summary: &Summary, inputs: InputKinds) -> Self {
        TurnAttachments {
            last: summary.last,
            inputs,
            withheld: summary.withheld.iter().cloned().collect(),
            carried: HashSet::new(),
        }
    }

    /// True when the conversation has an attachment, so a request must be walked for
    /// its parts.
    pub(crate) fn any(&self) -> bool {
        self.last > 0
    }

    /// Takes `ids` as attachments that the conversation already has, such as those of
    /// the turn's prompt.
    pub(crate) fn seen<'a>(&mut self, ids: impl IntoIterator<Item = &'a AttachmentId>) {
        let highest = ids.into_iter().filter_map(AttachmentId::number).max().unwrap_or(0);
        self.last = self.last.max(highest);
    }

    /// True when the turn's model gets `attachment` as content: efrd keeps the bytes
    /// for a model, the model reads its kind, and it is not withheld.
    pub(crate) fn reads(&self, attachment: &Attachment) -> bool {
        let kind = match attachment.kind {
            AttachmentKind::Image { .. } => self.inputs.image,
            AttachmentKind::Pdf { .. } => self.inputs.pdf,
            _ => false,
        };
        kind && attachment.sent.is_some() && !self.withheld.contains(&attachment.id)
    }

    /// The tokens of the attachments of `block` that the turn's model gets as content
    /// ([`reads`](Self::reads)), counted with no data: for a window that no turn
    /// filled, as `conversation.context` and a manual compaction build it.
    pub(crate) fn content_tokens(&self, block: &ContentBlock) -> u64 {
        block_parts(block)
            .iter()
            .filter(|part| self.reads(&part.attachment))
            .map(|part| efr_provider::content_tokens(&part.attachment))
            .sum()
    }

    /// The [`content_tokens`](Self::content_tokens) of each block of `messages`.
    pub(crate) fn tokens<'a>(&self, messages: impl Iterator<Item = &'a Message>) -> u64 {
        messages.flat_map(|message| &message.content).map(|block| self.content_tokens(block)).sum()
    }

    /// The attachments of a prompt that the turn's model gets as a path, for
    /// `turn_started`.
    pub(crate) fn unread(&self, attachments: &[Attachment]) -> Vec<AttachmentId> {
        attachments.iter().filter(|a| !self.reads(a)).map(|a| a.id.clone()).collect()
    }

    /// `ids` go as a label from now on.
    pub(crate) fn withhold(&mut self, ids: &[AttachmentId]) {
        self.withheld.extend(ids.iter().cloned());
    }

    /// A model call that sent `sent` as content got an answer.
    pub(crate) fn carried(&mut self, sent: Vec<AttachmentId>) {
        self.carried.extend(sent);
    }

    /// What a request that sent `sent` as content and that the provider refused
    /// withholds: at the first refusal of a request the attachments that no good call
    /// carried yet, else, or when there are none, every one of `sent`. Empty when the
    /// request sent none, and then the refusal has another cause.
    pub(crate) fn refused(&self, sent: &[AttachmentId], first: bool) -> Vec<AttachmentId> {
        let new: Vec<AttachmentId> =
            sent.iter().filter(|id| !self.carried.contains(id)).cloned().collect();
        if first && !new.is_empty() { new } else { sent.to_vec() }
    }
}

/// True when `error` can be the refusal of an attachment: a 400 or a 413 of the API that
/// is no overflow of the window, no limit and no login error (those have their own
/// variants).
pub(crate) fn is_refusal(error: &ProviderError) -> bool {
    matches!(error, ProviderError::Api { status: Some(400 | 413), .. })
}

/// The parts of `block` that can go as content: an attachment block, or the attachments
/// of a tool result that a prune did not cut.
pub(crate) fn block_parts(block: &ContentBlock) -> &[AttachmentPart] {
    match block {
        ContentBlock::Attachment(part) => std::slice::from_ref(part),
        ContentBlock::ToolResult { output, attachments, .. } if output != PRUNED_OUTPUT_STUB => {
            attachments
        }
        _ => &[],
    }
}

/// The parts of `message` that can go as content: those of the message itself and of
/// each tool result that a prune did not cut, whose attachments go as labels.
pub(crate) fn parts_mut(message: &mut Message) -> Vec<&mut AttachmentPart> {
    let mut parts = Vec::new();
    for block in &mut message.content {
        match block {
            ContentBlock::Attachment(part) => parts.push(part),
            ContentBlock::ToolResult { output, attachments, .. }
                if output != PRUNED_OUTPUT_STUB =>
            {
                parts.extend(attachments.iter_mut());
            }
            _ => {}
        }
    }
    parts
}

/// The ids of the parts of `messages` that have data, in their order.
pub(crate) fn with_data<'a>(messages: impl Iterator<Item = &'a mut Message>) -> Vec<AttachmentId> {
    messages
        .flat_map(parts_mut)
        .filter(|part| part.data.is_some())
        .map(|part| part.attachment.id.clone())
        .collect()
}

/// Drops the data of each part of `messages` that `ids` names, so it goes as its label.
pub(crate) fn clear<'a>(messages: impl Iterator<Item = &'a mut Message>, ids: &[AttachmentId]) {
    let ids: HashSet<&AttachmentId> = ids.iter().collect();
    for part in messages.flat_map(parts_mut) {
        if ids.contains(&part.attachment.id) {
            part.data = None;
        }
    }
}

/// The line that a tool result gets for an image of `attachment` that no model gets as
/// content: efrd keeps no bytes for it. A valid image is too large; any other file of
/// an image type is broken.
pub(crate) fn tool_note(attachment: &Attachment) -> Option<String> {
    if attachment.sent.is_some() {
        return None;
    }
    Some(match attachment.kind {
        AttachmentKind::Image { width, height } => {
            format!("[efr: too large to show; {width}x{height}]")
        }
        _ => BROKEN_IMAGE.to_owned(),
    })
}

/// The line of a tool result for an image that is cut or broken.
pub(crate) const BROKEN_IMAGE: &str = "[efr: the image is broken]";

/// The line of a tool result for an image that efrd could not keep.
pub(crate) const NOT_KEPT: &str = "[efr: efr could not keep the image, so you do not see it]";

/// Drops the data of every part of `message`, of a pruned tool result too: the history,
/// the saved messages and a compaction request hold only the metadata.
pub(crate) fn strip(message: &mut Message) {
    for block in &mut message.content {
        let parts = match block {
            ContentBlock::Attachment(part) => std::slice::from_mut(part),
            ContentBlock::ToolResult { attachments, .. } => attachments.as_mut_slice(),
            _ => continue,
        };
        for part in parts {
            part.data = None;
        }
    }
}

#[cfg(test)]
mod tests;
