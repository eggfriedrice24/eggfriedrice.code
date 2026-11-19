//! The attachments of the tests: one image that each test changes with the struct
//! update syntax, so a new field of `Attachment` changes one place.

use efr_protocol::{Attachment, AttachmentId, AttachmentKind, AttachmentOrigin, SentForm};

/// A PNG of 1 x 1 pixel with alpha and no metadata, the smallest whole image: the
/// intake keeps its bytes as they are.
pub const PIXEL_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\nIDATx\x9cc\0\x01\0\0\x05\0\x01\r\n-\xb4\0\0\0\0IEND\xaeB`\x82";

/// The size of the file of [`image`].
pub const IMAGE_BYTES: u64 = 9_000;

/// The attachment `a<n>`, `img-<n>.png`, a PNG of `width` x `height` pixels and
/// [`IMAGE_BYTES`] that a tool read at `/home/u/img-<n>.png`, with no copy. A model
/// gets it as it is.
pub fn image(n: u32, width: u32, height: u32) -> Attachment {
    Attachment {
        id: AttachmentId::nth(n),
        name: format!("img-{n}.png"),
        media_type: "image/png".to_owned(),
        bytes: IMAGE_BYTES,
        sha256: "ab".repeat(32),
        kind: AttachmentKind::Image { width, height },
        origin: AttachmentOrigin::Tool { path: format!("/home/u/img-{n}.png").into() },
        copy: None,
        sent: Some(SentForm {
            media_type: "image/png".to_owned(),
            bytes: IMAGE_BYTES,
            width: Some(width),
            height: Some(height),
        }),
    }
}
