use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{MediaType, SNIFF_BYTES, sniff};

#[rstest]
#[case::png(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", Some(MediaType::Png))]
#[case::jpeg(b"\xff\xd8\xff\xe0\0\x10JFIF\0", Some(MediaType::Jpeg))]
#[case::gif87(b"GIF87a\x01\0\x01\0", Some(MediaType::Gif))]
#[case::gif89(b"GIF89a\x01\0\x01\0", Some(MediaType::Gif))]
#[case::webp(b"RIFF\x24\0\0\0WEBPVP8 ", Some(MediaType::Webp))]
#[case::pdf(b"%PDF-1.7\n%\xe2\xe3", Some(MediaType::Pdf))]
#[case::a_riff_that_is_no_webp(b"RIFF\x24\0\0\0WAVEfmt ", None)]
#[case::text(b"fn main() {}\n", None)]
#[case::empty(b"", None)]
#[case::a_cut_png(b"\x89PNG\r\n", None)]
#[case::a_cut_webp(b"RIFF\x24\0\0\0WE", None)]
fn the_type_comes_from_the_first_bytes(#[case] head: &[u8], #[case] want: Option<MediaType>) {
    assert_eq!(sniff(head), want);
}

#[test]
fn each_signature_fits_in_the_head_that_a_caller_reads() {
    let longest = super::MAGIC.iter().map(|(magic, _)| magic.len()).max();
    assert!(longest.is_some_and(|len| len <= SNIFF_BYTES));
    assert!(b"RIFF\0\0\0\0WEBP".len() <= SNIFF_BYTES);
}

#[test]
fn each_type_has_its_wire_name_its_short_name_and_its_extension() {
    let names = [MediaType::Png, MediaType::Jpeg, MediaType::Gif, MediaType::Webp, MediaType::Pdf]
        .map(|media| (media.as_str(), media.name(), media.extension(), media.is_image()));
    assert_eq!(
        names,
        [
            ("image/png", "PNG", "png", true),
            ("image/jpeg", "JPEG", "jpg", true),
            ("image/gif", "GIF", "gif", true),
            ("image/webp", "WebP", "webp", true),
            ("application/pdf", "PDF", "pdf", false),
        ]
    );
}
