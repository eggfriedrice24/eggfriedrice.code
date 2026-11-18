//! Attachments on the wire (ADR 0026): a file that goes with a prompt, or an image that
//! the model read with `read_file`.
//!
//! An attachment is metadata in the event log: its id, its name, its type, its size, the
//! SHA-256 of its bytes, where it came from, the copy that the user, the model and the
//! shell can open, and what a model gets as content. The bytes stay in efrd's data dir.
//! A client uploads the bytes of a file in chunks (`attachment.put`) and names the
//! finished uploads in `prompt.send`.

use std::path::PathBuf;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{numbered_id, uuid_id};

/// The longest name of an attachment, in bytes. The copy of an attachment is the file
/// `<id>-<name>`, and a file name has at most 255 bytes.
pub const NAME_MAX_BYTES: usize = 240;

/// The longest extension, with its dot, that [`fit_name`] keeps.
pub const KEPT_EXTENSION_BYTES: usize = 16;

/// The most bytes of one chunk of `attachment.put`: 512 KiB, which is 699,052 base64
/// characters and fits in the 1 MiB message of the phone's socket.
pub const CHUNK_BYTES: u64 = 512 * 1024;

/// The most uploads that one owner keeps open at a time: a connection of the Unix
/// socket, or a paired device.
pub const MAX_OPEN_UPLOADS: usize = 20;

/// How long an upload of a paired device waits with no chunk and no prompt, before efrd
/// removes it. Only the tailnet listener has it: on the Unix socket an upload ends with
/// its connection.
pub const UPLOAD_TTL: Duration = Duration::from_secs(60 * 60);

numbered_id!(
    /// The id of an attachment: `a1`, `a2`, ... in the order in which a conversation
    /// took them. Two attachments with the same bytes have two ids.
    AttachmentId,
    "a",
    "attachment",
    "an attachment"
);

uuid_id!(
    /// One upload of `attachment.put`. The client makes it, as a [`CommandId`](crate::CommandId),
    /// and names it in `prompt.send` when the upload is complete.
    UploadId
);

/// One attachment, as `prompt_queued` and `tool_call_completed` record it. Metadata only:
/// no event holds the bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attachment {
    /// The id in its conversation.
    pub id: AttachmentId,
    /// The name of the file, such as `shot.png`: no `/`, no control character, at most
    /// [`NAME_MAX_BYTES`] bytes.
    pub name: String,
    /// The type, such as `image/png`, from the first bytes of the file, never from a
    /// client. `text/plain` for a text file, `application/octet-stream` for each other
    /// file.
    pub media_type: String,
    /// The size of the file in bytes.
    pub bytes: u64,
    /// The SHA-256 of the bytes of the file, in lowercase hex.
    pub sha256: String,
    /// What kind of file it is.
    pub kind: AttachmentKind,
    /// Where it came from.
    pub origin: AttachmentOrigin,
    /// The copy of the file in `$SCRATCH/attachments`, an absolute path, with the bytes
    /// of the file as it was attached. Absent for an image that a tool read: that file is
    /// on disk already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<PathBuf>,
    /// What a model gets as content. Absent when the file goes to a model only as a label
    /// with its path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<SentForm>,
}

/// What kind of file an attachment is.
///
/// On the wire an object with a `kind` member: `{"kind": "image", "width": 1920,
/// "height": 1080}`, `{"kind": "pdf", "pages": 12}`, `{"kind": "text"}` or
/// `{"kind": "other"}`. A kind that this build does not know reads as `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AttachmentKind {
    /// A PNG, JPEG, GIF or WebP image.
    Image {
        /// The width in pixels.
        width: u32,
        /// The height in pixels.
        height: u32,
    },
    /// A PDF document.
    Pdf {
        /// The count of pages. Absent when efrd could not count them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pages: Option<u32>,
    },
    /// A UTF-8 text file.
    Text,
    /// Each other file.
    Other,
    /// A kind that this build does not know.
    #[serde(other)]
    Unknown,
}

/// Where an attachment came from.
///
/// On the wire an object with a `kind` member: `{"kind": "path", "path": "..."}`,
/// `{"kind": "clipboard"}`, `{"kind": "phone"}` or `{"kind": "tool", "path": "..."}`. An
/// origin that this build does not know reads as `unknown`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AttachmentOrigin {
    /// An `@path` of a `,` line in the terminal.
    Path {
        /// The user's file, an absolute path.
        path: PathBuf,
    },
    /// A paste into a `,` line in the terminal.
    Clipboard,
    /// The plus button of the phone app.
    Phone,
    /// An image that the model read with `read_file`.
    Tool {
        /// The file that the tool read, an absolute path.
        path: PathBuf,
    },
    /// An origin that this build does not know.
    #[serde(other)]
    Unknown,
}

/// What a model gets of an attachment as content: the prepared image, or the PDF.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SentForm {
    /// The type of the bytes that the model gets, such as `image/jpeg` for a PNG with no
    /// alpha that efrd made smaller.
    pub media_type: String,
    /// The count of those bytes.
    pub bytes: u64,
    /// The width in pixels, for an image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// The height in pixels, for an image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// A finished upload, before a prompt takes it: what efrd found in its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadInfo {
    /// The name of the file.
    pub name: String,
    /// The type, from the first bytes of the file.
    pub media_type: String,
    /// What kind of file it is.
    pub kind: AttachmentKind,
    /// The size of the file in bytes.
    pub bytes: u64,
    /// What a model gets as content. Absent when the file goes as a label with its path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<SentForm>,
}

/// The limits of the attachments of this efrd, which the capability `attachments`
/// announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentLimits {
    /// The largest file of an attachment, in bytes.
    pub max_file_bytes: u64,
    /// The most attachments of one prompt.
    pub max_per_message: u32,
    /// The most bytes of one chunk of `attachment.put`.
    pub chunk_bytes: u64,
    /// The longest edge in pixels of an image that a model gets: efrd makes a larger
    /// PNG or JPEG smaller.
    pub max_edge_px: u32,
}

/// Why efrd sends attachments as labels with their paths from now on
/// (`attachments_withheld`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WithheldReason {
    /// A request had too many images, too many bytes or too many tokens of attachments.
    Budget,
    /// The provider refused a request that held the attachments.
    Refused,
    /// A reason that this build does not know.
    #[serde(other)]
    Unknown,
}

impl WithheldReason {
    /// Every reason, in the order of the enum.
    pub const ALL: [WithheldReason; 3] =
        [WithheldReason::Budget, WithheldReason::Refused, WithheldReason::Unknown];
}

/// `name` cut to at most [`NAME_MAX_BYTES`] bytes: the stem is cut at a character
/// border, and an extension of at most [`KEPT_EXTENSION_BYTES`] bytes stays. A name that
/// fits stays as it is.
///
/// A client cuts a name with this rule before its upload, and the phone app has the
/// same rule; efrd refuses a longer name and does not cut it.
///
/// ```
/// use efr_protocol::{NAME_MAX_BYTES, fit_name};
///
/// let name = format!("{}.png", "x".repeat(300));
/// let fit = fit_name(&name);
/// assert_eq!(fit.len(), NAME_MAX_BYTES);
/// assert!(fit.ends_with("x.png"));
/// ```
pub fn fit_name(name: &str) -> String {
    if name.len() <= NAME_MAX_BYTES {
        return name.to_owned();
    }
    let extension = match name.rfind('.') {
        Some(at) if at > 0 && name.len() - at <= KEPT_EXTENSION_BYTES => &name[at..],
        _ => "",
    };
    let stem = &name[..name.len() - extension.len()];
    let keep = stem.floor_char_boundary(NAME_MAX_BYTES - extension.len());
    format!("{}{extension}", &stem[..keep])
}

#[cfg(test)]
mod tests;
