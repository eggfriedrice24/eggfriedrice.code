//! The attachments of the converter tests of both providers.

use efr_protocol::{Attachment, AttachmentKind, AttachmentOrigin, SentForm};
use efr_provider::{AttachmentPart, EncodedBytes};
use efr_test_support::attachments;

/// The base64 text of every part with data.
pub(crate) const DATA: &str = "iVBORw0KGgo=";

/// The image `a1`: a screenshot of the user's `@path`, which goes as a JPEG of 2000 px.
pub(crate) fn image(data: bool) -> AttachmentPart {
    part(
        Attachment {
            name: "shot.png".to_owned(),
            bytes: 2_100_000,
            origin: AttachmentOrigin::Path { path: "/home/u/shot.png".into() },
            copy: Some("/s/attachments/a1-shot.png".into()),
            sent: Some(SentForm {
                media_type: "image/jpeg".to_owned(),
                bytes: 400_000,
                width: Some(2000),
                height: Some(1000),
            }),
            ..attachments::image(1, 3000, 1500)
        },
        data,
    )
}

/// The PDF `a2` of the phone.
pub(crate) fn pdf(data: bool) -> AttachmentPart {
    part(
        Attachment {
            name: "report.pdf".to_owned(),
            media_type: "application/pdf".to_owned(),
            bytes: 90_000,
            sha256: "cd".repeat(32),
            kind: AttachmentKind::Pdf { pages: Some(3) },
            origin: AttachmentOrigin::Phone,
            copy: Some("/s/attachments/a2-report.pdf".into()),
            sent: Some(SentForm {
                media_type: "application/pdf".to_owned(),
                bytes: 90_000,
                width: None,
                height: None,
            }),
            ..attachments::image(2, 1, 1)
        },
        data,
    )
}

/// The image `a3` that `read_file` read.
pub(crate) fn tool_image(data: bool) -> AttachmentPart {
    part(
        Attachment {
            name: "plot.png".to_owned(),
            sha256: "ef".repeat(32),
            origin: AttachmentOrigin::Tool { path: "/home/u/plot.png".into() },
            ..attachments::image(3, 64, 32)
        },
        data,
    )
}

fn part(attachment: Attachment, data: bool) -> AttachmentPart {
    AttachmentPart { attachment, data: data.then(|| EncodedBytes::new(DATA)) }
}
