//! The type of a file from its first bytes: the one table of magic bytes that
//! `read_file`, the image library of efrd and the intake of an attachment share.
//!
//! A client never names the type of a file that efr sends to a model; efr reads it
//! from the bytes with [`sniff`].

/// A file type that efr can send to a model as content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MediaType {
    /// A PNG image.
    Png,
    /// A JPEG image.
    Jpeg,
    /// A GIF image.
    Gif,
    /// A WebP image.
    Webp,
    /// A PDF document.
    Pdf,
}

impl MediaType {
    /// The media type as it goes on the wire, such as `image/png`.
    pub const fn as_str(self) -> &'static str {
        match self {
            MediaType::Png => "image/png",
            MediaType::Jpeg => "image/jpeg",
            MediaType::Gif => "image/gif",
            MediaType::Webp => "image/webp",
            MediaType::Pdf => "application/pdf",
        }
    }

    /// True for the four image types.
    pub const fn is_image(self) -> bool {
        !matches!(self, MediaType::Pdf)
    }
}

/// The longest head that [`sniff`] reads. A caller that reads the head of a file
/// reads this many bytes, or the whole file when it is shorter.
pub const SNIFF_BYTES: usize = 16;

/// The type of a file whose first bytes are `head`, or `None` for every other file,
/// a text file too. `head` can be longer or shorter than [`SNIFF_BYTES`]; a head
/// that is too short for a signature does not match it.
pub fn sniff(head: &[u8]) -> Option<MediaType> {
    // NOTE: a WebP file is a RIFF container: `RIFF`, the size in 4 bytes, then `WEBP`.
    if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP") {
        return Some(MediaType::Webp);
    }
    MAGIC.iter().find(|(magic, _)| head.starts_with(magic)).map(|(_, media)| *media)
}

/// The signatures at the start of each file type but WebP, which [`sniff`] checks on
/// its own.
const MAGIC: &[(&[u8], MediaType)] = &[
    (b"\x89PNG\r\n\x1a\n", MediaType::Png),
    (b"\xff\xd8\xff", MediaType::Jpeg),
    (b"GIF87a", MediaType::Gif),
    (b"GIF89a", MediaType::Gif),
    (b"%PDF-", MediaType::Pdf),
];

#[cfg(test)]
mod tests;
