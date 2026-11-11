//! Timing marks for the latency benches: when a step on the way from a prompt to the
//! screen ran.
//!
//! The marks are off unless a bench turns them on. A mark that is off reads one atomic
//! and returns, so the marked code keeps its behaviour and its speed. No mark stands on
//! a hot path of `CONVENTIONS.md` (`feed`, `vt_write`, the PTY reader).
//!
//! Two places can keep the marks, and both can be on:
//!
//! - memory ([`enable`]): a bench that runs efrd in its own process reads them back
//!   with [`drain`], with an [`Instant`] for each mark;
//! - a file ([`enable_file`], or [`Var::Mark`] for a process
//!   that a bench starts): efrd and `efr` append to one file, one line for each mark
//!   with one `write`:
//!
//! ```text
//! <unix time in ns> <pid> <name> <a> <b>
//! ```
//!
//! A mark has a name, a detail (the method of a request, say), and two numbers. A bench
//! that follows tokens puts a marker in each one: `zq` and five digits. A mark of text
//! ([`text`]) keeps the largest marker in its bytes plus 1 in `a` (0 means "none") and
//! the count of the bytes in `b`. In the file a detail follows its name after a `:`.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

use crate::env::{self, Var};
use crate::time::{Clock as _, SystemClock};

/// The state before the first mark looked at the environment.
const UNKNOWN: u8 = 0;
/// Bits of the state: the marks are off, go to memory, go to the file.
const OFF: u8 = 1;
const MEMORY: u8 = 2;
const FILE: u8 = 4;

static STATE: AtomicU8 = AtomicU8::new(UNKNOWN);
static MARKS: Mutex<Vec<Mark>> = Mutex::new(Vec::new());
static SINK: OnceLock<File> = OnceLock::new();

/// One mark in memory.
#[derive(Debug, Clone, Copy)]
pub struct Mark {
    /// When the mark ran.
    pub at: Instant,
    /// The step.
    pub name: &'static str,
    /// What the step worked on; empty when the name says it all.
    pub detail: &'static str,
    /// The first number, such as the largest marker plus 1.
    pub a: u64,
    /// The second number, such as a count of bytes.
    pub b: u64,
}

/// Keeps the marks of this process in memory from now on, for [`drain`].
pub fn enable() {
    STATE.store((state() & !OFF) | MEMORY, Ordering::Release);
}

/// Appends the marks of this process to the file at `path` from now on.
///
/// # Errors
///
/// The file cannot be opened for an append.
pub fn enable_file(path: &Path) -> std::io::Result<()> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    // NOTE: a second call keeps the first file; one process has one file.
    let _ = SINK.set(file);
    STATE.store((STATE.load(Ordering::Acquire) & !OFF) | FILE, Ordering::Release);
    Ok(())
}

/// True when the marks go somewhere.
#[inline]
pub fn on() -> bool {
    state() & (MEMORY | FILE) != 0
}

#[inline]
fn state() -> u8 {
    match STATE.load(Ordering::Acquire) {
        UNKNOWN => from_env(),
        state => state,
    }
}

/// The state of a process whose bench did not turn the marks on: the file of
/// [`Var::Mark`], else off.
#[cold]
fn from_env() -> u8 {
    let path = env::path(Var::Mark).ok().flatten();
    if !path.is_some_and(|path| enable_file(&path).is_ok()) {
        // NOTE: an enable of another thread in the meantime wins.
        let _ = STATE.compare_exchange(UNKNOWN, OFF, Ordering::AcqRel, Ordering::Acquire);
    }
    STATE.load(Ordering::Acquire)
}

/// Records that the step `name` ran now.
#[inline]
pub fn hit(name: &'static str) {
    if on() {
        record(name, "", 0, 0);
    }
}

/// Records that the step `name` ran now for `detail`.
#[inline]
pub fn hit_for(name: &'static str, detail: &'static str) {
    if on() {
        record(name, detail, 0, 0);
    }
}

/// Records that the step `name` ran now with the numbers `a` and `b`.
#[inline]
pub fn value(name: &'static str, a: u64, b: u64) {
    if on() {
        record(name, "", a, b);
    }
}

/// Records that the step `name` ran now on `bytes`: the largest marker in them plus 1,
/// and their count.
#[inline]
pub fn text(name: &'static str, bytes: &[u8]) {
    if on() {
        record(name, "", scan(bytes), bytes.len() as u64);
    }
}

/// The largest marker in `bytes` plus 1; 0 when there is none or the marks are off.
#[inline]
pub fn marker(bytes: &[u8]) -> u64 {
    if on() { scan(bytes) } else { 0 }
}

/// The largest marker in `bytes` plus 1; 0 when there is none. It also works with the
/// marks off, for a bench's own reads.
pub fn scan(bytes: &[u8]) -> u64 {
    let mut best = 0;
    let mut at = 0;
    while at + 7 <= bytes.len() {
        if bytes[at] == b'z' && bytes[at + 1] == b'q' {
            let digits = &bytes[at + 2..at + 7];
            if digits.iter().all(u8::is_ascii_digit) {
                let id = digits.iter().fold(0_u64, |id, digit| id * 10 + u64::from(digit - b'0'));
                best = best.max(id + 1);
                at += 7;
                continue;
            }
        }
        at += 1;
    }
    best
}

/// The time of the file's marks: Unix time in nanoseconds, the same in every process.
pub fn now_ns() -> u64 {
    u64::try_from(SystemClock.now().as_nanosecond()).unwrap_or(0)
}

#[cold]
fn record(name: &'static str, detail: &'static str, a: u64, b: u64) {
    let state = state();
    if state & MEMORY != 0 {
        let mark = Mark { at: Instant::now(), name, detail, a, b };
        MARKS.lock().unwrap_or_else(PoisonError::into_inner).push(mark);
    }
    if state & FILE != 0
        && let Some(mut file) = SINK.get()
    {
        let (sep, detail) = if detail.is_empty() { ("", "") } else { (":", detail) };
        let line = format!("{} {} {name}{sep}{detail} {a} {b}\n", now_ns(), std::process::id());
        // NOTE: a mark that cannot be written is lost; the marked code must not fail.
        let _ = file.write_all(line.as_bytes());
    }
}

/// Takes every mark in memory so far, in the order of their time.
pub fn drain() -> Vec<Mark> {
    let mut marks = std::mem::take(&mut *MARKS.lock().unwrap_or_else(PoisonError::into_inner));
    marks.sort_by_key(|mark| mark.at);
    marks
}

#[cfg(test)]
mod tests;
