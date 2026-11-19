# efr-media

## Purpose

The images and PDFs of efr's attachments (ADR 0026), as pure functions of their bytes.
efrd calls it from the intake of an attachment (`crates/efr-daemon/src/attachments/intake.rs`),
which decides what goes to a model.

- `probe`: `Probe`, the type and the size of a PNG, JPEG, GIF or WebP image from its
  header, with no decode. `MAX_PIXELS` (50 megapixels) is the largest image that efrd decodes: a 48 MP
  photo of a phone still fits.
- `strip`: the metadata strip of a PNG, JPEG, GIF or WebP. It walks the JPEG segments,
  the PNG chunks, the GIF blocks or the RIFF chunks and keeps only the parts that show
  the first image, with the pixel data as it is. It is an allow list: a JPEG keeps
  APP0, APP2 with an ICC profile and APP14 of its APP segments and ends at its first
  end mark (an MPF image or the video of a motion photo after it goes); a PNG keeps its
  critical chunks and the colour and transparency chunks; a WebP keeps `VP8X`, `VP8 `,
  `VP8L`, `ALPH` and `ICCP`, loses the EXIF and XMP flags of `VP8X` and so its
  orientation; a GIF keeps its first image with its graphic control extension. An
  animated WebP is `MediaError::Animated`. For a GIF or a WebP the walk is the check:
  each block and chunk must have a size that fits in the file.
- `decode`: `Decoded`, the whole image of a PNG or JPEG, decoded once, with no more
  than `MAX_DECODE_BYTES` (200 MB). `turned()` is true when the EXIF orientation of a
  JPEG turns the image, so efrd encodes it again upright: the strip drops that tag.
- `scale`: `Prepared`, the decoded image turned upright, with its longer edge at most
  the edge that efrd gives: a box filter for more than twice that edge, which allocates
  only its output, else a triangle filter. It is a JPEG at quality `JPEG_QUALITY` (85),
  or a PNG when a pixel is not opaque.
- `pdf_facts`: `PdfFacts`, whether a PDF has `/Encrypt` and the largest `/Count` of a
  `/Type /Pages` object, in plain objects and in object streams. It is no PDF parser.
  It inflates object streams up to `PDF_INFLATE_MAX` (64 MiB) in all; past that, the
  count is unknown. The largest count is never under the real count of a correct file.
- `MediaError`: why a file failed.

## Tier

Tier 1.

## Allowed dependencies

`efr-stdx`, for the one table of magic bytes (`efr_stdx::media::sniff`). Only efrd
links this crate (`efr-daemon -> efr-media`), so the image library stays out of `efr`
and `efr-sbx`.

Third-party crates: `image` with the `png` and `jpeg` features only, `miniz_oxide`,
which inflates the object streams of a PDF (the version that `png` brings already),
and `thiserror`. The probe build of 0.0.13 measured 0.62 MB of efrd for `image`. The
lean set (`png`, `zune-jpeg` and `jpeg-encoder`, 0.53 MB) has a JPEG crate with the IJG
licence, which `cargo deny` refuses.

## Invariant

No IO, no tokio and no rule of what goes to a model: each function takes bytes and
gives bytes or facts. A cut or a changed file gives an error, never a panic.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-media
```

The tests build their images in memory (`src/testing.rs`). Property tests feed cut and
changed images to `strip` and cut and changed PDFs to `pdf_facts`, and check that
neither panics.
