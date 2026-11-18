//! The chip row of the attachments of a prompt, and the row of the attachments that
//! efrd stops to send (TM-Attach 2 and 9).
//!
//! ```text
//! @ image shot.png 1920x1080 2.1 MB, pdf report.pdf 3.4 MB, text data.csv 48 KB
//! @ pdf manual.pdf 18 MB as a path
//! not sent: shot.png. The provider refused it; the model gets the path.
//! ```
//!
//! The chip row is an event row, as the `↪` and `⇄` rows are: the mark `@` in `accent`,
//! then one chip for each attachment: its kind, its name and its size in `muted`, an
//! image with its pixels before its size. The name is an OSC 8 `file://` link to the
//! copy of the file, in `link`; with no link it is `muted` too. A file that the model
//! gets as a path says `as a path` in `warning`. A chip never breaks: a row that has no
//! room for the next chip goes on 2 columns in, as every line of efr's own does.

use std::collections::HashMap;

use efr_protocol::{Attachment, AttachmentId, AttachmentKind, WithheldReason};
use efr_render::{RenderOptions, file_url_exact, text_width};
use efr_stdx::text::byte_size;

use super::route::Pieces;
use super::{Tone, columns, one_line, paint};

/// The mark at the start of the chip row: the sign that the user types.
const MARK: &str = "@";

/// Where a row of chips goes on after the first: under the first chip.
const GOES_ON: &str = "  ";

/// The words of a chip whose file the model gets as a path.
const AS_A_PATH: &str = "as a path";

/// The chip row of `attachments`, with its newline: `unread` names those that the
/// turn's model does not read as content. `links` is false where a `file://` link
/// cannot open the file, such as an `efr` that runs over SSH.
pub(crate) fn chip_rows(
    attachments: &[Attachment],
    unread: &[AttachmentId],
    links: bool,
    options: &RenderOptions,
) -> String {
    let links = links && options.is_terminal() && options.hyperlinks();
    let method = options.width_method();
    let width = columns(options).map(|(width, _)| width);
    let mut out = paint(MARK, Tone::Accent, options);
    let mut column = text_width(MARK, method);
    for (at, attachment) in attachments.iter().enumerate() {
        let last = at + 1 == attachments.len();
        let (painted, plain) = chip(attachment, unread.contains(&attachment.id), links, options);
        let (painted, chip_width) = if last {
            (painted, text_width(&plain, method))
        } else {
            (painted + &paint(",", Tone::Dim, options), text_width(&plain, method) + 1)
        };
        if at > 0 && width.is_some_and(|width| column + 1 + chip_width > width) {
            out.push('\n');
            out.push_str(GOES_ON);
            column = GOES_ON.len();
        } else {
            out.push(' ');
            column += 1;
        }
        out.push_str(&painted);
        column += chip_width;
    }
    out.push('\n');
    out
}

/// The chip of `attachment`, painted, and as plain text for its width.
fn chip(
    attachment: &Attachment,
    unread: bool,
    links: bool,
    options: &RenderOptions,
) -> (String, String) {
    let name = one_line(&attachment.name);
    let size = match attachment.kind {
        AttachmentKind::Image { width, height } => {
            format!("{width}x{height} {}", byte_size(attachment.bytes))
        }
        _ => byte_size(attachment.bytes),
    };
    let kind = kind_word(attachment.kind);
    let as_path = unread || goes_as_path(attachment);
    let link = attachment.copy.as_ref().filter(|_| links).and_then(|copy| copy.to_str());
    let shown_name = match link {
        Some(copy) => hyperlink(&file_url_exact(copy), &paint(&name, Tone::Link, options)),
        None => paint(&name, Tone::Dim, options),
    };
    let mut painted = format!(
        "{} {shown_name} {}",
        paint(kind, Tone::Dim, options),
        paint(&size, Tone::Dim, options)
    );
    let mut plain = format!("{kind} {name} {size}");
    if as_path {
        painted.push(' ');
        painted.push_str(&paint(AS_A_PATH, Tone::Attention, options));
        plain.push(' ');
        plain.push_str(AS_A_PATH);
    }
    (painted, plain)
}

/// The kind of a chip: `image`, `pdf`, `text`, else `file`.
fn kind_word(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image { .. } => "image",
        AttachmentKind::Pdf { .. } => "pdf",
        AttachmentKind::Text => "text",
        _ => "file",
    }
}

/// True for an image or a PDF that efrd sends no form of: the intake made no blob of
/// it, so every model gets its path.
fn goes_as_path(attachment: &Attachment) -> bool {
    matches!(attachment.kind, AttachmentKind::Image { .. } | AttachmentKind::Pdf { .. })
        && attachment.sent.is_none()
}

/// `painted` as an OSC 8 link to `url`, a target with no byte that ends the sequence.
fn hyperlink(url: &str, painted: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{painted}\x1b]8;;\x1b\\")
}

/// What a view or a transcript knows of the attachments of a conversation: the name
/// and the kind of each one that it saw, for the row of those that efrd stops to send.
#[derive(Debug, Default)]
pub(crate) struct Known(HashMap<AttachmentId, (String, AttachmentKind)>);

impl Known {
    /// Keeps the names and the kinds of `attachments`.
    pub(crate) fn add(&mut self, attachments: &[Attachment]) {
        for attachment in attachments {
            self.0.insert(attachment.id.clone(), (attachment.name.clone(), attachment.kind));
        }
    }

    /// The row of an `attachments_withheld` of `ids` for `reason`: `not sent: shot.png.
    /// The provider refused it; the model gets the path.` in `warning`, or `not sent from
    /// now on: 46 older images. The request had too many; the model gets the paths.` in
    /// `muted`. An attachment that this view did not see stands as its id.
    pub(crate) fn withheld(&self, ids: &[AttachmentId], reason: WithheldReason) -> Pieces {
        let many = ids.len() > 1;
        let (it, path) = if many { ("them", "paths") } else { ("it", "path") };
        match reason {
            WithheldReason::Budget => {
                let images = ids.iter().all(|id| {
                    self.0
                        .get(id)
                        .is_none_or(|(_, kind)| matches!(kind, AttachmentKind::Image { .. }))
                });
                let what = match (images, many) {
                    (true, true) => "images",
                    (true, false) => "image",
                    (false, true) => "files",
                    (false, false) => "file",
                };
                let text = format!(
                    "not sent from now on: {} older {what}. The request had too many; the model gets the {path}.",
                    ids.len()
                );
                vec![(text, Tone::Dim)]
            }
            WithheldReason::Refused => {
                let text = format!(
                    "not sent: {}. The provider refused {it}; the model gets the {path}.",
                    self.names(ids)
                );
                vec![(text, Tone::Attention)]
            }
            _ => {
                let text = format!(
                    "not sent from now on: {}; the model gets the {path}.",
                    self.names(ids)
                );
                vec![(text, Tone::Dim)]
            }
        }
    }

    /// The names of `ids`, safe to print, with `, ` between them.
    fn names(&self, ids: &[AttachmentId]) -> String {
        let names: Vec<String> = ids
            .iter()
            .map(|id| match self.0.get(id) {
                Some((name, _)) => one_line(name),
                None => id.to_string(),
            })
            .collect();
        names.join(", ")
    }
}

#[cfg(test)]
mod tests;
