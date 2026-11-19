//! Attachments as a model gets them: the label that every provider writes around one,
//! the bytes that only the request copy holds, the budget of a request and the token
//! count of an attachment.
//!
//! The history, the store's saved turn messages and the cached turns hold only the
//! metadata ([`Attachment`]). Before each model call the conversation fills
//! [`AttachmentPart::data`] in a copy of the request, for the parts that the model reads
//! as content; every other part goes as its label with the path.

use std::fmt;
use std::sync::Arc;

use efr_protocol::{Attachment, AttachmentId, AttachmentKind, AttachmentOrigin};
use efr_stdx::text::{byte_size, one_line};
use serde::{Deserialize, Serialize};

use crate::{ContentBlock, Message, Role};

/// The most image blocks that a request may hold. Anthropic takes 100 for a model with
/// a window under 1M tokens; this leaves a margin.
pub const MAX_IMAGE_BLOCKS: usize = 90;

/// The most bytes of base64 that a request may hold in attachments. Anthropic takes a
/// request of at most 128 MB.
pub const MAX_DATA_BYTES: u64 = 100_000_000;

/// The largest share of the model's window, in percent, that the attachments of a
/// request may take.
pub const WINDOW_SHARE_PERCENT: u64 = 50;

/// The largest PDF that goes to a model as content. A larger PDF goes as a path.
pub const PDF_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// The most pages of a PDF that goes to a model as content: the limit of Anthropic for
/// a model with a window under 1M tokens.
pub const PDF_MAX_PAGES: u32 = 100;

/// The tokens of one page of a PDF: its text and one image of the page.
pub const PDF_PAGE_TOKENS: u64 = 4_500;

/// The bytes of a PDF with no page count that count as one token.
pub const PDF_BYTES_PER_TOKEN: u64 = 4;

/// The edge in pixels of one image patch, the unit in which both providers count the
/// tokens of an image.
pub const IMAGE_PATCH_PX: u32 = 28;

/// The most tokens of one image: the larger of the two providers' counts for the
/// largest image that efrd sends.
pub const IMAGE_MAX_TOKENS: u64 = 4_784;

/// The longest value of a label member, in characters, after [`one_line`].
const LABEL_VALUE_MAX: usize = 4_096;

/// The bytes of an attachment as a provider sends them: standard base64, shared and
/// never copied between the requests of a turn. `Debug` shows only the length.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedBytes(Arc<str>);

impl EncodedBytes {
    /// Wraps `base64`, standard base64 with padding.
    pub fn new(base64: impl Into<Arc<str>>) -> Self {
        EncodedBytes(base64.into())
    }

    /// The base64 text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The length of the base64 text in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// True when the text is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for EncodedBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EncodedBytes(<{} bytes>)", self.0.len())
    }
}

/// One attachment in a message: its metadata, and its bytes only in the copy of a
/// request.
///
/// `data` is never serialised, so the history, the saved turn messages and the cached
/// turns hold only the metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentPart {
    /// What the attachment is.
    pub attachment: Attachment,
    /// The bytes that the model gets as content. `None` sends the label with the path.
    #[serde(skip)]
    pub data: Option<EncodedBytes>,
}

impl AttachmentPart {
    /// A part with no data: it goes to a model as its label with the path.
    pub fn new(attachment: Attachment) -> Self {
        AttachmentPart { attachment, data: None }
    }
}

/// The text that a provider writes around an attachment ([`label`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// The opening tag. For an attachment that does not go as content, it ends with the
    /// line that says so, and a provider writes `open` and `close` as one text.
    pub open: String,
    /// The closing tag.
    pub close: &'static str,
}

/// The closing tag of every label.
pub const LABEL_CLOSE: &str = "</attachment>";

/// The label of `attachment`, the one form that every provider writes around it:
///
/// ```text
/// <attachment id="a1" name="shot.png" type="image/png" size="1920x1080, 2.1 MB" path="..." from="...">
/// ...the image or the document, or the line that it is not sent as content
/// </attachment>
/// ```
///
/// `path` is the file that the model opens: the copy, or the file of a tool read. For a
/// text file or another file of an `@path`, the model must work on the user's file, so
/// `path` is the user's file and `copy` names the copy. Each value is on one line, and
/// `"`, `<`, `>` and `&` become `_`, so a name cannot close the tag.
pub fn label(attachment: &Attachment, as_content: bool) -> Label {
    let users_file = match (&attachment.origin, attachment.kind) {
        (
            AttachmentOrigin::Path { path },
            AttachmentKind::Text | AttachmentKind::Other | AttachmentKind::Unknown,
        ) => Some(path),
        _ => None,
    };
    let opened = match &attachment.origin {
        AttachmentOrigin::Tool { path } => Some(path),
        _ => users_file.or(attachment.copy.as_ref()),
    };
    let mut open = String::from("<attachment");
    member(&mut open, "id", attachment.id.as_str());
    member(&mut open, "name", &attachment.name);
    member(&mut open, "type", &attachment.media_type);
    member(&mut open, "size", &size(attachment));
    if let Some(path) = opened {
        member(&mut open, "path", &path.to_string_lossy());
    }
    match (&attachment.origin, users_file, &attachment.copy) {
        (_, Some(_), Some(copy)) => member(&mut open, "copy", &copy.to_string_lossy()),
        (AttachmentOrigin::Path { path }, None, _) => {
            member(&mut open, "from", &path.to_string_lossy());
        }
        _ => {}
    }
    open.push('>');
    if !as_content {
        open.push('\n');
        open.push_str(if users_file.is_some() { USERS_FILE_NOTE } else { NOT_SENT_NOTE });
    }
    Label { open, close: LABEL_CLOSE }
}

/// The line of a label whose attachment does not go as content.
const NOT_SENT_NOTE: &str = "[efr: not sent as content. Open the file at the path.]";

/// The line of a label of a text file or another file of an `@path`.
const USERS_FILE_NOTE: &str = "[efr: not sent as content. The user's file is at the path. \
                               The copy is the file as it was when the prompt was sent.]";

/// Appends ` name="value"` with `value` on one line and with no character that could
/// end the member or the tag.
fn member(open: &mut String, name: &str, value: &str) {
    open.push(' ');
    open.push_str(name);
    open.push_str("=\"");
    open.extend(one_line(value, LABEL_VALUE_MAX).chars().map(|c| match c {
        '"' | '<' | '>' | '&' => '_',
        c => c,
    }));
    open.push('"');
}

/// The `size` of a label: the pixels or the pages, then the size of the file.
fn size(attachment: &Attachment) -> String {
    let bytes = byte_size(attachment.bytes);
    match attachment.kind {
        AttachmentKind::Image { width, height } => format!("{width}x{height}, {bytes}"),
        AttachmentKind::Pdf { pages: Some(1) } => format!("1 page, {bytes}"),
        AttachmentKind::Pdf { pages: Some(pages) } => format!("{pages} pages, {bytes}"),
        _ => bytes,
    }
}

/// The tokens of the attachments of `message` that go as content: the parts of the
/// message and of its tool results that have data. A part with no data counts nothing
/// here, because its label is text that the caller counts.
///
/// An image counts `ceil(w / 28) * ceil(h / 28)` tokens of the size that the model gets,
/// at most [`IMAGE_MAX_TOKENS`]. A PDF counts [`PDF_PAGE_TOKENS`] for each page, or
/// its bytes divided by [`PDF_BYTES_PER_TOKEN`] when the page count is not known.
pub fn attachment_tokens(message: &Message) -> u64 {
    parts(message)
        .filter(|part| part.data.is_some())
        .map(|part| content_tokens(&part.attachment))
        .sum()
}

/// The parts of `message`, of its own blocks and of its tool results, in their order.
fn parts(message: &Message) -> impl Iterator<Item = &AttachmentPart> {
    message.content.iter().flat_map(|block| match block {
        ContentBlock::Attachment(part) => std::slice::from_ref(part),
        ContentBlock::ToolResult { attachments, .. } => attachments.as_slice(),
        _ => &[],
    })
}

/// The tokens of `attachment` when a model gets it as content, by the rule of
/// [`attachment_tokens`]. A caller with no data, such as the breakdown of
/// `conversation.context`, decides itself which attachments go as content.
pub fn content_tokens(attachment: &Attachment) -> u64 {
    let sent = attachment.sent.as_ref();
    match attachment.kind {
        AttachmentKind::Image { width, height } => {
            let width = sent.and_then(|sent| sent.width).unwrap_or(width);
            let height = sent.and_then(|sent| sent.height).unwrap_or(height);
            let patches = u64::from(width.div_ceil(IMAGE_PATCH_PX))
                * u64::from(height.div_ceil(IMAGE_PATCH_PX));
            patches.min(IMAGE_MAX_TOKENS)
        }
        AttachmentKind::Pdf { pages: Some(pages) } => u64::from(pages) * PDF_PAGE_TOKENS,
        _ => sent.map_or(attachment.bytes, |sent| sent.bytes) / PDF_BYTES_PER_TOKEN,
    }
}

/// The attachments that a request copy must send as labels from now on, so that it
/// stays within the budget of every provider: at most [`MAX_IMAGE_BLOCKS`] images, at
/// most [`MAX_DATA_BYTES`] of base64, and attachments of at most
/// [`WINDOW_SHARE_PERCENT`] of `window` tokens. Empty when the copy is within each
/// limit.
///
/// Only parts with data count. A part that is over a limit by itself loses its data
/// first. When the copy is still over the limit, the oldest parts lose their data until
/// the copy is at half of that limit, so the cached prefix of the requests changes once
/// and not at each new image. The parts of the newest prompt go last. The ids come in
/// the order of the request.
pub fn fit<'a>(messages: impl IntoIterator<Item = &'a Message>, window: u64) -> Vec<AttachmentId> {
    let messages: Vec<&Message> = messages.into_iter().collect();
    let newest_prompt = messages.iter().rposition(|message| {
        message.role == Role::User
            && message.content.iter().any(|block| matches!(block, ContentBlock::Attachment(_)))
    });
    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    for (at, message) in messages.iter().enumerate() {
        for block in &message.content {
            let (in_prompt, list) = match block {
                ContentBlock::Attachment(part) => (true, std::slice::from_ref(part)),
                ContentBlock::ToolResult { attachments, .. } => (false, attachments.as_slice()),
                _ => continue,
            };
            for part in list {
                let Some(data) = &part.data else { continue };
                let image = matches!(part.attachment.kind, AttachmentKind::Image { .. });
                candidates.push(Candidate {
                    id: &part.attachment.id,
                    weights: [
                        u64::from(image),
                        data.len() as u64,
                        content_tokens(&part.attachment),
                    ],
                    newest: in_prompt && Some(at) == newest_prompt,
                    withheld: false,
                });
            }
        }
    }
    // NOTE: the order of removal: the oldest first, the parts of the newest prompt last.
    // A stable sort keeps the order of the request inside each group.
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by_key(|&index| candidates[index].newest);
    let limits = [
        MAX_IMAGE_BLOCKS as u64,
        MAX_DATA_BYTES,
        window.saturating_mul(WINDOW_SHARE_PERCENT) / 100,
    ];
    for (measure, limit) in limits.into_iter().enumerate() {
        let mut total: u64 = candidates
            .iter()
            .filter(|candidate| !candidate.withheld)
            .map(|candidate| candidate.weights[measure])
            .sum();
        if total <= limit {
            continue;
        }
        // NOTE: an older part cannot make room for a part that is over the limit alone.
        for candidate in &mut candidates {
            if !candidate.withheld && candidate.weights[measure] > limit {
                candidate.withheld = true;
                total -= candidate.weights[measure];
            }
        }
        if total <= limit {
            continue;
        }
        for &index in &order {
            if total <= limit / 2 {
                break;
            }
            let candidate = &mut candidates[index];
            let weight = candidate.weights[measure];
            if candidate.withheld || weight == 0 {
                continue;
            }
            candidate.withheld = true;
            total -= weight;
        }
    }
    candidates
        .into_iter()
        .filter(|candidate| candidate.withheld)
        .map(|candidate| candidate.id.clone())
        .collect()
}

/// One part with data, as [`fit`] weighs it.
struct Candidate<'a> {
    id: &'a AttachmentId,
    /// What the part counts against each limit, in the order of the limits: an image
    /// block, the bytes of its data, its tokens.
    weights: [u64; 3],
    newest: bool,
    withheld: bool,
}

#[cfg(test)]
mod tests;
