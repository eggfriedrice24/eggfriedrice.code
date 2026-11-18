//! The one error of the crate.

use efr_stdx::media::MediaType;

/// Why a file fails a check or cannot be made ready for a model.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MediaError {
    /// The file is not of a type that the function reads.
    #[error("the file is not a {expected}")]
    WrongType {
        /// What the function reads, such as `PNG or JPEG image`.
        expected: &'static str,
    },
    /// The file ends inside a part, or a part has a size that does not fit in the file.
    #[error("the {media} file is cut or broken at byte {at}")]
    Broken {
        /// The type of the file.
        media: &'static str,
        /// The offset at which the walk stopped.
        at: usize,
    },
    /// The image has more pixels than efr decodes.
    #[error("the image has {width}x{height} pixels, more than efr decodes")]
    TooManyPixels {
        /// The width in pixels.
        width: u32,
        /// The height in pixels.
        height: u32,
    },
    /// The image library could not decode or encode the image.
    #[error("the image library refused the image")]
    Codec(#[source] image::ImageError),
}

impl MediaError {
    /// A walk of a file of `media` that stopped at the offset `at`.
    pub(crate) fn broken(media: MediaType, at: usize) -> Self {
        MediaError::Broken { media: name(media), at }
    }
}

/// The short name of `media` in an error text.
fn name(media: MediaType) -> &'static str {
    match media {
        MediaType::Png => "PNG",
        MediaType::Jpeg => "JPEG",
        MediaType::Gif => "GIF",
        MediaType::Webp => "WebP",
        MediaType::Pdf => "PDF",
        _ => "media",
    }
}
