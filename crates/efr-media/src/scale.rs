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

/// The most bytes of one decoded image: 200 MB, a 50 MP image with 8 bits for each of 4
/// channels. A 16-bit image of the same pixels needs twice that and is refused before
/// its decode.
pub const MAX_DECODE_BYTES: u64 = 200_000_000;

/// A PNG or JPEG image after one decode, with its EXIF orientation. The pixels turn
/// only in [`scale`], after the downscale, so a turn never copies the large image.
#[derive(Debug)]
pub struct Decoded {
    image: DynamicImage,
    orientation: Orientation,
}

impl Decoded {
    /// The width in pixels, after the turn.
    pub fn width(&self) -> u32 {
        if self.swaps() { self.image.height() } else { self.image.width() }
    }

    /// The height in pixels, after the turn.
    pub fn height(&self) -> u32 {
        if self.swaps() { self.image.width() } else { self.image.height() }
    }

    /// The longer edge in pixels.
    pub fn edge(&self) -> u32 {
        self.image.width().max(self.image.height())
    }

    /// True when the EXIF orientation turns or flips the pixels: the file then shows
    /// right only with its EXIF block, which [`strip`](crate::strip) removes, so a model
    /// must get the image of [`scale`].
    pub fn turned(&self) -> bool {
        self.orientation != Orientation::NoTransforms
    }

    /// True when the turn swaps the width and the height.
    fn swaps(&self) -> bool {
        matches!(
            self.orientation,
            Orientation::Rotate90
                | Orientation::Rotate270
                | Orientation::Rotate90FlipH
                | Orientation::Rotate270FlipH
        )
    }
}

/// An image that [`scale`] wrote for a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The type of `bytes`: PNG for an image with a pixel that is not opaque, else JPEG.
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
/// [`MediaError::TooManyPixels`] over [`MAX_PIXELS`] and [`MediaError::TooManyBytes`]
/// over [`MAX_DECODE_BYTES`], before any decode; [`MediaError::Broken`] for a JPEG with
/// no end mark after its image data, and [`MediaError::Codec`] for each image that the
/// library cannot decode.
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
    let needed = decoder.total_bytes();
    if needed > MAX_DECODE_BYTES {
        return Err(MediaError::TooManyBytes { bytes: needed });
    }
    let orientation = decoder.orientation().map_err(MediaError::Codec)?;
    let image = DynamicImage::from_decoder(decoder).map_err(MediaError::Codec)?;
    Ok(Decoded { image, orientation })
}

/// True when the walk of the JPEG `bytes` reaches an end mark after its image data. The
/// decoder of the library fills a cut scan with grey and gives no error, so this is
/// the check of a cut JPEG.
fn jpeg_ends(bytes: &[u8]) -> bool {
    jpeg::whole(bytes).any(|segment| segment.is_ok_and(|segment| segment.marker == jpeg::EOI))
}

/// `decoded` as a model gets it: turned upright, with its longer edge at most
/// `max_edge` pixels with the same aspect ratio, and encoded with no metadata, as a PNG
/// when a pixel is not opaque, else as a JPEG of quality [`JPEG_QUALITY`].
///
/// An image of more than twice `max_edge` shrinks with a box filter, which allocates
/// only its output; a smaller one with a triangle filter, whose buffer of 16 bytes for
/// each pixel of the source width and the new height stays under 32 x `max_edge`² bytes
/// (128 MB at 2000 px).
///
/// # Errors
///
/// [`MediaError::Codec`] when the encoder fails.
pub fn scale(decoded: Decoded, max_edge: u32) -> Result<Prepared, MediaError> {
    let Decoded { image, orientation } = decoded;
    let edge = image.width().max(image.height());
    let mut image = if edge / 2 > max_edge {
        image.thumbnail(max_edge, max_edge)
    } else if edge > max_edge {
        image.resize(max_edge, max_edge, FilterType::Triangle)
    } else {
        image
    };
    image.apply_orientation(orientation);
    let media = if opaque(&image) { MediaType::Jpeg } else { MediaType::Png };
    let bytes = encode(&image, media)?;
    Ok(Prepared { media, bytes, width: image.width(), height: image.height() })
}

/// True when each pixel of `image` is opaque: an image with an alpha channel of only
/// full values, such as most screenshots, goes as a JPEG, which is far smaller.
fn opaque(image: &DynamicImage) -> bool {
    match image {
        DynamicImage::ImageRgba8(buffer) => buffer.pixels().all(|pixel| pixel.0[3] == u8::MAX),
        DynamicImage::ImageLumaA8(buffer) => buffer.pixels().all(|pixel| pixel.0[1] == u8::MAX),
        DynamicImage::ImageRgba16(buffer) => buffer.pixels().all(|pixel| pixel.0[3] == u16::MAX),
        DynamicImage::ImageLumaA16(buffer) => buffer.pixels().all(|pixel| pixel.0[1] == u16::MAX),
        other => !other.color().has_alpha(),
    }
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
