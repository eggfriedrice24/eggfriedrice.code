use efr_protocol::AttachmentKind;
use pretty_assertions::assert_eq;

use super::{Note, check};
use crate::attachments::tests::PIXEL_PNG;

/// A GIF of `width` x 1 pixel with one comment.
fn gif(width: u16) -> Vec<u8> {
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&width.to_le_bytes());
    gif.extend_from_slice(&[1, 0, 0x80, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff]);
    gif.extend_from_slice(b"\x21\xfe\x07comment\0");
    gif.extend_from_slice(&[0x2c, 0, 0, 0, 0]);
    gif.extend_from_slice(&width.to_le_bytes());
    gif.extend_from_slice(&[1, 0, 0, 2, 2, 0x4c, 0x01, 0, 0x3b]);
    gif
}

/// A PDF of a browser print with `pages` pages, then `trailer` in its trailer.
fn pdf(pages: u32, trailer: &str) -> Vec<u8> {
    format!(
        "%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
         2 0 obj\n<< /Type /Pages /Kids [] /Count {pages} >>\nendobj\n\
         trailer\n<< /Size 3 /Root 1 0 R {trailer}>>\n%%EOF\n"
    )
    .into_bytes()
}

#[test]
fn a_whole_png_goes_as_content_with_its_size() {
    let checked = check(PIXEL_PNG, 2000);
    assert_eq!(checked.media_type, "image/png");
    assert_eq!(checked.kind, AttachmentKind::Image { width: 1, height: 1 });
    let (form, bytes) = checked.sent.expect("content");
    assert_eq!(
        (form.media_type.as_str(), form.width, form.height),
        ("image/png", Some(1), Some(1))
    );
    assert_eq!(form.bytes, bytes.len() as u64);
}

#[test]
fn a_cut_png_is_broken_and_its_kind_is_no_image() {
    let checked = check(&PIXEL_PNG[..40], 2000);
    assert_eq!(
        (checked.kind, checked.sent, checked.note),
        (AttachmentKind::Other, None, Some(Note::Broken))
    );
}

#[test]
fn an_image_over_50_megapixels_goes_as_a_path_with_its_size() {
    let mut header = PIXEL_PNG.to_vec();
    header[16..20].copy_from_slice(&10_000_u32.to_be_bytes());
    header[20..24].copy_from_slice(&5_001_u32.to_be_bytes());
    let checked = check(&header, 2000);
    assert_eq!(checked.kind, AttachmentKind::Image { width: 10_000, height: 5_001 });
    assert_eq!((checked.sent, checked.note), (None, Some(Note::TooLarge)));
}

#[test]
fn a_gif_loses_its_comment_and_one_wider_than_the_edge_goes_as_a_path() {
    let checked = check(&gif(40), 2000);
    let (form, bytes) = checked.sent.expect("content");
    assert_eq!(form.media_type, "image/gif");
    assert!(!bytes.windows(7).any(|w| w == b"comment"));

    let wide = check(&gif(3000), 2000);
    assert_eq!(wide.kind, AttachmentKind::Image { width: 3000, height: 1 });
    assert_eq!((wide.sent, wide.note), (None, Some(Note::TooLarge)));
    let cut = gif(40);
    assert_eq!(check(&cut[..cut.len() - 4], 2000).note, Some(Note::Broken));
}

#[test]
fn a_pdf_goes_as_content_within_the_limits_and_as_a_path_with_its_note() {
    let small = check(&pdf(3, ""), 2000);
    assert_eq!(small.kind, AttachmentKind::Pdf { pages: Some(3) });
    assert_eq!(small.sent.map(|(form, _)| form.media_type), Some("application/pdf".to_owned()));

    let mut large = pdf(3, "");
    large.resize(5 * 1024 * 1024 + 1, b' ');
    let cases = [
        (large, Note::PdfTooLarge),
        (pdf(101, ""), Note::PdfTooLarge),
        (pdf(3, "/Encrypt 4 0 R "), Note::PdfPassword),
        (b"%PDF-1.4\n<< /Type /Catalog >>\n".to_vec(), Note::PdfPages),
    ];
    for (bytes, expected) in cases {
        let checked = check(&bytes, 2000);
        assert_eq!((checked.sent.is_none(), checked.note), (true, Some(expected)), "{expected:?}");
    }
    assert_eq!(Note::PdfPages.text(), "as a path: efr cannot count the pages of this PDF");
}

#[test]
fn a_text_file_and_any_other_file_go_as_a_path_with_no_note() {
    let text = check("fn main() {}\n".as_bytes(), 2000);
    assert_eq!((text.media_type, text.kind, text.note), ("text/plain", AttachmentKind::Text, None));
    let cut_char = check(&"é".repeat(5000).into_bytes(), 2000);
    assert_eq!(cut_char.kind, AttachmentKind::Text, "a cut inside a character at the probe end");
    let binary = check(b"\x7fELF\x02\x01\x01\0", 2000);
    assert_eq!(
        (binary.media_type, binary.kind),
        ("application/octet-stream", AttachmentKind::Other)
    );
    assert!(binary.sent.is_none());
}
