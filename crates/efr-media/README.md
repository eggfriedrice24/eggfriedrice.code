# efr-media

## Purpose

The images and PDFs of efr's attachments (ADR 0026), as pure functions of their bytes:
the size from the header, the decode check, the strip of EXIF, GPS, XMP and comment
metadata, the downscale, the thumbnail and the facts of a PDF. efrd calls it from the
intake of an attachment. The functions come with lane L1 of 0.0.13; this crate holds
the dependencies for them now.

## Tier

Tier 1.

## Allowed dependencies

`efr-stdx`, for the one table of magic bytes (`efr_stdx::media::sniff`). Only efrd
links this crate (`efr-daemon -> efr-media`), so the image library stays out of `efr`
and `efr-sbx`.

Third-party crates: `image` with the `png` and `jpeg` features only, and `miniz_oxide`,
which inflates the object streams of a PDF. The probe build of 0.0.13 measured
0.62 MB of efrd for `image`.

## Invariant

No IO, no tokio and no rule of what goes to a model: each function takes bytes and
gives bytes or facts. A cut or a changed file gives an error, never a panic.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-media
```
