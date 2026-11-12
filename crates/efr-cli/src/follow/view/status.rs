//! The status row of a running turn: the last row of the live zone, with a spinner, what
//! the turn does now, how long it has run, how full the model's context is, the model
//! and the scope, such as `⠼ thinking   9s   ctx 12%   work/claude-opus-5-5 high   app`
//! (TM-Live). The gauge (`format::context`) comes from the newest `context` draft or
//! compaction, and it is in the colour of its level.
//!
//! The order is fixed (`DECISIONS.md` 2): the spinner, a space, the state with its note,
//! then the time, the gauge, the model and the scope, three spaces apart, and last the
//! detail. What changes all the time stands last: the title or the newest words of a
//! reasoning, and the size and the newest words of a tool input that the model writes
//! ([`State::detail`]). So the time, the gauge and the scope stand still while the model
//! thinks or writes.
//!
//! The note stands next to the state, two spaces after it: `no data for 25s` after 20 s
//! of silence of the model and `away` while efr counts the user away from the
//! conversation (`presence_changed`), both in the `warning` role without bold; what the
//! reviewer reviews in the `text` role.
//!
//! The model is `account/model effort`, as `,model` takes it, such as
//! `work/claude-opus-5-5 high`, from the settings of the turn ([`Model`]). The scope is
//! the scope of the prompt, such as `app` or `⌂ /etc/nftables.d`, muted; a turn in full
//! mode adds ` · full` with `full` in the `error` role. A turn that waits behind another
//! one shows no gauge and no model.
//!
//! A narrow row drops, in this order: the detail, the account and the effort of the
//! model, the model, the gauge. The state keeps [`MIN_WORDS`] columns, and the state,
//! the time, the scope and `full` never go. A long directory of the scope is cut from the
//! left (`⌂ …/nftables.d`) and keeps at least [`MIN_WORDS`] columns.
//!
//! While a model call waits for its answer, the row says why when the daemon tells: a
//! wait before another attempt counts down, such as `trying again in 2s (the provider
//! answered 503)`, and a connect that takes [`CONNECT_AFTER`] or more says
//! `connecting`. A faster connect shows nothing, so the row does not flash.
//!
//! The row exists only while the turn runs on a terminal, and it goes before the prompt
//! comes back. It hides while the user is asked something, and the time stops then.
//! While a call runs and this row hides, the call's row shows the tag `full` (`call.rs`).
//!
//! Events only change the state. The times come from the frame that shows the row
//! ([`Status::at`]): the follow loop writes a frame within one frame time of every
//! change, so the view needs no clock of its own.

use std::time::Duration;

use efr_protocol::{ContextUse, EffectiveSettings};
use efr_render::{Glyphs, RenderOptions, Role, WidthMethod, text_width};
use jiff::Timestamp;

use crate::follow::since_then;
use crate::format::context::Gauge;
use crate::format::{self, Tone};

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

/// The columns that the state keeps, and that a directory of the scope keeps, before
/// a field leaves the row (TM-Live 28, 30: "the state keeps 10 columns").
const MIN_WORDS: usize = 10;

/// The columns that the detail of the state keeps at least; with fewer the row has none.
const MIN_DETAIL: usize = 8;

/// How long a connect lasts before the row says `connecting`.
const CONNECT_AFTER: Duration = Duration::from_millis(500);

/// Between the fields of the row.
const GAP: &str = "   ";

/// Between the state and its note, and inside the note.
const NOTE_GAP: &str = "  ";

/// What the row says while efr counts the user away.
const AWAY: &str = "away";

/// What the row says while the turn runs in full mode; the running call's row says it
/// too, while the status row hides.
pub(crate) const FULL: &str = "full";

/// What the state's words start with while the reviewer judges an exit; the rest is
/// the note, what it reviews.
const REVIEWING: &str = "reviewing";

/// The model of the turn as the status row names it: `work/claude-opus-5-5 high`, the
/// way `,model` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Model {
    /// The account of the provider, such as `work` of `anthropic-api.work`; `None` for
    /// the default account of a provider.
    pub(crate) account: Option<String>,
    /// The model id, such as `claude-opus-5-5`.
    pub(crate) name: String,
    /// The reasoning effort, such as `high`.
    pub(crate) effort: Option<String>,
}

impl Model {
    /// The model of a turn with `settings`.
    pub(crate) fn of(settings: &EffectiveSettings) -> Model {
        // NOTE: a turn that sends no effort runs on the model's default, which `,model`
        // names too (TM-Live 1).
        let effort = settings.effort.as_deref().or(settings.default_effort.as_deref());
        Model::named(settings.provider.as_deref(), &settings.model, effort)
    }

    /// The model of the new side of a switch of model (`model_switched`).
    pub(crate) fn switched_to(side: &efr_protocol::SwitchedModel) -> Model {
        let effort = side.effort.as_deref().or(side.default_effort.as_deref());
        Model::named(side.provider.as_deref(), &side.model, effort)
    }

    /// The model `name` of `provider`, whose account is the part after its `.`, with
    /// `effort`.
    fn named(provider: Option<&str>, name: &str, effort: Option<&str>) -> Model {
        let account = provider
            .and_then(|provider| provider.split_once('.'))
            .map(|(_, account)| format::one_line(account));
        Model { account, name: format::one_line(name), effort: effort.map(format::one_line) }
    }

    /// The whole field: `work/claude-opus-5-5 high`.
    fn whole(&self) -> String {
        let mut text = match &self.account {
            Some(account) => format!("{account}/{}", self.name),
            None => self.name.clone(),
        };
        if let Some(effort) = &self.effort {
            text.push(' ');
            text.push_str(effort);
        }
        text
    }
}

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
    /// The model writes the input of call `call` of its answer, `bytes` so far. The
    /// size and the newest words of a long input are the detail at the end of the row.
    Preparing { call: u32, tool: String, bytes: u64, words: Option<String> },
    /// A tool call runs.
    Tool(String),
    /// An approval waits for an answer from another client.
    Answer,
    /// The reviewer judges an exit of a call: the words name it, such as `reviewing:
    /// ./scripts/gen.sh`, already safe to print. The row says `reviewing` and what it
    /// reviews as the note.
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
    /// True while the model sends nothing for `silence` and the state waits for it:
    /// the row says `waiting for the model` with the note `no data for 25s`.
    fn stalled(&self, silence: Duration) -> bool {
        // NOTE: `connecting` lasts until the response headers come, so a request that
        // gets none is a stall too.
        matches!(self, State::Model | State::Writing | State::Connecting) && silence >= STALL
    }

    /// The state in words; `silence` is how long the model has sent nothing, and
    /// `lasted` how long the state has shown.
    fn words(&self, silence: Duration, lasted: Duration) -> String {
        match self {
            _ if self.stalled(silence) => "waiting for the model".to_owned(),
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
            State::Preparing { tool, .. } => format!("preparing {tool}"),
            State::Tool(tool) => format!("running {tool}"),
            State::Answer => "waiting for an answer".to_owned(),
            State::Reviewing(words) => match reviewed(words) {
                Some(_) => REVIEWING.to_owned(),
                None => words.clone(),
            },
            State::Compacting => "compacting context".to_owned(),
        }
    }

    /// What the state works on, when it changes while the state lasts: the title or the
    /// newest words of a reasoning, or the size and the newest words of a tool input.
    /// It ends the row, so the fields before it stand still.
    fn detail(&self) -> Option<String> {
        match self {
            State::Thinking(detail) => detail.clone(),
            State::Preparing { bytes: 0, words, .. } => words.clone(),
            State::Preparing { bytes, words: None, .. } => Some(format::size(*bytes)),
            State::Preparing { bytes, words: Some(words), .. } => {
                Some(format!("{}{NOTE_GAP}{words}", format::size(*bytes)))
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

/// What the reviewer reviews, from the words of a [`State::Reviewing`]: the part after
/// `reviewing: `.
fn reviewed(words: &str) -> Option<&str> {
    words.strip_prefix(REVIEWING)?.strip_prefix(": ").filter(|rest| !rest.is_empty())
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
    /// The scope of the prompt.
    scope: Option<String>,
    /// The model of the turn; boxed, as the view keeps the row.
    model: Option<Box<Model>>,
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
            model: None,
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

    /// The row names `model`, or no model.
    pub(crate) fn set_model(&mut self, model: Option<Model>) {
        self.model = model.map(Box::new);
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

    /// The notes next to the state: the stall, what the reviewer reviews and `away`.
    fn notes(&self, silence: Duration) -> Vec<(String, Tone)> {
        let mut notes = Vec::new();
        if self.state.stalled(silence) {
            notes.push((format!("no data for {}", format::elapsed(silence)), Tone::Attention));
        }
        if let State::Reviewing(words) = &self.state
            && let Some(subject) = reviewed(words)
        {
            notes.push((subject.to_owned(), Tone::Text));
        }
        if self.away {
            notes.push((AWAY.to_owned(), Tone::Attention));
        }
        notes
    }

    /// The row at `now`, painted with `options` and cut to their width, with its
    /// newline.
    pub(crate) fn row(&self, now: Timestamp, options: &RenderOptions) -> String {
        let method = options.width_method();
        let glyphs = options.glyphs();
        let ticks = self.shown.map_or(0, |shown| ticks(since_then(shown, now)));
        let spinner = spinner(self.motion, ticks, glyphs);
        let silence = self.data.map_or(Duration::ZERO, |data| since_then(data, now));
        let lasted = self.since.map_or(Duration::ZERO, |since| since_then(since, now));
        let elapsed = self.elapsed(now);
        let time = (elapsed >= SHOW_ELAPSED).then(|| format::elapsed(elapsed));
        let queued = self.state == State::Queued;
        let scope = Scope::new(self.scope.as_deref(), self.full, glyphs, method);
        let gap = GAP.len();
        // The fields that never go: the spinner and its space, the time and the scope at
        // their least; the state keeps its columns on top.
        let mut room = usize::from(options.width())
            .saturating_sub(text_width(&spinner.to_string(), method) + 1)
            .saturating_sub(time.as_ref().map_or(0, |time| gap + time.len()))
            .saturating_sub(scope.as_ref().map_or(0, |scope| gap + scope.least));
        // The others in the opposite order of the one in which they leave a narrow row.
        let mut take = |width: usize| match room.checked_sub(width) {
            Some(left) if left >= MIN_WORDS => {
                room = left;
                true
            }
            _ => false,
        };
        let gauge = self
            .context
            .as_ref()
            .and_then(Gauge::of)
            .filter(|_| !queued)
            .filter(|gauge| take(gap + text_width(&gauge.text(), method)));
        let model = self.model.as_deref().filter(|_| !queued);
        let named = model.filter(|model| take(gap + text_width(&model.name, method)));
        let whole = named.map(Model::whole).filter(|whole| {
            let name = named.map_or(0, |model| text_width(&model.name, method));
            take(text_width(whole, method).saturating_sub(name))
        });
        let model = whole.clone().or_else(|| named.map(|model| model.name.clone()));
        // The state with its notes, cut to what the row has left.
        let words = self.state.words(silence, lasted);
        let mut state = vec![(words, Tone::Dim)];
        for note in self.notes(silence) {
            state.push((NOTE_GAP.to_owned(), Tone::Plain));
            state.push(note);
        }
        let state = cut_pieces(&state, room, glyphs, method);
        room = room.saturating_sub(pieces_width(&state, method));
        // The scope grows into what is left; the detail takes the rest.
        let scope = scope.map(|scope| scope.fit(&mut room, glyphs, method));
        // NOTE: the detail leaves a narrow row first, so it shows only when no other
        // field had to go.
        let gauged = self.context.as_ref().and_then(Gauge::of).is_some();
        let dropped =
            !queued && ((gauged && gauge.is_none()) || (self.model.is_some() && whole.is_none()));
        let detail = self.state.detail().filter(|_| !dropped).and_then(|detail| {
            let left = room.checked_sub(gap)?;
            (left >= MIN_DETAIL).then(|| cut(&detail, left, glyphs, method))
        });
        let mut row = options.paint(Role::Accent, &spinner.to_string());
        row.push(' ');
        for (at, (text, tone)) in state.iter().enumerate() {
            if at == 0 {
                let band = self.motion.then(|| band(ticks, text.chars().count())).flatten();
                row.push_str(&shimmer(text, band, options));
            } else {
                row.push_str(&format::paint(text, *tone, options));
            }
        }
        let field = |row: &mut String, text: &str, tone: Tone| {
            row.push_str(GAP);
            row.push_str(&format::paint(text, tone, options));
        };
        if let Some(time) = &time {
            field(&mut row, time, Tone::Dim);
        }
        if let Some(gauge) = gauge {
            row.push_str(GAP);
            row.push_str(&gauge.paint(options));
        }
        if let Some(model) = &model {
            field(&mut row, model, Tone::Dim);
        }
        if let Some(scope) = scope {
            row.push_str(GAP);
            row.push_str(&scope.paint(self.full, options));
        }
        if let Some(detail) = &detail {
            field(&mut row, detail, Tone::Dim);
        }
        row.push('\n');
        row
    }
}

/// The scope field of the row: the scope of the prompt with ` · full` after it, or
/// `full` alone, and the columns it keeps at least.
struct Scope<'a> {
    /// The scope; a directory, such as `⌂ /etc/nftables.d`, is its mark and its path.
    scope: Option<&'a str>,
    /// The columns of the field at its least: a directory keeps [`MIN_WORDS`] columns.
    least: usize,
    /// The columns that ` · full` or `full` takes.
    full: usize,
}

impl<'a> Scope<'a> {
    fn new(
        scope: Option<&'a str>,
        full: bool,
        glyphs: &Glyphs,
        method: WidthMethod,
    ) -> Option<Self> {
        let scope = scope.filter(|scope| !scope.trim().is_empty());
        let full = match (full, scope) {
            (false, _) => 0,
            (true, None) => FULL.len(),
            (true, Some(_)) => text_width(&separator(glyphs), method) + FULL.len(),
        };
        let scope_least = scope.map_or(0, |scope| match directory(scope) {
            Some((mark, path)) => {
                let whole = text_width(path, method);
                text_width(mark, method) + 1 + whole.min(MIN_WORDS)
            }
            None => text_width(scope, method),
        });
        (scope.is_some() || full > 0).then_some(Scope { scope, least: scope_least + full, full })
    }

    /// The scope text in the room the row has left on top of its least, which it takes:
    /// a directory is cut from the left, at a `/` when it can.
    fn fit(self, room: &mut usize, glyphs: &Glyphs, method: WidthMethod) -> Fitted {
        let Some(scope) = self.scope else {
            return Fitted { scope: None };
        };
        let Some((mark, path)) = directory(scope) else {
            return Fitted { scope: Some(scope.to_owned()) };
        };
        let least = self.least - self.full - text_width(mark, method) - 1;
        let width = text_width(path, method);
        let columns = least + (*room).min(width.saturating_sub(least));
        *room -= columns - least;
        Fitted { scope: Some(format!("{mark} {}", cut_left(path, columns, glyphs, method))) }
    }
}

/// The scope field once it is cut.
struct Fitted {
    scope: Option<String>,
}

impl Fitted {
    /// The field, muted, with ` · full` after it in a turn in full mode, `full` in the
    /// `error` role.
    fn paint(&self, full: bool, options: &RenderOptions) -> String {
        let mut out = String::new();
        if let Some(scope) = &self.scope {
            out.push_str(&format::paint(scope, Tone::Dim, options));
            if full {
                out.push_str(&format::paint(&separator(options.glyphs()), Tone::Dim, options));
            }
        }
        if full {
            out.push_str(&format::paint(FULL, Tone::Failure, options));
        }
        out
    }
}

/// ` · `, the separator between the scope and `full`.
fn separator(glyphs: &Glyphs) -> String {
    format!(" {} ", glyphs.separator)
}

/// The mark and the path of a scope that names a directory, such as `⌂` and
/// `/etc/nftables.d`; `None` for the name of a project.
fn directory(scope: &str) -> Option<(&str, &str)> {
    let (mark, path) = scope.split_once(' ')?;
    (path.contains('/') && !mark.is_empty()).then_some((mark, path))
}

/// `path` in `columns`, cut from the left with `…` before what stays: at a `/` when the
/// last part fits, else inside the last part.
fn cut_left(path: &str, columns: usize, glyphs: &Glyphs, method: WidthMethod) -> String {
    if text_width(path, method) <= columns {
        return path.to_owned();
    }
    let more = glyphs.more;
    let room = columns.saturating_sub(text_width(more, method));
    let tail = path
        .match_indices('/')
        .map(|(at, _)| &path[at..])
        .find(|tail| text_width(tail, method) <= room);
    if let Some(tail) = tail {
        return format!("{more}{tail}");
    }
    // NOTE: a directory cut inside its last part still shows the `/` of the cut, as
    // `…/tables.d`, so the cut reads as one of a path (TM-Live 28: `…/nftables.d`).
    let slash = path.contains('/') && room > 1;
    let room = if slash { room - 1 } else { room };
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    for c in path.chars().rev() {
        let width = text_width(&c.to_string(), method);
        if used + width > room {
            break;
        }
        used += width;
        kept.push(c);
    }
    kept.reverse();
    let slash = if slash { "/" } else { "" };
    format!("{more}{slash}{}", kept.into_iter().collect::<String>())
}

/// The width of `pieces`.
fn pieces_width(pieces: &[(String, Tone)], method: WidthMethod) -> usize {
    pieces.iter().map(|(text, _)| text_width(text, method)).sum()
}

/// `pieces` cut to `columns` from the end, with `…` at the cut; the first piece keeps
/// at least one character.
fn cut_pieces(
    pieces: &[(String, Tone)],
    columns: usize,
    glyphs: &Glyphs,
    method: WidthMethod,
) -> Vec<(String, Tone)> {
    if pieces_width(pieces, method) <= columns {
        return pieces.to_vec();
    }
    let mut out = Vec::new();
    let mut left = columns;
    for (text, tone) in pieces {
        let width = text_width(text, method);
        if width < left {
            out.push((text.clone(), *tone));
            left -= width;
            continue;
        }
        let cut = cut(text, left, glyphs, method);
        if !cut.trim().is_empty() {
            out.push((cut, *tone));
        }
        break;
    }
    // A note gap at the end shows nothing.
    while out.last().is_some_and(|(text, _)| text.trim().is_empty()) {
        out.pop();
    }
    out
}

/// `text` cut to `columns` with the mark of more text of `glyphs` at the cut.
fn cut(text: &str, columns: usize, glyphs: &Glyphs, method: WidthMethod) -> String {
    let unicode = Glyphs::UNICODE.more;
    if glyphs.more == unicode || text_width(text, method) <= columns {
        return format::cut(text, columns, method);
    }
    let extra = text_width(glyphs.more, method).saturating_sub(text_width(unicode, method));
    let cut = format::cut(text, columns.saturating_sub(extra), method);
    match cut.strip_suffix(unicode) {
        Some(kept) => format!("{kept}{}", glyphs.more),
        None => cut,
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

impl Status {
    /// The frame of the spinner at `now` from `glyphs`, which a running call's line
    /// shows in place of the row.
    pub(crate) fn spinner(&self, now: Timestamp, glyphs: &Glyphs) -> char {
        let ticks = self.shown.map_or(0, |shown| ticks(since_then(shown, now)));
        spinner(self.motion, ticks, glyphs)
    }
}

/// The frame of the spinner of `glyphs` at tick `ticks`, or the still dot without
/// `motion`. Every frame of both tables is one character.
pub(crate) fn spinner(motion: bool, ticks: usize, glyphs: &Glyphs) -> char {
    let frame = if motion {
        glyphs.spinner.get(ticks % glyphs.spinner.len().max(1)).copied().unwrap_or(glyphs.still)
    } else {
        glyphs.still
    };
    frame.chars().next().unwrap_or(' ')
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
