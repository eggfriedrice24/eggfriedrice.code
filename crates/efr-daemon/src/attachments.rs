//! The attachments of the conversations (ADR 0026): the blob dir with the bytes that
//! efrd sends to a model, the intake that checks each file, the open uploads of
//! `attachment.put`, and the cache of the base64 text of the blobs.
//!
//! [`DaemonAttachments`] is the store that the conversations use. Each new attachment
//! goes through the intake in a blocking task, one decode at a time, so a prompt with
//! ten photos never holds ten decoded images. Only an image or a PDF that a model can
//! read as content gets a blob. No bytes go into SQLite, the event log or a log line.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use efr_config::Settings;
use efr_conversation::Attachments;
use efr_protocol::{
    Attachment, AttachmentId, AttachmentLimits, AttachmentOrigin, CHUNK_BYTES, ConversationId,
    UploadId, fit_name,
};
use efr_provider::EncodedBytes;
use efr_sandbox::ATTACH_DIR;
use efr_stdx::paths::Dirs;
use efr_stdx::time::Clock;
use efr_store::Readers;
use tokio::sync::{Semaphore, watch};

use self::cache::{CACHE_BYTES, Cache};
use self::intake::check;
use self::store::Blobs;
use self::uploads::{Finished, Uploads};
use crate::owned_dir;

mod cache;
mod intake;
mod store;
pub(crate) mod uploads;

pub(crate) use self::store::ATTACHMENTS_DIR;

/// The store of the attachments that the conversations use, in the data dir of `dirs`,
/// with the limits of the latest `settings`, and the open uploads in its state dir that
/// the prompts take. Both share one decode at a time.
pub(crate) fn store(
    dirs: &Dirs,
    settings: watch::Receiver<Arc<Settings>>,
    clock: Arc<dyn Clock>,
    readers: Readers,
) -> (Arc<dyn Attachments>, Uploads) {
    let decode = Arc::new(Semaphore::new(1));
    let uploads = Uploads::new(dirs.state(), clock, Arc::clone(&decode));
    let store = DaemonAttachments::new(dirs.data(), settings, uploads.clone(), readers, decode);
    (Arc::new(store), uploads)
}

/// The limits of the capability `attachments` under `settings`, or `None` when this
/// efrd takes no attachments.
pub(crate) fn limits(settings: &Settings) -> Option<AttachmentLimits> {
    let attachments = &settings.attachments;
    attachments.enabled.then(|| AttachmentLimits {
        max_file_bytes: u64::from(attachments.max_file_mib) * efr_stdx::text::MIB,
        max_per_message: attachments.max_per_message,
        chunk_bytes: CHUNK_BYTES,
        max_edge_px: attachments.max_edge_px,
    })
}

/// The blob dir, the cache and the intake of efrd.
#[derive(Debug)]
pub(crate) struct DaemonAttachments {
    blobs: Blobs,
    cache: Cache,
    /// One decode at a time: a decoded image holds up to `efr_media::MAX_DECODE_BYTES`
    /// (200 MB), and its downscale up to 128 MB more at the default edge.
    decode: Arc<Semaphore>,
    settings: watch::Receiver<Arc<Settings>>,
    uploads: Uploads,
    /// For the rows that name a blob, before a withdrawn prompt removes it.
    readers: Readers,
}

impl DaemonAttachments {
    /// The store below the data dir `data`, which binds the finished `uploads`.
    pub(crate) fn new(
        data: &Path,
        settings: watch::Receiver<Arc<Settings>>,
        uploads: Uploads,
        readers: Readers,
        decode: Arc<Semaphore>,
    ) -> Self {
        DaemonAttachments {
            blobs: Blobs::new(data),
            cache: Cache::new(CACHE_BYTES),
            decode,
            settings,
            uploads,
            readers,
        }
    }

    /// The first attachment of `conversation_id` whose blob holds the bytes with the
    /// SHA-256 `sha256` (`efr_store::attachments::blob_attachment`). A failed read counts
    /// as none: the intake then runs.
    async fn blob_attachment(
        &self,
        conversation_id: ConversationId,
        sha256: &str,
    ) -> Option<Attachment> {
        let sha256 = sha256.to_owned();
        let read = self
            .readers
            .with(move |conn| {
                efr_store::attachments::blob_attachment(conn, conversation_id, &sha256)
            })
            .await;
        read.unwrap_or_else(|error| {
            tracing::warn!(%conversation_id, error = %error, "the rows of a blob could not be read");
            None
        })
    }
}

#[async_trait]
impl Attachments for DaemonAttachments {
    async fn bind(
        &self,
        conversation_id: ConversationId,
        id: AttachmentId,
        upload: UploadId,
        scratch: &Path,
    ) -> Option<Attachment> {
        let finished = self.uploads.finished(upload)?;
        // NOTE: a blob of the same bytes holds the form of its first attachment, which
        // the intake of this upload can give differently after a change of
        // attachments.max_edge_px.
        let known = self.blob_attachment(conversation_id, &finished.sha256).await;
        let blobs = self.blobs.clone();
        let scratch = scratch.to_path_buf();
        let copy_name = format!("{id}-{}", finished.name);
        let written = tokio::task::spawn_blocking({
            let finished = Arc::clone(&finished);
            let copy_name = copy_name.clone();
            let blob = known.is_none().then(|| finished.sent_file.clone()).flatten();
            move || {
                write_bound(
                    &blobs,
                    conversation_id,
                    &finished,
                    blob.as_deref(),
                    &scratch,
                    &copy_name,
                )
            }
        });
        let copy = match written.await {
            Ok(Ok(copy)) => copy,
            Ok(Err(error)) => {
                tracing::warn!(%conversation_id, attachment = %id, error = %error, "an upload could not be bound");
                return None;
            }
            Err(error) => {
                tracing::warn!(%conversation_id, error = %error, "the bind of an upload stopped");
                return None;
            }
        };
        Some(Attachment {
            id,
            name: finished.name.clone(),
            media_type: finished.media_type.to_owned(),
            bytes: finished.bytes,
            sha256: finished.sha256.clone(),
            kind: finished.kind,
            origin: finished.origin.clone(),
            copy: Some(copy),
            sent: known.map_or_else(|| finished.sent.clone(), |known| known.sent),
        })
    }

    async fn forget(&self, conversation_id: ConversationId, attachments: &[Attachment]) {
        if attachments.is_empty() {
            return;
        }
        let hashes: Vec<String> = attachments
            .iter()
            .filter(|attachment| attachment.sent.is_some())
            .map(|attachment| attachment.sha256.clone())
            .collect();
        let unnamed = self
            .readers
            .with(move |conn| {
                let mut unnamed = Vec::new();
                for sha256 in hashes {
                    if !efr_store::attachments::names_blob(conn, conversation_id, &sha256)? {
                        unnamed.push(sha256);
                    }
                }
                Ok(unnamed)
            })
            .await;
        let unnamed = unnamed.unwrap_or_else(|error| {
            tracing::warn!(%conversation_id, error = %error, "the rows of the blobs could not be read; the blobs stay");
            Vec::new()
        });
        let blobs: Vec<PathBuf> =
            unnamed.iter().map(|sha256| self.blobs.dir(conversation_id).join(sha256)).collect();
        let copies: Vec<PathBuf> =
            attachments.iter().filter_map(|attachment| attachment.copy.clone()).collect();
        let removed = tokio::task::spawn_blocking(move || remove_bound(&copies, &blobs)).await;
        if let Ok(Err(error)) = removed {
            tracing::warn!(%conversation_id, error = %error, "the files of a withdrawn prompt could not be removed");
        }
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
        let size = bytes.len() as u64;
        let hashed = tokio::task::spawn_blocking(move || (crate::judge::sha256_hex(&bytes), bytes));
        let (sha256, bytes) = hashed.await.ok()?;
        let name = path
            .file_name()
            .map_or_else(|| id.to_string(), |name| fit_name(&name.to_string_lossy()));
        let origin = AttachmentOrigin::Tool { path: path.to_path_buf() };
        // NOTE: a second read of the same image in a conversation takes the form of its
        // blob and needs no decode.
        if let Some(known) = self.blob_attachment(conversation_id, &sha256).await {
            return Some(Attachment { id, name, origin, copy: None, ..known });
        }
        let _decoding = self.decode.acquire().await.ok()?;
        let blobs = self.blobs.clone();
        let intake = tokio::task::spawn_blocking({
            let sha256 = sha256.clone();
            move || {
                let checked = check(&bytes, max_edge);
                if let Some((_, sent)) = &checked.sent {
                    blobs.write(conversation_id, &sha256, sent)?;
                }
                Ok::<_, io::Error>(checked)
            }
        });
        let checked = match intake.await {
            Ok(Ok(checked)) => checked,
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
        Some(Attachment {
            id,
            name,
            media_type: checked.media_type.to_owned(),
            bytes: size,
            sha256,
            kind: checked.kind,
            origin,
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

/// Writes what a prompt takes of the upload `finished`: its blob from the file `blob`,
/// when a model gets bytes of it that no blob holds yet, and its copy `copy_name` in
/// `scratch`/attachments, whose path it gives. It blocks.
fn write_bound(
    blobs: &Blobs,
    conversation_id: ConversationId,
    finished: &Finished,
    blob: Option<&Path>,
    scratch: &Path,
    copy_name: &str,
) -> io::Result<PathBuf> {
    if let Some(blob) = blob {
        blobs.write(conversation_id, &finished.sha256, &std::fs::read(blob)?)?;
    }
    owned_dir::make(scratch, ATTACH_DIR)?;
    let dir = owned_dir::open(scratch, ATTACH_DIR)?;
    owned_dir::write_new(&dir, copy_name, &File::open(&finished.part)?)?;
    Ok(scratch.join(ATTACH_DIR).join(copy_name))
}

/// Removes the scratch `copies` of a withdrawn prompt and the `blobs` that no row
/// names any more. Each copy goes through the descriptor of its dir, so a link there
/// is removed itself and never followed. It blocks.
fn remove_bound(copies: &[PathBuf], blobs: &[PathBuf]) -> io::Result<()> {
    for copy in copies {
        let (Some(name), Some(scratch)) = (copy.file_name(), copy.parent().and_then(Path::parent))
        else {
            continue;
        };
        let dir = owned_dir::open(scratch, ATTACH_DIR)?;
        owned_dir::remove(&dir, &name.to_string_lossy())?;
    }
    for blob in blobs {
        match std::fs::remove_file(blob) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
