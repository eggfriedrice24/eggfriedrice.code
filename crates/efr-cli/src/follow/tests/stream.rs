//! `bench_perf_stream_matrix`: the real follow loop on the real clock against a fake
//! daemon that sends text drafts at a real pace. It records each frame through
//! `testing::probe` and replays the frames into a `vt100` screen to see when each
//! character first stands on a visible row.
//!
//! The cases: one paragraph of 5,000 characters at 80x24, 120x40 and 200x60 and two of
//! 1,500 at 80x12 (a long source line, slice 2 of `research/streaming/plan.md`); a
//! table whose source passes 4 KiB, a fenced block in a list item and a fenced block
//! of 200 lines (blocks that land whole, slice 9); six paragraphs of prose at 120x40
//! (the bytes of a frame, slice 10); a mixed answer at 60 tokens a second, in bursts of
//! 15 tokens each 250 ms, and in drafts 17 ms apart (the two gates, slice 8). The cases of one group run at the same time, each in a thread of
//! its own. `EFR_STREAM_SHAPES`, `EFR_STREAM_PATTERN` (`tok60`) and `EFR_STREAM_SIZES`
//! (`80x24,120x40,200x60`) run other cases by hand, and `EFR_STREAM_DUMP` names a
//! directory for each frame and draft of a run.
//!
//! The text is made of words `x` + four letters of `a` to `w`, each one unique, so a
//! word on the screen names its place in the source.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use efr_protocol::{
    ConversationSubscribeItem, Draft, DraftPart, Event, Mode, Origin, Seq, ShellContext,
    TurnSettings,
};
use efr_stdx::time::SystemClock;
use efr_test_support::bench::Report;

use super::super::{Compose, Look, Row, Target, TurnView, follow};
use super::{subscribed, turn_completed};
use crate::context::Context;
use crate::keys::Keys as _;
use crate::live::effective_width;
use crate::output::Output;
use crate::terminal::{Size, TermFacts};
use crate::testing::probe::{self, Probe};
use crate::testing::{FixedScreen, ScriptedKeys, TestEnv, conversation, item, turn};

const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvw";

/// Word `index`: `x` and four letters, the most significant first.
fn word(index: usize) -> String {
    let base = LETTERS.len();
    let mut out = String::from("x");
    for place in (0..4).rev() {
        out.push(char::from(LETTERS[(index / base.pow(place)) % base]));
    }
    out
}

/// The index of the word made of `letters` (four letters after the `x`).
fn word_index(letters: &[u8]) -> usize {
    letters.iter().fold(0, |index, letter| index * LETTERS.len() + usize::from(letter - b'a'))
}

/// A markdown text and where each of its words is.
#[derive(Debug, Default, Clone)]
struct Doc {
    text: String,
    /// The byte range of each word.
    words: Vec<(usize, usize)>,
}

impl Doc {
    fn word(&mut self) {
        let word = word(self.words.len());
        let start = self.text.len();
        self.text.push_str(&word);
        self.words.push((start, self.text.len()));
    }

    fn raw(&mut self, text: &str) {
        self.text.push_str(text);
    }

    /// About `chars` characters of prose on one source line, with some commas, full
    /// stops, bold words and inline code.
    fn prose(&mut self, chars: usize) {
        let start = self.text.len();
        loop {
            let at = self.words.len();
            if at % 13 == 5 {
                self.raw("**");
                self.word();
                self.raw("**");
            } else if at % 29 == 11 {
                self.raw("`");
                self.word();
                self.raw("`");
            } else {
                self.word();
            }
            if self.text.len() - start >= chars {
                self.raw(".");
                return;
            }
            if at % 17 == 16 {
                self.raw(". ");
            } else if at % 9 == 8 {
                self.raw(", ");
            } else {
                self.raw(" ");
            }
        }
    }

    fn paragraph(&mut self, chars: usize) {
        self.prose(chars);
        self.raw("\n\n");
    }

    fn code(&mut self, indent: &str, lines: usize) {
        let _ = writeln!(self.text, "{indent}```rust");
        for _ in 0..lines {
            self.raw(indent);
            self.raw("    let ");
            self.word();
            self.raw(" = ");
            self.word();
            self.raw(".");
            self.word();
            self.raw("(");
            self.word();
            self.raw(", 42);\n");
        }
        let _ = writeln!(self.text, "{indent}```");
    }

    fn table(&mut self, rows: usize) {
        for row in 0..rows + 1 {
            self.raw("|");
            for _ in 0..4 {
                self.raw(" ");
                self.word();
                self.raw(" ");
                self.word();
                self.raw(" |");
            }
            self.raw("\n");
            if row == 0 {
                self.raw("|---|---|---|---|\n");
            }
        }
        self.raw("\n");
    }
}

/// The shapes of a run by hand with `EFR_STREAM_SHAPES` unset.
const SHAPES: [&str; 11] = [
    "para_300",
    "para_1500",
    "para_5000",
    "hard_breaks",
    "paragraphs",
    "nested_list",
    "table_20",
    "code_200",
    "code_in_list",
    "mix",
    "table_wide_40",
];

fn shape(name: &str) -> Doc {
    let mut doc = Doc::default();
    match name {
        // Five paragraphs, each one source line of 300 characters.
        "para_300" => (0..5).for_each(|_| doc.paragraph(300)),
        "para_1500" => (0..2).for_each(|_| doc.paragraph(1500)),
        "para_5000" => {
            doc.paragraph(5000);
            doc.paragraph(300);
        }
        // Three paragraphs of eight source lines of about 72 characters.
        "hard_breaks" => {
            for _ in 0..3 {
                for _ in 0..8 {
                    doc.prose(72);
                    doc.raw("\n");
                }
                doc.raw("\n");
            }
        }
        "paragraphs" => [200, 450, 800, 300, 600, 350].iter().for_each(|n| doc.paragraph(*n)),
        // Six items, each with four items below it: 30 items.
        "nested_list" => {
            for _ in 0..6 {
                doc.raw("- ");
                doc.prose(50);
                doc.raw("\n");
                for _ in 0..4 {
                    doc.raw("  - ");
                    doc.prose(55);
                    doc.raw("\n");
                }
            }
            doc.raw("\n");
            doc.paragraph(100);
        }
        "table_20" => {
            doc.paragraph(120);
            doc.table(20);
            doc.paragraph(100);
        }
        // A table whose source passes 4 KiB: 40 rows of four cells of four words.
        "table_wide_40" => {
            doc.paragraph(120);
            for row in 0..41 {
                doc.raw("|");
                for _ in 0..4 {
                    for _ in 0..4 {
                        doc.raw(" ");
                        doc.word();
                    }
                    doc.raw(" |");
                }
                doc.raw("\n");
                if row == 0 {
                    doc.raw("|---|---|---|---|\n");
                }
            }
            doc.raw("\n");
            doc.paragraph(100);
        }
        "code_200" => {
            doc.paragraph(80);
            doc.code("", 200);
            doc.raw("\n");
            doc.paragraph(80);
        }
        "code_in_list" => {
            doc.raw("1. ");
            doc.prose(60);
            doc.raw("\n\n");
            doc.code("   ", 40);
            doc.raw("2. ");
            doc.prose(60);
            doc.raw("\n\n");
            doc.paragraph(80);
        }
        "mix" => {
            doc.raw("## ");
            doc.prose(20);
            doc.raw("\n\n");
            doc.paragraph(400);
            for _ in 0..5 {
                doc.raw("- ");
                doc.prose(60);
                doc.raw("\n");
            }
            doc.raw("\n");
            doc.code("", 15);
            doc.raw("\n");
            doc.table(5);
            doc.paragraph(600);
        }
        other => panic!("no shape {other}"),
    }
    doc
}

/// When each draft goes out, from the start of the stream, and where the text ends
/// with it. Every pattern but `fast` and `d10` sends 240 bytes a second.
fn schedule(len: usize, pattern: &str) -> Vec<(Duration, usize)> {
    // The bytes of a draft and the time between two drafts in microseconds.
    let (bytes, micros) = match pattern {
        // 60 tokens of 4 bytes a second, one draft each.
        "tok60" => (4, 16_667),
        // The same text in bursts: 15 tokens in one draft every 250 ms.
        "burst" => (60, 250_000),
        // One draft each 16 ms, as the daemon's draft gate sends them at best.
        "d16" => (4, 16_000),
        "d17" => (4, 17_000),
        // Faster than the client's frames.
        "d10" => (4, 10_000),
        // 200 tokens a second behind the daemon's 16 ms gate.
        "fast" => (13, 16_000),
        other => panic!("no pattern {other}"),
    };
    let mut out = Vec::new();
    let mut end = 0;
    let mut at = 0_u64;
    while end < len {
        end = (end + bytes).min(len);
        out.push((Duration::from_micros(at), end));
        at += micros;
    }
    out
}

/// What the output received: the sizes of the write calls of each flush.
#[derive(Debug, Default)]
struct WriteLog {
    groups: Vec<Vec<usize>>,
    open: Vec<usize>,
}

struct Recorder(Arc<Mutex<WriteLog>>);

impl Write for Recorder {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().open.push(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut log = self.0.lock().unwrap();
        let group = std::mem::take(&mut log.open);
        log.groups.push(group);
        Ok(())
    }
}

fn turn_started() -> Event {
    Event::TurnStarted {
        turn_id: turn(),
        cwd: std::path::PathBuf::from("/home/user/project"),
        scope: efr_protocol::Scope::Machine,
        settings: None,
    }
}

fn compose() -> Compose {
    Compose {
        context: ShellContext::new("/home/user/project"),
        last_command: None,
        recent_commands: Vec::new(),
        settings: TurnSettings {
            mode: Some(Mode::Auto),
            provider: None,
            model: None,
            effort: None,
        },
    }
}

/// The fake daemon: the turn starts, the drafts go out at their times with a persisted
/// update each 200 ms, then the message and the turn complete.
fn daemon(env: &TestEnv, ready: &mpsc::Sender<()>, text: &str, schedule: &[(Duration, usize)]) {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let daemon = env.listen();
        ready.send(()).unwrap();
        let mut conn = daemon.accept().await;
        let id = subscribed(&mut conn, 10).await;
        let mut seq = 11;
        conn.item(id, &item(seq, turn_started())).await;
        // The status row runs for a moment first, as it does while the model starts.
        std::thread::sleep(Duration::from_millis(350));
        let start = Instant::now();
        let mut sent = 0;
        let mut persisted = 0;
        let mut updated = start;
        for (at, end) in schedule {
            let due = start + *at;
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            let part = DraftPart::Text {
                index: 0,
                offset: sent as u64,
                delta: text[sent..*end].to_owned(),
            };
            let draft = Draft { turn_id: turn(), after_seq: Seq::new(seq), draft: part };
            conn.item(id, &ConversationSubscribeItem::Draft(draft)).await;
            sent = *end;
            if updated.elapsed() >= Duration::from_millis(200) {
                seq += 1;
                let event = Event::AssistantMessageUpdated {
                    turn_id: turn(),
                    index: 0,
                    offset: persisted as u64,
                    delta: text[persisted..sent].to_owned(),
                };
                conn.item(id, &item(seq, event)).await;
                persisted = sent;
                updated = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(120));
        seq += 1;
        let event =
            Event::AssistantMessageCompleted { turn_id: turn(), index: 0, text: text.to_owned() };
        conn.item(id, &item(seq, event)).await;
        seq += 1;
        conn.item(id, &item(seq, turn_completed())).await;
        conn.until_closed().await;
    });
}

/// A terminal as the user's: Ghostty with true colour, so widths count by grapheme.
fn ghostty() -> TermFacts {
    TermFacts {
        term: Some("xterm-ghostty".to_owned()),
        colorterm: Some("truecolor".to_owned()),
        stdout_tty: true,
        stderr_tty: true,
        stdin_tty: true,
        term_program: Some("ghostty".to_owned()),
        term_program_version: Some("1.3.1".to_owned()),
        ..TermFacts::default()
    }
}

/// One run: the real follow loop with the input row on a screen of `size`.
fn run(doc: &Doc, pattern: &str, size: Size) -> (Probe, WriteLog) {
    let env = TestEnv::new();
    let schedule = schedule(doc.text.len(), pattern);
    let log = Arc::new(Mutex::new(WriteLog::default()));
    let (ready, listening) = mpsc::channel();
    let probe = std::thread::scope(|scope| {
        scope.spawn(|| daemon(&env, &ready, &doc.text, &schedule));
        listening.recv().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let keys = Arc::new(ScriptedKeys::default());
            let ctx = Context {
                clock: Arc::new(SystemClock),
                term: ghostty(),
                screen: Arc::new(FixedScreen(size)),
                keys: keys.clone(),
                ..env.context()
            };
            let render = &ctx.settings.render;
            let look = Look::of(render, crate::progress::wanted(render.progress, &ctx.term));
            let options = ctx.render_options(effective_width(size));
            let mut view =
                TurnView::new(turn(), options).with_look(look).with_input().writing_changes();
            view.start();
            // NOTE: no line buffer, as the process's stdout (`output.rs`).
            let mut out =
                Output::from_writers(Box::new(Recorder(Arc::clone(&log))), Box::new(io::sink()));
            let reader = keys.keep().unwrap();
            let client = ctx.connect(Origin::Shell, None).await.unwrap();
            let row = Row { compose: compose(), reader, origin: Origin::Shell };
            let target = Target { conversation: conversation(), turn: turn(), after: Seq::new(10) };
            probe::start();
            follow(&ctx, &client, &mut out, &mut view, target, Some(row)).await.unwrap();
            probe::take()
        })
    });
    let log = std::mem::take(&mut *log.lock().unwrap());
    (probe, log)
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The words on the screen: the indexes of the whole ones, and the letters of the
/// last one that is cut off (fewer than four letters after its `x`).
fn words_on(contents: &str) -> (Vec<usize>, Vec<(usize, Vec<u8>)>) {
    let bytes = contents.as_bytes();
    let mut whole = Vec::new();
    let mut cut = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'x' {
            at += 1;
            continue;
        }
        let mut end = at + 1;
        while end < bytes.len() && end - at <= 4 && (b'a'..=b'w').contains(&bytes[end]) {
            end += 1;
        }
        if end - at == 5 {
            whole.push(word_index(&bytes[at + 1..end]));
        } else {
            cut.push((whole.len(), bytes[at + 1..end].to_vec()));
        }
        at = end;
    }
    (whole, cut)
}

/// The sorted values of `values`, with the value at the quantile `q`; 0 for none.
fn quantile(values: &[f64], q: f64) -> f64 {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    values.get(((values.len().max(1) - 1) as f64 * q).round() as usize).copied().unwrap_or(0.0)
}

/// What one run showed.
#[derive(Debug, Default)]
struct Shown {
    source_bytes: usize,
    drafts: usize,
    frames_text: u64,
    frames_status_only: u64,
    frames_empty: u64,
    /// The time from the draft that brought each character to the frame that showed it.
    reveal: Vec<Duration>,
    chars_late_100ms: u64,
    chars_late_1s: u64,
    chars_never_on_screen: u64,
    jump_chars_max: u64,
    first_text: Option<Duration>,
    /// The CPU time of each text frame.
    cpu: Vec<Duration>,
    /// The wall time of each text frame.
    wall: Vec<Duration>,
    cpu_over_4ms: u64,
    cpu_over_16ms: u64,
    status_cpu: Vec<Duration>,
    draft_to_draw: Vec<Duration>,
    interval: Vec<Duration>,
    frame_bytes: Vec<f64>,
    total_bytes: u64,
    write_calls: Vec<f64>,
    erased_rows: Vec<f64>,
    committed_rows_max_frame: u64,
    rows_jump_max: u64,
    frames_with_hidden_live: u64,
    hidden_rows_max: u64,
}

impl Shown {
    fn report(&self, report: &mut Report, case: &str) {
        let label = |what: &str| format!("{case}|{what}");
        report.count(&label("source bytes"), self.source_bytes as u64);
        report.count(&label("drafts"), self.drafts as u64);
        report.time(&label("reveal: draft -> on a visible row"), &self.reveal);
        report.count(&label("chars more than 100 ms late"), self.chars_late_100ms);
        report.count(&label("chars more than 1 s late"), self.chars_late_1s);
        report.count(&label("chars never on a visible row"), self.chars_never_on_screen);
        report.count(&label("largest jump (chars in one frame)"), self.jump_chars_max);
        if let Some(first) = self.first_text {
            report.time(&label("first draft -> first text on screen"), &[first]);
        }
        report.count(&label("text frames"), self.frames_text);
        report.count(&label("status-only frames"), self.frames_status_only);
        report.count(&label("empty frames"), self.frames_empty);
        report.time(&label("cpu of a text frame"), &self.cpu);
        report.time(&label("wall time of a text frame"), &self.wall);
        report.count(&label("text frames over 4 ms of cpu"), self.cpu_over_4ms);
        report.count(&label("text frames over 16 ms of cpu"), self.cpu_over_16ms);
        report.time(&label("cpu of a status-only frame"), &self.status_cpu);
        report.time(&label("draft -> next draw"), &self.draft_to_draw);
        report.time(&label("interval between text frames"), &self.interval);
        report.count(&label("bytes of a text frame, p50"), quantile(&self.frame_bytes, 0.5) as u64);
        report.count(&label("bytes of a text frame, max"), quantile(&self.frame_bytes, 1.0) as u64);
        report.bytes(&label("bytes out"), self.total_bytes);
        let per = self.total_bytes as f64 / self.source_bytes.max(1) as f64;
        report.count(&label("bytes out for each source byte"), per.round() as u64);
        report.count(&label("write calls a frame, p50"), quantile(&self.write_calls, 0.5) as u64);
        report.count(&label("write calls a frame, max"), quantile(&self.write_calls, 1.0) as u64);
        report.count(
            &label("rows erased a text frame, p50"),
            quantile(&self.erased_rows, 0.5) as u64,
        );
        report.count(
            &label("rows erased a text frame, max"),
            quantile(&self.erased_rows, 1.0) as u64,
        );
        report.count(&label("rows committed in one frame, max"), self.committed_rows_max_frame);
        report.count(&label("rows of the largest jump"), self.rows_jump_max);
        report.count(&label("frames with live rows off the screen"), self.frames_with_hidden_live);
        report.count(&label("live rows off the screen, max"), self.hidden_rows_max);
    }
}

/// What one run showed.
fn analyse(doc: &Doc, size: Size, probe: &Probe, log: &WriteLog) -> Shown {
    let mut shown =
        Shown { source_bytes: doc.text.len(), drafts: probe.arrivals.len(), ..Shown::default() };
    let mut screen = vt100::Parser::new(size.rows, size.cols, 0);
    // When each word first stood whole on a visible row.
    let mut seen: Vec<Option<Instant>> = vec![None; doc.words.len()];
    // The frontier after each frame: the bytes of the source that showed so far.
    let mut frontier = 0_usize;
    let mut frontiers: Vec<(Instant, usize)> = Vec::new();
    for frame in &probe.frames {
        if frame.text.is_empty() {
            continue;
        }
        // NOTE: the terminal's ONLCR makes each newline a carriage return and a newline.
        screen.process(frame.text.replace('\n', "\r\n").as_bytes());
        let contents = screen.screen().contents();
        let (whole, cut) = words_on(&contents);
        let mut top = None;
        for index in &whole {
            if let Some(first) = seen.get_mut(*index) {
                first.get_or_insert(frame.at);
                top = top.max(Some(*index));
            }
        }
        let mut now = top.map_or(0, |top| doc.words[top].1);
        // A cut word after the last whole one that starts the next word shows too.
        if let Some(next) =
            top.map_or(Some(0), |top| Some(top + 1)).filter(|n| *n < doc.words.len())
        {
            let expected = word(next);
            for (_, letters) in &cut {
                if expected.as_bytes()[1..].starts_with(letters) {
                    now = now.max(doc.words[next].0 + 1 + letters.len());
                }
            }
        }
        if now > frontier {
            shown.jump_chars_max = shown.jump_chars_max.max((now - frontier) as u64);
            frontier = now;
            frontiers.push((frame.at, frontier));
        }
    }

    // The latency of each character of a word: from the draft that brought it to the
    // frame whose frontier passed it.
    let mut arrival = 0;
    let mut passed = 0;
    for (index, (start, end)) in doc.words.iter().enumerate() {
        if seen[index].is_none() {
            shown.chars_never_on_screen += (end - start) as u64;
        }
        for offset in *start..*end {
            while arrival < probe.arrivals.len() && probe.arrivals[arrival].end <= offset {
                arrival += 1;
            }
            while passed < frontiers.len() && frontiers[passed].1 <= offset {
                passed += 1;
            }
            let (Some(arrived), Some((at, _))) =
                (probe.arrivals.get(arrival), frontiers.get(passed))
            else {
                continue;
            };
            let latency = at.saturating_duration_since(arrived.at);
            shown.reveal.push(latency);
            if latency > Duration::from_millis(100) {
                shown.chars_late_100ms += 1;
            }
            if latency > Duration::from_secs(1) {
                shown.chars_late_1s += 1;
            }
        }
    }

    // The frames while the text streams.
    let first_arrival = probe.arrivals.first().map(|arrival| arrival.at);
    let last_arrival = probe.arrivals.last().map(|arrival| arrival.at);
    let streaming = |at: Instant| {
        first_arrival.is_some_and(|first| at >= first)
            && last_arrival.is_some_and(|last| at <= last + Duration::from_millis(50))
    };
    let mut last_text: Option<Instant> = None;
    for frame in probe.frames.iter().filter(|frame| streaming(frame.at)) {
        let Some(zone) = frame.zone else { continue };
        shown.total_bytes += frame.text.len() as u64;
        if zone.hidden_rows > 0 {
            shown.frames_with_hidden_live += 1;
            shown.hidden_rows_max = shown.hidden_rows_max.max(zone.hidden_rows as u64);
        }
        if zone.empty {
            shown.frames_empty += 1;
        } else if zone.status_only {
            shown.frames_status_only += 1;
            shown.status_cpu.push(frame.cpu);
        } else {
            shown.frames_text += 1;
            shown.cpu.push(frame.cpu);
            shown.wall.push(frame.wall);
            shown.frame_bytes.push(frame.text.len() as f64);
            shown.erased_rows.push(zone.erased_rows as f64);
            shown.committed_rows_max_frame =
                shown.committed_rows_max_frame.max(zone.committed_rows as u64);
            // NOTE: the rows that the frame adds to the screen. A frame that writes only
            // what changed erases fewer rows than the old live zone holds, so the erased
            // rows do not tell the growth.
            let jump = (zone.committed_rows + zone.live_rows).saturating_sub(zone.old_rows);
            shown.rows_jump_max = shown.rows_jump_max.max(jump as u64);
            if frame.cpu > Duration::from_millis(4) {
                shown.cpu_over_4ms += 1;
            }
            if frame.cpu > Duration::from_millis(16) {
                shown.cpu_over_16ms += 1;
            }
            if let Some(last) = last_text {
                shown.interval.push(frame.at.duration_since(last));
            }
            last_text = Some(frame.at);
        }
    }

    // From each draft to the next draw of the view.
    let draws: Vec<Instant> =
        probe.frames.iter().filter(|frame| frame.zone.is_some()).map(|frame| frame.at).collect();
    let mut next = 0;
    for arrived in &probe.arrivals {
        while next < draws.len() && draws[next] < arrived.at {
            next += 1;
        }
        if let Some(at) = draws.get(next) {
            shown.draft_to_draw.push(at.duration_since(arrived.at));
        }
    }

    // The write calls of each frame.
    shown.write_calls = log
        .groups
        .iter()
        .filter(|group| !group.is_empty())
        .map(|group| group.len() as f64)
        .collect();
    shown.first_text = match (first_arrival, frontiers.first()) {
        (Some(first), Some((at, _))) => Some(at.saturating_duration_since(first)),
        _ => None,
    };
    shown
}

/// Writes each frame and each draft of the run of `case` to `$EFR_STREAM_DUMP`, one
/// file for each case, in the order of their times, for a look at single frames.
fn dump(probe: &Probe, case: &str) {
    let Some(dir) = var("EFR_STREAM_DUMP") else { return };
    let Some(start) = probe.frames.first().map(|frame| frame.at) else { return };
    let mut lines: Vec<(Instant, String)> = Vec::new();
    for arrival in &probe.arrivals {
        lines.push((arrival.at, format!("draft end={}", arrival.end)));
    }
    for frame in &probe.frames {
        let text: String = crate::testing::readable(&frame.text).chars().take(400).collect();
        lines.push((
            frame.at,
            format!(
                "frame tick={} cpu_us={:.0} bytes={} zone={:?}\n    {}",
                frame.tick,
                frame.cpu.as_secs_f64() * 1e6,
                frame.text.len(),
                frame.zone,
                text.replace('\n', "\\n"),
            ),
        ));
    }
    lines.sort_by_key(|(at, _)| *at);
    let mut out = String::new();
    for (at, line) in lines {
        let _ = writeln!(out, "{:9.3} {line}", millis(at.saturating_duration_since(start)));
    }
    let path = std::path::Path::new(&dir).join(format!("{}.txt", case.replace('|', "-")));
    std::fs::write(path, out).unwrap();
}

/// The sizes of a list such as `80x24,120x40`.
fn sizes(text: &str) -> Vec<Size> {
    text.split(',')
        .map(|size| {
            let (cols, rows) = size.split_once('x').expect("a size is COLSxROWS");
            Size { cols: cols.parse().unwrap(), rows: rows.parse().unwrap() }
        })
        .collect()
}

/// One case: a shape, a pattern of drafts and a screen.
type Case = (&'static str, &'static str, Size);

const fn size(cols: u16, rows: u16) -> Size {
    Size { cols, rows }
}

/// The groups of cases of a run with no variable; the cases of a group run at the same
/// time.
const GROUPS: [&[Case]; 3] = [
    &[
        ("para_5000", "tok60", size(80, 24)),
        ("para_5000", "tok60", size(120, 40)),
        ("para_5000", "tok60", size(200, 60)),
        ("para_1500", "tok60", size(80, 12)),
    ],
    &[
        ("table_wide_40", "tok60", size(120, 40)),
        ("code_in_list", "tok60", size(80, 24)),
        ("code_200", "tok60", size(80, 24)),
    ],
    &[
        ("paragraphs", "tok60", size(120, 40)),
        ("mix", "tok60", size(120, 40)),
        ("mix", "burst", size(120, 40)),
        ("mix", "d17", size(120, 40)),
    ],
];

/// The bench reads its own switches.
#[expect(clippy::disallowed_methods, reason = "the bench reads its own switches")]
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_stream_matrix() {
    let mut report = Report::new("bench_perf_stream_matrix");
    let picked = ["EFR_STREAM_SHAPES", "EFR_STREAM_PATTERN", "EFR_STREAM_SIZES"]
        .iter()
        .any(|name| var(name).is_some());
    let groups: Vec<Vec<(String, String, Size)>> = if picked {
        let pattern = var("EFR_STREAM_PATTERN").unwrap_or_else(|| "tok60".to_owned());
        let shapes = var("EFR_STREAM_SHAPES").unwrap_or_else(|| SHAPES.join(","));
        let sizes = sizes(&var("EFR_STREAM_SIZES").unwrap_or_else(|| "80x24,120x40,200x60".into()));
        shapes
            .split(',')
            .map(|shape| {
                sizes.iter().map(|size| (shape.to_owned(), pattern.clone(), *size)).collect()
            })
            .collect()
    } else {
        GROUPS
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|(shape, pattern, size)| {
                        ((*shape).to_owned(), (*pattern).to_owned(), *size)
                    })
                    .collect()
            })
            .collect()
    };
    let mut budget = Vec::new();
    let mut code = Vec::new();
    let mut writes = Vec::new();
    for group in groups {
        let shown: Vec<(String, Shown)> = std::thread::scope(|scope| {
            let runs: Vec<_> = group
                .iter()
                .map(|(shape, pattern, size)| {
                    scope.spawn(move || {
                        let doc = self::shape(shape);
                        let (probe, log) = run(&doc, pattern, *size);
                        let case = format!("{shape}|{pattern}|{}x{}", size.cols, size.rows);
                        dump(&probe, &case);
                        (case, analyse(&doc, *size, &probe, &log))
                    })
                })
                .collect();
            runs.into_iter().map(|run| run.join().unwrap()).collect()
        });
        for (case, shown) in &shown {
            shown.report(&mut report, case);
            if case.starts_with("para_") || case.starts_with("table_wide_40|") {
                budget.push((case.clone(), shown.chars_never_on_screen, shown.chars_late_100ms));
            }
            if case.starts_with("code_") {
                code.push((
                    case.clone(),
                    [shown.rows_jump_max, shown.chars_late_100ms],
                    [shown.frames_with_hidden_live, shown.cpu_over_4ms],
                ));
            }
            let per = shown.total_bytes as f64 / shown.source_bytes.max(1) as f64;
            let calls = quantile(&shown.write_calls, 1.0);
            writes.push((case.clone(), per, calls));
        }
    }
    report.finish();
    // The budget: each character of a long paragraph or a wide table reaches a visible
    // row, and none of them comes more than 100 ms late.
    for (case, never, late) in budget {
        assert_eq!(never, 0, "{case}: chars never on a visible row");
        assert_eq!(late, 0, "{case}: chars more than 100 ms late");
    }
    // The budget of a code block, also in a list item (slice 9): each line commits when
    // it ends, so no frame adds more than 3 rows, no live row is off the screen, and no
    // character comes more than 100 ms late. The syntax data loads off the frames, so
    // no frame, the first code block's included, takes more than 4 ms of CPU.
    for (case, [jump, late], [hidden, slow]) in code {
        assert!(jump <= 3, "{case}: a frame added {jump} rows");
        assert_eq!(late, 0, "{case}: chars more than 100 ms late");
        assert_eq!(hidden, 0, "{case}: frames with live rows off the screen");
        assert_eq!(slow, 0, "{case}: text frames over 4 ms of CPU");
    }
    // Each frame is one write call, and a frame writes only what changed: a long
    // paragraph and some paragraphs of prose stay below these bytes for each source byte
    // (before: 454 and 108, when each frame wrote the whole live zone).
    for (case, per, calls) in writes {
        assert!(calls <= 1.0, "{case}: {calls} write calls in a frame");
        let most = match case.as_str() {
            "para_5000|tok60|120x40" => 60.0,
            "paragraphs|tok60|120x40" => 20.0,
            _ => continue,
        };
        assert!(per <= most, "{case}: {per:.1} bytes out for each source byte, budget {most}");
    }
}
