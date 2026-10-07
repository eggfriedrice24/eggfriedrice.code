//! The status row of a running turn: the last row of the live zone, with a spinner, what
//! the turn does now and how long it has run, such as `⠼ thinking: Reading the logs  4s`.
//!
//! The row exists only while the turn runs on a terminal, and it goes before the prompt
//! comes back. It hides while the user is asked something, and the time stops then.
//!
//! Events only change the state. The times come from the frame that shows the row
//! ([`Status::at`]): the follow loop writes a frame within one frame time of every
//! change, so the view needs no clock of its own.

use std::time::Duration;

use efr_render::{RenderOptions, Role};
use jiff::Timestamp;

use crate::format;

/// The frames of the spinner, one per tick: one turn a second. Braille takes one column
/// in every font, so the row never changes its width.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The spinner without motion.
const STILL: char = '•';

/// One tick: a frame of the spinner and a step of the band.
pub(crate) const TICK: Duration = Duration::from_millis(100);

/// How long the model may send nothing before the row says so.
pub(crate) const STALL: Duration = Duration::from_secs(20);

/// The characters in the text role that move over the muted state.
const BAND: usize = 3;

/// The ticks the band waits after it crossed the state: one second.
const REST: usize = 10;

/// The time from which the row shows how long the turn has run.
const SHOW_ELAPSED: Duration = Duration::from_secs(1);

/// What the turn does now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum State {
    /// The turn waits behind the running one.
    Queued,
    /// The turn waits for the next answer of the model.
    Model,
    /// The model reasons, under the newest title of its reasoning when there is one.
    Thinking(Option<String>),
    /// The model writes text.
    Writing,
    /// The model writes the input of call `call` of its answer, `bytes` so far.
    Preparing { call: u32, tool: String, bytes: u64 },
    /// A tool call runs.
    Tool(String),
    /// An approval waits for an answer from another client.
    Answer,
}

impl State {
    /// The state in words; `silence` is how long the model has sent nothing.
    fn words(&self, silence: Duration) -> String {
        match self {
            State::Model | State::Writing if silence >= STALL => {
                format!("waiting for the model, no data for {}", format::elapsed(silence))
            }
            State::Queued => "waiting for the running turn".to_owned(),
            State::Model => "waiting for the model".to_owned(),
            State::Thinking(None) => "thinking".to_owned(),
            State::Thinking(Some(title)) => format!("thinking: {title}"),
            State::Writing => "writing".to_owned(),
            State::Preparing { tool, bytes: 0, .. } => format!("preparing {tool}"),
            State::Preparing { tool, bytes, .. } => {
                format!("preparing {tool}, {}", format::size(*bytes))
            }
            State::Tool(tool) => format!("running {tool}"),
            State::Answer => "waiting for an answer".to_owned(),
        }
    }
}

/// The status row's state and its times.
#[derive(Debug)]
pub(crate) struct Status {
    state: State,
    /// The spinner turns and the band moves.
    motion: bool,
    /// When the row was first drawn: the spinner and the band count from here.
    shown: Option<Timestamp>,
    /// When the turn started; the time it has run counts from here.
    started: Option<Timestamp>,
    /// The turn started since the last frame.
    starting: bool,
    /// The time the user was asked something, which does not count.
    paused: Duration,
    /// Since when the user is asked something, while that lasts.
    pause: Option<Timestamp>,
    /// The last sign of life of the model.
    data: Option<Timestamp>,
    /// A sign of life came since the last frame.
    stirred: bool,
}

impl Status {
    /// A row that waits for the model, with motion or without.
    pub(crate) fn new(motion: bool) -> Status {
        Status {
            state: State::Model,
            motion,
            shown: None,
            started: None,
            starting: false,
            paused: Duration::ZERO,
            pause: None,
            data: None,
            stirred: false,
        }
    }

    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    /// The turn does `state` now.
    pub(crate) fn set(&mut self, state: State) {
        self.state = state;
    }

    /// The turn started: its time counts from the next frame.
    pub(crate) fn turn_started(&mut self) {
        self.starting = true;
        self.state = State::Model;
    }

    /// The model showed a sign of life: an event or a draft of the turn.
    pub(crate) fn stir(&mut self) {
        self.stirred = true;
    }

    /// Brings the times up to `now`, the time of the frame being drawn; `asking` while
    /// the user is asked something.
    pub(crate) fn at(&mut self, now: Timestamp, asking: bool) {
        self.shown.get_or_insert(now);
        if std::mem::take(&mut self.starting) {
            self.started = Some(now);
            self.paused = Duration::ZERO;
            self.pause = asking.then_some(now);
        }
        if std::mem::take(&mut self.stirred) || self.data.is_none() {
            self.data = Some(now);
        }
        match (asking, self.pause) {
            (true, None) => self.pause = Some(now),
            (false, Some(since)) => {
                self.pause = None;
                self.paused += since_then(since, now);
            }
            _ => {}
        }
    }

    /// How long the turn has run at `now`, without the time the user was asked
    /// something; zero before it started.
    pub(crate) fn elapsed(&self, now: Timestamp) -> Duration {
        let Some(started) = self.started else {
            return Duration::ZERO;
        };
        let asked = self.pause.map_or(Duration::ZERO, |since| since_then(since, now));
        since_then(started, now).saturating_sub(self.paused).saturating_sub(asked)
    }

    /// The row at `now`, painted with `options` and cut to their width, with its
    /// newline.
    pub(crate) fn row(&self, now: Timestamp, options: &RenderOptions) -> String {
        let ticks = self.shown.map_or(0, |shown| ticks(since_then(shown, now)));
        let spinner = spinner(self.motion, ticks);
        let silence = self.data.map_or(Duration::ZERO, |data| since_then(data, now));
        let elapsed = self.elapsed(now);
        let suffix = (elapsed >= SHOW_ELAPSED).then(|| format::elapsed(elapsed));
        let columns = usize::from(options.width());
        // The spinner, a space, and the time after two spaces.
        let room = columns.saturating_sub(2 + suffix.as_ref().map_or(0, |time| time.len() + 2));
        let words = if room == 0 {
            String::new()
        } else {
            format::cut(&self.state.words(silence), room, options.width_method())
        };
        let mut row = options.paint(Role::Accent, &spinner.to_string());
        if !words.is_empty() {
            row.push(' ');
            let band = self.motion.then(|| band(ticks, words.chars().count())).flatten();
            row.push_str(&shimmer(&words, band, options));
        }
        if let Some(time) = suffix {
            row.push_str("  ");
            row.push_str(&options.paint(Role::Muted, &time));
        }
        row.push('\n');
        row
    }
}

impl Status {
    /// The frame of the spinner at `now`, which a running call's line shows in place of
    /// the row.
    pub(crate) fn spinner(&self, now: Timestamp) -> char {
        spinner(self.motion, self.shown.map_or(0, |shown| ticks(since_then(shown, now))))
    }
}

/// The frame of the spinner at tick `ticks`, or the still dot without `motion`.
pub(crate) fn spinner(motion: bool, ticks: usize) -> char {
    if motion { SPINNER[ticks % SPINNER.len()] } else { STILL }
}

/// The time from `then` to `now`; zero when `now` is earlier.
fn since_then(then: Timestamp, now: Timestamp) -> Duration {
    Duration::try_from(now.duration_since(then)).unwrap_or_default()
}

/// The ticks in `time`.
fn ticks(time: Duration) -> usize {
    usize::try_from(time.as_millis() / TICK.as_millis()).unwrap_or(usize::MAX)
}

/// The characters of a state `length` characters long that the band covers at tick
/// `ticks`: it comes in from the left one character per tick, leaves on the right,
/// then rests for a second. `None` while it rests.
fn band(ticks: usize, length: usize) -> Option<std::ops::Range<usize>> {
    let step = ticks % (length + BAND + REST);
    if step == 0 || step >= length + BAND {
        return None;
    }
    let end = step.min(length);
    Some(step.saturating_sub(BAND)..end)
}

/// `words` in the muted role, with the characters in `band` in the text role. The band
/// starts with a reset, so it leaves both kinds of muted: dim (the default, which also
/// works with 16 colours and in a terminal that draws dim text faint) and a colour of
/// the palette.
fn shimmer(words: &str, band: Option<std::ops::Range<usize>>, options: &RenderOptions) -> String {
    let (Some(band), Some(muted)) = (band, options.sgr(Role::Muted)) else {
        return options.paint(Role::Muted, words);
    };
    let text = options.sgr(Role::Text).map_or_else(String::new, |text| format!(";{text}"));
    let mut out = format!("\x1b[{muted}m");
    for (at, c) in words.chars().enumerate() {
        if at == band.start {
            out.push_str(&format!("\x1b[0{text}m"));
        }
        if at == band.end {
            out.push_str(&format!("\x1b[0;{muted}m"));
        }
        out.push(c);
    }
    out.push_str("\x1b[0m");
    out
}

#[cfg(test)]
mod tests;
