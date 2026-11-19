//! `attachment.put`: one chunk of the bytes of a file that a prompt will name.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AttachmentOrigin, Base64Bytes, UploadId, UploadInfo};

/// The params of `attachment.put`, a unary method with the scope `operate`. efrd
/// announces it with the capability `attachments`.
///
/// efrd writes a chunk only when `offset` equals the bytes that it has of the upload.
/// In each other case it writes nothing and answers with what it has, with no error, so
/// a repeated chunk, a lost answer and a resume after a lost link are one case: the
/// client goes on at `received`. A put with empty `data` asks for `received`.
///
/// efrd answers `invalid` for a bad name, a name over
/// [`NAME_MAX_BYTES`](crate::NAME_MAX_BYTES), `data` over the chunk limit, `total_bytes`
/// over the file limit, a type that efrd refuses, an origin that the listener does not
/// allow, a path of the secret class and an upload over
/// [`MAX_OPEN_UPLOADS`](crate::MAX_OPEN_UPLOADS) of one owner; `forbidden` for the
/// upload of another owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentPut {
    /// The upload, a UUID that the client makes.
    pub upload_id: UploadId,
    /// The name of the file, cut with [`fit_name`](crate::fit_name).
    pub name: String,
    /// The size of the whole file in bytes.
    pub total_bytes: u64,
    /// Where `data` starts in the file.
    pub offset: u64,
    /// The bytes from `offset` on, at most [`CHUNK_BYTES`](crate::CHUNK_BYTES).
    pub data: Base64Bytes,
    /// Where the file came from. On the Unix socket it must be `path` or `clipboard`. On
    /// the tailnet listener it must be absent, and efrd sets `phone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<AttachmentOrigin>,
}

/// The `data` of the `invalid` error of an `attachment.put` whose `@path` names a file
/// of the secret class: `{"secret_file": true}`, so a client says so in its own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SecretFile {
    /// Always true.
    pub secret_file: bool,
}

/// The result of `attachment.put`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentPutResult {
    /// The bytes that efrd has of the upload. The next chunk starts here.
    pub received: u64,
    /// What the file is, after its last chunk. Absent while bytes are missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload: Option<UploadInfo>,
}
