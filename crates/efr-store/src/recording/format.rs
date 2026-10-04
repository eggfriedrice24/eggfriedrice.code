//! The segment file format.
//!
//! A segment file is a sequence of chunks. Each chunk is a 16-byte header followed by
//! the bytes the PTY produced:
//!
//! | Bytes | Content |
//! |---|---|
//! | 0..3 | the magic `efr` |
//! | 3 | the chunk format: 1 for output bytes in this format |
//! | 4..8 | the length of the bytes, `u32` little-endian |
//! | 8..16 | when the bytes were read, microseconds since the Unix epoch, `i64` little-endian |
//!
//! The bytes of all chunks, in order, are the PTY's output stream. A byte's stream
//! sequence number is its segment's `start_seq` plus the count of bytes before it in
//! the segment; headers do not count.
//!
//! A crash can cut the last chunk short or leave zeros after it. Such a tail is torn,
//! not corrupt: [`parse`] stops before it, and the writer cuts it off before it
//! appends again.

use std::ops::Range;

/// The size of a chunk header.
pub(crate) const HEADER_LEN: usize = 16;

/// The largest number of bytes in one chunk. A larger append becomes several chunks,
/// so a segment ends within one chunk of its size limit.
pub(crate) const MAX_CHUNK: usize = 64 * 1024;

const MAGIC: [u8; 3] = *b"efr";
const OUTPUT_V1: u8 = 1;

/// The header of a chunk of `len` bytes read at `at_micros`. `len` is at most
/// [`MAX_CHUNK`].
pub(crate) fn header(at_micros: i64, len: usize) -> [u8; HEADER_LEN] {
    let len = u32::try_from(len).unwrap_or(u32::MAX);
    let mut header = [0; HEADER_LEN];
    header[..3].copy_from_slice(&MAGIC);
    header[3] = OUTPUT_V1;
    header[4..8].copy_from_slice(&len.to_le_bytes());
    header[8..].copy_from_slice(&at_micros.to_le_bytes());
    header
}

/// Appends the chunks for `data`, read at `at_micros`, to `out`: one chunk per
/// [`MAX_CHUNK`] bytes.
pub(crate) fn encode(at_micros: i64, data: &[u8], out: &mut Vec<u8>) {
    for chunk in data.chunks(MAX_CHUNK) {
        out.extend_from_slice(&header(at_micros, chunk.len()));
        out.extend_from_slice(chunk);
    }
}

/// One whole chunk in a segment file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chunk {
    /// The file offset of the chunk's header.
    pub(crate) offset: usize,
    /// When the bytes were read, in microseconds since the Unix epoch.
    pub(crate) at_micros: i64,
    /// Where the bytes are in the file.
    pub(crate) data: Range<usize>,
}

/// The whole chunks of a segment file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parsed {
    pub(crate) chunks: Vec<Chunk>,
    /// The length of the file without a torn tail.
    pub(crate) valid_len: usize,
}

impl Parsed {
    /// The number of stream bytes in the whole chunks.
    pub(crate) fn stream_len(&self) -> u64 {
        self.chunks.iter().map(|chunk| chunk.data.len() as u64).sum()
    }
}

/// The whole chunks of `file`, stopping before a torn tail. A header that is neither
/// valid nor part of a torn tail is corruption; the error is its file offset.
pub(crate) fn parse(file: &[u8]) -> Result<Parsed, usize> {
    let mut chunks = Vec::new();
    let mut offset = 0;
    while let Some(rest) = file.get(offset..).filter(|rest| !rest.is_empty()) {
        let Some(head) = rest.get(..HEADER_LEN) else {
            return if is_torn(rest) {
                Ok(Parsed { chunks, valid_len: offset })
            } else {
                Err(offset)
            };
        };
        if head[..3] != MAGIC || head[3] != OUTPUT_V1 {
            return if is_zeros(rest) {
                Ok(Parsed { chunks, valid_len: offset })
            } else {
                Err(offset)
            };
        }
        let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let at_micros = i64::from_le_bytes([
            head[8], head[9], head[10], head[11], head[12], head[13], head[14], head[15],
        ]);
        let start = offset + HEADER_LEN;
        if file.len() - start < len {
            // The bytes were cut short: a torn tail.
            break;
        }
        chunks.push(Chunk { offset, at_micros, data: start..start + len });
        offset = start + len;
    }
    Ok(Parsed { chunks, valid_len: offset })
}

/// A tail shorter than a header is torn when it is the start of a valid header or a
/// run of zeros.
fn is_torn(tail: &[u8]) -> bool {
    let mut prefix = [0; 4];
    prefix[..3].copy_from_slice(&MAGIC);
    prefix[3] = OUTPUT_V1;
    let n = tail.len().min(prefix.len());
    tail[..n] == prefix[..n] || is_zeros(tail)
}

fn is_zeros(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == 0)
}

/// How many of the next `remaining` stream bytes the open segment takes before it
/// must rotate: whole chunks, in the order [`encode`] cuts them, that keep the file
/// within `limit`. An empty segment always takes one chunk, so a limit smaller than a
/// chunk still makes progress. Zero means rotate first.
pub(crate) fn fits(file_len: u64, limit: u64, remaining: usize) -> usize {
    let mut file_len = file_len;
    let mut taken = 0;
    while taken < remaining {
        let chunk = (remaining - taken).min(MAX_CHUNK);
        let cost = (HEADER_LEN + chunk) as u64;
        if file_len > 0 && file_len + cost > limit {
            break;
        }
        file_len += cost;
        taken += chunk;
    }
    taken
}

/// The part of a chunk at stream offset `seq` that falls in `start..end`, with its
/// offset, or `None` when they do not overlap.
pub(crate) fn clip(seq: u64, data: &[u8], start: u64, end: u64) -> Option<(u64, &[u8])> {
    let chunk_end = seq + data.len() as u64;
    let from = start.max(seq);
    let to = end.min(chunk_end);
    if from >= to {
        return None;
    }
    let from_index = usize::try_from(from - seq).ok()?;
    let to_index = usize::try_from(to - seq).ok()?;
    Some((from, data.get(from_index..to_index)?))
}

#[cfg(test)]
mod tests;
