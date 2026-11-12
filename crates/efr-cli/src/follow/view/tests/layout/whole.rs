//! Whole turns against the frames of the terminal design, end to end: the mid-size
//! turn of `TM-BeforeAfter` 4, the turn in time of `TM-Sequence` and the long turn of
//! `TM-LongTurn` 3 and 4 ("Quiet + thought row", with titles and without). Real events
//! go through one real view, and the rows that a terminal keeps are compared with the
//! frame row by row: the text, the colour and the attributes of each cell. The other
//! design tests check one element at a time; these check what falls between them: the
//! blank lines between a run of calls, prose, a steer and an answer, the order of the
//! event lines and the first call, and the end lines after a diff.
//!
//! The colours inside the rows of a diff are the render slice's (`efr_render::
//! diff_lines`, its own tests): the checks here compare those cells by their text and
//! their fill only.

use std::ops::{Range, RangeBounds};
use std::path::PathBuf;

use efr_protocol::{
    ApprovalDecision, ChangeKind, ContextUse, Draft, DraftPart, Event, EventEnvelope, FileChange,
    FileChanges, Origin, RouteLabel, RouteScope, Seq, Switch, SwitchedModel, Usage,
};
use efr_test_support::design::{Depth, Frame, cells, check_rows_where};
use serde_json::{Value, json};

use super::super::super::{Look, TurnView};
use super::super::at;
use super::{Screen, asked, id, message, shell, started, turn_started};
use crate::format::route;
use crate::terminal::Size;
use crate::testing::{compaction, design_fixed, design_options, turn};

/// The look of the frames: no motion, the end line, 20 lines of a diff, 8 in a long
/// turn.
const LOOK: Look = Look {
    motion: false,
    summary: true,
    progress: false,
    diff_lines: 20,
    diff_lines_long: 8,
    bell: false,
};

/// A terminal's screen with the paint of each cell: the cursor moves (`CSI A`, `B`,
/// `C`, `D`), the erases (`CSI J`, `K`, `2K`) and the paint (`CSI m`) act, a row
/// wraps at the width, and the screen grows downwards without scrolling. An erase to
/// the end of a row on a background fills the row with it, as a terminal does.
#[derive(Default)]
struct Term {
    cols: usize,
    rows: Vec<TermRow>,
    row: usize,
    col: usize,
    /// The paint that a new cell takes: the `CSI m` sequences since the last reset.
    pen: String,
}

#[derive(Clone, Default)]
struct TermRow {
    cells: Vec<(char, String)>,
    fill: Option<String>,
}

impl Term {
    fn new(cols: u16) -> Term {
        Term { cols: usize::from(cols), ..Term::default() }
    }

    fn write(&mut self, text: &str) {
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => match chars.next() {
                    Some('[') => {
                        let mut params = String::new();
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                self.csi(&params, c);
                                break;
                            }
                            params.push(c);
                        }
                    }
                    Some(']') => {
                        while let Some(c) = chars.next() {
                            if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                '\r' => self.col = 0,
                '\n' => {
                    self.row += 1;
                    self.col = 0;
                }
                c => {
                    if self.col >= self.cols {
                        self.row += 1;
                        self.col = 0;
                    }
                    let (col, pen) = (self.col, self.pen.clone());
                    let row = self.line();
                    if row.cells.len() <= col {
                        row.cells.resize(col + 1, (' ', String::new()));
                    }
                    row.cells[col] = (c, pen);
                    self.col += 1;
                }
            }
        }
    }

    fn line(&mut self) -> &mut TermRow {
        if self.rows.len() <= self.row {
            self.rows.resize(self.row + 1, TermRow::default());
        }
        &mut self.rows[self.row]
    }

    fn csi(&mut self, params: &str, last: char) {
        let count = params.parse::<usize>().unwrap_or(1).max(1);
        let erase = |row: &mut TermRow, col: usize, pen: &str| {
            row.cells.truncate(col);
            row.fill = (!pen.is_empty()).then(|| pen.to_owned());
        };
        match (last, params) {
            ('m', "" | "0") => self.pen.clear(),
            ('m', _) => {
                let params =
                    params.strip_prefix("0;").inspect(|_| self.pen.clear()).unwrap_or(params);
                self.pen.push_str(&format!("\x1b[{params}m"));
            }
            ('A', _) => self.row = self.row.saturating_sub(count),
            ('B', _) => self.row += count,
            ('C', _) => self.col = (self.col + count).min(self.cols.saturating_sub(1)),
            ('D', _) => self.col = self.col.saturating_sub(count),
            ('J', "" | "0") => {
                let (col, pen) = (self.col, self.pen.clone());
                erase(self.line(), col, &pen);
                self.rows.truncate(self.row + 1);
            }
            ('K', "" | "0") => {
                let (col, pen) = (self.col, self.pen.clone());
                erase(self.line(), col, &pen);
            }
            ('K', "2") => *self.line() = TermRow::default(),
            _ => {}
        }
    }

    /// The rows with their paint, each with its newline, without empty rows at the
    /// bottom.
    fn shown(&self) -> String {
        let mut rows: Vec<String> = self
            .rows
            .iter()
            .map(|row| {
                let mut out = String::new();
                let mut pen = "";
                for (c, paint) in &row.cells {
                    if paint != pen {
                        out.push_str("\x1b[0m");
                        out.push_str(paint);
                        pen = paint;
                    }
                    out.push(*c);
                }
                out.push_str("\x1b[0m");
                if let Some(fill) = &row.fill {
                    out.push_str(fill);
                    out.push_str("\x1b[K\x1b[0m");
                }
                out
            })
            .collect();
        while rows
            .last()
            .is_some_and(|row| cells(row).first().is_none_or(|row| row.text().trim().is_empty()))
        {
            rows.pop();
        }
        rows.iter().map(|row| format!("{row}\n")).collect()
    }
}

/// A turn played on a view with an input row and the screen of a terminal `cols` wide.
struct Play {
    view: TurnView,
    screen: Screen,
    /// The same screen as a terminal keeps it while the turn runs, the live zone too.
    term: Term,
    size: Size,
    seq: u64,
    now: i64,
    /// The bytes of the turn's reasoning so far.
    thought: u64,
}

impl Play {
    /// A view that the `↪` line of `switch` starts, when there is one, as `efr send`
    /// writes it before the first event.
    fn new(depth: Depth, cols: u16, switch: Option<&Switch>) -> Play {
        Play::with(depth, cols, switch, None)
    }

    /// [`Play::new`] for a turn with `settings`, in the project `app`: the status row
    /// names the model and the scope.
    fn with(
        depth: Depth,
        cols: u16,
        switch: Option<&Switch>,
        settings: Option<&efr_protocol::EffectiveSettings>,
    ) -> Play {
        let mut view = TurnView::new(turn(), design_options(depth, cols))
            .with_look(LOOK)
            .with_home(Some(PathBuf::from("/home/user")))
            .with_input();
        if settings.is_some() {
            view = view.with_model(settings).with_scope(Some("app".to_owned()));
        }
        let size = Size { cols, rows: 60 };
        let mut screen = Screen::new(cols);
        if let Some(switch) = switch {
            let line = route::switch_line(
                switch,
                &app(),
                at(0),
                Some(std::path::Path::new("/home/user")),
                view.options.glyphs(),
            );
            let step = view.event_row(&line, size);
            screen.write(&step.out);
        }
        view.start();
        // NOTE: the follow loop shows the input row once it reads keys.
        view.show_row(true);
        let mut term = Term::new(cols);
        term.write(&screen.raw());
        let mut play = Play { view, screen, term, size, seq: 10, now: 0, thought: 0 };
        play.event(0, turn_started());
        play
    }

    /// The daemon sends `event`, `millis` after the prompt.
    fn event(&mut self, millis: i64, event: Event) -> &mut Play {
        self.now = millis;
        self.seq += 1;
        let envelope =
            EventEnvelope { seq: Seq::new(self.seq), conversation_id: None, at: at(millis), event };
        let step = self.view.envelope(&envelope, self.size, true);
        self.screen.write(&step.out);
        self.term.write(&step.out);
        self.frame();
        self
    }

    /// The daemon sends a draft `part`, `millis` after the prompt.
    fn draft(&mut self, millis: i64, part: DraftPart) -> &mut Play {
        self.now = millis;
        let draft = Draft { turn_id: turn(), after_seq: Seq::new(self.seq), draft: part };
        self.view.draft(&draft, self.size);
        self.frame();
        self
    }

    /// A frame at `millis`, with no event.
    fn tick(&mut self, millis: i64) -> &mut Play {
        self.now = millis;
        self.frame();
        self
    }

    /// The model thinks from `from` for `millis`, under `title`, with `words`.
    fn thinks(&mut self, from: i64, millis: i64, title: Option<&str>, words: &str) -> &mut Play {
        let part = DraftPart::Reasoning {
            offset: self.thought,
            delta: words.to_owned(),
            title: title.map(str::to_owned),
        };
        self.thought += words.len() as u64;
        self.draft(from, part).tick(from + millis)
    }

    /// The user typed `text` as a steer, and the next model call reads it at `millis`.
    fn steer_read(&mut self, millis: i64, text: &str) -> &mut Play {
        self.seq += 1;
        let seq = Seq::new(self.seq);
        self.view.steered(seq, text.to_owned());
        self.event(millis, Event::SteeringDelivered { turn_id: turn(), steers: vec![seq] })
    }

    /// The user answers the question of call `n` with `decision` at `millis`.
    fn answer(&mut self, n: u8, decision: ApprovalDecision, millis: i64) -> &mut Play {
        let step = self.view.answered(id(n), decision, self.size);
        self.screen.write(&step.out);
        self.term.write(&step.out);
        self.view.show_row(true);
        self.event(
            millis,
            Event::ApprovalResolved {
                turn_id: turn(),
                call_id: id(n),
                decision,
                origin: Origin::Shell,
            },
        )
    }

    fn frame(&mut self) {
        let frame = self.view.frame(self.size, at(self.now));
        self.screen.write(&frame);
        self.term.write(&frame);
    }

    /// The rows of the screen now, the live zone too, each with its newline.
    fn shown(&self) -> String {
        self.term.shown()
    }

    /// The rows that the scrollback keeps once the view ends.
    fn committed(&mut self) -> String {
        self.view.close();
        self.frame();
        self.screen.raw()
    }
}

// --- events ----------------------------------------------------------------------

fn app() -> RouteLabel {
    RouteLabel {
        scope: RouteScope::Project("019a9b1c-3d00-7a10-8b20-0000000000e1".parse().unwrap()),
        name: Some("app".to_owned()),
        dir: PathBuf::from("/home/user/p/app"),
    }
}

fn ended(n: u8, exit_code: Option<i32>, output: &str) -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: id(n),
        output: output.to_owned(),
        truncated: false,
        is_error: exit_code.is_some_and(|code| code != 0),
        exit_code,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: false,
    }
}

fn tool(n: u8, name: &str, input: Value) -> Event {
    started(n, name, input, None)
}

fn file(path: &str, kind: ChangeKind, added: u32, removed: u32) -> FileChange {
    FileChange { path: path.to_owned(), kind, from: None, added, removed, binary: false }
}

fn changes(files: Vec<FileChange>, more: u32) -> FileChanges {
    let added = files.iter().map(|file| file.added).sum();
    let removed = files.iter().map(|file| file.removed).sum();
    FileChanges { files, more, added, removed }
}

/// The end of call `n`, which wrote `changes` with `diff`, or ran with exit code 0.
fn wrote(n: u8, changes: FileChanges, diff: Option<&str>) -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: id(n),
        output: String::new(),
        truncated: false,
        is_error: false,
        exit_code: diff.is_none().then_some(0),
        sandbox: None,
        refusal: None,
        changes: Some(changes),
        diff: diff.map(str::to_owned),
        left_running: false,
    }
}

/// Call `n` runs `command` from `from` for `took` milliseconds and ends well.
fn ran(play: &mut Play, n: u8, command: &str, from: i64, took: i64) {
    play.event(from, shell(n, command));
    play.event(from + took, ended(n, Some(0), ""));
}

/// Call `n` runs `command` from `from` for `took` milliseconds and fails with `code`
/// and `tail`.
fn failed(play: &mut Play, n: u8, command: &str, from: i64, took: i64, code: i32, tail: &str) {
    play.event(from, shell(n, command));
    play.event(from + took, ended(n, Some(code), tail));
}

/// Reads `paths` as calls `first`, `first + 1`, ..., from `from` on.
fn reads(play: &mut Play, first: u8, paths: &[&str], from: i64) {
    for (n, path) in (first..).zip(paths) {
        let at = from + i64::from(n - first) * 10;
        play.event(at, tool(n, "read_file", json!({ "path": path })));
        play.event(at + 5, ended(n, None, ""));
    }
}

/// Call `n` writes `path` with the edit tool, which changed `added` and `removed` lines
/// as `diff` shows.
fn edited(play: &mut Play, n: u8, path: &str, added: u32, removed: u32, diff: &str, at: i64) {
    play.event(at, tool(n, "edit", json!({ "path": path, "old_string": "a", "new_string": "b" })));
    let changed = changes(vec![file(path, ChangeKind::Modified, added, removed)], 0);
    let diff = format!("--- a/{path}\n+++ b/{path}\n{diff}");
    play.event(at + 5, wrote(n, changed, Some(&diff)));
}

/// Call `n` runs `command` after a question that `why` explains, which the user
/// answers with `decision`.
fn asked_shell(play: &mut Play, n: u8, command: &str, why: &str, from: i64) {
    play.event(from, shell(n, command));
    play.event(from + 10, asked(n, &format!("shell: {why}; run {command:?}"), None));
    // NOTE: the follow loop hides the input row while the keys answer a question.
    play.view.show_row(false);
    play.frame();
}

/// The end of the turn at `millis`: `percent` of the context, `out` tokens written,
/// `cached` percent of the input from the cache, and the files that it changed.
fn completed(
    play: &mut Play,
    millis: i64,
    percent: u64,
    out: u64,
    cached: u64,
    files: FileChanges,
) {
    let mut usage = Usage::new(100_000, out);
    usage.cached_input_tokens = cached * 1_000;
    play.event(
        millis,
        Event::TurnCompleted {
            turn_id: turn(),
            usage: Some(usage),
            changes: Some(files),
            context: Some(ContextUse { tokens: percent * 1_000, limit: 100_000, window: 272_000 }),
            stop: None,
            undo: None,
        },
    );
}

// --- the checks ------------------------------------------------------------------

/// Rows `range` of `text`, each with its newline.
fn rows(text: &str, range: Range<usize>) -> String {
    let all: Vec<&str> = text.split_inclusive('\n').collect();
    all.get(range.clone())
        .unwrap_or_else(|| panic!("rows {range:?} of {} rows:\n{text}", all.len()))
        .concat()
}

/// The cells of the code of a diff's rows: from `column` on, in the rows of `real` that
/// are rows of a diff. Without colour the sign of a removed row takes the weight of the
/// row (the render slice's), so it is text only there too.
fn code_cells(real: &str, column: usize, depth: Depth) -> impl Fn(usize, usize) -> bool {
    let code: Vec<bool> = cells(real)
        .iter()
        .map(|row| {
            let text = row.text();
            let indent = column.saturating_sub(2);
            let lead = text.get(..indent).is_some_and(|lead| lead.trim().is_empty());
            let sign = text.chars().nth(indent);
            lead && (matches!(sign, Some('+' | '-' | ' ')) || row.fill.is_some())
                && !text.trim_start().starts_with('@')
                && !text.trim_start().starts_with('\u{2026}')
                && !text.trim().is_empty()
        })
        .collect();
    let sign_too = depth == Depth::NoColor;
    move |row, col| {
        code.get(row).copied().unwrap_or(false)
            && (col >= column || (sign_too && col + 2 == column))
    }
}

/// The mark of an item of a list in prose: its first column. NOTE: TM-LongTurn draws
/// the `-` of its list in the colour of prose; TM-Markdown, which owns the marks of
/// Markdown, draws it `muted` (its legend: "muted marker"; spec 1.2), and so does efr.
/// The mark is compared by its text.
fn list_marks(real: &str) -> impl Fn(usize, usize) -> bool {
    let marked: Vec<bool> = cells(real).iter().map(|row| row.text().starts_with("- ")).collect();
    move |row, col| col == 0 && marked.get(row).copied().unwrap_or(false)
}

/// Checks rows `board` of `frame` against all of `real`, every cell but the code of a
/// diff and the mark of a list item by its colour and attributes.
fn check(frame: &Frame, board: impl RangeBounds<usize>, real: &str, depth: Depth) {
    let code = code_cells(real, 4, depth);
    let marks = list_marks(real);
    let text_only = |row, col| code(row, col) || marks(row, col);
    if let Err(problem) = check_rows_where(frame, board, real, depth, text_only) {
        panic!(
            "the output differs from the design: {problem}\n--- the whole output:\n{}",
            plain(real)
        );
    }
}

/// With `EFR_WHOLE_PRINT` set, prints each row of `real` next to the row of `frame`
/// that it stands for, for a person who looks at a difference (`--no-capture`).
#[expect(clippy::print_stderr, reason = "a person reads the rows next to the frame's")]
fn shown_if_asked(frame: &Frame, real: &str) {
    if std::env::var_os("EFR_WHOLE_PRINT").is_none() {
        return;
    }
    let board: Vec<&str> = frame.rows.iter().map(|row| row.text.as_str()).collect();
    for (at, row) in cells(real).iter().enumerate() {
        let want = board.get(at + 1).copied().unwrap_or("");
        eprintln!("{at:>3} {:<100} | {want}", row.text());
    }
}

/// `text` without its escape sequences.
fn plain(text: &str) -> String {
    cells(text).iter().map(|row| format!("{}\n", row.text())).collect()
}

// --- TM-BeforeAfter 4 --------------------------------------------------------------

/// The mid-size turn of TM-BeforeAfter 4 (and TM-Depths 1, 5, 9): back to an older
/// conversation, two calls, prose, an edit, a failed call and its fix, a steer, a
/// question and the end.
fn mid_size(depth: Depth) -> String {
    let back = Switch::Back {
        title: Some("fix steer race".to_owned()),
        updated_at: at(0) - jiff::SignedDuration::from_mins(125),
    };
    let mut play = Play::new(depth, 100, Some(&back));
    let turn_rs = "crates/efr-daemon/src/turn.rs";
    reads(&mut play, 1, &[turn_rs], 10);
    ran(&mut play, 2, "rg -n 'fn steer|fn finish' crates/efr-daemon", 100, 30);
    play.event(
        200,
        message(
            0,
            // NOTE: the board wraps its prose at 96 of its 100 columns, the box of its
            // text; a reply wraps at the width. The reply holds the board's line break,
            // as a model that wraps its own lines writes it.
            "The steer goes into `pending` while `finish()` already runs, so nothing drains \
             it. I drain the\nchannel before the end check.",
        ),
    );
    let first = "@@ -88,2 +88,5 @@ impl Turn {\n     pub fn finish(&mut self) -> usize {\n-        self.state = State::Done;\n+        while let Ok(msg) = self.rx.try_recv() {\n+            self.pending.push(msg);\n+        }\n+        self.state = State::Done;\n";
    edited(&mut play, 3, turn_rs, 5, 1, first, 300);
    ran(&mut play, 4, "cargo test -p efr-daemon steer", 400, 38_000);
    let tail = "error: this `if` has identical blocks\n   --> crates/efr-daemon/src/turn.rs:97:9\nerror: could not compile `efr-daemon` (lib) due to 1 previous error\n";
    failed(&mut play, 5, "cargo clippy -p efr-daemon -- -D warnings", 38_500, 9_000, 101, tail);
    let second =
        "@@ -95,2 +95,0 @@ impl Turn {\n-        } else {\n-            self.pending.clear();\n";
    edited(&mut play, 6, turn_rs, 0, 2, second, 47_600);
    ran(&mut play, 7, "cargo clippy -p efr-daemon -- -D warnings", 47_700, 7_800);
    play.steer_read(55_600, "use TurnState, not a new enum");
    asked_shell(&mut play, 8, "git push -u origin fix/steer-race", "network (github.com)", 56_000);
    play.answer(8, ApprovalDecision::Allow, 57_000);
    play.event(60_100, ended(8, Some(0), ""));
    play.event(
        61_000,
        message(
            1,
            "Pushed. `finish()` drains the channel first, and the new test sends a steer \
             during the last call.",
        ),
    );
    completed(
        &mut play,
        102_000,
        43,
        4_200,
        91,
        changes(vec![file(turn_rs, ChangeKind::Modified, 5, 3)], 0),
    );
    play.committed()
}

#[test]
fn the_mid_size_turn_is_the_board_from_its_first_line_to_its_end() {
    // TM-BeforeAfter 4 and TM-Depths 1, 5, 9: every row between the two prompt lines.
    for (depth, board, number) in [
        (Depth::TrueColor, "BeforeAfter", 4),
        (Depth::TrueColor, "Depths", 1),
        (Depth::Ansi16, "Depths", 5),
        (Depth::NoColor, "Depths", 9),
    ] {
        let shown = mid_size(depth);
        let frame = Frame::load(board, number).unwrap();
        let last = frame.rows.len() - 1;
        shown_if_asked(&frame, &shown);
        check(&frame, 1..last, &shown, depth);
    }
}

// --- TM-LongTurn 3 and 4 -------------------------------------------------------------

/// The five phases of reasoning of the long turn: the title of each (TM-LongTurn 3) and
/// the words of each (TM-LongTurn 4, a model that writes no titles).
const THOUGHTS: [(&str, &str); 5] = [
    (
        "reading the store",
        "The user wants sessions in SQLite. First I need to see how the store reads them.",
    ),
    (
        "adding the sqlite store",
        "Plan: a SqliteStore behind the same trait, the JSON reader behind a feature.",
    ),
    (
        "fixing the failing import",
        "The test panics at line 88: ended_at is None for sessions that never ended.",
    ),
    (
        "checking the old data",
        "Before touching real data I should run the importer as a dry run on a copy.",
    ),
    // NOTE: TM-LongTurn 4 draws this phase with no words: `▸ thought 3s`.
    ("finishing up", ""),
];

/// The long turn of TM-LongTurn 3 (`titled`: a model that titles its reasoning, after a
/// switch of model that compacted the context) and 4 (a model with no titles, no
/// switch): 40 calls, three edits, a failure, two questions, a steer and five phases of
/// reasoning.
fn long_turn(depth: Depth, titled: bool) -> String {
    let mut play = Play::new(depth, 100, None);
    if titled {
        let side = |provider: &str, model: &str| SwitchedModel {
            provider: Some(provider.to_owned()),
            model: model.to_owned(),
            effort: Some("high".to_owned()),
        };
        play.event(
            0,
            Event::ModelSwitched {
                turn_id: turn(),
                from: side("anthropic-api.work", "claude-opus-5-5"),
                to: side("openai-api.work", "gpt-5.5"),
            },
        );
        let mut compacted = compaction(efr_protocol::CompactionTrigger::Auto, 5_000);
        compacted.tokens_before = 41_000;
        play.event(100, Event::ConversationCompacted(compacted));
    }
    let think = |play: &mut Play, phase: usize, from: i64, millis: i64| {
        let (title, words) = THOUGHTS[phase];
        play.thinks(from, millis, titled.then_some(title), words);
    };
    let store = |name: &str| format!("crates/efr-store/src/{name}");
    think(&mut play, 0, 200, 6_000);
    let eleven: Vec<String> = [
        "lib.rs",
        "session.rs",
        "json.rs",
        "store.rs",
        "open.rs",
        "import.rs",
        "error.rs",
        "paths.rs",
        "lock.rs",
        "index.rs",
        "meta.rs",
    ]
    .iter()
    .map(|name| store(name))
    .collect();
    let eleven: Vec<&str> = eleven.iter().map(String::as_str).collect();
    reads(&mut play, 1, &eleven, 6_200);
    ran(&mut play, 12, "cargo tree -p efr-store -i rusqlite", 6_400, 400);
    think(&mut play, 1, 6_800, 11_000);
    play.event(
        17_800,
        message(
            0,
            // NOTE: the board wraps its prose at 96 of its 100 columns, the box of its
            // text; the reply holds the board's line breaks.
            "The store reads 412 JSON files at start, one per session, and that is what \
             makes efrd slow to\ncome up. I add a SQLite store behind the same \
             `SessionStore` trait and keep the JSON reader behind\nthe `legacy-store` \
             feature, off by default.",
        ),
    );
    play.event(17_900, shell(13, "cargo add rusqlite -p efr-store --features bundled"));
    let cargo = changes(
        vec![
            file("crates/efr-store/Cargo.toml", ChangeKind::Modified, 1, 0),
            file("Cargo.lock", ChangeKind::Modified, 18, 0),
        ],
        0,
    );
    play.event(19_100, wrote(13, cargo, None));
    play.event(
        19_200,
        tool(14, "write_file", json!({ "path": store("sqlite.rs"), "content": "x" })),
    );
    let mut sqlite = "--- /dev/null\n+++ b/crates/efr-store/src/sqlite.rs\n@@ -0,0 +1,142 @@\n+use rusqlite::{Connection, params};\n+\n+pub struct SqliteStore {\n+    conn: Connection,\n+}\n+\n+impl SqliteStore {\n+    // WAL keeps the turn loop's writes off the read path\n".to_owned();
    for n in 9..=142 {
        sqlite.push_str(&format!("+    // line {n}\n"));
    }
    let new = changes(vec![file(&store("sqlite.rs"), ChangeKind::Added, 142, 0)], 0);
    play.event(19_210, wrote(14, new, Some(&sqlite)));
    // NOTE: `\x66` is `f`: the text of a diff, not a `cfg` of this crate.
    let lib = "@@ -3,2 +3,4 @@ mod json\n mod session;\n-mod json;\n+#[cfg(\x66eature = \"legacy-store\")]\n+mod json;\n+mod sqlite;\n";
    edited(&mut play, 15, &store("lib.rs"), 3, 1, lib, 19_300);
    ran(&mut play, 16, "cargo build -p efr-store", 19_400, 18_000);
    think(&mut play, 2, 37_400, 4_000);
    let tail = "test sqlite::tests::import_missing_end ... FAILED\nthread 'sqlite::tests::import_missing_end' panicked at crates/efr-store/src/sqlite.rs:88:31:\ncalled `Option::unwrap()` on a `None` value\n";
    failed(&mut play, 17, "cargo test -p efr-store", 41_400, 41_000, 101, tail);
    reads(&mut play, 18, &[&store("sqlite.rs")], 82_500);
    let fix = "@@ -88,1 +88,1 @@ fn import\n-        let ended = row.ended_at.unwrap();\n+        let ended = row.ended_at.unwrap_or(row.started_at);\n";
    edited(&mut play, 19, &store("sqlite.rs"), 1, 1, fix, 82_600);
    ran(&mut play, 20, "cargo test -p efr-store", 82_700, 39_000);
    think(&mut play, 3, 121_700, 7_000);
    let four = [store("json.rs"), store("import.rs"), store("old.json"), store("migrate.rs")];
    let four: Vec<&str> = four.iter().map(String::as_str).collect();
    reads(&mut play, 21, &four, 128_700);
    ran(&mut play, 25, "cargo clippy -p efr-store --all-features -- -D warnings", 128_800, 22_000);
    play.event(
        150_900,
        message(
            1,
            "The importer runs once at start and leaves the JSON files in place, so a \
             downgrade still works.\nNext I try it on your real sessions without writing \
             anything.",
        ),
    );
    let migrate = "cargo run -p efr-store --bin migrate -- --dry-run ~/.local/share/efr/sessions";
    asked_shell(&mut play, 26, migrate, "read outside the project", 151_000);
    play.answer(26, ApprovalDecision::Allow, 151_500);
    play.event(155_600, ended(26, Some(0), ""));
    ran(&mut play, 27, "ls ~/.local/share/efr/sessions | wc -l", 155_700, 100);
    let three =
        [store("migrate.rs"), store("open.rs"), "crates/efr-store/src/CHANGELOG.md".to_owned()];
    let three: Vec<&str> = three.iter().map(String::as_str).collect();
    reads(&mut play, 28, &three, 155_900);
    ran(&mut play, 31, "cargo fmt --all", 156_000, 900);
    ran(&mut play, 32, "cargo bench -p efr-store --bench open -- --quick", 157_000, 48_000);
    think(&mut play, 4, 205_000, 3_000);
    ran(&mut play, 33, "cargo test --workspace", 208_000, 132_000);
    asked_shell(&mut play, 34, "cargo update -p rusqlite", "network (index.crates.io)", 340_100);
    play.answer(34, ApprovalDecision::Deny, 340_200);
    play.event(
        340_210,
        Event::ToolCallCompleted {
            turn_id: turn(),
            call_id: id(34),
            output: String::new(),
            truncated: false,
            is_error: true,
            exit_code: None,
            sandbox: None,
            refusal: None,
            changes: None,
            diff: None,
            left_running: false,
        },
    );
    play.steer_read(340_300, "skip the update, the lock is fine");
    ran(&mut play, 35, "git diff --stat", 340_400, 100);
    ran(&mut play, 36, "just lint", 340_600, 31_000);
    ran(&mut play, 37, "git add -A", 371_700, 100);
    ran(
        &mut play,
        38,
        "git commit -m 'store: sessions in SQLite, JSON reader behind legacy-store'",
        371_900,
        300,
    );
    play.event(
        372_300,
        message(
            2,
            "Done. efrd now opens one SQLite file at start: 38 ms instead of 1.9 s for your \
             412 sessions.\n\n\
             - the JSON reader builds only with `--features legacy-store`\n\
             - the importer ran as a dry run on your sessions: 412 read, 0 errors\n\
             - `cargo update -p rusqlite` did not run (you denied the network); the lock is \
             unchanged",
        ),
    );
    let files = changes(
        vec![
            file(&store("sqlite.rs"), ChangeKind::Added, 142, 0),
            file(&store("lib.rs"), ChangeKind::Modified, 3, 1),
            file("crates/efr-store/Cargo.toml", ChangeKind::Modified, 1, 0),
            file("Cargo.lock", ChangeKind::Modified, 1, 2),
        ],
        0,
    );
    completed(&mut play, 372_400, 71, 18_400, 88, files);
    play.committed()
}

/// Checks the rows of `frame` from row 1 to the row before its last (the prompt lines)
/// against `real`, but the rows of `skipped`, which `real` does not hold.
fn check_but(frame: &Frame, skipped: &[usize], real: &str, depth: Depth) {
    shown_if_asked(frame, real);
    let last = frame.rows.len() - 1;
    let mut from = 1;
    let mut written = 0;
    for end in skipped.iter().copied().chain([last]) {
        let part = rows(real, written..written + (end - from));
        check(frame, from..end, &part, depth);
        written += end - from;
        from = end + 1;
    }
    assert_eq!(written, cells(real).len(), "rows after the frame:\n{}", plain(real));
}

#[test]
fn the_long_turn_is_the_board_with_titles_and_without() {
    // TM-LongTurn 3 (titles) and 4 (no titles), "Quiet + thought row": every row
    // between the two prompt lines, but the two `search` rows of each, which wait for
    // a search tool (there is none yet).
    for (number, titled, skipped, changes_row) in [(3, true, [4, 45], 75), (4, false, [3, 44], 74)]
    {
        let base = Frame::load("LongTurn", number).unwrap();
        // NOTE: the board draws truecolor only; 16 colours is the frame that the table of
        // `design::expect` derives. Without colour, inline code keeps its backticks
        // (TM-Legend 6), which a derived frame cannot know: TM-Depths 9 draws that
        // depth of a whole turn, in the test of the mid-size turn.
        for depth in [Depth::TrueColor, Depth::Ansi16] {
            // NOTE: the board's changes line counts 3 files and names three and one
            // more (mock data, spec 2.2 item 8): the line counts the files that it
            // names, 4.
            let frame = design_fixed(&base, depth, &[(changes_row, "3 files", "4 files")]);
            let shown = long_turn(depth, titled);
            check_but(&frame, &skipped, &shown, depth);
        }
    }
}

// --- TM-Sequence -------------------------------------------------------------------

/// The settings of the turns of TM-Sequence: the status row names the model.
fn work_opus() -> efr_protocol::EffectiveSettings {
    efr_protocol::EffectiveSettings {
        mode: efr_protocol::Mode::Auto,
        model: "claude-opus-5-5".to_owned(),
        effort: Some("high".to_owned()),
        provider: Some("anthropic-api.work".to_owned()),
        overridden: efr_protocol::OverriddenSettings::default(),
        fallback: None,
    }
}

/// The time of a status row, the field after the state that the board's moments draw
/// on clocks of their own: `writing   21s   ctx 31%` becomes `writing   T   ctx 31%`.
fn timeless(row: &str) -> String {
    let fields: Vec<&str> = row.split("   ").collect();
    let time = |field: &str| {
        let field = field.trim();
        !field.is_empty()
            && field.split(' ').all(|part| {
                part.strip_suffix(['s', 'm']).is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
            })
    };
    fields.iter().map(|field| if time(field) { "T" } else { field }).collect::<Vec<_>>().join("   ")
}

/// Checks the bottom of the screen of `play` against frame `number` of TM-Sequence from
/// its row `from` on (row 0 is the prompt line, or a row that the board cut out of the
/// scrollback): the rows that the scrollback keeps by their text, colour and
/// attributes, and the rows of the live zone by their text. NOTE: the live rows are
/// the live slice's (the status row, the input row, the card, the running call) and
/// have their own tests by colour; here they show where they stand. The board's
/// moments do not share one clock (frame 6 says `40s` after a call of 38 s that frame
/// 5 starts after 21 s), so the time of a status row is not compared.
fn moment(play: &mut Play, number: u32, from: usize, depth: Depth) {
    play.frame();
    let mut frame =
        efr_test_support::design::expect(&Frame::load("Sequence", number).unwrap(), depth).unwrap();
    // NOTE: frames 6, 7 and 8 draw a blank row between a call and the status row;
    // TM-Scope (the `↪` line, then the status row) draws none. DECISIONS.md: no row is
    // added to the live zone for status-like information. The status row stands right
    // under the scrollback, and the blank row comes with the prose that follows it, so
    // it is left out of these three.
    if let Some(at) = frame.rows.iter().position(|row| row.live)
        && matches!(number, 6..=8)
        && at > from
        && frame.rows[at - 1].text.trim().is_empty()
        && frame.rows[at].text.starts_with("\u{283c} ")
        && !frame.rows[at].text.starts_with("\u{283c} $ ")
    {
        frame.rows.remove(at - 1);
    }
    let frame = frame;
    let wanted = &frame.rows[from..];
    let screen = play.shown();
    let all: Vec<&str> = screen.split_inclusive('\n').collect();
    assert!(
        all.len() >= wanted.len(),
        "frame {number}: {} rows on the screen:\n{}",
        all.len(),
        plain(&screen)
    );
    let real = all[all.len() - wanted.len()..].concat();
    let live = wanted.iter().position(|row| row.live).unwrap_or(wanted.len());
    // NOTE: the printed board starts with a row that stands for the prompt line.
    shown_if_asked(&Frame { rows: [&frame.rows[..1], wanted].concat(), ..frame.clone() }, &real);
    if live > 0 {
        check(&frame, from..from + live, &rows(&real, 0..live), depth);
    }
    let spinner = efr_render::Glyphs::UNICODE;
    // NOTE: while a shell call runs, the hint of the input row starts with `ctrl+b
    // background`: TM-Live 29, the board of the live zone ("ctrl+b leads the hint"),
    // beats the short hint of TM-Sequence 5 and 10.
    let running = wanted.iter().any(|row| row.live && row.text.starts_with("\u{283c} $ "));
    for (row, want) in cells(&rows(&real, live..wanted.len())).iter().zip(&wanted[live..]) {
        let mut text = row.text();
        for glyph in spinner.spinner.iter().chain([&spinner.still]) {
            text = text.replace(glyph, "\u{283c}");
        }
        let mut want = want.text.clone();
        // NOTE: frame 8 says `writing`, but no text of the model follows the edit
        // (frame 9 shows the next call first, then the prose): with no text, the
        // status row says that it waits for the model.
        if number == 8 {
            want = want.replace("\u{283c} writing", "\u{283c} waiting for the model");
        }
        if running {
            want = want.replace(
                "\u{203a}   enter steer",
                "\u{203a}   ctrl+b background \u{b7} enter steer",
            );
        }
        assert_eq!(
            timeless(text.trim_end()),
            timeless(want.trim_end()),
            "TM-Sequence {number}, a live row\n--- the screen:\n{}",
            plain(&screen)
        );
    }
}

/// The turn of TM-Sequence up to the second clippy (frames 1 to 8), with a check of
/// each moment in `depth`.
fn sequence_start(depth: Depth) -> Play {
    let mut play = Play::with(depth, 100, None, Some(&work_opus()));
    // 1: the first frame, with a line typed in the input row.
    for byte in "also add a test".bytes() {
        play.view.row_key(crate::keys::Key::Byte(byte));
    }
    moment(&mut play, 1, 1, depth);
    play.view.row_key(crate::keys::Key::Byte(0x15));
    // 2: nothing arrives for 25 s after the gauge.
    play.draft(100, DraftPart::Context(ContextUse { tokens: 12, limit: 100, window: 100 }));
    play.tick(25_100);
    moment(&mut play, 2, 1, depth);
    // NOTE: frame 3 draws a live row of the reasoning, which DECISIONS.md rejects (no
    // live line for the reasoning): its words stand at the end of the status row,
    // which the live slice tests (TM-Live 3).
    let turn_rs = "crates/efr-daemon/src/turn.rs";
    reads(&mut play, 1, &[turn_rs], 25_200);
    ran(&mut play, 2, "rg -n 'fn steer|fn finish' crates/efr-daemon", 25_300, 30);
    play.draft(25_400, DraftPart::Context(ContextUse { tokens: 31, limit: 100, window: 100 }));
    let prose = "The steer goes into `pending` while `finish()` already runs, so nothing drains \
                 it. I drain the\nchannel before the end check.";
    let open = "The steer goes into `pending` while `finish()` already runs, so nothing drains \
                it. I drain the\nchannel before the end";
    play.draft(25_500, DraftPart::Text { index: 0, offset: 0, delta: open.to_owned() });
    moment(&mut play, 4, 1, depth);
    play.event(25_600, message(0, prose));
    play.event(25_700, shell(3, "cargo test -p efr-daemon steer"));
    play.draft(25_800, DraftPart::Context(ContextUse { tokens: 38, limit: 100, window: 100 }));
    play.event(
        25_900,
        Event::ToolCallOutputUpdated {
            turn_id: turn(),
            call_id: id(3),
            tail: "Compiling efr-daemon v0.0.5 (crates/efr-daemon)\ntest turn::tests::steer_during_turn ... ok\ntest turn::tests::steer_after_finish ... ok\n".to_owned(),
            bytes: 120,
        },
    );
    play.tick(37_700);
    moment(&mut play, 5, 1, depth);
    play.event(63_700, ended(3, Some(0), ""));
    moment(&mut play, 6, 1, depth);
    let tail = "error: this `if` has identical blocks\n   --> crates/efr-daemon/src/turn.rs:97:9\nerror: could not compile `efr-daemon` (lib) due to 1 previous error\n";
    failed(&mut play, 4, "cargo clippy -p efr-daemon -- -D warnings", 63_800, 9_000, 101, tail);
    play.draft(72_900, DraftPart::Context(ContextUse { tokens: 41, limit: 100, window: 100 }));
    play.draft(73_000, DraftPart::Reasoning { offset: 0, delta: String::new(), title: None });
    moment(&mut play, 7, 0, depth);
    let edit = "@@ -95,4 +95,2 @@ impl Turn {\n             self.state = State::Draining;\n-        } else {\n-            self.pending.clear();\n         }\n";
    edited(&mut play, 5, turn_rs, 0, 2, edit, 73_500);
    // NOTE: frame 8 shows the last row of the failed call's tail only, under its row.
    moment(&mut play, 8, 1, depth);
    ran(&mut play, 6, "cargo clippy -p efr-daemon -- -D warnings", 73_700, 7_800);
    play
}

#[test]
fn the_turn_in_time_is_the_board_at_each_moment() {
    // NOTE: the board draws truecolor only; 16 colours is the derived frame. Without
    // colour inline code keeps its backticks (TM-Legend 6), which a derived frame
    // cannot know; TM-Depths 9 covers that depth of the same turn.
    for depth in [Depth::TrueColor, Depth::Ansi16] {
        // 9, 10 and 13: a question, its answer and the end.
        let mut play = sequence_start(depth);
        play.event(81_600, message(1, "Pushing the branch so CI runs."));
        play.event(81_700, shell(7, "git push -u origin fix/steer-race"));
        play.event(
            81_710,
            Event::ApprovalRequested {
                turn_id: turn(),
                call_id: id(7),
                summary: "shell: network (github.com); run \"git push -u origin fix/steer-race\""
                    .to_owned(),
                diff_preview: None,
                interactive: false,
                exit: None,
                expires_in_secs: None,
                local_only: false,
                reason: None,
            },
        );
        play.view.show_row(false);
        // NOTE: `the model says` and the `a always…` key of frame 9 come with the facts
        // of a sandbox exit (the model's reason and the rule); the card's own test
        // draws them (TM-Sequence 9 in `format/card`). This question has none, so the
        // card is compared to its `why` row, and the keys row by its first keys.
        play.frame();
        let screen = play.shown();
        let all: Vec<&str> = screen.split_inclusive('\n').collect();
        let real = all[all.len() - 8..].concat();
        let frame = Frame::load("Sequence", 9).unwrap();
        check(&frame, 0..3, &rows(&real, 0..3), depth);
        let card = cells(&rows(&real, 3..8));
        let wanted = [3, 4, 5, 6, 8].map(|row| frame.rows[row].text.trim_end().to_owned());
        for (row, want) in card.iter().zip(&wanted) {
            let text = row.text();
            let text = text.trim_end();
            assert!(
                want.starts_with(text) && !text.is_empty() || want == text,
                "TM-Sequence 9, the card: {text:?} for {want:?}"
            );
        }
        play.answer(7, ApprovalDecision::Allow, 82_000);
        play.draft(82_100, DraftPart::Context(ContextUse { tokens: 42, limit: 100, window: 100 }));
        play.event(
            83_000,
            Event::ToolCallOutputUpdated {
                turn_id: turn(),
                call_id: id(7),
                tail: "Enumerating objects: 9, done.\nWriting objects: 100% (5/5), 1.21 KiB | 1.21 MiB/s, done.\n".to_owned(),
                bytes: 90,
            },
        );
        play.tick(84_000);
        moment(&mut play, 10, 0, depth);
        play.event(85_100, ended(7, Some(0), ""));
        play.event(
            86_000,
            message(
                2,
                "Pushed. `finish()` drains the channel first, and the new test sends a steer \
                 during the last call.",
            ),
        );
        let turn_rs = "crates/efr-daemon/src/turn.rs";
        completed(
            &mut play,
            102_000,
            43,
            4_200,
            91,
            changes(vec![file(turn_rs, ChangeKind::Modified, 5, 3)], 0),
        );
        play.view.close();
        play.frame();
        let shown = play.screen.raw();
        let frame = Frame::load("Sequence", 13).unwrap();
        let last = frame.rows.len() - 1;
        let all: Vec<&str> = shown.split_inclusive('\n').collect();
        check(&frame, 0..last, &all[all.len() - last..].concat(), depth);

        // 11 and 12: a steer while the model writes, then read at its next call.
        let mut play = sequence_start(depth);
        play.draft(81_600, DraftPart::Context(ContextUse { tokens: 42, limit: 100, window: 100 }));
        let open = "Pushing the branch so CI runs. Then I add the test for a steer that comes \
                    during the last\ncall";
        play.draft(81_700, DraftPart::Text { index: 1, offset: 0, delta: open.to_owned() });
        play.view.steered(Seq::new(90), "use TurnState, not a new enum".to_owned());
        moment(&mut play, 11, 0, depth);
        play.event(82_000, message(1, &format!("{open}.")));
        play.draft(82_100, DraftPart::Context(ContextUse { tokens: 43, limit: 100, window: 100 }));
        play.event(
            82_200,
            Event::SteeringDelivered { turn_id: turn(), steers: vec![Seq::new(90)] },
        );
        play.draft(82_300, DraftPart::Reasoning { offset: 0, delta: String::new(), title: None });
        moment(&mut play, 12, 0, depth);
    }
}
