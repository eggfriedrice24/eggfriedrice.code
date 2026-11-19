//! The size of an image from its header, with no decode.

use efr_stdx::media::{MediaType, sniff};

use crate::{MediaError, jpeg};

/// The most pixels of an image that efr decodes: 50 megapixels, so that a 48 MP photo
/// of a current phone camera decodes. [`MAX_DECODE_BYTES`](crate::MAX_DECODE_BYTES)
/// limits the bytes of the decoded image.
pub const MAX_PIXELS: u64 = 50_000_000;

/// What the header of an image says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    /// The type, from the first bytes.
    pub media: MediaType,
    /// The width in pixels, as the file stores it.
    pub width: u32,
    /// The height in pixels, as the file stores it.
    pub height: u32,
}

impl Probe {
    /// The count of pixels.
    pub fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// The longer edge in pixels.
    pub fn edge(&self) -> u32 {
        self.width.max(self.height)
    }
}

/// The type and the size of the PNG, JPEG, GIF or WebP image `bytes`, from its header.
///
/// # Errors
///
/// [`MediaError::WrongType`] for each other file, [`MediaError::Broken`] when the header
/// is cut or names a size of 0.
pub fn probe(bytes: &[u8]) -> Result<Probe, MediaError> {
    let media = sniff(bytes)
        .filter(|media| media.is_image())
        .ok_or(MediaError::WrongType { expected: "PNG, JPEG, GIF or WebP image" })?;
    let size = match media {
        MediaType::Png => png(bytes),
        MediaType::Jpeg => jpeg(bytes),
        MediaType::Gif => gif(bytes),
        MediaType::Webp => webp(bytes),
        _ => None,
    };
    match size {
        Some((width, height)) if width > 0 && height > 0 => Ok(Probe { media, width, height }),
        _ => Err(MediaError::broken(media, 0)),
    }
}

/// The size in the `IHDR` chunk, the first chunk after the signature.
fn png(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((be32(bytes, 16)?, be32(bytes, 20)?))
}

/// The size in the first start-of-frame segment: the precision, the height, the width.
fn jpeg(bytes: &[u8]) -> Option<(u32, u32)> {
    let frame = jpeg::segments(bytes).map_while(Result::ok).find(|segment| segment.is_frame())?;
    let height = be16(bytes, frame.start + 5)?;
    let width = be16(bytes, frame.start + 7)?;
    Some((u32::from(width), u32::from(height)))
}

/// The size of the logical screen.
fn gif(bytes: &[u8]) -> Option<(u32, u32)> {
    Some((u32::from(le16(bytes, 6)?), u32::from(le16(bytes, 8)?)))
}

/// The size in the first chunk: the canvas of `VP8X`, or the frame of `VP8 ` or `VP8L`.
fn webp(bytes: &[u8]) -> Option<(u32, u32)> {
    let data = 20;
    match bytes.get(12..16)? {
        b"VP8X" => Some((le24(bytes, data + 4)? + 1, le24(bytes, data + 7)? + 1)),
        b"VP8 " => {
            // NOTE: a frame tag of 3 bytes, the start code 9d 01 2a, then the two sizes
            // with a scale in their top 2 bits.
            if bytes.get(data + 3..data + 6)? != b"\x9d\x01\x2a" {
                return None;
            }
            let width = le16(bytes, data + 6)? & 0x3fff;
            let height = le16(bytes, data + 8)? & 0x3fff;
            Some((u32::from(width), u32::from(height)))
        }
        b"VP8L" => {
            // NOTE: the signature 0x2f, then the width and the height less 1, 14 bits each.
            if *bytes.get(data)? != 0x2f {
                return None;
            }
            let bits = le32(bytes, data + 1)?;
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        _ => None,
    }
}

/// The big-endian `u16` at `at`.
pub(crate) fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// The big-endian `u32` at `at`.
pub(crate) fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The little-endian `u16` at `at`.
fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// The little-endian 24-bit number at `at`.
fn le24(bytes: &[u8], at: usize) -> Option<u32> {
    let &[a, b, c] = bytes.get(at..at + 3)? else { return None };
    Some(u32::from_le_bytes([a, b, c, 0]))
}

/// The little-endian `u32` at `at`.
pub(crate) fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests;
