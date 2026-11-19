//! The intake: the checks of a finished upload and of an image of a tool, before a blob
//! is written. One path for every origin.
//!
//! A file that fails a check gets no blob: it goes to a model as its label with the
//! path, and [`Checked::note`] says why. A PNG or JPEG is decoded once, for the check
//! and the downscale; a GIF or WebP is walked, because efrd has no decoder for them. A
//! PDF goes as content only within the limits of decision D8.

use efr_media::{MAX_PIXELS, MediaError, Probe, decode, pdf_facts, probe, scale, strip};
use efr_protocol::{AttachmentKind, SentForm};
use efr_provider::{PDF_MAX_BYTES, PDF_MAX_PAGES};
use efr_stdx::media::{MediaType, sniff};

/// The most bytes of an image that a model gets: 7.5 MB, which is 10 MB of base64, the
/// limit of one image of the providers. A larger PNG or JPEG is made smaller, and a
/// larger GIF or WebP goes as a path.
pub(crate) const IMAGE_MAX_BYTES: usize = 7_500_000;

/// How much of the start of a file the text check reads, as `read_file` does.
const TEXT_PROBE_BYTES: usize = 8 * 1024;

/// What the intake found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Checked {
    /// The type from the first bytes: `text/plain` for a text file,
    /// `application/octet-stream` for each other file that is no image or PDF.
    pub(crate) media_type: &'static str,
    /// What kind of file it is. An image that fails its check is `other`: its header
    /// is not to be trusted.
    pub(crate) kind: AttachmentKind,
    /// The bytes that a model gets as content and what they are; `None` when the file
    /// goes as a path.
    pub(crate) sent: Option<(SentForm, Vec<u8>)>,
    /// Why a file of a kind that a model can read goes as a path.
    pub(crate) note: Option<Note>,
}

/// Why an image or a PDF goes as a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Note {
    /// The image is cut or broken.
    Broken,
    /// The image has more pixels than efrd decodes, or more bytes or a longer edge than
    /// a model takes, and efrd cannot make it smaller.
    TooLarge,
    /// The image is an animated WebP.
    Animated,
    /// The PDF has more bytes or pages than a model takes as content.
    PdfTooLarge,
    /// The PDF has a password.
    PdfPassword,
    /// efrd cannot count the pages of the PDF.
    PdfPages,
}

impl Note {
    /// The reason, for the debug line of a tool image. No label and no row shows it.
    pub(crate) fn text(self) -> &'static str {
        match self {
            Note::Broken => "as a path: the image is broken",
            Note::TooLarge => "as a path: the image is too large to send as content",
            Note::Animated => "as a path: the image is animated",
            Note::PdfTooLarge => "as a path: the PDF is too large to send as content",
            Note::PdfPassword => "as a path: the PDF has a password",
            Note::PdfPages => "as a path: efr cannot count the pages of this PDF",
        }
    }
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

/// A PNG or JPEG: the size from the header, one decode, then the downscale, or the strip
/// when the image fits as it is.
fn decoded(bytes: &[u8], media: MediaType, max_edge: u32) -> Checked {
    let header = match probe(bytes) {
        Ok(header) => header,
        Err(_) => return broken(media),
    };
    if header.pixels() > MAX_PIXELS {
        return too_large(media, header);
    }
    let image = match decode(bytes) {
        Ok(image) => image,
        Err(MediaError::TooManyPixels { .. } | MediaError::TooManyBytes { .. }) => {
            return too_large(media, header);
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
        Ok((_, sent, ..)) if sent.len() > IMAGE_MAX_BYTES => {
            Checked { media_type: media.as_str(), kind, sent: None, note: Some(Note::TooLarge) }
        }
        Ok((sent_media, sent, width, height)) => Checked {
            media_type: media.as_str(),
            kind,
            sent: Some((image_form(sent_media, &sent, width, height), sent)),
            note: None,
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
            return Checked { note: Some(Note::Animated), ..too_large(media, header) };
        }
        Err(_) => return broken(media),
    };
    if header.edge() > max_edge || stripped.len() > IMAGE_MAX_BYTES {
        return too_large(media, header);
    }
    let form = image_form(media, &stripped, header.width, header.height);
    Checked {
        media_type: media.as_str(),
        kind: AttachmentKind::Image { width: header.width, height: header.height },
        sent: Some((form, stripped)),
        note: None,
    }
}

/// A PDF: as content only with at most [`PDF_MAX_BYTES`], no password, and a known
/// count of at most [`PDF_MAX_PAGES`] pages.
fn pdf(bytes: &[u8]) -> Checked {
    let facts = pdf_facts(bytes);
    let note = if bytes.len() as u64 > PDF_MAX_BYTES {
        Some(Note::PdfTooLarge)
    } else if facts.encrypted {
        Some(Note::PdfPassword)
    } else {
        match facts.pages {
            None => Some(Note::PdfPages),
            Some(pages) if pages > PDF_MAX_PAGES => Some(Note::PdfTooLarge),
            Some(_) => None,
        }
    };
    let media_type = MediaType::Pdf.as_str();
    let sent = note.is_none().then(|| {
        let form = SentForm {
            media_type: media_type.to_owned(),
            bytes: bytes.len() as u64,
            width: None,
            height: None,
        };
        (form, bytes.to_vec())
    });
    Checked { media_type, kind: AttachmentKind::Pdf { pages: facts.pages }, sent, note }
}

/// True for valid UTF-8 with no NUL in the first [`TEXT_PROBE_BYTES`]: a text file.
fn is_text(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(TEXT_PROBE_BYTES)];
    let valid = match std::str::from_utf8(head) {
        Ok(_) => true,
        // NOTE: the cut can fall inside a character, which is no error.
        Err(error) => error.error_len().is_none(),
    };
    valid && !head.contains(&0)
}

/// A file that goes as a path with no note.
fn plain(media_type: &'static str, kind: AttachmentKind) -> Checked {
    Checked { media_type, kind, sent: None, note: None }
}

/// An image that failed its check.
fn broken(media: MediaType) -> Checked {
    Checked {
        media_type: media.as_str(),
        kind: AttachmentKind::Other,
        sent: None,
        note: Some(Note::Broken),
    }
}

/// A valid image that no model gets as content.
fn too_large(media: MediaType, header: Probe) -> Checked {
    Checked {
        media_type: media.as_str(),
        kind: AttachmentKind::Image { width: header.width, height: header.height },
        sent: None,
        note: Some(Note::TooLarge),
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
