//! The open uploads of `attachment.put`, by owner: a connection of the Unix socket, or
//! the device of the proof on the tailnet listener.
//!
//! An upload is the file `<state dir>/uploads/<upload id>.part` (0600, on disk: the
//! runtime dir is a tmpfs). efrd writes a chunk only at the end of what it has, so a
//! repeated chunk, a lost answer and a resume after a lost link are one case: the
//! client goes on at `received`. After the last chunk the intake checks the file once,
//! and what a model gets of it waits in `<upload id>.sent` until a prompt takes the
//! upload ([`DaemonAttachments`](super::DaemonAttachments)). A start of efrd removes
//! every part, a closed connection of the Unix socket removes its own, a revoke removes
//! those of its device, and an upload of a device that got no chunk and no prompt for
//! [`UPLOAD_TTL`] goes at the next put or the next look of the collector.

use std::collections::HashMap;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, FileExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_protocol::{
    AttachmentKind, AttachmentOrigin, AttachmentPutResult, DeviceId, ErrorBody, ErrorCode,
    MAX_OPEN_UPLOADS, SentForm, UPLOAD_TTL, UploadId, UploadInfo,
};
use efr_stdx::time::Clock;
use efr_transport::{ConnId, ConnectionContext};
use jiff::{SignedDuration, Timestamp};
use tokio::sync::Semaphore;

use super::intake::check;
use crate::DaemonError;

/// The dir of the uploads below the state dir.
pub(crate) const UPLOADS_DIR: &str = "uploads";

/// Who owns an upload: only its owner may add to it, drop it or name it in a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Owner {
    /// A connection of the Unix socket: the upload ends with the connection.
    Connection(ConnId),
    /// The device of the proof on the tailnet listener: an upload goes on over a new
    /// connection.
    Device(DeviceId),
}

impl Owner {
    /// The owner of the uploads of `context`: the device of the proof on the tailnet
    /// listener, else the connection.
    pub(crate) fn of(context: &ConnectionContext) -> Owner {
        crate::remote::prove::proved_device(context)
            .map_or(Owner::Connection(context.conn_id()), Owner::Device)
    }
}

/// One chunk of an upload, from `attachment.put`, after the checks of its params.
#[derive(Debug)]
pub(crate) struct Chunk {
    pub(crate) upload_id: UploadId,
    pub(crate) name: String,
    pub(crate) total_bytes: u64,
    pub(crate) offset: u64,
    pub(crate) data: Vec<u8>,
    pub(crate) origin: AttachmentOrigin,
}

/// Why a put, a discard or a prompt cannot use an upload.
#[derive(Debug, thiserror::Error)]
pub(crate) enum UploadError {
    /// The upload belongs to another owner.
    #[error("the upload belongs to another connection or device")]
    Forbidden,
    /// The owner has [`MAX_OPEN_UPLOADS`] open uploads.
    #[error("there are too many open uploads")]
    TooMany,
    /// The chunk names another file than the first chunk of the upload did.
    #[error("an upload keeps the name, the size and the origin of its first chunk")]
    Changed,
    /// The chunk ends after `total_bytes`.
    #[error("the chunk ends after the size of the file")]
    PastEnd,
    /// The part or the check of the upload failed.
    #[error("the upload could not be written")]
    Io(#[source] io::Error),
}

impl UploadError {
    /// The error of a method: `forbidden`, `invalid`, or `internal` for a file that
    /// could not be written, which a retry can pass.
    pub(crate) fn into_daemon(self) -> DaemonError {
        let code = match &self {
            UploadError::Forbidden => ErrorCode::Forbidden,
            UploadError::TooMany | UploadError::Changed | UploadError::PastEnd => {
                ErrorCode::Invalid
            }
            UploadError::Io(error) => {
                tracing::warn!(error = %error, "an upload could not be written");
                ErrorCode::Internal
            }
        };
        DaemonError::Rejected { body: ErrorBody::new(code, self.to_string()) }
    }
}

/// A finished upload, as a prompt takes it.
#[derive(Debug)]
pub(crate) struct Finished {
    pub(crate) name: String,
    pub(crate) origin: AttachmentOrigin,
    /// The size of the file.
    pub(crate) bytes: u64,
    /// The SHA-256 of the bytes, in lowercase hex.
    pub(crate) sha256: String,
    /// The type from the first bytes.
    pub(crate) media_type: &'static str,
    pub(crate) kind: AttachmentKind,
    /// What a model gets as content, whose bytes are in `sent_file`.
    pub(crate) sent: Option<SentForm>,
    /// The bytes of the file.
    pub(crate) part: PathBuf,
    /// The bytes that a model gets, when it gets any.
    pub(crate) sent_file: Option<PathBuf>,
}

impl Finished {
    fn info(&self) -> UploadInfo {
        UploadInfo {
            name: self.name.clone(),
            media_type: self.media_type.to_owned(),
            kind: self.kind,
            bytes: self.bytes,
            sent: self.sent.clone(),
        }
    }
}

/// The open uploads. A clone shares them.
#[derive(Debug, Clone)]
pub(crate) struct Uploads {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    dir: PathBuf,
    clock: Arc<dyn Clock>,
    /// The one decode at a time of efrd, which the intake of a finished upload takes.
    decode: Arc<Semaphore>,
    open: Mutex<HashMap<UploadId, Upload>>,
}

#[derive(Debug)]
struct Upload {
    owner: Owner,
    name: String,
    total: u64,
    origin: AttachmentOrigin,
    received: u64,
    /// When a chunk or a prompt last named it.
    touched: Timestamp,
    /// True while a chunk or the intake runs: a put meanwhile writes nothing.
    busy: bool,
    /// What the intake found, once the last chunk is in.
    finished: Option<Arc<Finished>>,
}

impl Upload {
    fn result(&self) -> AttachmentPutResult {
        AttachmentPutResult {
            received: self.received,
            upload: self.finished.as_deref().map(Finished::info),
        }
    }

    /// What a put of `chunk` does with this upload at `now`.
    fn next(&mut self, chunk: &Chunk, now: Timestamp) -> Result<Step, UploadError> {
        self.touched = now;
        let len = chunk.data.len() as u64;
        let at_end = chunk.offset == self.received;
        let whole = self.received + len == self.total;
        // NOTE: an empty chunk only asks for `received`, but the first one of an empty
        // file is the whole file.
        let writes = at_end && (len > 0 || whole);
        if self.busy || self.finished.is_some() || !writes {
            return Ok(Step::Answer(self.result()));
        }
        if self.received + len > self.total {
            return Err(UploadError::PastEnd);
        }
        self.busy = true;
        Ok(Step::Write { whole })
    }
}

/// What a put does after its look at the table.
enum Step {
    /// Nothing to write: the answer.
    Answer(AttachmentPutResult),
    /// Write the chunk; `whole` when the file is whole then, so the intake runs.
    Write { whole: bool },
}

impl Uploads {
    /// The uploads in `<state>/uploads`, whose intake takes `decode`.
    pub(crate) fn new(state: &Path, clock: Arc<dyn Clock>, decode: Arc<Semaphore>) -> Self {
        let inner = Inner { dir: state.join(UPLOADS_DIR), clock, decode, open: Mutex::default() };
        Uploads { inner: Arc::new(inner) }
    }

    /// Removes each part that an earlier efrd left: a client uploads its files again
    /// when a prompt names a missing upload.
    pub(crate) async fn sweep(&self) {
        let dir = self.inner.dir.clone();
        let removed = tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        });
        match removed.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "the old uploads could not be removed")
            }
            Err(error) => tracing::warn!(error = %error, "the sweep of the old uploads stopped"),
        }
    }

    /// Removes the open uploads of the connection `conn_id`, which closed.
    pub(crate) fn closed(&self, conn_id: ConnId) {
        let owner = Owner::Connection(conn_id);
        let gone = self.take_where(|upload| upload.owner == owner);
        self.remove_files(gone);
    }

    /// Removes the open uploads of the device `device`, whose pairing was revoked.
    pub(crate) fn revoked(&self, device: DeviceId) {
        let owner = Owner::Device(device);
        let gone = self.take_where(|upload| upload.owner == owner);
        self.remove_files(gone);
    }

    /// Removes each upload of a device that no chunk and no prompt named for
    /// [`UPLOAD_TTL`]. Each put and the collector of efrd (`gc::collect`) call it.
    pub(crate) fn expire(&self) {
        let now = self.inner.clock.now();
        let expired = self.take_where(|upload| is_expired(upload, now));
        self.remove_files(expired);
    }

    /// Takes the chunk `chunk` of `owner`, and answers with the bytes that efrd has of
    /// the upload. The last chunk runs the intake, whose images have at most `max_edge`
    /// pixels on their longer edge.
    pub(crate) async fn put(
        &self,
        owner: Owner,
        chunk: Chunk,
        max_edge: u32,
    ) -> Result<AttachmentPutResult, UploadError> {
        let whole = match self.step(owner, &chunk)? {
            Step::Answer(result) => return Ok(result),
            Step::Write { whole } => whole,
        };
        let Chunk { upload_id, name, offset, data, origin, .. } = chunk;
        let end = offset + data.len() as u64;
        let written = self.write(upload_id, offset, data).await;
        let finished = match written {
            Ok(()) if whole => self.check(upload_id, name, origin, max_edge).await.map(Some),
            Ok(()) => Ok(None),
            Err(error) => Err(error),
        };
        let mut open = self.lock();
        let Some(upload) = open.get_mut(&upload_id) else {
            // NOTE: a discard or a closed connection took the upload while the chunk was
            // written, so its files go now.
            drop(open);
            self.remove_files(vec![upload_id]);
            return finished.map(|_| AttachmentPutResult { received: 0, upload: None });
        };
        upload.busy = false;
        if let Some(finished) = finished? {
            upload.finished = Some(Arc::new(finished));
        }
        upload.received = end;
        Ok(upload.result())
    }

    /// Drops the upload `id` of `owner`. An unknown upload is no error.
    pub(crate) fn discard(&self, owner: Owner, id: UploadId) -> Result<(), UploadError> {
        let mut open = self.lock();
        match open.get(&id) {
            None => return Ok(()),
            Some(upload) if upload.owner != owner => return Err(UploadError::Forbidden),
            Some(_) => {}
        }
        open.remove(&id);
        drop(open);
        self.remove_files(vec![id]);
        Ok(())
    }

    /// The uploads of `ids` that are no finished upload of `owner`, in their order: a
    /// prompt that names one waits for it. The others count as named now.
    pub(crate) fn missing(
        &self,
        owner: Owner,
        ids: &[UploadId],
    ) -> Result<Vec<UploadId>, UploadError> {
        let now = self.inner.clock.now();
        let mut open = self.lock();
        let mut missing = Vec::new();
        for id in ids {
            match open.get_mut(id) {
                Some(upload) if upload.owner != owner => return Err(UploadError::Forbidden),
                Some(upload) if upload.finished.is_some() => upload.touched = now,
                _ => missing.push(*id),
            }
        }
        Ok(missing)
    }

    /// The finished upload `id`, for the prompt that takes it.
    pub(crate) fn finished(&self, id: UploadId) -> Option<Arc<Finished>> {
        self.lock().get(&id).and_then(|upload| upload.finished.clone())
    }

    /// Removes the uploads `ids`, which a queued prompt took.
    pub(crate) fn remove(&self, ids: &[UploadId]) {
        let mut open = self.lock();
        let gone: Vec<UploadId> =
            ids.iter().filter(|id| open.remove(id).is_some()).copied().collect();
        drop(open);
        self.remove_files(gone);
    }

    /// What `chunk` of `owner` does. A new upload takes a place of its owner.
    fn step(&self, owner: Owner, chunk: &Chunk) -> Result<Step, UploadError> {
        self.expire();
        let now = self.inner.clock.now();
        let mut open = self.lock();
        let owned = open.values().filter(|upload| upload.owner == owner).count();
        let upload = match open.get_mut(&chunk.upload_id) {
            Some(upload) if upload.owner != owner => return Err(UploadError::Forbidden),
            Some(upload)
                if upload.name != chunk.name
                    || upload.total != chunk.total_bytes
                    || upload.origin != chunk.origin =>
            {
                return Err(UploadError::Changed);
            }
            Some(upload) => upload,
            None if owned >= MAX_OPEN_UPLOADS => return Err(UploadError::TooMany),
            None => open.entry(chunk.upload_id).or_insert(Upload {
                owner,
                name: chunk.name.clone(),
                total: chunk.total_bytes,
                origin: chunk.origin.clone(),
                received: 0,
                touched: now,
                busy: false,
                finished: None,
            }),
        };
        upload.next(chunk, now)
    }

    /// Writes `data` at `offset` of the part of `id`.
    async fn write(&self, id: UploadId, offset: u64, data: Vec<u8>) -> Result<(), UploadError> {
        let dir = self.inner.dir.clone();
        tokio::task::spawn_blocking(move || {
            if offset == 0 {
                DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
            }
            private_file(&part_path(&dir, id))?.write_all_at(&data, offset)
        })
        .await
        .map_err(|_| UploadError::Io(io::Error::other("the write of a chunk stopped")))?
        .map_err(UploadError::Io)
    }

    /// The intake of the whole upload `id`, one decode at a time: its type, its kind and
    /// what a model gets of it, which waits in its `.sent` file.
    async fn check(
        &self,
        id: UploadId,
        name: String,
        origin: AttachmentOrigin,
        max_edge: u32,
    ) -> Result<Finished, UploadError> {
        let _decoding = self
            .inner
            .decode
            .acquire()
            .await
            .map_err(|_| UploadError::Io(io::Error::other("the intake is closed")))?;
        let dir = self.inner.dir.clone();
        tokio::task::spawn_blocking(move || {
            let part = part_path(&dir, id);
            let bytes = std::fs::read(&part)?;
            let sha256 = crate::judge::sha256_hex(&bytes);
            let checked = check(&bytes, max_edge);
            let (sent, sent_file) = match checked.sent {
                Some((form, sent)) => {
                    let path = sent_path(&dir, id);
                    let file = private_file(&path)?;
                    file.set_len(0)?;
                    file.write_all_at(&sent, 0)?;
                    (Some(form), Some(path))
                }
                None => (None, None),
            };
            Ok(Finished {
                name,
                origin,
                bytes: bytes.len() as u64,
                sha256,
                media_type: checked.media_type,
                kind: checked.kind,
                sent,
                part,
                sent_file,
            })
        })
        .await
        .map_err(|_| UploadError::Io(io::Error::other("the intake of an upload stopped")))?
        .map_err(UploadError::Io)
    }

    /// Takes each upload out that `gone` names, and gives their ids.
    fn take_where(&self, gone: impl Fn(&Upload) -> bool) -> Vec<UploadId> {
        let mut open = self.lock();
        let ids: Vec<UploadId> =
            open.iter().filter(|(_, upload)| gone(upload)).map(|(id, _)| *id).collect();
        for id in &ids {
            open.remove(id);
        }
        ids
    }

    /// Removes the files of the uploads `ids`, in a blocking task.
    fn remove_files(&self, ids: Vec<UploadId>) {
        if ids.is_empty() {
            return;
        }
        let dir = self.inner.dir.clone();
        tokio::task::spawn_blocking(move || {
            for id in ids {
                for path in [part_path(&dir, id), sent_path(&dir, id)] {
                    if let Err(error) = std::fs::remove_file(&path)
                        && error.kind() != io::ErrorKind::NotFound
                    {
                        tracing::warn!(path = %path.display(), error = %error, "a file of an upload could not be removed");
                    }
                }
            }
        });
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<UploadId, Upload>> {
        self.inner.open.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// True for an upload of a device that no chunk and no prompt named for
/// [`UPLOAD_TTL`] up to `now`.
fn is_expired(upload: &Upload, now: Timestamp) -> bool {
    let ttl = SignedDuration::try_from(UPLOAD_TTL).unwrap_or(SignedDuration::MAX);
    matches!(upload.owner, Owner::Device(_))
        && !upload.busy
        && now.duration_since(upload.touched) >= ttl
}

/// Opens the file `path` of an upload for writing (0600), made when it is missing.
fn private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(path)
}

/// The bytes of the upload `id` below the uploads dir `dir`.
fn part_path(dir: &Path, id: UploadId) -> PathBuf {
    dir.join(format!("{id}.part"))
}

/// The bytes that a model gets of the upload `id` below the uploads dir `dir`.
fn sent_path(dir: &Path, id: UploadId) -> PathBuf {
    dir.join(format!("{id}.sent"))
}

#[cfg(test)]
mod tests;
