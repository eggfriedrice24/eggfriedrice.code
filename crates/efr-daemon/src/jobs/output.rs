//! A job's output (efr's auto spec, section 3.18): efrd reads the launcher's stdout, a
//! pipe, and writes `$SCRATCH/jobs/<id>.out` itself.
//!
//! The file keeps the first [`HEAD_BYTES`] of the output and the rest after them. When
//! the rest passes [`REST_BYTES`], efrd writes a new file with the head, the line
//! `[N bytes omitted]` and the last [`KEEP_BYTES`], and renames it over the old one, so
//! a reader never sees half a file. Every open goes below the descriptor of
//! `$SCRATCH/jobs` with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`: a link that a call
//! planted is never followed. Every contained call sees `$SCRATCH/jobs` read-only, so no
//! command can cut, change or replace an output.
//!
//! The same reader keeps the last [`TAIL_BYTES`] for the job's end, and splits the
//! output into lines of at most [`MAX_LINE_BYTES`] for a `watch` pattern. A write that
//! fails (a full disk, a refused open) stops the file, never the reader: it drains the
//! pipe to its end, so the job never blocks on a full pipe, and the end says
//! `output_truncated`.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::FileExt as _;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use rustix::fs::{Mode, OFlags, ResolveFlags};
use tokio::sync::{mpsc, watch};

use crate::sandbox::fs::DaemonFs;

/// The bytes at the start of the output that the file always keeps.
pub(crate) const HEAD_BYTES: u64 = 1024 * 1024;

/// The bytes after the head that the file holds before efrd cuts it.
pub(crate) const REST_BYTES: u64 = 16 * 1024 * 1024;

/// The last bytes of the output that a cut file keeps.
pub(crate) const KEEP_BYTES: u64 = 8 * 1024 * 1024;

/// The last bytes of the output that the end of a job names.
pub(crate) const TAIL_BYTES: usize = 8 * 1024;

/// The last lines of the output that the end of a job names.
pub(crate) const TAIL_LINES: usize = 50;

/// The longest line that a watch pattern reads; the rest of a longer line is cut.
pub(crate) const MAX_LINE_BYTES: usize = 4 * 1024;

/// The marker file that efrd keeps in `$SCRATCH/jobs`, so it knows that it made the dir.
pub(crate) const MARKER_FILE: &str = ".efr-jobs";

/// The longest watch pattern, in characters.
pub(crate) const MAX_PATTERN_CHARS: usize = efr_protocol::Watch::MAX_PATTERN_CHARS;

/// The largest compiled watch pattern.
const PATTERN_SIZE_LIMIT: usize = 1024 * 1024;

/// How many bytes the reader takes from the pipe at a time.
const READ_CHUNK: usize = 64 * 1024;

/// The stack of a reader thread: its loop is shallow.
const READER_STACK: usize = 256 * 1024;

/// The sizes of the output file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    /// The head that the file always keeps.
    pub(crate) head: u64,
    /// The bytes after the head before a cut.
    pub(crate) rest: u64,
    /// The last bytes that a cut keeps.
    pub(crate) keep: u64,
}

impl Limits {
    /// The sizes of efrd: [`HEAD_BYTES`], [`REST_BYTES`] and [`KEEP_BYTES`].
    pub(crate) const DEFAULT: Limits =
        Limits { head: HEAD_BYTES, rest: REST_BYTES, keep: KEEP_BYTES };
}

/// The output file of one job, below the descriptor of `$SCRATCH/jobs`.
#[derive(Debug)]
pub(crate) struct OutputFile {
    dir: OwnedFd,
    name: OsString,
    file: File,
    limits: Limits,
    /// The bytes of the head in the file.
    head: u64,
    /// The bytes of the `[N bytes omitted]` line in the file, 0 before the first cut.
    marker: u64,
    /// The bytes after the head and the marker.
    body: u64,
    /// The bytes that cuts left out, in all.
    omitted: u64,
}

impl OutputFile {
    /// Creates `name` (0600) in `$SCRATCH/jobs` of `scratch`, whose dir efrd made
    /// ([`make_dir`]). An old file of that name is cut to nothing.
    pub(crate) fn create(scratch: &Path, name: &str, limits: Limits) -> io::Result<OutputFile> {
        let dir = open_jobs_dir(scratch)?;
        let file = open_beneath(&dir, OsStr::new(name), OFlags::TRUNC)?;
        Ok(OutputFile {
            dir,
            name: OsString::from(name),
            file,
            limits,
            head: 0,
            marker: 0,
            body: 0,
            omitted: 0,
        })
    }

    /// Adds `bytes` at the end; cuts the file when its rest passes the limit.
    pub(crate) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.write_all(bytes)?;
        let len = bytes.len() as u64;
        let to_head = len.min(self.limits.head.saturating_sub(self.head));
        self.head += to_head;
        self.body += len - to_head;
        if self.body > self.limits.rest {
            self.cut()?;
        }
        Ok(())
    }

    /// Writes a new file with the head, the marker and the last bytes of the body,
    /// and renames it over the old one.
    fn cut(&mut self) -> io::Result<()> {
        let keep = self.limits.keep.min(self.body);
        self.omitted += self.body - keep;
        let marker = format!("\n[{} bytes omitted]\n", self.omitted);
        let mut temp_name = self.name.clone();
        temp_name.push(".tmp");
        let mut temp = open_beneath(&self.dir, &temp_name, OFlags::TRUNC)?;
        copy_range(&self.file, 0, self.head, &mut temp)?;
        temp.write_all(marker.as_bytes())?;
        let from = self.head + self.marker + self.body - keep;
        copy_range(&self.file, from, keep, &mut temp)?;
        rustix::fs::renameat(&self.dir, temp_name.as_os_str(), &self.dir, self.name.as_os_str())?;
        self.file = temp;
        self.marker = marker.len() as u64;
        self.body = keep;
        Ok(())
    }
}

/// Copies `len` bytes of `from` at `offset` to the end of `to`.
fn copy_range(from: &File, offset: u64, len: u64, to: &mut File) -> io::Result<()> {
    let mut buffer = vec![0_u8; READ_CHUNK];
    let mut done = 0_u64;
    while done < len {
        let want = usize::try_from((len - done).min(READ_CHUNK as u64)).unwrap_or(READ_CHUNK);
        let read = from.read_at(&mut buffer[..want], offset + done)?;
        if read == 0 {
            break;
        }
        to.write_all(&buffer[..read])?;
        done += read as u64;
    }
    Ok(())
}

/// Opens `$SCRATCH/jobs` of `scratch` with no link on the way.
fn open_jobs_dir(scratch: &Path) -> io::Result<OwnedFd> {
    let scratch = DaemonFs::open(scratch, OFlags::RDONLY | OFlags::DIRECTORY)?;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    Ok(rustix::fs::openat2(
        &scratch,
        efr_sandbox::JOB_DIR,
        flags,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?)
}

/// Opens `name` below `dir` for reading and writing (0600), with `extra` flags.
fn open_beneath(dir: &OwnedFd, name: &OsStr, extra: OFlags) -> io::Result<File> {
    // NOTE: read and write: a cut copies the head and the end of the file it replaces.
    let flags = OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW | extra;
    let fd = rustix::fs::openat2(
        dir,
        name,
        flags,
        Mode::from_raw_mode(0o600),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?;
    Ok(File::from(fd))
}

/// Makes `$SCRATCH/jobs` of `scratch` (0700) with efrd's marker file, below the
/// descriptor of `scratch`. An entry `jobs` without the marker (a call made it before
/// efrd did) goes aside to `jobs.moved-<n>` first. A `scratch` that does not exist yet
/// holds nothing to make. It blocks.
pub(crate) fn make_dir(scratch: &Path) -> io::Result<()> {
    let scratch = match DaemonFs::open(scratch, OFlags::RDONLY | OFlags::DIRECTORY) {
        Ok(fd) => fd,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let name = efr_sandbox::JOB_DIR;
    let beneath = ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS;
    let marked = |dir: &OwnedFd| {
        rustix::fs::openat2(
            dir,
            MARKER_FILE,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
            beneath,
        )
        .is_ok()
    };
    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    match rustix::fs::statat(&scratch, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            let is_dir = rustix::fs::FileType::from_raw_mode(stat.st_mode)
                == rustix::fs::FileType::Directory;
            let ours = is_dir
                && rustix::fs::openat2(&scratch, name, dir_flags, Mode::empty(), beneath)
                    .is_ok_and(|dir| marked(&dir));
            if ours {
                return Ok(());
            }
            move_aside(&scratch, name)?;
        }
        Err(rustix::io::Errno::NOENT) => {}
        Err(errno) => return Err(errno.into()),
    }
    match rustix::fs::mkdirat(&scratch, name, Mode::from_raw_mode(0o700)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(errno) => return Err(errno.into()),
    }
    let dir = rustix::fs::openat2(&scratch, name, dir_flags, Mode::empty(), beneath)?;
    let marker_flags = OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    rustix::fs::openat2(&dir, MARKER_FILE, marker_flags, Mode::from_raw_mode(0o600), beneath)?;
    Ok(())
}

/// Renames the entry `name` of `dir` to the first free `name.moved-<n>`.
fn move_aside(dir: &OwnedFd, name: &str) -> io::Result<()> {
    for n in 1..1000_u32 {
        let aside = format!("{name}.moved-{n}");
        match rustix::fs::statat(dir, aside.as_str(), rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => continue,
            Err(rustix::io::Errno::NOENT) => {
                rustix::fs::renameat(dir, name, dir, aside.as_str())?;
                tracing::warn!(
                    aside,
                    "a jobs entry in scratch that efrd did not make was moved aside"
                );
                return Ok(());
            }
            Err(errno) => return Err(errno.into()),
        }
    }
    Err(io::Error::other("no free name to move the jobs entry aside"))
}

/// A watch pattern, compiled once for the job.
#[derive(Debug, Clone)]
pub(crate) struct Matcher(regex::Regex);

impl Matcher {
    /// The pattern `pattern`, or what the model reads when it is too long or no regular
    /// expression.
    pub(crate) fn new(pattern: &str) -> Result<Matcher, String> {
        if pattern.chars().count() > MAX_PATTERN_CHARS {
            return Err(format!(
                "the watch pattern has more than {MAX_PATTERN_CHARS} characters; use a shorter one"
            ));
        }
        regex::RegexBuilder::new(pattern)
            .size_limit(PATTERN_SIZE_LIMIT)
            .build()
            .map(Matcher)
            .map_err(|error| {
                format!("the watch pattern is not a valid regular expression: {error}")
            })
    }

    fn is_match(&self, line: &str) -> bool {
        self.0.is_match(line)
    }
}

/// What a job's reader knows so far; the job and the tools read it while the job runs.
#[derive(Debug, Default)]
pub(crate) struct Progress {
    /// The bytes that the job wrote to its output.
    pub(crate) bytes: u64,
    /// The last [`TAIL_BYTES`] of the output.
    tail: VecDeque<u8>,
    /// True when the file holds less than the output, such as on a full disk.
    pub(crate) truncated: bool,
}

impl Progress {
    /// The last [`TAIL_LINES`] lines of the output, within [`TAIL_BYTES`].
    pub(crate) fn tail(&self) -> String {
        let bytes: Vec<u8> = self.tail.iter().copied().collect();
        last_lines(&bytes, TAIL_LINES)
    }

    fn take(&mut self, bytes: &[u8]) {
        self.bytes += bytes.len() as u64;
        let start = bytes.len().saturating_sub(TAIL_BYTES);
        self.tail.extend(&bytes[start..]);
        let over = self.tail.len().saturating_sub(TAIL_BYTES);
        self.tail.drain(..over);
    }
}

/// The last `lines` lines of `bytes` as text, with no last empty line.
pub(crate) fn last_lines(bytes: &[u8], lines: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    if text.is_empty() {
        return String::new();
    }
    let kept: Vec<&str> = text.rsplit('\n').take(lines).collect();
    kept.into_iter().rev().collect::<Vec<_>>().join("\n")
}

/// The shared view of a job's output, and the end of its reader.
#[derive(Debug, Clone)]
pub(crate) struct Output {
    progress: Arc<Mutex<Progress>>,
    done: watch::Receiver<bool>,
}

impl Output {
    /// The bytes so far.
    pub(crate) fn bytes(&self) -> u64 {
        self.lock().bytes
    }

    /// The last lines so far.
    pub(crate) fn tail(&self) -> String {
        self.lock().tail()
    }

    /// True when the file holds less than the output.
    pub(crate) fn truncated(&self) -> bool {
        self.lock().truncated
    }

    /// Waits until the reader read the pipe to its end.
    pub(crate) async fn ended(&self) {
        let mut done = self.done.clone();
        let _ = done.wait_for(|done| *done).await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Progress> {
        // The counts are replaced whole, so a poisoned lock still holds usable ones.
        self.progress.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Splits the output into lines for a watch pattern.
#[derive(Debug, Default)]
struct Lines {
    line: Vec<u8>,
    /// True while the rest of a line longer than [`MAX_LINE_BYTES`] is dropped.
    cut: bool,
}

impl Lines {
    /// Feeds `bytes`, and hands each whole line to `each`.
    fn feed(&mut self, bytes: &[u8], each: &mut dyn FnMut(&[u8])) {
        for &byte in bytes {
            if byte == b'\n' {
                each(&self.line);
                self.line.clear();
                self.cut = false;
            } else if self.line.len() < MAX_LINE_BYTES {
                self.line.push(byte);
            } else {
                self.cut = true;
            }
        }
    }

    /// The last line, when the output did not end with a newline.
    fn finish(&mut self, each: &mut dyn FnMut(&[u8])) {
        if !self.line.is_empty() {
            each(&self.line);
            self.line.clear();
        }
    }
}

/// A line as text, cut to [`MAX_LINE_BYTES`] on a character boundary.
fn line_text(line: &[u8]) -> String {
    let text = String::from_utf8_lossy(line);
    let mut end = text.len().min(MAX_LINE_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Starts the reader of `pipe` on a thread of its own: it writes `file` (when there is
/// one), keeps the tail, and sends each line that `matcher` matches to `matches`. The
/// thread ends at the pipe's end.
pub(crate) fn spawn_reader(
    name: &str,
    mut pipe: impl io::Read + Send + 'static,
    file: Option<OutputFile>,
    matcher: Option<Matcher>,
    matches: mpsc::UnboundedSender<String>,
) -> io::Result<Output> {
    let progress = Arc::new(Mutex::new(Progress::default()));
    let (done_tx, done) = watch::channel(false);
    let shared = Arc::clone(&progress);
    if file.is_none() {
        shared.lock().unwrap_or_else(PoisonError::into_inner).truncated = true;
    }
    let mut file = file;
    efr_stdx::thread::spawn_named(format!("job-{name}"), READER_STACK, move || {
        let mut buffer = vec![0_u8; READ_CHUNK];
        let mut lines = Lines::default();
        let mut send = |line: &[u8]| {
            if let Some(matcher) = &matcher {
                let text = line_text(line);
                if matcher.is_match(&text) {
                    let _ = matches.send(text);
                }
            }
        };
        loop {
            let read = match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    tracing::warn!(%error, "reading a job's output failed");
                    break;
                }
            };
            let bytes = &buffer[..read];
            let mut failed = false;
            if let Some(out) = &mut file
                && let Err(error) = out.write(bytes)
            {
                tracing::warn!(%error, "a job's output file could not be written; the rest is dropped");
                failed = true;
            }
            if failed {
                file = None;
            }
            {
                let mut progress = shared.lock().unwrap_or_else(PoisonError::into_inner);
                progress.take(bytes);
                progress.truncated |= failed;
            }
            lines.feed(bytes, &mut send);
        }
        lines.finish(&mut send);
        let _ = done_tx.send(true);
    })
    .map_err(io::Error::other)?;
    Ok(Output { progress, done })
}

/// What the `jobs` tool shows of an output file: from byte `offset` on, or else the
/// last `lines` lines, at most `max` bytes. It blocks.
pub(crate) fn read(
    path: &Path,
    offset: Option<u64>,
    lines: usize,
    max: usize,
) -> io::Result<String> {
    let mut file = File::from(DaemonFs::open(path, OFlags::RDONLY)?);
    let size = file.metadata()?.len();
    match offset {
        Some(offset) => {
            let mut buffer = vec![0_u8; max];
            let read = file.read_at(&mut buffer, offset.min(size))?;
            buffer.truncate(read);
            Ok(String::from_utf8_lossy(&buffer).into_owned())
        }
        None => {
            // NOTE: the last lines lie in the last bytes: up to `max` per line is more
            // than any tail the tool shows.
            let from = size.saturating_sub((max as u64).saturating_mul(4));
            let mut bytes = Vec::new();
            io::Seek::seek(&mut file, io::SeekFrom::Start(from))?;
            file.read_to_end(&mut bytes)?;
            let text = last_lines(&bytes, lines);
            let cut = efr_tools::truncate_middle(&text, max);
            Ok(cut.text)
        }
    }
}

/// The name of the output file of the job `id` in `$SCRATCH/jobs`.
pub(crate) fn file_name(id: &efr_protocol::JobId) -> String {
    format!("{id}.out")
}

/// `env` as the bytes of an `env` file: NUL-separated `NAME=value` entries.
pub(crate) fn env_bytes(env: &std::collections::BTreeMap<String, String>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, value) in env {
        bytes.extend_from_slice(OsStr::new(name).as_bytes());
        bytes.push(b'=');
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
    }
    bytes
}

#[cfg(test)]
mod tests;
