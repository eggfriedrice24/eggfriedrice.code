//! The status row of a running turn: the last row of the live zone, with a spinner, what
//! the turn does now, how long it has run and how full the model's context is, such as
//! `⠼ thinking  4s  ctx 43%  Reading the logs`. The gauge (`format::context`) comes
//! from the newest `context` draft or compaction, and it is in the colour of its level.
//!
//! What changes all the time stands last: the title or the newest words of a reasoning,
//! and the newest words of a long tool input that the model writes ([`State::detail`]),
//! end the row, after the time, the gauge and the scope. So those keep their place while
//! the model thinks or writes, and a narrow screen cuts the detail first.
//!
//! While a model call waits for its answer, the row says why when the daemon tells: a
//! wait before another attempt counts down, such as `trying again in 2s (the provider
//! answered 503)`, and a connect that takes [`CONNECT_AFTER`] or more says
//! `connecting`. A faster connect shows nothing, so the row does not flash.
//!
//! The row exists only while the turn runs on a terminal, and it goes before the prompt
//! comes back. It hides while the user is asked something, and the time stops then.
//! While efr counts the user away from the conversation (`presence_changed`), `away`
//! follows the time in the `warning` role: a question then waits only a short time. A
//! turn in full mode shows `full` in the `error` role before it: its commands run as
//! the user, with no sandbox and no question. While a call runs and this row hides, the
//! call's row shows the tag (`call.rs`).
//!
//! The row ends with the scope of the prompt, muted: the project's name, such as `app`,
//! or `○ ~/notes` outside projects (DV52). On a narrow screen the scope leaves the row
//! first, before the gauge.
//!
//! Events only change the state. The times come from the frame that shows the row
//! ([`Status::at`]): the follow loop writes a frame within one frame time of every
//! change, so the view needs no clock of its own.

use std::time::Duration;

use efr_protocol::ContextUse;
use efr_render::{RenderOptions, Role, text_width};
use jiff::Timestamp;

use crate::follow::since_then;
use crate::format;
use crate::format::context::Gauge;

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

/// The columns that the state keeps at least before the gauge leaves the row: on a
/// screen too narrow for both, what the turn does comes first.
const MIN_WORDS: usize = 10;

/// The columns that the detail of the state keeps at least; with fewer the row has none.
const MIN_DETAIL: usize = 8;

/// How long a connect lasts before the row says `connecting`.
const CONNECT_AFTER: Duration = Duration::from_millis(500);

/// What the row says while efr counts the user away.
const AWAY: &str = "away";

/// What the row says while the turn runs in full mode; the running call's row says it
/// too, while the status row hides.
pub(crate) const FULL: &str = "full";

/// What the turn does now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum State {
    /// The turn waits behind the running one.
    Queued,
    /// The turn waits for the next answer of the model.
    Model,
    /// The model reasons. With the newest title of its reasoning, or its newest words
    /// when it has no title: the detail at the end of the row.
    Thinking(Option<String>),
    /// The model writes text.
    Writing,
    /// The model writes the input of call `call` of its answer, `bytes` so far. With
    /// the newest words of a long input: the detail at the end of the row.
    Preparing { call: u32, tool: String, bytes: u64, words: Option<String> },
    /// A tool call runs.
    Tool(String),
    /// An approval waits for an answer from another client.
    Answer,
    /// The reviewer judges an exit of a call: the words name it, such as `reviewing:
    /// ./scripts/gen.sh`, already safe to print.
    Reviewing(String),
    /// The turn compacts the model's context before its next model call.
    Compacting,
    /// The model call connects to the provider, or sends its request.
    Connecting,
    /// An attempt of the model call failed: the daemon waits `wait`, then tries again.
    /// `reason` says why, safe to print.
    Retrying { attempt: u32, wait: Duration, reason: String },
}

impl State {
    /// The state in words; `silence` is how long the model has sent nothing, and
    /// `lasted` how long the state has shown.
    fn words(&self, silence: Duration, lasted: Duration) -> String {
        match self {
            State::Model | State::Writing if silence >= STALL => {
                format!("waiting for the model, no data for {}", format::elapsed(silence))
            }
            State::Queued => "waiting for the running turn".to_owned(),
            State::Model => "waiting for the model".to_owned(),
            State::Thinking(_) => "thinking".to_owned(),
            State::Writing => "writing".to_owned(),
            State::Connecting if lasted < CONNECT_AFTER => "waiting for the model".to_owned(),
            State::Connecting => "connecting".to_owned(),
            State::Retrying { attempt, wait, reason } => {
                let left = wait.saturating_sub(lasted);
                let when = if left.is_zero() {
                    String::new()
                } else {
                    // Whole seconds, rounded up, so the count never says 0s before the try.
                    let seconds =
                        u64::try_from(left.as_millis().div_ceil(1_000)).unwrap_or(u64::MAX);
                    format!(" in {}", format::elapsed(Duration::from_secs(seconds)))
                };
                if *attempt > 1 {
                    format!("trying again{when} (try {}, {reason})", attempt + 1)
                } else {
                    format!("trying again{when} ({reason})")
                }
            }
            State::Preparing { tool, bytes: 0, .. } => format!("preparing {tool}"),
            State::Preparing { tool, bytes, .. } => {
                format!("preparing {tool}, {}", format::size(*bytes))
            }
            State::Tool(tool) => format!("running {tool}"),
            State::Answer => "waiting for an answer".to_owned(),
            State::Reviewing(words) => words.clone(),
            State::Compacting => "compacting context".to_owned(),
        }
    }

    /// What the state works on, when it changes while the state lasts: the title or the
    /// newest words of a reasoning, or the newest words of a tool input. It ends the
    /// row, so the fields before it stand still.
    fn detail(&self) -> Option<&str> {
        match self {
            State::Thinking(Some(detail)) | State::Preparing { words: Some(detail), .. } => {
                Some(detail)
            }
            _ => None,
        }
    }

    /// True when `next` is a new state, whose time counts again: another kind, or
    /// another wait before an attempt.
    fn starts_anew(&self, next: &State) -> bool {
        std::mem::discriminant(self) != std::mem::discriminant(next)
            || matches!(next, State::Retrying { .. })
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
    /// The first frame that showed the state; `None` until a frame shows it.
    since: Option<Timestamp>,
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
    /// How full the model's context is, from the newest count.
    context: Option<ContextUse>,
    /// efr counts the user away from the conversation.
    away: bool,
    /// The turn runs in full mode.
    full: bool,
    /// The scope of the prompt, at the row's end.
    scope: Option<String>,
}

impl Status {
    /// A row that waits for the model, with motion or without.
    pub(crate) fn new(motion: bool) -> Status {
        Status {
            state: State::Model,
            motion,
            shown: None,
            started: None,
            since: None,
            starting: false,
            paused: Duration::ZERO,
            pause: None,
            data: None,
            stirred: false,
            context: None,
            away: false,
            full: false,
            scope: None,
        }
    }

    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    /// The turn does `state` now.
    pub(crate) fn set(&mut self, state: State) {
        if self.state.starts_anew(&state) {
            self.since = None;
        }
        self.state = state;
    }

    /// The newest words of the input of call `call`, which the model writes now, for
    /// the end of the row.
    pub(crate) fn set_input_words(&mut self, call: u32, words: Option<String>) {
        if let State::Preparing { call: shown, words: held, .. } = &mut self.state
            && *shown == call
        {
            *held = words;
        }
    }

    /// The model's context holds `context` now.
    pub(crate) fn set_context(&mut self, context: ContextUse) {
        self.context = Some(context);
    }

    /// How full the model's context is, from the newest count.
    pub(crate) fn context(&self) -> Option<&ContextUse> {
        self.context.as_ref()
    }

    /// efr counts the user away from the conversation, or present.
    pub(crate) fn set_away(&mut self, away: bool) {
        self.away = away;
    }

    /// The turn runs in full mode, or not.
    pub(crate) fn set_full(&mut self, full: bool) {
        self.full = full;
    }

    /// True while the turn runs in full mode.
    pub(crate) fn full(&self) -> bool {
        self.full
    }

    /// The row ends with `scope`, or with no scope.
    pub(crate) fn set_scope(&mut self, scope: Option<String>) {
        self.scope = scope;
    }

    /// The turn started: its time counts from the next frame.
    pub(crate) fn turn_started(&mut self) {
        self.starting = true;
        self.set(State::Model);
    }

    /// The model showed a sign of life: an event or a draft of the turn.
    pub(crate) fn stir(&mut self) {
        self.stirred = true;
    }

    /// Brings the times up to `now`, the time of the frame being drawn; `asking` while
    /// the user is asked something.
    pub(crate) fn at(&mut self, now: Timestamp, asking: bool) {
        self.shown.get_or_insert(now);
        self.since.get_or_insert(now);
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
                // The model sends nothing while the user is asked: no stall.
                self.data = Some(now);
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
        let lasted = self.since.map_or(Duration::ZERO, |since| since_then(since, now));
        let elapsed = self.elapsed(now);
        let suffix = (elapsed >= SHOW_ELAPSED).then(|| format::elapsed(elapsed));
        let columns = usize::from(options.width());
        // The spinner, a space, the time after two spaces, and `full` and `away` after two
        // more each.
        let away = if self.away { 2 + AWAY.len() } else { 0 };
        let full = if self.full { 2 + FULL.len() } else { 0 };
        let mut room = columns
            .saturating_sub(2 + suffix.as_ref().map_or(0, |time| time.len() + 2))
            .saturating_sub(away)
            .saturating_sub(full);
        let gauge = gauge_room(self.context.as_ref(), &mut room, options);
        let scope = scope_room(self.scope.as_deref(), &mut room, options);
        let words = if room == 0 {
            String::new()
        } else {
            format::cut(&self.state.words(silence, lasted), room, options.width_method())
        };
        // The detail takes what the row has left, after two spaces.
        let detail = self.state.detail().and_then(|detail| {
            let method = options.width_method();
            let left = room.saturating_sub(text_width(&words, method) + 2);
            (left >= MIN_DETAIL).then(|| format::cut(detail, left, method))
        });
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
        if self.full {
            row.push_str("  ");
            row.push_str(&options.paint(Role::Error, FULL));
        }
        if self.away {
            row.push_str("  ");
            row.push_str(&options.paint(Role::Warning, AWAY));
        }
        if let Some(gauge) = gauge {
            row.push_str("  ");
            row.push_str(&gauge.paint(options));
        }
        if let Some(scope) = scope {
            row.push_str("  ");
            row.push_str(&options.paint(Role::Muted, scope));
        }
        if let Some(detail) = detail {
            row.push_str("  ");
            row.push_str(&options.paint(Role::Muted, &detail));
        }
        row.push('\n');
        row
    }
}

/// The gauge of `context` for a row that has `room` columns left, which it then takes
/// with the two spaces before it; `None` when there is no count, or when the rest of the
/// row would keep fewer than [`MIN_WORDS`] columns.
pub(crate) fn gauge_room(
    context: Option<&ContextUse>,
    room: &mut usize,
    options: &RenderOptions,
) -> Option<Gauge> {
    let gauge = context.and_then(Gauge::of)?;
    let width = 2 + text_width(&gauge.text(), options.width_method());
    let left = room.checked_sub(width).filter(|left| *left >= MIN_WORDS)?;
    *room = left;
    Some(gauge)
}

/// The scope for a row that has `room` columns left after the gauge, which it then
/// takes with the two spaces before it; `None` when the rest of the row would keep
/// fewer than [`MIN_WORDS`] columns. So the scope leaves a narrow row before the gauge.
fn scope_room<'a>(
    scope: Option<&'a str>,
    room: &mut usize,
    options: &RenderOptions,
) -> Option<&'a str> {
    let scope = scope.filter(|scope| !scope.is_empty())?;
    let width = 2 + text_width(scope, options.width_method());
    let left = room.checked_sub(width).filter(|left| *left >= MIN_WORDS)?;
    *room = left;
    Some(scope)
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
