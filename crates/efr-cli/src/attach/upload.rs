//! The upload of the files of a prompt and the send of the prompt with them, on one
//! connection: efrd keeps the uploads of a connection until it closes, so a killed
//! `efr` leaves no part.
//!
//! efr checks the limits of the capability `attachments` first, so a prompt never goes
//! without a file that it names. Each file goes in chunks of `chunk_bytes`, from the
//! offset that efrd says it has. When efrd answers `prompt.send` with
//! `missing_uploads`, those files go again under the same upload ids and the prompt
//! goes again with the same command id, at most [`RESENDS`] times. On a failure after
//! an upload, efr discards the uploads that no prompt took.

use std::io::SeekFrom;

use efr_protocol::{
    AttachmentDiscard, AttachmentDiscardResult, AttachmentLimits, AttachmentOrigin, AttachmentPut,
    AttachmentPutResult, Base64Bytes, ErrorCode, Method, MissingUploads, PromptSend,
    PromptSendResult, SecretFile, UploadId,
};
use efr_stdx::text::{MIB, byte_size};
use efr_transport::client::{Client, ClientError};
use serde::Deserialize as _;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

use super::{Found, Words};
use crate::context::Context;

/// How often a prompt goes again after efrd said that some of its uploads are missing.
const RESENDS: usize = 3;

/// Why a prompt with files did not go.
#[derive(Debug)]
pub(crate) enum Failure {
    /// A file cannot attach, and nothing was sent: the note says why, such as
    /// `not attached: 12 files. The limit is 10 for one prompt
    /// (attachments.max_per_message).`
    NotAttached(String),
    /// The uploads did not arrive: efrd still missed some after [`RESENDS`] sends, or it
    /// took no byte of a chunk.
    Lost,
    /// The connection failed, or efrd refused the prompt.
    Client(ClientError),
}

impl From<ClientError> for Failure {
    fn from(error: ClientError) -> Self {
        Failure::Client(error)
    }
}

/// Sends `prompt` on `client` with the files of `words`; the clock and the randomness
/// of `ctx` make the ids of the uploads. A prompt with no file is one `prompt.send`.
pub(crate) async fn send(
    ctx: &Context,
    client: &Client,
    mut prompt: PromptSend,
    words: &Words,
) -> Result<PromptSendResult, Failure> {
    if words.files.is_empty() {
        return Ok(client.call(Method::PromptSend(prompt)).await?);
    }
    let limits = checked(client.hello().capabilities.attachments.as_ref(), &words.files)?;
    let mut ids = Vec::with_capacity(words.files.len());
    for file in &words.files {
        let id = UploadId::from_uuid(efr_stdx::id::uuid_v7(&*ctx.clock, &*ctx.rng));
        ids.push(id);
        if let Err(failure) = upload(client, id, file, limits.chunk_bytes).await {
            discard(client, &ids).await;
            return Err(failure);
        }
    }
    prompt.attachments.clone_from(&ids);
    for resend in 0..=RESENDS {
        let error = match client.call(Method::PromptSend(prompt.clone())).await {
            Ok(result) => {
                remove_clips(&words.files);
                return Ok(result);
            }
            Err(error) => error,
        };
        let Some(missing) = missing_uploads(&error) else {
            discard(client, &ids).await;
            return Err(Failure::Client(error));
        };
        if resend == RESENDS {
            break;
        }
        for (id, file) in ids.iter().zip(&words.files).filter(|(id, _)| missing.contains(id)) {
            if let Err(failure) = upload(client, *id, file, limits.chunk_bytes).await {
                discard(client, &ids).await;
                return Err(failure);
            }
        }
    }
    discard(client, &ids).await;
    Err(Failure::Lost)
}

/// The uploads that efrd misses, when `error` is its `not_found` of a prompt whose
/// uploads are missing.
fn missing_uploads(error: &ClientError) -> Option<Vec<UploadId>> {
    let ClientError::Server { body } = error else {
        return None;
    };
    let data = body.data.as_ref().filter(|_| body.code == ErrorCode::NotFound)?;
    MissingUploads::deserialize(data).ok().map(|missing| missing.missing_uploads)
}

/// The limits of `limits` when `files` fit in them; else the note of the first limit
/// that they break.
pub(super) fn checked(
    limits: Option<&AttachmentLimits>,
    files: &[Found],
) -> Result<AttachmentLimits, Failure> {
    let Some(limits) = limits else {
        return Err(Failure::NotAttached(
            "not attached: efrd does not take attachments (attachments.enabled, or an efrd before 0.0.13).".to_owned(),
        ));
    };
    if files.len() > usize::try_from(limits.max_per_message).unwrap_or(usize::MAX) {
        return Err(Failure::NotAttached(format!(
            "not attached: {} files. The limit is {} for one prompt (attachments.max_per_message).",
            files.len(),
            limits.max_per_message
        )));
    }
    if let Some(file) = files.iter().find(|file| file.bytes > limits.max_file_bytes) {
        // NOTE: the limit in the number of the key, so 50 MiB is not `52 MB`.
        let limit = limits.max_file_bytes;
        let limit = if limit % MIB == 0 { format!("{} MB", limit / MIB) } else { byte_size(limit) };
        return Err(Failure::NotAttached(format!(
            "not attached: {} is {}. The limit is {limit} (attachments.max_file_mib).",
            file.typed,
            byte_size(file.bytes),
        )));
    }
    Ok(*limits)
}

/// Uploads `file` as `id` in chunks of at most `chunk` bytes, from the offset that efrd
/// has of it, until efrd says that it is whole.
async fn upload(client: &Client, id: UploadId, file: &Found, chunk: u64) -> Result<(), Failure> {
    let unreadable = |error: std::io::Error| {
        Failure::NotAttached(format!("not attached: {} ({})", file.typed, error.kind()))
    };
    let mut source = tokio::fs::File::open(&file.path).await.map_err(unreadable)?;
    let origin = if file.clip {
        AttachmentOrigin::Clipboard
    } else {
        AttachmentOrigin::Path { path: file.path.clone() }
    };
    let mut offset = 0;
    loop {
        // NOTE: a chunk has at most `chunk_bytes`, 512 KiB, so it fits in a usize.
        let mut data = vec![0; file.bytes.saturating_sub(offset).min(chunk) as usize];
        source.seek(SeekFrom::Start(offset)).await.map_err(unreadable)?;
        source.read_exact(&mut data).await.map_err(unreadable)?;
        let put = AttachmentPut {
            upload_id: id,
            name: file.name.clone(),
            total_bytes: file.bytes,
            offset,
            data: Base64Bytes::new(data),
            origin: Some(origin.clone()),
        };
        let result = match client.call::<AttachmentPutResult>(Method::AttachmentPut(put)).await {
            Ok(result) => result,
            Err(ClientError::Server { body }) => {
                let secret = body
                    .data
                    .as_ref()
                    .and_then(|data| SecretFile::deserialize(data).ok())
                    .is_some_and(|data| data.secret_file);
                let note = if secret {
                    format!("not attached: {} is a secret file.", file.typed)
                } else {
                    format!(
                        "not attached: {} ({})",
                        file.typed,
                        crate::format::one_line(&body.message)
                    )
                };
                return Err(Failure::NotAttached(note));
            }
            Err(error) => return Err(Failure::Client(error)),
        };
        if result.upload.is_some() {
            return Ok(());
        }
        // NOTE: efrd writes a chunk only at the end of what it has; an answer that
        // takes no byte of a chunk that starts there would repeat for ever.
        if result.received == offset || result.received > file.bytes {
            return Err(Failure::Lost);
        }
        offset = result.received;
    }
}

/// Drops the uploads `ids`, which no prompt took. A failure is only logged: efrd drops
/// them with the connection anyway.
async fn discard(client: &Client, ids: &[UploadId]) {
    if client.is_closed() {
        return;
    }
    for id in ids {
        let method = Method::AttachmentDiscard(AttachmentDiscard { upload_id: *id });
        if let Err(error) = client.call::<AttachmentDiscardResult>(method).await {
            tracing::debug!(%error, "an upload that no prompt took could not be discarded");
        }
    }
}

/// Removes the clip files of `files`, whose prompt went: efrd keeps their copies.
fn remove_clips(files: &[Found]) {
    for file in files.iter().filter(|file| file.clip) {
        if let Err(error) = std::fs::remove_file(&file.path) {
            tracing::debug!(%error, path = %file.path.display(), "a clip file could not be removed");
        }
    }
}
