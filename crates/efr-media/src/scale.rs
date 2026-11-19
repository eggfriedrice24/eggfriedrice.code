//! The decode of a PNG or JPEG image and its downscale for a model.

use std::io::Cursor;

use efr_stdx::media::MediaType;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{ColorType, DynamicImage, ImageDecoder as _, ImageFormat, ImageReader};

use crate::probe::{MAX_PIXELS, probe};
use crate::{MediaError, jpeg};

/// The quality of the JPEG that [`scale`] writes.
pub const JPEG_QUALITY: u8 = 85;

/// A PNG or JPEG image after one decode, turned as its EXIF orientation says.
#[derive(Debug)]
pub struct Decoded {
    image: DynamicImage,
    turned: bool,
}

impl Decoded {
    /// The width in pixels, after the turn.
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    /// The height in pixels, after the turn.
    pub fn height(&self) -> u32 {
        self.image.height()
    }

    /// The longer edge in pixels.
    pub fn edge(&self) -> u32 {
        self.width().max(self.height())
    }

    /// True when the EXIF orientation turned or flipped the pixels: the file then
    /// shows right only with its EXIF block, which [`strip`](crate::strip) removes, so
    /// a model must get the image of [`scale`].
    pub fn turned(&self) -> bool {
        self.turned
    }
}

/// An image that [`scale`] wrote for a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The type of `bytes`: PNG for an image with alpha, else JPEG.
    pub media: MediaType,
    /// The encoded image, with no metadata.
    pub bytes: Vec<u8>,
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
}

/// The PNG or JPEG image `bytes`, decoded once: the check that it is whole, and the
/// source of [`scale`].
///
/// # Errors
///
/// [`MediaError::WrongType`] for a GIF, a WebP and each other file;
/// [`MediaError::TooManyPixels`] over [`MAX_PIXELS`], before any decode;
/// [`MediaError::Broken`] for a JPEG with no end mark after its image data, and
/// [`MediaError::Codec`] for each image that the library cannot decode.
pub fn decode(bytes: &[u8]) -> Result<Decoded, MediaError> {
    let header = probe(bytes)?;
    let format = match header.media {
        MediaType::Png => ImageFormat::Png,
        MediaType::Jpeg => ImageFormat::Jpeg,
        _ => return Err(MediaError::WrongType { expected: "PNG or JPEG image" }),
    };
    if header.pixels() > MAX_PIXELS {
        return Err(MediaError::TooManyPixels { width: header.width, height: header.height });
    }
    if format == ImageFormat::Jpeg && !jpeg_ends(bytes) {
        return Err(MediaError::broken(header.media, bytes.len()));
    }
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .map_err(MediaError::Codec)?;
    let orientation = decoder.orientation().map_err(MediaError::Codec)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(MediaError::Codec)?;
    let turned = orientation != Orientation::NoTransforms;
    image.apply_orientation(orientation);
    Ok(Decoded { image, turned })
}

/// True when the JPEG `bytes` has an end mark after the start of its image data. The
/// decoder of the library fills a cut scan with grey and gives no error, so this is
/// the check of a cut JPEG. An end mark cannot occur inside the data, because each
/// `0xff` byte there is followed by `0x00` or a restart marker.
fn jpeg_ends(bytes: &[u8]) -> bool {
    let scan = jpeg::segments(bytes).find_map(|segment| match segment {
        Ok(segment) if segment.marker == jpeg::SOS => Some(Some(segment.end)),
        Ok(_) => None,
        Err(_) => Some(None),
    });
    let Some(Some(data)) = scan else { return false };
    bytes[data..].windows(2).any(|pair| pair == [0xff, jpeg::EOI])
}

/// `decoded` as a model gets it: with its longer edge at most `max_edge` pixels (a
/// triangle filter keeps the aspect ratio), and encoded with no metadata, as a PNG when
/// it has alpha, else as a JPEG of quality [`JPEG_QUALITY`].
///
/// # Errors
///
/// [`MediaError::Codec`] when the encoder fails.
pub fn scale(decoded: &Decoded, max_edge: u32) -> Result<Prepared, MediaError> {
    let resized = (decoded.edge() > max_edge)
        .then(|| decoded.image.resize(max_edge, max_edge, FilterType::Triangle));
    let image = resized.as_ref().unwrap_or(&decoded.image);
    let media = if image.color().has_alpha() { MediaType::Png } else { MediaType::Jpeg };
    let bytes = encode(image, media)?;
    Ok(Prepared { media, bytes, width: image.width(), height: image.height() })
}

/// `image` as a PNG or a JPEG of 8 bits for each channel, converted only when the
/// encoder cannot take its colour type.
fn encode(image: &DynamicImage, media: MediaType) -> Result<Vec<u8>, MediaError> {
    let mut out = Vec::new();
    let written = if media == MediaType::Png {
        let encoder = PngEncoder::new(&mut out);
        match image.color() {
            ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8 => {
                image.write_with_encoder(encoder)
            }
            color if color.has_alpha() => image.to_rgba8().write_with_encoder(encoder),
            _ => image.to_rgb8().write_with_encoder(encoder),
        }
    } else {
        let encoder = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
        match image.color() {
            ColorType::L8 | ColorType::Rgb8 => image.write_with_encoder(encoder),
            _ => image.to_rgb8().write_with_encoder(encoder),
        }
    };
    written.map(|()| out).map_err(MediaError::Codec)
}

#[cfg(test)]
mod tests;
