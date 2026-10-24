//! The collector of recordings: which segments go, and in which order.
//!
//! [`Recordings::collect`] runs one pass. The recording of a PTY whose shell exited more
//! than [`Retention::max_age`] ago goes whole. Then, while all recording files take more
//! than [`Retention::max_bytes`], the oldest segments go first. The newest segment of a
//! PTY that can still start a writer always stays, and so do the segments before it up
//! to [`Retention::live_tail`] stream bytes.
//!
//! A PTY can still start a writer while its shell runs, while the shells projection does
//! not know it yet, and for [`WRITER_GRACE`] after its exit: the reader drains the output
//! that is left after the exit, and that output gets a writer of its own.
//!
//! A pass lists the files first, then reads the rows, deletes the rows in one writer
//! transaction and unlinks the files last. A writer adds a segment's row before it
//! creates the file, so a listed file without a row lost its row to an earlier pass that
//! stopped before its unlink: an orphan, which this pass removes. A PTY whose recording
//! went whole keeps one row without a file at the offset after its last byte, so a read
//! of it reports that later start and a client knows that the bytes are gone.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use efr_protocol::{PtyId, Seq};
use jiff::{SignedDuration, Timestamp};
use rusqlite::{Connection, Row, params};

use super::{RawSegment, Recordings, Segment, format, relative_path};
use crate::shells::ShellStatus;
use crate::{StoreError, sql};

/// A segment by its PTY and its start offset.
pub(crate) type SegmentKey = (PtyId, u64);

/// How long after its shell's exit a PTY still counts as one that can start a writer.
pub const WRITER_GRACE: Duration = Duration::from_secs(3600);

/// What a pass of the collector keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Retention {
    /// The recording of a PTY whose shell exited more than this long ago goes whole;
    /// `None` keeps it.
    pub max_age: Option<Duration>,
    /// While all recording files take more than this many bytes, the oldest segments go
    /// first; `None` sets no limit.
    pub max_bytes: Option<u64>,
    /// The stream bytes at the end of a PTY that can still start a writer that stay with
    /// its newest segment.
    pub live_tail: u64,
}

impl Retention {
    /// Keeps the recordings of exited shells for `max_age`, all recordings within
    /// `max_bytes`, and `live_tail` stream bytes of each PTY that can still write.
    pub fn new(max_age: Option<Duration>, max_bytes: Option<u64>, live_tail: u64) -> Self {
        Retention { max_age, max_bytes, live_tail }
    }
}

/// What a pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Collected {
    /// The index rows that went.
    pub segments: u64,
    /// The PTYs whose recording went whole.
    pub ptys: u64,
    /// The files without an index row that went.
    pub orphans: u64,
    /// The bytes of the files that went.
    pub freed: u64,
    /// The bytes of the files that stay.
    pub held: u64,
    /// The files that could not be removed; the next pass removes them as orphans.
    pub failed: u64,
}

/// The rows that a pass deleted, and what its unlink step needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Pruned {
    /// The segments whose rows went, by PTY and start offset.
    pub(crate) deleted: Vec<SegmentKey>,
    /// The PTYs whose recording went whole.
    pub(crate) whole: Vec<PtyId>,
    /// The listed files without a row.
    pub(crate) orphans: Vec<SegmentKey>,
    /// The size of each listed file.
    sizes: HashMap<SegmentKey, u64>,
    held: u64,
}

impl Recordings {
    /// Runs one pass of the collector at the clock's time: deletes what `retention` does
    /// not keep, and the files that have no index row. Its cost grows with the number of
    /// segments.
    pub async fn collect(&self, retention: Retention) -> Result<Collected, StoreError> {
        let pruned = self.prune_index(retention).await?;
        let root = self.root.clone();
        super::writer::blocking(move || Ok(remove_files(&root, &pruned))).await
    }

    /// The first part of a pass: lists the files, reads the rows, and deletes the rows
    /// of the segments that go. The files stay until [`remove_files`].
    pub(crate) async fn prune_index(&self, retention: Retention) -> Result<Pruned, StoreError> {
        // Times in the store are whole microseconds.
        let now = sql::truncate_to_micros(self.clock.now());
        let root = self.root.clone();
        // NOTE: the files are listed before the rows are read. A file that the list
        // holds had its row then, so a file without a row in the later read is an
        // orphan and never the new segment of a writer.
        let files = super::writer::blocking(move || list_files(&root)).await?;
        let rows = self.readers.with(rows).await?;
        let root = self.root.clone();
        let plan = super::writer::blocking(move || {
            Ok(plan(&rows, &files, &retention, now, |segment| stream_end(&root, segment)))
        })
        .await?;
        let Plan { doomed, ends, orphans, sizes, held } = plan;
        let (deleted, whole) = self
            .writer
            .run(move |state| state.write(move |conn, at| prune(conn, &doomed, &ends, at)))
            .await?;
        Ok(Pruned { deleted, whole, orphans, sizes, held })
    }
}

/// A segment file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SegmentFile {
    pub(crate) pty_id: PtyId,
    pub(crate) start_seq: u64,
    pub(crate) len: u64,
}

/// A segment's row with the state of its shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SegmentRow {
    pub(crate) segment: Segment,
    /// The shell's status and exit time; `None` when the shells projection does not
    /// know the PTY.
    pub(crate) shell: Option<(ShellStatus, Option<Timestamp>)>,
}

/// One segment row that goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Doomed {
    pub(crate) pty_id: PtyId,
    pub(crate) start_seq: u64,
    /// The PTY can still start a writer: only a closed row before its newest goes.
    pub(crate) writable: bool,
}

/// What a pass deletes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) doomed: Vec<Doomed>,
    /// For each PTY whose recording goes whole, the offset after its last byte.
    pub(crate) ends: Vec<SegmentKey>,
    pub(crate) orphans: Vec<SegmentKey>,
    sizes: HashMap<SegmentKey, u64>,
    /// The bytes of the files with a row that stay.
    pub(crate) held: u64,
}

/// Every segment row with its shell, by PTY and then oldest first: one scan in the
/// order of the primary key.
pub(crate) fn rows(conn: &Connection) -> Result<Vec<SegmentRow>, StoreError> {
    let mut stmt = conn.prepare(ROWS)?;
    let raw = stmt.query_map([], RawRow::from_row)?.collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawRow::decode).collect()
}

/// The query of [`rows`].
pub(crate) const ROWS: &str = "SELECT r.pty_id, r.start_seq, r.path, r.bytes, r.started_at, r.closed_at, \
     s.status, s.exited_at FROM recording_segments AS r \
     LEFT JOIN shells AS s ON s.pty_id = r.pty_id ORDER BY r.pty_id, r.start_seq";

/// Decides what goes at `now`. `stream_end` gives the offset after the last byte of an
/// open segment, which its row does not hold.
pub(crate) fn plan(
    rows: &[SegmentRow],
    files: &[SegmentFile],
    retention: &Retention,
    now: Timestamp,
    mut stream_end: impl FnMut(&Segment) -> u64,
) -> Plan {
    let sizes: HashMap<SegmentKey, u64> =
        files.iter().map(|file| ((file.pty_id, file.start_seq), file.len)).collect();
    let size = |segment: &Segment| {
        sizes.get(&(segment.pty_id, segment.start_seq.get())).copied().unwrap_or(0)
    };
    let known: HashSet<SegmentKey> =
        rows.iter().map(|row| (row.segment.pty_id, row.segment.start_seq.get())).collect();
    let orphans = files
        .iter()
        .map(|file| (file.pty_id, file.start_seq))
        .filter(|key| !known.contains(key))
        .collect();

    let groups: Vec<&[SegmentRow]> =
        rows.chunk_by(|a, b| a.segment.pty_id == b.segment.pty_id).collect();
    let mut doomed = vec![0; groups.len()];
    let mut writable = vec![false; groups.len()];
    let mut held = 0;
    // The segments that the limit may take, oldest first in each PTY.
    let mut takeable: Vec<&[SegmentRow]> = Vec::with_capacity(groups.len());
    for (index, group) in groups.iter().enumerate() {
        let shell = group[0].shell;
        writable[index] = can_write(shell, now);
        if collected(group, &size) {
            takeable.push(&[]);
            continue;
        }
        if !writable[index] && retention.max_age.is_some_and(|age| aged(shell, now, age)) {
            doomed[index] = group.len();
            takeable.push(&[]);
            continue;
        }
        held += group.iter().map(|row| size(&row.segment)).sum::<u64>();
        let kept_from =
            if writable[index] { tail_start(group, retention.live_tail) } else { group.len() };
        takeable.push(&group[..kept_from]);
    }

    if let Some(max) = retention.max_bytes {
        // The oldest head of all PTYs goes first, so each PTY loses a prefix.
        let mut heads = BinaryHeap::new();
        for (index, takeable) in takeable.iter().enumerate() {
            if let Some(row) = takeable.first() {
                heads.push(Reverse((age_key(&row.segment), index)));
            }
        }
        while held > max {
            let Some(Reverse((_, index))) = heads.pop() else { break };
            let row = &takeable[index][doomed[index]];
            held -= size(&row.segment).min(held);
            doomed[index] += 1;
            if let Some(next) = takeable[index].get(doomed[index]) {
                heads.push(Reverse((age_key(&next.segment), index)));
            }
        }
    }

    let mut plan = Plan { orphans, held, ..Plan::default() };
    for (index, group) in groups.iter().enumerate() {
        for row in &group[..doomed[index]] {
            plan.doomed.push(Doomed {
                pty_id: row.segment.pty_id,
                start_seq: row.segment.start_seq.get(),
                writable: writable[index],
            });
        }
        if doomed[index] == group.len()
            && let Some(newest) = group.last()
        {
            let end = match newest.segment.closed_at {
                Some(_) => newest.segment.start_seq.get() + newest.segment.bytes,
                None => stream_end(&newest.segment),
            };
            plan.ends.push((newest.segment.pty_id, end));
        }
    }
    plan.sizes = sizes;
    plan
}

/// True while the PTY of a shell in `shell` can start a writer at `now`.
fn can_write(shell: Option<(ShellStatus, Option<Timestamp>)>, now: Timestamp) -> bool {
    match shell {
        Some((ShellStatus::Exited, Some(at))) => !older_than(at, now, WRITER_GRACE),
        // NOTE: a PTY that the projection does not know yet, or an exit without a time,
        // keeps what a running shell keeps.
        _ => true,
    }
}

/// True when the shell in `shell` exited more than `age` before `now`.
fn aged(shell: Option<(ShellStatus, Option<Timestamp>)>, now: Timestamp, age: Duration) -> bool {
    matches!(shell, Some((ShellStatus::Exited, Some(at))) if older_than(at, now, age))
}

/// True when `at` is more than `age` before `now`.
fn older_than(at: Timestamp, now: Timestamp, age: Duration) -> bool {
    let age = SignedDuration::try_from(age).unwrap_or(SignedDuration::MAX);
    now.duration_since(at) > age
}

/// True for a recording that went whole before: one row without a file or bytes.
fn collected(group: &[SegmentRow], size: &impl Fn(&Segment) -> u64) -> bool {
    match group {
        [row] => {
            let segment = &row.segment;
            segment.closed_at.is_some() && segment.bytes == 0 && size(segment) == 0
        }
        _ => false,
    }
}

/// The index of the first segment that stays for a PTY that can still write: the
/// newest, and the ones before it until they hold `live_tail` stream bytes. An open
/// segment counts as empty, so more stays.
fn tail_start(group: &[SegmentRow], live_tail: u64) -> usize {
    let Some(newest) = group.last() else { return 0 };
    let newest = &newest.segment;
    let end = newest.start_seq.get() + newest.closed_at.map_or(0, |_| newest.bytes);
    let mut from = group.len() - 1;
    while from > 0 && end - group[from].segment.start_seq.get().min(end) < live_tail {
        from -= 1;
    }
    from
}

/// The time by which the limit takes segments, oldest first: when the segment closed,
/// or when it opened for a segment that never closed.
fn age_key(segment: &Segment) -> (Timestamp, Seq) {
    (segment.closed_at.unwrap_or(segment.started_at), segment.start_seq)
}

/// The offset after the last whole chunk of the open segment `segment`, read from its
/// file. A file that cannot be read gives an offset after any byte it can hold.
fn stream_end(root: &Path, segment: &Segment) -> u64 {
    let start = segment.start_seq.get();
    match fs::read(root.join(&segment.path)) {
        Ok(bytes) => match format::parse(&bytes) {
            Ok(parsed) => start + parsed.stream_len(),
            Err(_) => start + bytes.len() as u64,
        },
        Err(_) => start,
    }
}

/// Deletes the rows of `doomed` and adds an empty row at each end of `ends`, inside the
/// writer's transaction; the rows that went and the PTYs that went whole.
///
/// A row of a PTY that can still write goes only when it is closed and not the newest
/// of its PTY, as the plan said; a row that changed since is left.
fn prune(
    conn: &Connection,
    doomed: &[Doomed],
    ends: &[SegmentKey],
    at: Timestamp,
) -> Result<(Vec<SegmentKey>, Vec<PtyId>), StoreError> {
    let mut delete = conn.prepare(
        "DELETE FROM recording_segments WHERE pty_id = ?1 AND start_seq = ?2 AND (?3 = 0 OR \
         (closed_at IS NOT NULL AND start_seq < \
         (SELECT MAX(start_seq) FROM recording_segments WHERE pty_id = ?1)))",
    )?;
    let mut deleted = Vec::with_capacity(doomed.len());
    for row in doomed {
        let start = sql::seq(Seq::new(row.start_seq));
        if delete.execute(params![row.pty_id.to_string(), start, row.writable])? > 0 {
            deleted.push((row.pty_id, row.start_seq));
        }
    }
    let mut insert = conn.prepare(
        "INSERT OR IGNORE INTO recording_segments \
         (pty_id, start_seq, path, bytes, started_at, closed_at) VALUES (?1, ?2, ?3, 0, ?4, ?4)",
    )?;
    let mut whole = Vec::with_capacity(ends.len());
    for (pty_id, end) in ends {
        let path = sql::path_text(&relative_path(*pty_id, *end));
        let at = sql::micros(at);
        insert.execute(params![pty_id.to_string(), sql::seq(Seq::new(*end)), path, at])?;
        whole.push(*pty_id);
    }
    Ok((deleted, whole))
}

/// The second part of a pass: unlinks the files of the rows that went and the orphans,
/// and the directories of the PTYs that went whole when they are empty. It blocks.
pub(crate) fn remove_files(root: &Path, pruned: &Pruned) -> Collected {
    let mut collected = Collected {
        segments: pruned.deleted.len() as u64,
        ptys: pruned.whole.len() as u64,
        held: pruned.held,
        ..Collected::default()
    };
    for (pty_id, start_seq) in &pruned.deleted {
        let len = pruned.sizes.get(&(*pty_id, *start_seq)).copied().unwrap_or(0);
        match remove(root, *pty_id, *start_seq) {
            Ok(true) => collected.freed += len,
            Ok(false) => {}
            Err(_) => collected.failed += 1,
        }
    }
    for (pty_id, start_seq) in &pruned.orphans {
        let len = pruned.sizes.get(&(*pty_id, *start_seq)).copied().unwrap_or(0);
        match remove(root, *pty_id, *start_seq) {
            Ok(true) => {
                collected.orphans += 1;
                collected.freed += len;
            }
            Ok(false) => {}
            Err(_) => collected.failed += 1,
        }
    }
    for pty_id in &pruned.whole {
        // A directory that still holds a file stays.
        let _ = fs::remove_dir(root.join(pty_id.to_string()));
    }
    collected
}

/// Unlinks the segment file of `pty_id` at `start_seq`; false when it was not there.
fn remove(root: &Path, pty_id: PtyId, start_seq: u64) -> io::Result<bool> {
    match fs::remove_file(root.join(relative_path(pty_id, start_seq))) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Every segment file under `root`: `<pty_id>/<start_seq>.rec`, where both names are in
/// the form that the writer gives them. Other entries and links are left alone.
pub(crate) fn list_files(root: &Path) -> Result<Vec<SegmentFile>, StoreError> {
    let mut files = Vec::new();
    let Some(dirs) = read_dir(root)? else { return Ok(files) };
    for dir in dirs {
        let dir = dir.map_err(|source| io_error(root, source))?;
        let is_dir = dir.file_type().is_ok_and(|kind| kind.is_dir());
        let Some(pty_id) = dir.file_name().to_str().and_then(pty_name).filter(|_| is_dir) else {
            continue;
        };
        let path = dir.path();
        let Some(entries) = read_dir(&path)? else { continue };
        for entry in entries {
            let entry = entry.map_err(|source| io_error(&path, source))?;
            let Some(start_seq) = entry.file_name().to_str().and_then(segment_name) else {
                continue;
            };
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => return Err(io_error(&entry.path(), source)),
            };
            if meta.is_file() {
                files.push(SegmentFile { pty_id, start_seq, len: meta.len() });
            }
        }
    }
    Ok(files)
}

/// The entries of `dir`, or `None` when it is gone.
fn read_dir(dir: &Path) -> Result<Option<fs::ReadDir>, StoreError> {
    match fs::read_dir(dir) {
        Ok(entries) => Ok(Some(entries)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(dir, source)),
    }
}

fn io_error(path: &Path, source: io::Error) -> StoreError {
    StoreError::RecordingIo { path: path.to_path_buf(), source }
}

/// The PTY of a directory name that is the writer's form of its id.
fn pty_name(name: &str) -> Option<PtyId> {
    name.parse::<PtyId>().ok().filter(|pty_id| pty_id.to_string() == name)
}

/// The start offset of a file name that is the writer's `<start_seq>.rec`.
fn segment_name(name: &str) -> Option<u64> {
    let digits = name.strip_suffix(".rec")?;
    digits.parse::<u64>().ok().filter(|start| start.to_string() == digits)
}

struct RawRow {
    segment: RawSegment,
    status: Option<String>,
    exited_at: Option<i64>,
}

impl RawRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawRow {
            segment: RawSegment::from_row(row)?,
            status: row.get(6)?,
            exited_at: row.get(7)?,
        })
    }

    fn decode(self) -> Result<SegmentRow, StoreError> {
        let segment = self.segment.decode()?;
        let shell = match self.status {
            Some(status) => {
                let status = ShellStatus::from_column(&status)?;
                let exited_at = self
                    .exited_at
                    .map(|value| sql::to_timestamp(value, "shells", "exited_at"))
                    .transpose()?;
                Some((status, exited_at))
            }
            None => None,
        };
        Ok(SegmentRow { segment, shell })
    }
}

#[cfg(test)]
mod tests;
