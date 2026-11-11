//! The measure points of the streaming bench (`follow/tests/stream.rs`): what each
//! frame of the follow loop cost and wrote, what the live zone erased and wrote
//! again, and when each draft reached the view.
//!
//! The points record only while a probe is on in the thread ([`start`]), so they change
//! nothing for the other tests, and they exist only in test builds.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use rustix::time::{ClockId, clock_gettime};

/// What one `LiveZone::draw` did.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Zone {
    /// The draw wrote nothing: the live zone did not change.
    pub(crate) empty: bool,
    /// Only the status row was written again.
    pub(crate) status_only: bool,
    /// The rows of committed output that the draw wrote once.
    pub(crate) committed_rows: usize,
    /// The rows of the old live zone that the draw erased.
    pub(crate) erased_rows: usize,
    /// The rows of the live zone after the draw.
    pub(crate) live_rows: usize,
    /// The rows of the live zone that the clip to the screen keeps off the screen.
    pub(crate) hidden_rows: usize,
    /// The bytes of the live zone that the clip keeps off the screen.
    pub(crate) hidden_bytes: usize,
}

/// One frame of the follow loop.
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    /// When the frame was made (after the view returned it).
    pub(crate) at: Instant,
    /// The wall time and the CPU time of the thread that the view took to make it.
    pub(crate) wall: Duration,
    pub(crate) cpu: Duration,
    /// A tick's frame.
    pub(crate) tick: bool,
    /// The bytes of the frame.
    pub(crate) text: String,
    /// What the live zone did in this frame; `None` when it was not drawn.
    pub(crate) zone: Option<Zone>,
}

/// One text draft as it reached the view.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Arrival {
    pub(crate) at: Instant,
    /// The length of the message's text after this draft, in bytes.
    pub(crate) end: usize,
}

/// What a probe recorded.
#[derive(Debug, Default)]
pub(crate) struct Probe {
    pub(crate) arrivals: Vec<Arrival>,
    pub(crate) frames: Vec<Frame>,
    zone: Option<Zone>,
}

thread_local! {
    static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

/// The CPU time of this thread so far.
pub(crate) fn thread_cpu() -> Duration {
    let time = clock_gettime(ClockId::ThreadCPUTime);
    Duration::new(u64::try_from(time.tv_sec).unwrap_or(0), u32::try_from(time.tv_nsec).unwrap_or(0))
}

/// Turns the probe of this thread on.
pub(crate) fn start() {
    PROBE.with(|probe| *probe.borrow_mut() = Some(Probe::default()));
}

/// Turns the probe of this thread off and gives what it recorded.
pub(crate) fn take() -> Probe {
    PROBE.with(|probe| probe.borrow_mut().take()).unwrap_or_default()
}

/// A text draft reached the view; the message's text is `end` bytes long with it.
pub(crate) fn arrival(end: usize) {
    PROBE.with(|probe| {
        if let Some(probe) = probe.borrow_mut().as_mut() {
            probe.arrivals.push(Arrival { at: Instant::now(), end });
        }
    });
}

/// The live zone drew.
pub(crate) fn zone(zone: Zone) {
    PROBE.with(|probe| {
        if let Some(probe) = probe.borrow_mut().as_mut() {
            probe.zone = Some(zone);
        }
    });
}

/// The start of a frame: the wall clock and the CPU clock, when a probe is on.
pub(crate) fn frame_begin() -> Option<(Instant, Duration)> {
    PROBE.with(|probe| probe.borrow().is_some()).then(|| (Instant::now(), thread_cpu()))
}

/// The end of the frame that started at `begin`.
pub(crate) fn frame_end(begin: Option<(Instant, Duration)>, text: &str, tick: bool) {
    let Some((start, cpu_start)) = begin else { return };
    let cpu = thread_cpu().saturating_sub(cpu_start);
    let at = Instant::now();
    PROBE.with(|probe| {
        if let Some(probe) = probe.borrow_mut().as_mut() {
            let zone = probe.zone.take();
            probe.frames.push(Frame {
                at,
                wall: at.duration_since(start),
                cpu,
                tick,
                text: text.to_owned(),
                zone,
            });
        }
    });
}
