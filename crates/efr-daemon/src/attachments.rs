//! The attachments of the conversations (ADR 0026): the blob dir with the bytes that
//! efrd sends to a model, the intake that checks each file, the open uploads of
//! `attachment.put`, and the cache of the base64 text of the blobs.
//!
//! [`DaemonAttachments`] is the store that the conversations use. Each new attachment
//! goes through the intake in a blocking task, one decode at a time, so a prompt with
//! ten photos never holds ten decoded images. Only an image or a PDF that a model can
//! read as content gets a blob. No bytes go into SQLite, the event log or a log line.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use efr_config::Settings;
use efr_conversation::Attachments;
use efr_protocol::{
    Attachment, AttachmentId, AttachmentLimits, AttachmentOrigin, ConversationId, UploadId,
    fit_name,
};
use efr_provider::EncodedBytes;
use efr_stdx::paths::Dirs;
use tokio::sync::{Semaphore, watch};

use self::cache::{CACHE_BYTES, Cache};
use self::intake::check;
use self::store::Blobs;

mod cache;
mod intake;
mod store;
pub(crate) mod uploads;

pub(crate) use self::store::ATTACHMENTS_DIR;

/// The store of the attachments that the conversations use, in the data dir of `dirs`,
/// with the limits of the latest `settings`.
pub(crate) fn store(dirs: &Dirs, settings: watch::Receiver<Arc<Settings>>) -> Arc<dyn Attachments> {
    Arc::new(DaemonAttachments::new(dirs.data(), settings))
}

/// The limits of the capability `attachments` under `settings`, or `None` when this
/// efrd takes no attachments.
pub(crate) fn limits(_settings: &Settings) -> Option<AttachmentLimits> {
    None
}

/// The blob dir, the cache and the intake of efrd.
#[derive(Debug)]
pub(crate) struct DaemonAttachments {
    blobs: Blobs,
    cache: Cache,
    /// One decode at a time: an image of 50 megapixels takes about 200 MB while it is
    /// decoded.
    decode: Semaphore,
    settings: watch::Receiver<Arc<Settings>>,
}

impl DaemonAttachments {
    /// The store below the data dir `data`.
    pub(crate) fn new(data: &Path, settings: watch::Receiver<Arc<Settings>>) -> Self {
        DaemonAttachments {
            blobs: Blobs::new(data),
            cache: Cache::new(CACHE_BYTES),
            decode: Semaphore::new(1),
            settings,
        }
    }
}

#[async_trait]
impl Attachments for DaemonAttachments {
    async fn bind(
        &self,
        _conversation_id: ConversationId,
        _id: AttachmentId,
        _upload: UploadId,
        _scratch: &Path,
    ) -> Option<Attachment> {
        // NOTE: `attachment.put` takes no chunk yet, so no upload is ever finished.
        None
    }

    async fn keep(
        &self,
        conversation_id: ConversationId,
        id: AttachmentId,
        path: &Path,
        bytes: Vec<u8>,
    ) -> Option<Attachment> {
        let settings = Arc::clone(&self.settings.borrow());
        if !settings.attachments.enabled {
            return None;
        }
        let max_edge = settings.attachments.max_edge_px;
        let _decoding = self.decode.acquire().await.ok()?;
        let blobs = self.blobs.clone();
        let intake = tokio::task::spawn_blocking(move || {
            let sha256 = crate::judge::sha256_hex(&bytes);
            let checked = check(&bytes, max_edge);
            if let Some((_, sent)) = &checked.sent {
                blobs.write(conversation_id, &sha256, sent)?;
            }
            Ok::<_, std::io::Error>((sha256, bytes.len() as u64, checked))
        });
        let (sha256, size, checked) = match intake.await {
            Ok(Ok(kept)) => kept,
            Ok(Err(error)) => {
                tracing::warn!(%conversation_id, error = %error, "the blob of an image could not be written");
                return None;
            }
            Err(error) => {
                tracing::warn!(%conversation_id, error = %error, "the intake of an image stopped");
                return None;
            }
        };
        if let Some(note) = checked.note {
            tracing::debug!(%conversation_id, attachment = %id, note = note.text(), "an image of a tool goes as a path");
        }
        let name = path
            .file_name()
            .map_or_else(|| id.to_string(), |name| fit_name(&name.to_string_lossy()));
        Some(Attachment {
            id,
            name,
            media_type: checked.media_type.to_owned(),
            bytes: size,
            sha256,
            kind: checked.kind,
            origin: AttachmentOrigin::Tool { path: path.to_path_buf() },
            copy: None,
            sent: checked.sent.map(|(form, _)| form),
        })
    }

    async fn load(
        &self,
        conversation_id: ConversationId,
        attachment: &Attachment,
    ) -> Option<EncodedBytes> {
        attachment.sent.as_ref()?;
        let sha256 = &attachment.sha256;
        if let Some(data) = self.cache.get(conversation_id, sha256) {
            return Some(data);
        }
        let blobs = self.blobs.clone();
        let blob = sha256.clone();
        let read = tokio::task::spawn_blocking(move || {
            blobs
                .read(conversation_id, &blob)
                .map(|bytes| EncodedBytes::new(STANDARD.encode(bytes)))
        });
        match read.await {
            Ok(Ok(data)) => {
                self.cache.insert(conversation_id, sha256, data.clone());
                Some(data)
            }
            Ok(Err(error)) => {
                tracing::warn!(%conversation_id, attachment = %attachment.id, error = %error, "the blob of an attachment could not be read; it goes as a path");
                None
            }
            Err(error) => {
                tracing::warn!(%conversation_id, error = %error, "the read of a blob stopped");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
