//! The writer of one PTY's recording.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{PtyId, Seq, Size};
use efr_stdx::time::Clock;

use super::{Recordings, Segment, format};
use crate::{StoreError, WriterHandle, sql};

/// Recordings hold command output, so only their owner may read them.
const FILE_MODE: u32 = 0o600;

/// Appends one PTY's output to its recording and rotates segments at the size limit.
///
/// Its methods are async: file work runs on tokio's blocking pool and index changes go
/// through the store's writer, so the PTY reader that calls it never blocks a tokio
/// worker.
#[derive(Debug)]
pub struct RecordingWriter {
    pty_id: PtyId,
    root: PathBuf,
    clock: Arc<dyn Clock>,
    writer: WriterHandle,
    limit: u64,
    /// `None` only after a write panicked; see [`StoreError::RecordingBroken`].
    segment: Option<OpenSegment>,
    /// The stream offset of the next byte.
    end_seq: u64,
}

#[derive(Debug)]
struct OpenSegment {
    file: File,
    path: PathBuf,
    start_seq: u64,
    file_len: u64,
}

impl RecordingWriter {
    /// Opens the writer: a first segment for a new recording, or the newest segment of
    /// an existing one, continued after its last whole chunk.
    pub(super) async fn start(
        recordings: &Recordings,
        pty_id: PtyId,
        last: Option<Segment>,
    ) -> Result<Self, StoreError> {
        let mut writer = RecordingWriter {
            pty_id,
            root: recordings.root.clone(),
            clock: Arc::clone(&recordings.clock),
            writer: recordings.writer.clone(),
            limit: recordings.limit,
            segment: None,
            end_seq: 0,
        };
        let Some(last) = last else {
            writer.run_index(move |conn, at| super::insert_segment(conn, pty_id, 0, at)).await?;
            writer.segment = Some(writer.open_segment(0).await?.0);
            return Ok(writer);
        };
        let start_seq = last.start_seq.get();
        let (segment, stream_len) = writer.open_segment(start_seq).await?;
        writer.end_seq = start_seq + stream_len;
        // NOTE: a segment of sizes alone never rotates: its successor would start at
        // the same offset, and the index holds one segment per offset.
        let full = segment.file_len >= writer.limit && stream_len > 0;
        writer.segment = Some(segment);
        if full {
            writer.rotate().await?;
        } else if last.closed_at.is_some() {
            writer
                .run_index(move |conn, _at| super::reopen_segment(conn, pty_id, start_seq))
                .await?;
        }
        Ok(writer)
    }

    /// The PTY this writer records.
    pub fn pty_id(&self) -> PtyId {
        self.pty_id
    }

    /// The stream offset that the next appended byte gets: the number of bytes the
    /// recording holds.
    pub fn end_seq(&self) -> Seq {
        Seq::new(self.end_seq)
    }

    /// Appends `bytes`, read from the PTY now, and returns the stream offset of the
    /// first one. The bytes go into chunks of at most 64 KiB, all stamped with the
    /// same time, and the segment rotates before a chunk that would take it past its
    /// limit.
    pub async fn append(&mut self, bytes: &[u8]) -> Result<Seq, StoreError> {
        let first = Seq::new(self.end_seq);
        let at = sql::micros(self.clock.now());
        let mut offset = 0;
        while offset < bytes.len() {
            // A segment of sizes alone counts as empty, so that it never rotates.
            let file_len = match self.segment()? {
                segment if segment.start_seq == self.end_seq => 0,
                segment => segment.file_len,
            };
            let take = format::fits(file_len, self.limit, bytes.len() - offset);
            if take == 0 {
                self.rotate().await?;
                continue;
            }
            let data = bytes[offset..offset + take].to_vec();
            let mut segment = self.take_segment()?;
            let (segment, written) = tokio::task::spawn_blocking(move || {
                let written = segment.write(at, &data);
                (segment, written)
            })
            .await
            .map_err(|_| StoreError::TaskPanicked { task: "recording" })?;
            self.segment = Some(segment);
            written?;
            self.end_seq += take as u64;
            offset += take;
        }
        Ok(first)
    }

    /// Records that the PTY took `size` now, at the stream offset of the next byte, and
    /// returns that offset. The size takes no stream bytes. The segment rotates first
    /// when the size chunk would take it past its limit and it holds stream bytes.
    pub async fn resize(&mut self, size: Size) -> Result<Seq, StoreError> {
        let at = sql::micros(self.clock.now());
        let segment = self.segment()?;
        let cost = format::RESIZE_CHUNK_LEN as u64;
        if segment.start_seq < self.end_seq && segment.file_len + cost > self.limit {
            self.rotate().await?;
        }
        let mut segment = self.take_segment()?;
        let (segment, written) = tokio::task::spawn_blocking(move || {
            let mut buf = Vec::with_capacity(format::RESIZE_CHUNK_LEN);
            format::encode_resize(at, size.cols, size.rows, &mut buf);
            let written = segment.write_encoded(&buf);
            (segment, written)
        })
        .await
        .map_err(|_| StoreError::TaskPanicked { task: "recording" })?;
        self.segment = Some(segment);
        written?;
        Ok(Seq::new(self.end_seq))
    }

    /// Flushes the open segment, records its size and closes it in the index, and
    /// returns the stream offset after the last byte.
    pub async fn close(mut self) -> Result<Seq, StoreError> {
        let segment = self.take_segment()?;
        let (pty_id, start_seq, end_seq) = (self.pty_id, segment.start_seq, self.end_seq);
        blocking(move || segment.flush()).await?;
        self.run_index(move |conn, at| {
            super::close_segment(conn, pty_id, start_seq, end_seq - start_seq, at)
        })
        .await?;
        Ok(Seq::new(end_seq))
    }

    /// Closes the full segment and opens the next one at the current end.
    async fn rotate(&mut self) -> Result<(), StoreError> {
        let full = self.take_segment()?;
        let (pty_id, old_start, new_start) = (self.pty_id, full.start_seq, self.end_seq);
        // The index says closed only once the bytes are on disk.
        blocking(move || full.flush()).await?;
        self.run_index(move |conn, at| {
            super::close_segment(conn, pty_id, old_start, new_start - old_start, at)?;
            super::insert_segment(conn, pty_id, new_start, at)
        })
        .await?;
        self.segment = Some(self.open_segment(new_start).await?.0);
        Ok(())
    }

    async fn open_segment(&self, start_seq: u64) -> Result<(OpenSegment, u64), StoreError> {
        let (root, pty_id) = (self.root.clone(), self.pty_id);
        blocking(move || OpenSegment::open(&root, pty_id, start_seq)).await
    }

    async fn run_index(
        &self,
        change: impl FnOnce(&rusqlite::Connection, jiff::Timestamp) -> Result<(), StoreError>
        + Send
        + 'static,
    ) -> Result<(), StoreError> {
        self.writer.run(move |state| state.write(change)).await
    }

    fn segment(&self) -> Result<&OpenSegment, StoreError> {
        self.segment.as_ref().ok_or(StoreError::RecordingBroken { pty_id: self.pty_id })
    }

    fn take_segment(&mut self) -> Result<OpenSegment, StoreError> {
        self.segment.take().ok_or(StoreError::RecordingBroken { pty_id: self.pty_id })
    }
}

impl OpenSegment {
    /// Opens, or creates, the segment of `pty_id` that starts at `start_seq`, cuts off
    /// a torn tail, and returns it with the number of stream bytes it holds.
    fn open(root: &Path, pty_id: PtyId, start_seq: u64) -> Result<(Self, u64), StoreError> {
        super::create_dir(root, pty_id)?;
        let path = root.join(super::relative_path(pty_id, start_seq));
        let opened = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(FILE_MODE)
            .open(&path)
            .and_then(|mut file| {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).map(|_| (file, bytes))
            });
        let (file, bytes) = match opened {
            Ok(opened) => opened,
            Err(source) => return Err(StoreError::RecordingIo { path, source }),
        };
        let parsed = match format::parse(&bytes) {
            Ok(parsed) => parsed,
            Err(offset) => {
                return Err(StoreError::CorruptRecording { path, offset: offset as u64 });
            }
        };
        if parsed.valid_len < bytes.len()
            && let Err(source) = file.set_len(parsed.valid_len as u64)
        {
            return Err(StoreError::RecordingIo { path, source });
        }
        let file_len = parsed.valid_len as u64;
        Ok((OpenSegment { file, path, start_seq, file_len }, parsed.stream_len()))
    }

    fn write(&mut self, at_micros: i64, data: &[u8]) -> Result<(), StoreError> {
        let chunks = data.len().div_ceil(format::MAX_CHUNK);
        let mut buf = Vec::with_capacity(data.len() + chunks * format::HEADER_LEN);
        format::encode(at_micros, data, &mut buf);
        self.write_encoded(&buf)
    }

    /// Appends whole chunks.
    fn write_encoded(&mut self, buf: &[u8]) -> Result<(), StoreError> {
        if let Err(source) = self.file.write_all(buf) {
            // A partial chunk in the middle of the file would hide every chunk after
            // it, so the file goes back to its last whole chunk.
            let _ = self.file.set_len(self.file_len);
            return Err(StoreError::RecordingIo { path: self.path.clone(), source });
        }
        self.file_len += buf.len() as u64;
        Ok(())
    }

    fn flush(self) -> Result<(), StoreError> {
        self.file.sync_data().map_err(|source| StoreError::RecordingIo { path: self.path, source })
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| StoreError::TaskPanicked { task: "recording" })?
}
