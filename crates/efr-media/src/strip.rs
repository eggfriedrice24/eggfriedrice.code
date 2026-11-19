//! The strip of EXIF, GPS, XMP, C2PA and comment metadata from an image, with no decode:
//! a walk of the parts of its container that copies only the parts that a viewer needs
//! to show the first image.
//!
//! The walk is also the check of a GIF or WebP file, which efrd cannot decode: each part
//! must have a size that fits in the file. The pixel data stays as it is.

use efr_stdx::media::MediaType;

use crate::probe::{WEBP_FIRST_CHUNK, WEBP_FIRST_DATA, be32, image_type, le32};
use crate::{MediaError, jpeg};

/// The ancillary PNG chunks that the strip keeps: transparency, colour and pixel size.
/// Each critical chunk stays too.
const PNG_KEPT: [&[u8; 4]; 11] = [
    b"tRNS", b"gAMA", b"cHRM", b"sRGB", b"iCCP", b"sBIT", b"cICP", b"mDCV", b"cLLI", b"pHYs",
    b"bKGD",
];

/// The WebP chunks that the strip keeps: the header, the frame, its alpha and the colour
/// profile.
const WEBP_KEPT: [&[u8; 4]; 5] = [b"VP8X", b"VP8 ", b"VP8L", b"ALPH", b"ICCP"];

/// The flag of `VP8X` for an animation.
const WEBP_ANIMATION: u8 = 0x02;

/// The JPEG markers of APP0 (JFIF), APP2 (an ICC profile among others), APP14 (the
/// colour transform of Adobe), and the range of every APP segment.
const APP0: u8 = 0xe0;
const APP2: u8 = 0xe2;
const APP14: u8 = 0xee;
const APP_ALL: std::ops::RangeInclusive<u8> = 0xe0..=0xef;

/// The JPEG marker of a comment.
const COM: u8 = 0xfe;

/// The first bytes of the GIF blocks: an extension, an image and the trailer.
const GIF_EXTENSION: u8 = 0x21;
const GIF_IMAGE: u8 = 0x2c;
const GIF_TRAILER: u8 = 0x3b;

/// The label of the GIF graphic control extension, which holds the transparent colour
/// of the next image.
const GIF_GRAPHIC_CONTROL: u8 = 0xf9;

/// `bytes` with only the parts that show its first image, for a PNG, JPEG, GIF or WebP
/// image: no EXIF (and so no GPS tag), XMP, IPTC, C2PA, comment or text metadata, and
/// no data after the image.
///
/// A JPEG keeps its segments but COM and the APP segments, of which only APP0 (JFIF),
/// APP2 with an ICC profile and APP14 (Adobe colour) stay, and ends at its first end
/// mark. A PNG keeps its critical chunks and its ancillary chunks of transparency,
/// colour and pixel size, so an animated PNG shows its default image. A WebP keeps
/// `VP8X`, `VP8 `, `VP8L`, `ALPH` and `ICCP`, and loses the EXIF and XMP flags of
/// `VP8X`. A GIF keeps its first image with its graphic control extension (the
/// transparency) and loses each other block. The orientation of an EXIF block goes
/// too: the caller turns the pixels of an image that needs it before the strip.
///
/// # Errors
///
/// [`MediaError::WrongType`] for each other file, [`MediaError::Animated`] for an
/// animated WebP, whose frames cannot be cut with no decoder, and
/// [`MediaError::Broken`] when a part does not fit in the file or the file ends before
/// its end mark.
pub fn strip(bytes: &[u8]) -> Result<Vec<u8>, MediaError> {
    let media = image_type(bytes)?;
    if media == MediaType::Webp
        && bytes.get(WEBP_FIRST_CHUNK) == Some(b"VP8X")
        && bytes.get(WEBP_FIRST_DATA).is_some_and(|flags| flags & WEBP_ANIMATION != 0)
    {
        return Err(MediaError::Animated);
    }
    let mut out = Vec::with_capacity(bytes.len());
    let walked = match media {
        MediaType::Png => png(bytes, &mut out),
        MediaType::Jpeg => jpeg(bytes, &mut out),
        MediaType::Gif => gif(bytes, &mut out),
        MediaType::Webp => webp(bytes, &mut out),
        _ => Err(0),
    };
    walked.map(|()| out).map_err(|at| MediaError::broken(media, at))
}

/// The PNG signature and its chunks to `IEND`. `Err` holds the offset of the chunk that
/// does not fit.
fn png(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), usize> {
    const SIGNATURE: usize = 8;
    out.extend_from_slice(bytes.get(..SIGNATURE).ok_or(0_usize)?);
    let mut at = SIGNATURE;
    loop {
        let length = be32(bytes, at).ok_or(at)?;
        // NOTE: the length, the type and the CRC around the data.
        let end = usize::try_from(length).ok().and_then(|length| at.checked_add(length + 12));
        let chunk = end.and_then(|end| bytes.get(at..end)).ok_or(at)?;
        let kind = chunk.get(4..8).ok_or(at)?;
        // NOTE: an upper-case first letter marks a critical chunk.
        if kind[0].is_ascii_uppercase() || PNG_KEPT.iter().any(|kept| kind == *kept) {
            out.extend_from_slice(chunk);
        }
        if kind == b"IEND" {
            return Ok(());
        }
        at += chunk.len();
    }
}

/// The JPEG segments to the first end mark, with no metadata segment. The bytes after
/// the end mark, such as the second image of an MPF file or the video of a motion photo,
/// go. `Err` holds the offset of the segment that does not fit.
fn jpeg(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), usize> {
    out.extend_from_slice(bytes.get(..2).ok_or(0_usize)?);
    for segment in jpeg::whole(bytes) {
        let segment = segment?;
        let part = &bytes[segment.start..segment.end];
        let kept = match segment.marker {
            APP0 | APP14 => true,
            APP2 => part.get(4..).is_some_and(|data| data.starts_with(b"ICC_PROFILE\0")),
            COM => false,
            marker if APP_ALL.contains(&marker) => false,
            _ => true,
        };
        if kept {
            out.extend_from_slice(part);
        }
        if segment.marker == jpeg::EOI {
            return Ok(());
        }
    }
    Err(bytes.len())
}

/// The GIF header, the screen and its colour table, then the first image with its
/// graphic control extension and the trailer. `Err` holds the offset of the block that
/// does not fit.
fn gif(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), usize> {
    const SCREEN: usize = 13;
    let flags = *bytes.get(10).ok_or(0_usize)?;
    let head = SCREEN + colour_table(flags);
    out.extend_from_slice(bytes.get(..head).ok_or(0_usize)?);
    let mut at = head;
    loop {
        let start = at;
        match *bytes.get(at).ok_or(at)? {
            GIF_TRAILER => {
                out.push(GIF_TRAILER);
                return Ok(());
            }
            GIF_EXTENSION => {
                // NOTE: comments, text and application data go.
                let label = *bytes.get(at + 1).ok_or(at)?;
                let end = sub_blocks(bytes, at + 2).ok_or(start)?;
                if label == GIF_GRAPHIC_CONTROL {
                    out.extend_from_slice(&bytes[start..end]);
                }
                at = end;
            }
            GIF_IMAGE => {
                // NOTE: the descriptor of 10 bytes, a local colour table, the minimum code
                // size, then the data as sub-blocks. A model sees only the first image,
                // so each later frame goes.
                let flags = *bytes.get(at + 9).ok_or(at)?;
                let end = sub_blocks(bytes, at + 10 + colour_table(flags) + 1).ok_or(start)?;
                out.extend_from_slice(&bytes[start..end]);
                out.push(GIF_TRAILER);
                return Ok(());
            }
            _ => return Err(at),
        }
    }
}

/// The bytes of the colour table that the `flags` of a GIF screen or image name.
fn colour_table(flags: u8) -> usize {
    if flags & 0x80 == 0 { 0 } else { 3 << ((flags & 0x07) + 1) }
}

/// The offset after the sub-blocks of a GIF that start at `at`: each a size byte and its
/// data, to a size of 0. `None` when they do not fit in the file.
fn sub_blocks(bytes: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let size = usize::from(*bytes.get(at)?);
        at += 1;
        if size == 0 {
            return Some(at);
        }
        at += size;
        if at > bytes.len() {
            return None;
        }
    }
}

/// The RIFF header and each chunk of a WebP file, with a new RIFF size. `Err` holds the
/// offset of the chunk that does not fit.
fn webp(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), usize> {
    // NOTE: the flags of VP8X for an EXIF and an XMP chunk.
    const FLAG_EXIF: u8 = 0x08;
    const FLAG_XMP: u8 = 0x04;
    const HEAD: usize = 12;
    let riff = usize::try_from(le32(bytes, 4).ok_or(0_usize)?).map_err(|_| 0_usize)?;
    let end =
        riff.checked_add(8).filter(|&end| end <= bytes.len() && end >= HEAD).ok_or(4_usize)?;
    out.extend_from_slice(&bytes[..HEAD]);
    let mut at = HEAD;
    while at < end {
        let size = usize::try_from(le32(bytes, at + 4).ok_or(at)?).map_err(|_| at)?;
        // NOTE: a chunk of an odd size has a pad byte.
        let next = (at + 8).checked_add(size + (size & 1)).filter(|&next| next <= end).ok_or(at)?;
        match &bytes[at..at + 4] {
            b"VP8X" if size > 0 => {
                out.extend_from_slice(&bytes[at..at + 8]);
                out.push(bytes[at + 8] & !(FLAG_EXIF | FLAG_XMP));
                out.extend_from_slice(&bytes[at + 9..next]);
            }
            kind if WEBP_KEPT.iter().any(|kept| kind == *kept) => {
                out.extend_from_slice(&bytes[at..next]);
            }
            _ => {}
        }
        at = next;
    }
    let size = u32::try_from(out.len() - 8).map_err(|_| 4_usize)?;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    Ok(())
}

#[cfg(test)]
mod tests;
