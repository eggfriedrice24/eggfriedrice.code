//! The intake: the checks of a finished upload and of an image of a tool, before a blob
//! is written. One path for every origin.
//!
//! A file that fails a check gets no blob: it goes to a model as its label with the
//! path, and [`Checked::reason`] says why. A PNG or JPEG is decoded once, for the check
//! and the downscale; a GIF or WebP is walked, because efrd has no decoder for them. A
//! PDF goes as content only within the limits of ADR 0026 ([`PDF_MAX_BYTES`],
//! [`PDF_MAX_PAGES`]).

use efr_media::{MediaError, decode, pdf_facts, probe, scale, strip};
use efr_protocol::{AttachmentKind, SentForm};
use efr_provider::{PDF_MAX_BYTES, PDF_MAX_PAGES};
use efr_stdx::media::{MediaType, sniff};

/// The most bytes of an image that a model gets: 7.5 MB, which is 10 MB of base64, the
/// limit of one image of the providers. A larger PNG or JPEG is made smaller, and a
/// larger GIF or WebP goes as a path.
pub(crate) const IMAGE_MAX_BYTES: usize = 7_500_000;

/// The head of a file that [`check`] needs when [`needs_whole`] is false: the text
/// check reads this much, as `read_file` does.
pub(crate) const HEAD_BYTES: usize = 8 * 1024;

/// The reason of an image that is cut or broken.
pub(crate) const BROKEN: &str = "the image is broken";

/// The reason of an image that has more pixels than efrd decodes, or more bytes or a
/// longer edge than a model takes, and that efrd cannot make smaller.
pub(crate) const TOO_LARGE: &str = "the image is too large to send as content";

/// The reason of an animated WebP.
pub(crate) const ANIMATED: &str = "the image is animated";

/// The reason of a PDF with more bytes or pages than a model takes as content.
pub(crate) const PDF_TOO_LARGE: &str = "the PDF is too large to send as content";

/// The reason of a PDF with a password.
pub(crate) const PDF_PASSWORD: &str = "the PDF has a password";

/// The reason of a PDF whose pages efrd cannot count.
pub(crate) const PDF_PAGES: &str = "efr cannot count the pages of this PDF";

/// What the intake found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Checked {
    /// The type from the first bytes: `text/plain` for a text file,
    /// `application/octet-stream` for each other file that is no image or PDF.
    pub(crate) media_type: &'static str,
    /// What kind of file it is. An image that fails its check is `other`: its header
    /// is not to be trusted.
    pub(crate) kind: AttachmentKind,
    /// What a model gets as content and where its bytes are; `None` when the file goes
    /// as a path.
    pub(crate) sent: Option<(SentForm, Sent)>,
    /// Why a file of a kind that a model can read goes as a path, for a debug line.
    pub(crate) reason: Option<&'static str>,
}

/// The bytes that a model gets of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sent {
    /// The file as it is: a PDF.
    Original,
    /// The image made ready for a model: smaller, upright or with no metadata.
    Prepared(Vec<u8>),
}

impl Sent {
    /// The bytes that a model gets of the file `original`.
    pub(crate) fn bytes<'a>(&'a self, original: &'a [u8]) -> &'a [u8] {
        match self {
            Sent::Original => original,
            Sent::Prepared(bytes) => bytes,
        }
    }
}

/// True when [`check`] needs the whole file of the head `head`: an image or a PDF. For
/// each other file it needs the first [`HEAD_BYTES`] only.
pub(crate) fn needs_whole(head: &[u8]) -> bool {
    sniff(head).is_some()
}

/// The checks of the file `bytes`, with the image of a model made from it: its longer
/// edge at most `max_edge` pixels, and no metadata. It blocks; one decode at a time is
/// the caller's rule.
pub(crate) fn check(bytes: &[u8], max_edge: u32) -> Checked {
    match sniff(bytes) {
        Some(media @ (MediaType::Png | MediaType::Jpeg)) => decoded(bytes, media, max_edge),
        Some(media @ (MediaType::Gif | MediaType::Webp)) => walked(bytes, media, max_edge),
        Some(MediaType::Pdf) => pdf(bytes),
        _ if is_text(bytes) => plain("text/plain", AttachmentKind::Text),
        _ => plain("application/octet-stream", AttachmentKind::Other),
    }
}

/// A PNG or JPEG: one decode, which checks the size from the header first, then the
/// downscale, or the strip when the image fits as it is.
fn decoded(bytes: &[u8], media: MediaType, max_edge: u32) -> Checked {
    let image = match decode(bytes) {
        Ok(image) => image,
        Err(MediaError::TooManyPixels { width, height }) => {
            return too_large(media, width, height);
        }
        Err(MediaError::TooManyBytes { .. }) => {
            return probe(bytes).map_or_else(
                |_| broken(media),
                |header| too_large(media, header.width, header.height),
            );
        }
        Err(_) => return broken(media),
    };
    let (width, height) = (image.width(), image.height());
    let kind = AttachmentKind::Image { width, height };
    // NOTE: a turned image needs its pixels turned, because the strip drops the EXIF
    // block that turns it.
    let fits = image.edge() <= max_edge && bytes.len() <= IMAGE_MAX_BYTES && !image.turned();
    let prepared = if fits {
        strip(bytes).map(|stripped| (media, stripped, width, height))
    } else {
        scale(image, max_edge)
            .map(|prepared| (prepared.media, prepared.bytes, prepared.width, prepared.height))
    };
    match prepared {
        Ok((_, sent, ..)) if sent.len() > IMAGE_MAX_BYTES => too_large(media, width, height),
        Ok((sent_media, sent, width, height)) => Checked {
            media_type: media.as_str(),
            kind,
            sent: Some((image_form(sent_media, &sent, width, height), Sent::Prepared(sent))),
            reason: None,
        },
        Err(_) => broken(media),
    }
}

/// A GIF or WebP: the walk of its container is the check and the strip. One that is too
/// large cannot be made smaller.
fn walked(bytes: &[u8], media: MediaType, max_edge: u32) -> Checked {
    let Ok(header) = probe(bytes) else { return broken(media) };
    let stripped = match strip(bytes) {
        Ok(stripped) => stripped,
        Err(MediaError::Animated) => {
            return Checked {
                reason: Some(ANIMATED),
                ..too_large(media, header.width, header.height)
            };
        }
        Err(_) => return broken(media),
    };
    if header.edge() > max_edge || stripped.len() > IMAGE_MAX_BYTES {
        return too_large(media, header.width, header.height);
    }
    let form = image_form(media, &stripped, header.width, header.height);
    Checked {
        media_type: media.as_str(),
        kind: AttachmentKind::Image { width: header.width, height: header.height },
        sent: Some((form, Sent::Prepared(stripped))),
        reason: None,
    }
}

/// A PDF: as content only with at most [`PDF_MAX_BYTES`], no password, and a known
/// count of at most [`PDF_MAX_PAGES`] pages.
fn pdf(bytes: &[u8]) -> Checked {
    let facts = pdf_facts(bytes);
    let reason = if bytes.len() as u64 > PDF_MAX_BYTES {
        Some(PDF_TOO_LARGE)
    } else if facts.encrypted {
        Some(PDF_PASSWORD)
    } else {
        match facts.pages {
            None => Some(PDF_PAGES),
            Some(pages) if pages > PDF_MAX_PAGES => Some(PDF_TOO_LARGE),
            Some(_) => None,
        }
    };
    let media_type = MediaType::Pdf.as_str();
    let sent = reason.is_none().then(|| {
        let form = SentForm {
            media_type: media_type.to_owned(),
            bytes: bytes.len() as u64,
            width: None,
            height: None,
        };
        (form, Sent::Original)
    });
    Checked { media_type, kind: AttachmentKind::Pdf { pages: facts.pages }, sent, reason }
}

/// True for valid UTF-8 with no NUL in the first [`HEAD_BYTES`]: a text file.
fn is_text(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(HEAD_BYTES)];
    let valid = match std::str::from_utf8(head) {
        Ok(_) => true,
        // NOTE: the cut can fall inside a character, which is no error.
        Err(error) => error.error_len().is_none(),
    };
    valid && !head.contains(&0)
}

/// A file that goes as a path with no reason.
fn plain(media_type: &'static str, kind: AttachmentKind) -> Checked {
    Checked { media_type, kind, sent: None, reason: None }
}

/// An image that failed its check.
fn broken(media: MediaType) -> Checked {
    Checked {
        media_type: media.as_str(),
        kind: AttachmentKind::Other,
        sent: None,
        reason: Some(BROKEN),
    }
}

/// A valid image of `width` x `height` pixels that no model gets as content.
fn too_large(media: MediaType, width: u32, height: u32) -> Checked {
    Checked {
        media_type: media.as_str(),
        kind: AttachmentKind::Image { width, height },
        sent: None,
        reason: Some(TOO_LARGE),
    }
}

/// What a model gets of an image of `media` with `bytes`.
fn image_form(media: MediaType, bytes: &[u8], width: u32, height: u32) -> SentForm {
    SentForm {
        media_type: media.as_str().to_owned(),
        bytes: bytes.len() as u64,
        width: Some(width),
        height: Some(height),
    }
}

#[cfg(test)]
mod tests;
