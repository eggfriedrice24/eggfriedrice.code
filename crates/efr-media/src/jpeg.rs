//! The walk of the segments of a JPEG file, which the probe, the strip and the decode
//! check share.

use crate::probe::be16;

/// The marker of the start of a scan: the image data follows its segment.
pub(crate) const SOS: u8 = 0xda;

/// The marker of the end of the image.
pub(crate) const EOI: u8 = 0xd9;

/// One segment: its marker and its bytes, `start..end`, with the marker. The segment of
/// a scan in [`whole`] ends after its image data.
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
    Segments { bytes, at: 2, done: false, whole: false, scanned: false }
}

/// The segments of the JPEG `bytes` after its start mark, to the first end mark: also
/// the scans and the segments between them. The end mark is the last `Ok` item; each
/// byte after it is not part of the image. An `Err` holds the offset where the walk
/// stopped, and is the last item.
pub(crate) fn whole(bytes: &[u8]) -> Segments<'_> {
    Segments { bytes, at: 2, done: false, whole: true, scanned: false }
}

/// The iterator of [`segments`] and [`whole`].
#[derive(Debug)]
pub(crate) struct Segments<'a> {
    bytes: &'a [u8],
    at: usize,
    done: bool,
    /// True to walk on after the first scan.
    whole: bool,
    /// True after the first scan: an end mark then ends the image.
    scanned: bool,
}

impl Iterator for Segments<'_> {
    type Item = Result<Segment, usize>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut segment = match self.segment() {
            Ok(segment) => segment,
            Err(at) => {
                self.done = true;
                return Some(Err(at));
            }
        };
        match segment.marker {
            SOS if !self.whole => self.done = true,
            SOS => {
                self.scanned = true;
                match scan_end(self.bytes, segment.end) {
                    Some(end) => segment.end = end,
                    None => {
                        self.done = true;
                        return Some(Err(self.bytes.len()));
                    }
                }
            }
            EOI => self.done = true,
            _ => {}
        }
        self.at = segment.end;
        Some(Ok(segment))
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
                EOI if self.scanned => 0,
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

/// The offset of the marker after the image data of a scan that starts at `at`. In the
/// data each `0xff` byte is followed by `0x00`, a restart marker or a fill byte, so the
/// first other marker ends it. `None` when the file ends first.
fn scan_end(bytes: &[u8], mut at: usize) -> Option<usize> {
    loop {
        at += bytes.get(at..)?.iter().position(|&byte| byte == 0xff)?;
        match *bytes.get(at + 1)? {
            0x00 | 0xd0..=0xd7 => at += 2,
            0xff => at += 1,
            _ => return Some(at),
        }
    }
}
