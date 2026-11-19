//! Small images and parts of images for the tests of the crate.

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{DynamicImage, ImageEncoder as _, Rgb, RgbImage, Rgba, RgbaImage};

/// A PNG of `width` x `height` with a gradient, with alpha when `alpha`.
pub(crate) fn png(width: u32, height: u32, alpha: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let encoder = PngEncoder::new(&mut out);
    if alpha {
        let image = RgbaImage::from_fn(width, height, |x, y| Rgba([x as u8, y as u8, 0, 128]));
        encoder.write_image(&image, width, height, image::ExtendedColorType::Rgba8).unwrap();
    } else {
        encoder
            .write_image(&gradient(width, height), width, height, image::ExtendedColorType::Rgb8)
            .unwrap();
    }
    out
}

/// A PNG of `width` x `height` with an alpha channel whose values are all full, as many
/// screenshot tools write it.
pub(crate) fn png_opaque_alpha(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let image = RgbaImage::from_fn(width, height, |x, y| Rgba([x as u8, y as u8, 0, 255]));
    PngEncoder::new(&mut out)
        .write_image(&image, width, height, image::ExtendedColorType::Rgba8)
        .unwrap();
    out
}

/// The PNG colour type of RGBA.
pub(crate) const PNG_RGBA: u8 = 6;

/// A PNG whose header names `width` x `height` with `depth` bits for each channel of
/// the colour type `colour`, with the pixel data of a 1 x 1 image.
pub(crate) fn png_header(width: u32, height: u32, depth: u8, colour: u8) -> Vec<u8> {
    let mut bytes = png(1, 1, false);
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    bytes[24] = depth;
    bytes[25] = colour;
    // NOTE: the CRC of the IHDR chunk covers its type and its 13 bytes of data.
    let crc = crc32(&bytes[12..29]);
    bytes[29..33].copy_from_slice(&crc.to_be_bytes());
    bytes
}

/// A JPEG of `width` x `height` with a gradient.
pub(crate) fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(gradient(width, height))
        .write_with_encoder(JpegEncoder::new_with_quality(&mut out, 90))
        .unwrap();
    out
}

fn gradient(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| Rgb([x as u8, y as u8, 64]))
}

/// `jpeg` with a segment of `marker` and `payload` after its start mark.
pub(crate) fn with_jpeg_segment(jpeg: &[u8], marker: u8, payload: &[u8]) -> Vec<u8> {
    let length = u16::try_from(payload.len() + 2).unwrap();
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xff, marker]);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// `png` with a chunk of `kind` and `data` after its `IHDR` chunk, with a right CRC.
pub(crate) fn with_png_chunk(png: &[u8], kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    // NOTE: the signature of 8 bytes and the IHDR chunk of 25.
    let at = 33;
    let mut chunk = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
    chunk.extend_from_slice(kind);
    chunk.extend_from_slice(data);
    let crc = crc32(&chunk[4..]);
    chunk.extend_from_slice(&crc.to_be_bytes());
    let mut out = png[..at].to_vec();
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&png[at..]);
    out
}

/// The CRC-32 of PNG.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// An EXIF block with a GPS IFD pointer, as a camera writes it: `Exif\0\0`, a TIFF
/// header and one IFD with the orientation `orientation` and a GPS pointer.
pub(crate) fn exif(orientation: u16) -> Vec<u8> {
    let mut out = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x02".to_vec();
    out.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1]);
    out.extend_from_slice(&orientation.to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&[0x88, 0x25, 0, 4, 0, 0, 0, 1, 0, 0, 0, 0x26]);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(b"GPS 52N 13E");
    out
}

/// A GIF of `width` x `height` with the blocks `extensions` before its one image.
pub(crate) fn gif(width: u16, height: u16, extensions: &[&[u8]]) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    // NOTE: a global colour table of 2 colours.
    out.extend_from_slice(&[0x80, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff]);
    for extension in extensions {
        out.extend_from_slice(extension);
    }
    out.extend_from_slice(&[0x2c, 0, 0, 0, 0]);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&[0, 2, 2, 0x4c, 0x01, 0, 0x3b]);
    out
}

/// A GIF extension of `label` with `blocks` as its sub-blocks.
pub(crate) fn gif_extension(label: u8, blocks: &[&[u8]]) -> Vec<u8> {
    let mut out = vec![0x21, label];
    for block in blocks {
        out.push(u8::try_from(block.len()).unwrap());
        out.extend_from_slice(block);
    }
    out.push(0);
    out
}

/// A lossy WebP of `width` x `height`: one `VP8 ` chunk with its frame header.
pub(crate) fn webp_lossy(width: u16, height: u16) -> Vec<u8> {
    let mut data = vec![0x30, 0x01, 0x00, 0x9d, 0x01, 0x2a];
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    riff(&[(b"VP8 ", &data)])
}

/// A lossless WebP of `width` x `height`: one `VP8L` chunk with its header.
pub(crate) fn webp_lossless(width: u16, height: u16) -> Vec<u8> {
    riff(&[(b"VP8L", &vp8l(width, height))])
}

fn vp8l(width: u16, height: u16) -> Vec<u8> {
    let bits = u32::from(width - 1) | (u32::from(height - 1) << 14);
    let mut data = vec![0x2f];
    data.extend_from_slice(&bits.to_le_bytes());
    data
}

/// An extended WebP of `width` x `height`: a `VP8X` chunk with the flags of `chunks`,
/// the chunks, then a `VP8L` frame.
pub(crate) fn webp_extended(width: u16, height: u16, chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let flags = chunks.iter().fold(0_u8, |flags, (kind, _)| match *kind {
        b"EXIF" => flags | 0x08,
        b"XMP " => flags | 0x04,
        _ => flags,
    });
    let mut header = vec![flags, 0, 0, 0];
    header.extend_from_slice(&(u32::from(width) - 1).to_le_bytes()[..3]);
    header.extend_from_slice(&(u32::from(height) - 1).to_le_bytes()[..3]);
    let frame = vp8l(width, height);
    let mut all: Vec<(&[u8; 4], &[u8])> = vec![(b"VP8X", &header)];
    all.extend_from_slice(chunks);
    all.push((b"VP8L", &frame));
    riff(&all)
}

/// A RIFF WebP container of `chunks`.
pub(crate) fn riff(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    for (kind, data) in chunks {
        body.extend_from_slice(*kind);
        body.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        body.extend_from_slice(data);
        if data.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&body);
    out
}
