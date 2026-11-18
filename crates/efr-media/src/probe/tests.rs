use efr_stdx::media::MediaType;
use rstest::rstest;

use super::*;
use crate::testing::{gif, jpeg, png, webp_extended, webp_lossless, webp_lossy};

#[rstest]
#[case::png(png(30, 20, false), MediaType::Png)]
#[case::jpeg(jpeg(30, 20), MediaType::Jpeg)]
#[case::gif(gif(30, 20, &[]), MediaType::Gif)]
#[case::webp_lossy(webp_lossy(30, 20), MediaType::Webp)]
#[case::webp_lossless(webp_lossless(30, 20), MediaType::Webp)]
#[case::webp_extended(webp_extended(30, 20, &[]), MediaType::Webp)]
fn the_header_gives_the_type_and_the_size(#[case] bytes: Vec<u8>, #[case] media: MediaType) {
    assert_eq!(probe(&bytes).unwrap(), Probe { media, width: 30, height: 20 });
}

#[test]
fn a_jpeg_with_metadata_before_its_frame_gives_the_size_of_the_frame() {
    let bytes = crate::testing::with_jpeg_segment(&jpeg(30, 20), 0xe1, b"Exif\0\0junk");
    assert_eq!(probe(&bytes).unwrap().width, 30);
}

#[rstest]
#[case::text(b"plain text".to_vec())]
#[case::pdf(b"%PDF-1.7\n".to_vec())]
fn a_file_that_is_no_image_is_the_wrong_type(#[case] bytes: Vec<u8>) {
    assert!(matches!(probe(&bytes), Err(MediaError::WrongType { .. })));
}

#[rstest]
#[case::png(png(30, 20, false))]
#[case::jpeg(jpeg(30, 20))]
#[case::gif(gif(30, 20, &[]))]
#[case::webp(webp_lossy(30, 20))]
fn a_cut_header_is_broken(#[case] bytes: Vec<u8>) {
    let cut = &bytes[..sniff_len(&bytes)];
    assert!(matches!(probe(cut), Err(MediaError::Broken { .. })), "{:?}", probe(cut));
}

/// The length of a head that still sniffs as its type but has no size.
fn sniff_len(bytes: &[u8]) -> usize {
    if bytes.starts_with(b"RIFF") { 16 } else { 8 }
}

#[test]
fn a_size_of_zero_is_broken() {
    let mut bytes = gif(30, 20, &[]);
    bytes[6..8].copy_from_slice(&[0, 0]);
    assert!(matches!(probe(&bytes), Err(MediaError::Broken { .. })));
}

#[test]
fn the_pixels_and_the_edge_of_a_probe() {
    let header = Probe { media: MediaType::Png, width: 8000, height: 6000 };
    assert_eq!(header.pixels(), 48_000_000);
    assert_eq!(header.edge(), 8000);
}
