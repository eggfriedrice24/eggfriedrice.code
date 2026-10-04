//! PTY recordings: append-only segment files indexed in `recording_segments`.
//!
//! Each PTY's output goes to `recordings/<pty_id>/<start_seq>.rec` under the data
//! directory, where `start_seq` is the stream offset of the segment's first byte. A
//! byte's stream offset is its [`Seq`] for `pty.attach(since_seq)` and for the shell
//! marks that bound a command's output, so [`Recordings::read_range`] can slice any
//! command's output back out. Every chunk carries the time it was read
//! (`format.rs` has the layout), and a segment rotates before it would pass 8 MiB.
//!
//! The daemon's `RecordingSink` holds one [`RecordingWriter`] per PTY. The writer
//! adds a segment's index row before it creates the file and closes the row after the
//! file is flushed, all through the store's single writer; a reader treats a row
//! without a file as empty. Appends are not flushed one by one: a recording is an
//! audit trail and a resume buffer, and a crash loses at most the unflushed tail.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{PtyId, Seq};
use efr_stdx::time::Clock;
use jiff::Timestamp;
use rusqlite::{Connection, Row, params};

use crate::{Readers, StoreError, WriterHandle, sql};

mod format;
mod writer;

pub use writer::RecordingWriter;

const TABLE: &str = "recording_segments";

/// The size a segment file stays within, headers included: 8 MiB.
pub const SEGMENT_LIMIT: u64 = 8 * 1024 * 1024;

/// The size of the header before each chunk of bytes.
pub const CHUNK_HEADER_LEN: usize = format::HEADER_LEN;

/// Recordings hold command output, so only their owner may read them.
const DIR_MODE: u32 = 0o700;

/// The recordings of every PTY under one directory: where writers start and ranges
/// are read. Cloning is cheap.
#[derive(Debug, Clone)]
pub struct Recordings {
    root: PathBuf,
    writer: WriterHandle,
    readers: Readers,
    clock: Arc<dyn Clock>,
    limit: u64,
}

/// A segment as the index lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Segment {
    /// The PTY.
    pub pty_id: PtyId,
    /// The stream offset of the segment's first byte.
    pub start_seq: Seq,
    /// The file, relative to the recordings directory.
    pub path: PathBuf,
    /// The number of stream bytes in the segment; final once `closed_at` is set.
    pub bytes: u64,
    /// When the segment was opened.
    pub started_at: Timestamp,
    /// When it was closed; `None` while a writer may still append to it.
    pub closed_at: Option<Timestamp>,
}

/// Bytes read back from a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecordedRange {
    /// The stream offset of the first byte returned. It is larger than the requested
    /// start when the recording no longer holds the bytes before it; a client that
    /// asked to resume from there needs a screen snapshot instead.
    pub start: Seq,
    /// The stream offset after the last byte returned. It is smaller than the
    /// requested end when the PTY has not produced those bytes yet.
    pub end: Seq,
    /// The bytes as the chunks they were recorded in, clipped to the range, in order.
    /// A gap between two chunks means bytes are missing from the recording.
    pub chunks: Vec<RecordedChunk>,
}

impl RecordedRange {
    /// The bytes of every chunk, joined.
    pub fn bytes(&self) -> Vec<u8> {
        self.chunks.iter().flat_map(|chunk| chunk.data.iter().copied()).collect()
    }
}

/// Bytes that the PTY produced at one time.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecordedChunk {
    /// The stream offset of the first byte.
    pub seq: Seq,
    /// When the bytes were read from the PTY, to the microsecond.
    pub at: Timestamp,
    /// The bytes.
    pub data: Vec<u8>,
}

impl Recordings {
    /// The recordings under `root` (`$XDG_DATA_HOME/efr/recordings`), indexed through
    /// `writer` and `readers`, with chunk times from `clock`.
    pub fn new(
        root: impl Into<PathBuf>,
        writer: WriterHandle,
        readers: Readers,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Recordings { root: root.into(), writer, readers, clock, limit: SEGMENT_LIMIT }
    }

    /// The same recordings with a smaller segment limit, so tests rotate quickly.
    #[cfg(test)]
    pub(crate) fn with_segment_limit(mut self, limit: u64) -> Self {
        self.limit = limit;
        self
    }

    /// The recordings directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Starts recording `pty_id`, or continues its recording after the last whole
    /// chunk when it has one: a torn tail left by a crash is cut off first. Only one
    /// writer may record a PTY at a time.
    pub async fn start(&self, pty_id: PtyId) -> Result<RecordingWriter, StoreError> {
        let segments = self.readers.with(move |conn| segments(conn, pty_id)).await?;
        RecordingWriter::start(self, pty_id, segments.last().cloned()).await
    }

    /// The bytes of `pty_id` with stream offsets in `start..end`, as far as the
    /// recording holds them. Pass `Seq::new(u64::MAX)` as `end` for everything after
    /// `start`.
    pub async fn read_range(
        &self,
        pty_id: PtyId,
        start: Seq,
        end: Seq,
    ) -> Result<RecordedRange, StoreError> {
        let segments = self.readers.with(move |conn| segments(conn, pty_id)).await?;
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || read_segments(&root, &segments, start.get(), end.get()))
            .await
            .map_err(|_| StoreError::TaskPanicked { task: "recording" })?
    }
}

/// Every segment of `pty_id`, oldest first.
pub fn segments(conn: &Connection, pty_id: PtyId) -> Result<Vec<Segment>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT pty_id, start_seq, path, bytes, started_at, closed_at FROM recording_segments \
         WHERE pty_id = ?1 ORDER BY start_seq",
    )?;
    let raw = stmt
        .query_map([pty_id.to_string()], RawSegment::from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawSegment::decode).collect()
}

/// The index row of a new segment, inside the writer's transaction.
pub(crate) fn insert_segment(
    conn: &Connection,
    pty_id: PtyId,
    start_seq: u64,
    at: Timestamp,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO recording_segments (pty_id, start_seq, path, bytes, started_at) \
         VALUES (?1, ?2, ?3, 0, ?4)",
        params![
            pty_id.to_string(),
            sql::seq(Seq::new(start_seq)),
            sql::path_text(&relative_path(pty_id, start_seq)),
            sql::micros(at)
        ],
    )?;
    Ok(())
}

/// Records a segment's final size and closes it.
pub(crate) fn close_segment(
    conn: &Connection,
    pty_id: PtyId,
    start_seq: u64,
    bytes: u64,
    at: Timestamp,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE recording_segments SET bytes = ?3, closed_at = ?4 \
         WHERE pty_id = ?1 AND start_seq = ?2",
        params![
            pty_id.to_string(),
            sql::seq(Seq::new(start_seq)),
            i64::try_from(bytes).unwrap_or(i64::MAX),
            sql::micros(at)
        ],
    )?;
    Ok(())
}

/// Opens a closed segment again, for a writer that continues it.
pub(crate) fn reopen_segment(
    conn: &Connection,
    pty_id: PtyId,
    start_seq: u64,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE recording_segments SET closed_at = NULL WHERE pty_id = ?1 AND start_seq = ?2",
        params![pty_id.to_string(), sql::seq(Seq::new(start_seq))],
    )?;
    Ok(())
}

/// `<pty_id>/<start_seq>.rec`.
pub(crate) fn relative_path(pty_id: PtyId, start_seq: u64) -> PathBuf {
    Path::new(&pty_id.to_string()).join(format!("{start_seq}.rec"))
}

/// Creates the directory of `pty_id`'s segments, and the recordings directory.
pub(crate) fn create_dir(root: &Path, pty_id: PtyId) -> Result<(), StoreError> {
    let dir = root.join(pty_id.to_string());
    DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(&dir)
        .map_err(|source| StoreError::RecordingIo { path: dir, source })
}

/// Reads the part of `segments` in `start..end`. Segments are contiguous, so a
/// segment covers the offsets up to the next segment's start.
fn read_segments(
    root: &Path,
    segments: &[Segment],
    start: u64,
    end: u64,
) -> Result<RecordedRange, StoreError> {
    let held_from = segments.first().map_or(0, |segment| segment.start_seq.get());
    let mut chunks = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let segment_start = segment.start_seq.get();
        let next_start = segments.get(index + 1).map(|next| next.start_seq.get());
        if next_start.is_some_and(|next| next <= start) {
            continue;
        }
        if segment_start >= end {
            break;
        }
        let path = root.join(&segment.path);
        let file = match fs::read(&path) {
            Ok(file) => file,
            // The row comes before the file, so a crash between them leaves a row
            // without a file: an empty segment.
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(StoreError::RecordingIo { path, source }),
        };
        let parsed = format::parse(&file).map_err(|offset| StoreError::CorruptRecording {
            path: path.clone(),
            offset: offset as u64,
        })?;
        let mut seq = segment_start;
        for chunk in &parsed.chunks {
            let data = file.get(chunk.data.clone()).unwrap_or_default();
            if let Some((from, part)) = format::clip(seq, data, start, end) {
                let at = Timestamp::from_microsecond(chunk.at_micros).map_err(|_| {
                    StoreError::CorruptRecording { path: path.clone(), offset: chunk.offset as u64 }
                })?;
                chunks.push(RecordedChunk { seq: Seq::new(from), at, data: part.to_vec() });
            }
            seq += data.len() as u64;
            if seq >= end {
                break;
            }
        }
    }
    let range_start = chunks.first().map_or(start.max(held_from), |chunk| chunk.seq.get());
    let range_end =
        chunks.last().map_or(range_start, |chunk| chunk.seq.get() + chunk.data.len() as u64);
    Ok(RecordedRange { start: Seq::new(range_start), end: Seq::new(range_end), chunks })
}

struct RawSegment {
    pty_id: String,
    start_seq: i64,
    path: String,
    bytes: i64,
    started_at: i64,
    closed_at: Option<i64>,
}

impl RawSegment {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawSegment {
            pty_id: row.get(0)?,
            start_seq: row.get(1)?,
            path: row.get(2)?,
            bytes: row.get(3)?,
            started_at: row.get(4)?,
            closed_at: row.get(5)?,
        })
    }

    fn decode(self) -> Result<Segment, StoreError> {
        Ok(Segment {
            pty_id: sql::parse(&self.pty_id, TABLE, "pty_id")?,
            start_seq: sql::to_seq(self.start_seq, TABLE, "start_seq")?,
            path: PathBuf::from(self.path),
            bytes: u64::try_from(self.bytes)
                .map_err(|source| sql::decode_error(TABLE, "bytes", source))?,
            started_at: sql::to_timestamp(self.started_at, TABLE, "started_at")?,
            closed_at: self
                .closed_at
                .map(|value| sql::to_timestamp(value, TABLE, "closed_at"))
                .transpose()?,
        })
    }
}

#[cfg(test)]
mod tests;
