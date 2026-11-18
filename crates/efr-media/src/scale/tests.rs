use efr_stdx::media::MediaType;
use rstest::rstest;

use super::*;
use crate::testing::{exif, gif, jpeg, png, with_jpeg_segment};
use crate::{probe, strip};

#[rstest]
#[case::wide(4000, 3000, 2000, 1500)]
#[case::tall(1000, 3000, 667, 2000)]
#[case::small(800, 600, 800, 600)]
fn scale_keeps_the_aspect_ratio_and_the_edge_limit(
    #[case] width: u32,
    #[case] height: u32,
    #[case] scaled_width: u32,
    #[case] scaled_height: u32,
) {
    let prepared = scale(&decode(&jpeg(width, height)).unwrap(), 2000).unwrap();
    assert_eq!((prepared.width, prepared.height), (scaled_width, scaled_height));
    let header = probe(&prepared.bytes).unwrap();
    assert_eq!((header.width, header.height), (scaled_width, scaled_height));
}

#[test]
fn an_image_with_alpha_stays_a_png_and_one_with_none_becomes_a_jpeg() {
    let alpha = scale(&decode(&png(300, 200, true)).unwrap(), 100).unwrap();
    assert_eq!(alpha.media, MediaType::Png);
    assert_eq!(probe(&alpha.bytes).unwrap().media, MediaType::Png);

    let opaque = scale(&decode(&png(300, 200, false)).unwrap(), 100).unwrap();
    assert_eq!(opaque.media, MediaType::Jpeg);
    assert_eq!(probe(&opaque.bytes).unwrap().media, MediaType::Jpeg);
}

#[test]
fn the_scaled_image_holds_no_metadata() {
    let tagged = with_jpeg_segment(&jpeg(300, 200), 0xe1, &exif(1));
    let prepared = scale(&decode(&tagged).unwrap(), 100).unwrap();
    assert_eq!(strip(&prepared.bytes).unwrap(), prepared.bytes);
}

#[test]
fn an_exif_orientation_turns_the_pixels() {
    // NOTE: orientation 6 is a turn of 90 degrees.
    let turned = with_jpeg_segment(&jpeg(300, 200), 0xe1, &exif(6));
    let decoded = decode(&turned).unwrap();
    assert!(decoded.turned());
    assert_eq!((decoded.width(), decoded.height()), (200, 300));

    let upright = decode(&with_jpeg_segment(&jpeg(300, 200), 0xe1, &exif(1))).unwrap();
    assert!(!upright.turned());
}

#[test]
fn a_cut_png_or_jpeg_fails_the_decode() {
    let png = png(64, 64, false);
    assert!(decode(&png[..png.len() - 20]).is_err());
    let jpeg = jpeg(64, 64);
    let cut = &jpeg[..jpeg.len() - 40];
    assert!(matches!(decode(cut), Err(MediaError::Broken { .. })), "{:?}", decode(cut));
}

#[test]
fn a_gif_is_not_decoded() {
    assert!(matches!(decode(&gif(8, 8, &[])), Err(MediaError::WrongType { .. })));
}

/// A PNG header that names `width` x `height`, with no pixel data.
fn header_only(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = png(1, 1, false);
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    bytes
}

#[test]
fn an_image_over_50_megapixels_is_refused_before_the_decode() {
    let refused = decode(&header_only(10_000, 5_001));
    assert!(matches!(refused, Err(MediaError::TooManyPixels { width: 10_000, height: 5_001 })));
}

#[test]
fn a_48_megapixel_photo_decodes() {
    let photo = jpeg(8000, 6000);
    let decoded = decode(&photo).unwrap();
    assert_eq!(decoded.edge(), 8000);
    assert_eq!(scale(&decoded, 2000).unwrap().width, 2000);
}

#[test]
fn a_thumbnail_fits_in_its_box_and_a_small_image_keeps_its_size() {
    let wide = thumbnail(&decode(&jpeg(2000, 500)).unwrap()).unwrap();
    let header = probe(&wide).unwrap();
    assert_eq!((header.media, header.width, header.height), (MediaType::Png, 512, 128));

    let small = thumbnail(&decode(&png(40, 30, true)).unwrap()).unwrap();
    assert_eq!(probe(&small).unwrap().width, 40);
}
