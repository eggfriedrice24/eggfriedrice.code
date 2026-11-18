//! The strip of EXIF, GPS, XMP and comment metadata from an image, with no decode: a
//! walk of the parts of its container that copies each part but the metadata.
//!
//! The walk is also the check of a GIF or WebP file, which efrd cannot decode: each part
//! must have a size that fits in the file. The pixel data stays as it is.

use efr_stdx::media::{MediaType, sniff};

use crate::probe::{be32, le32};
use crate::{MediaError, jpeg};

/// `bytes` with no EXIF (and so no GPS tag), XMP, comment or text metadata, for a PNG,
/// JPEG, GIF or WebP image.
///
/// A JPEG loses its APP1 (EXIF, XMP), APP13 (IPTC) and COM segments, a PNG its `eXIf`,
/// `tEXt`, `zTXt` and `iTXt` chunks, a WebP its `EXIF` and `XMP ` chunks and their two
/// flags in `VP8X`, a GIF its comments and each application extension but the loop
/// count (`NETSCAPE2.0`, `ANIMEXTS1.0`). The orientation of an EXIF block goes too: the
/// caller turns the pixels of an image that needs it before the strip.
///
/// # Errors
///
/// [`MediaError::WrongType`] for each other file, [`MediaError::Broken`] when a part does
/// not fit in the file or the file ends before its end mark.
pub fn strip(bytes: &[u8]) -> Result<Vec<u8>, MediaError> {
    let media = sniff(bytes)
        .filter(|media| media.is_image())
        .ok_or(MediaError::WrongType { expected: "PNG, JPEG, GIF or WebP image" })?;
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
        if !matches!(kind, b"eXIf" | b"tEXt" | b"zTXt" | b"iTXt") {
            out.extend_from_slice(chunk);
        }
        if kind == b"IEND" {
            return Ok(());
        }
        at += chunk.len();
    }
}

/// The JPEG segments to the start of the image data, then the rest as it is. `Err`
/// holds the offset of the segment that does not fit.
fn jpeg(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), usize> {
    out.extend_from_slice(bytes.get(..2).ok_or(0_usize)?);
    for segment in jpeg::segments(bytes) {
        let segment = segment?;
        if segment.marker == jpeg::SOS {
            // NOTE: the first scan: the image data and each later segment go as they
            // are; each writer puts the metadata before it.
            out.extend_from_slice(&bytes[segment.start..]);
            return Ok(());
        }
        // NOTE: APP1 holds EXIF and XMP, APP13 the IPTC data of Photoshop, COM a comment.
        if !matches!(segment.marker, 0xe1 | 0xed | 0xfe) {
            out.extend_from_slice(&bytes[segment.start..segment.end]);
        }
    }
    Err(bytes.len())
}

/// The GIF header, the screen and its colour table, then each block to the trailer.
/// `Err` holds the offset of the block that does not fit.
fn gif(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), usize> {
    const SCREEN: usize = 13;
    let flags = *bytes.get(10).ok_or(0_usize)?;
    let head = SCREEN + colour_table(flags);
    out.extend_from_slice(bytes.get(..head).ok_or(0_usize)?);
    let mut at = head;
    loop {
        let start = at;
        match *bytes.get(at).ok_or(at)? {
            0x3b => {
                out.push(0x3b);
                return Ok(());
            }
            0x21 => {
                let label = *bytes.get(at + 1).ok_or(at)?;
                let end = sub_blocks(bytes, at + 2).ok_or(start)?;
                let keep = match label {
                    0xfe => false,
                    // NOTE: the first sub-block of an application extension is its id.
                    0xff => {
                        matches!(bytes.get(at + 3..at + 14), Some(b"NETSCAPE2.0" | b"ANIMEXTS1.0"))
                    }
                    _ => true,
                };
                if keep {
                    out.extend_from_slice(&bytes[start..end]);
                }
                at = end;
            }
            0x2c => {
                // NOTE: the descriptor of 10 bytes, a local colour table, the minimum code
                // size, then the data as sub-blocks.
                let flags = *bytes.get(at + 9).ok_or(at)?;
                let end = sub_blocks(bytes, at + 10 + colour_table(flags) + 1).ok_or(start)?;
                out.extend_from_slice(&bytes[start..end]);
                at = end;
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
            b"EXIF" | b"XMP " => {}
            b"VP8X" if size > 0 => {
                out.extend_from_slice(&bytes[at..at + 8]);
                out.push(bytes[at + 8] & !(FLAG_EXIF | FLAG_XMP));
                out.extend_from_slice(&bytes[at + 9..next]);
            }
            _ => out.extend_from_slice(&bytes[at..next]),
        }
        at = next;
    }
    let size = u32::try_from(out.len() - 8).map_err(|_| 4_usize)?;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    Ok(())
}

#[cfg(test)]
mod tests;
