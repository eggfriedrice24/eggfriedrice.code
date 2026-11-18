//! The walk of the segments of a JPEG file to the start of its image data, which the
//! probe, the strip and the decode check share.

use crate::probe::be16;

/// The marker of the start of a scan: the image data follows its segment.
pub(crate) const SOS: u8 = 0xda;

/// The marker of the end of the image.
pub(crate) const EOI: u8 = 0xd9;

/// One segment: its marker and its bytes, `start..end`, with the marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Segment {
    pub(crate) marker: u8,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Segment {
    /// True for a start-of-frame segment, which holds the size of the image: SOF0 to
    /// SOF15 but DHT (`c4`), JPG (`c8`) and DAC (`cc`).
    pub(crate) fn is_frame(self) -> bool {
        matches!(self.marker, 0xc0..=0xcf) && !matches!(self.marker, 0xc4 | 0xc8 | 0xcc)
    }
}

/// The segments of the JPEG `bytes` after its start mark, to the first start of a scan.
/// An `Err` holds the offset of a segment that does not fit, or of an end mark before
/// any scan, and is the last item.
pub(crate) fn segments(bytes: &[u8]) -> Segments<'_> {
    Segments { bytes, at: 2, done: false }
}

/// The iterator of [`segments`].
#[derive(Debug)]
pub(crate) struct Segments<'a> {
    bytes: &'a [u8],
    at: usize,
    done: bool,
}

impl Iterator for Segments<'_> {
    type Item = Result<Segment, usize>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let segment = self.segment();
        self.done = !segment.is_ok_and(|segment| segment.marker != SOS);
        if let Ok(segment) = segment {
            self.at = segment.end;
        }
        Some(segment)
    }
}

impl Segments<'_> {
    /// The segment at `at`, after its fill bytes.
    fn segment(&self) -> Result<Segment, usize> {
        let bytes = self.bytes;
        let mut start = self.at;
        loop {
            if bytes.get(start) != Some(&0xff) {
                return Err(start);
            }
            let marker = *bytes.get(start + 1).ok_or(start)?;
            let length = match marker {
                // NOTE: a fill byte before a marker.
                0xff => {
                    start += 1;
                    continue;
                }
                // NOTE: TEM and the restart markers have no length.
                0x01 | 0xd0..=0xd7 => 0,
                EOI => return Err(start),
                _ => {
                    usize::from(be16(bytes, start + 2).filter(|&length| length >= 2).ok_or(start)?)
                }
            };
            let end = start + 2 + length;
            if end > bytes.len() {
                return Err(start);
            }
            return Ok(Segment { marker, start, end });
        }
    }
}
