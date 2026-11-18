use proptest::prelude::*;

use super::*;
use crate::testing::{
    exif, gif, gif_extension, jpeg, png, riff, webp_extended, with_jpeg_segment, with_png_chunk,
};
use crate::{decode, probe};

/// True when `needle` occurs in `bytes`.
fn holds(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

#[test]
fn a_jpeg_loses_its_exif_xmp_iptc_and_comment_and_keeps_its_pixels() {
    let plain = jpeg(40, 30);
    let mut tagged = with_jpeg_segment(&plain, 0xe1, &exif(1));
    tagged = with_jpeg_segment(&tagged, 0xe1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>");
    tagged = with_jpeg_segment(&tagged, 0xed, b"Photoshop 3.0\0IPTC city");
    tagged = with_jpeg_segment(&tagged, 0xfe, b"a comment");
    assert!(holds(&tagged, b"GPS 52N"));

    let stripped = strip(&tagged).unwrap();
    assert_eq!(stripped, plain);
    assert_eq!(decode(&stripped).unwrap().width(), 40);
}

#[test]
fn a_png_loses_its_exif_and_text_chunks_and_keeps_its_pixels() {
    let plain = png(40, 30, false);
    let mut tagged = with_png_chunk(&plain, b"eXIf", &exif(1)[6..]);
    tagged = with_png_chunk(&tagged, b"tEXt", b"Comment\0GPS 52N 13E");
    tagged = with_png_chunk(&tagged, b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0<x/>");
    tagged = with_png_chunk(&tagged, b"zTXt", b"Author\0\0xx");
    assert!(holds(&tagged, b"GPS 52N"));

    let stripped = strip(&tagged).unwrap();
    assert_eq!(stripped, plain);
    assert_eq!(decode(&stripped).unwrap().height(), 30);
}

#[test]
fn a_webp_loses_its_exif_and_xmp_chunks_and_their_flags() {
    let gps = exif(1);
    let tagged = webp_extended(40, 30, &[(b"EXIF", &gps[6..]), (b"XMP ", b"<x:xmpmeta/>")]);
    assert!(holds(&tagged, b"GPS 52N"));

    let stripped = strip(&tagged).unwrap();
    assert_eq!(stripped, webp_extended(40, 30, &[]));
    assert!(!holds(&stripped, b"EXIF") && !holds(&stripped, b"XMP "));
    assert_eq!(probe(&stripped).unwrap().width, 40);
}

#[test]
fn a_webp_with_an_odd_chunk_keeps_its_pad_byte() {
    let odd = riff(&[(b"VP8L", &[0x2f, 0, 0, 0, 0]), (b"ICCP", &[1, 2, 3])]);
    assert_eq!(strip(&odd).unwrap(), odd);
}

#[test]
fn a_gif_loses_its_comments_and_xmp_and_keeps_its_loop_count() {
    let looped = gif_extension(0xff, &[b"NETSCAPE2.0", &[1, 0, 0]]);
    let control = gif_extension(0xf9, &[&[0, 10, 0, 0]]);
    let comment = gif_extension(0xfe, &[b"GPS 52N 13E"]);
    let xmp = gif_extension(0xff, &[b"XMP DataXMP", b"<x:xmpmeta/>"]);
    let tagged = gif(40, 30, &[&looped, &comment, &xmp, &control]);
    assert!(holds(&tagged, b"GPS 52N"));

    let stripped = strip(&tagged).unwrap();
    assert_eq!(stripped, gif(40, 30, &[&looped, &control]));
}

#[test]
fn an_image_with_no_metadata_stays_the_same() {
    for image in [jpeg(8, 8), png(8, 8, true), gif(8, 8, &[]), webp_extended(8, 8, &[])] {
        assert_eq!(strip(&image).unwrap(), image);
    }
}

#[test]
fn a_cut_gif_or_webp_fails_the_walk() {
    let whole_gif = gif(40, 30, &[&gif_extension(0xfe, &[b"comment"])]);
    let whole_webp = webp_extended(40, 30, &[(b"EXIF", b"Exif data")]);
    for whole in [whole_gif, whole_webp] {
        let cut = &whole[..whole.len() - 3];
        assert!(matches!(strip(cut), Err(MediaError::Broken { .. })), "{:?}", strip(cut));
    }
}

#[test]
fn a_cut_png_or_jpeg_fails_the_walk() {
    let png = png(40, 30, false);
    assert!(matches!(strip(&png[..png.len() - 4]), Err(MediaError::Broken { .. })));
    let jpeg = jpeg(40, 30);
    // NOTE: cut in the first segment, before the image data.
    assert!(matches!(strip(&jpeg[..10]), Err(MediaError::Broken { .. })));
}

#[test]
fn a_webp_whose_riff_size_passes_the_file_is_broken() {
    let mut webp = webp_extended(8, 8, &[]);
    webp[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(strip(&webp), Err(MediaError::Broken { .. })));
}

#[test]
fn a_file_that_is_no_image_is_the_wrong_type() {
    assert!(matches!(strip(b"%PDF-1.4"), Err(MediaError::WrongType { .. })));
}

proptest! {
    #[test]
    fn a_cut_or_changed_image_never_panics(
        which in 0_usize..4,
        cut in 0.0_f64..1.0,
        flips in proptest::collection::vec((0.0_f64..1.0, any::<u8>()), 0..8),
    ) {
        let mut bytes = match which {
            0 => with_jpeg_segment(&jpeg(16, 16), 0xe1, &exif(6)),
            1 => with_png_chunk(&png(16, 16, true), b"tEXt", b"k\0v"),
            2 => gif(16, 16, &[&gif_extension(0xfe, &[b"c"])]),
            _ => webp_extended(16, 16, &[(b"EXIF", b"e")]),
        };
        for (at, byte) in flips {
            let at = (at * bytes.len() as f64) as usize;
            if let Some(slot) = bytes.get_mut(at) {
                *slot = byte;
            }
        }
        let keep = (cut * bytes.len() as f64) as usize;
        let bytes = &bytes[..keep.max(1)];
        let _ = strip(bytes);
        let _ = probe(bytes);
        if let Ok(decoded) = decode(bytes) {
            let _ = crate::scale(&decoded, 8);
        }
    }
}
