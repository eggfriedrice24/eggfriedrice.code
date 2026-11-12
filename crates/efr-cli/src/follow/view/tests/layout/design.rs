//! The rows of tool calls against the frames of the terminal design (`TM-Calls`,
//! `TM-Diffs`, `TM-Legend`, `TM-LongTurn`, `TM-BeforeAfter`, `TM-Sequence`,
//! `TM-Depths`): real events through a real view, the screen that a terminal keeps,
//! compared cell by cell with the frame in each depth that the board draws or that the
//! table of `efr_test_support::design::expect` derives.
//!
//! The colours inside the rows of a diff (the syntax colours of the file, and the
//! weight of a removed row without colour) are the render slice's (`efr_render::
//! diff_lines`): the checks here compare those cells by their text and their fill, and
//! everything around them (the call's row, the indent, the sign, the hunk header, the
//! cut) by colour and attributes too.

use std::ops::{Range, RangeBounds};
use std::path::PathBuf;

use efr_protocol::{
    BlockReason, Blocked, ChangeKind, ContextUse, Draft, DraftPart, Event, EventEnvelope,
    FileChange, FileChanges, InputWait, JobId, SandboxSummary, Seq,
};
use efr_test_support::design::{Depth, Frame, cells, check_rows_where};
use serde_json::{Value, json};

use super::super::super::{Look, TurnView};
use super::super::at;
use super::{Screen, id, message, shell, started, turn_started};
use crate::terminal::Size;
use crate::testing::{design_fixed, design_options, turn};

/// The look of the frames: no motion, 20 lines of a diff, 8 in a long turn.
const LOOK: Look = Look {
    motion: false,
    summary: false,
    progress: false,
    diff_lines: 20,
    diff_lines_long: 8,
    bell: false,
};

/// A turn played on a view and the screen of a terminal `cols` wide.
struct Play {
    view: TurnView,
    screen: Screen,
    size: Size,
    seq: u64,
    now: i64,
}

impl Play {
    fn new(depth: Depth, cols: u16) -> Play {
        let mut view = TurnView::new(turn(), design_options(depth, cols))
            .with_look(LOOK)
            .with_home(Some(PathBuf::from("/home/user")));
        view.start();
        let size = Size { cols, rows: 60 };
        let mut play = Play { view, screen: Screen::new(cols), size, seq: 10, now: 0 };
        play.event(0, turn_started());
        play
    }

    /// The daemon sends `event`, `millis` after the prompt.
    fn event(&mut self, millis: i64, event: Event) -> &mut Play {
        self.now = millis;
        self.seq += 1;
        let envelope =
            EventEnvelope { seq: Seq::new(self.seq), conversation_id: None, at: at(millis), event };
        self.view.envelope(&envelope, self.size, true);
        self.frame();
        self
    }

    /// The daemon sends a draft of the model's context: `percent` of its limit.
    fn context(&mut self, percent: u64) -> &mut Play {
        let part = DraftPart::Context(ContextUse { tokens: percent, limit: 100, window: 100 });
        let draft = Draft { turn_id: turn(), after_seq: Seq::new(self.seq), draft: part };
        self.view.draft(&draft, self.size);
        self.frame();
        self
    }

    fn frame(&mut self) {
        let frame = self.view.frame(self.size, at(self.now));
        self.screen.write(&frame);
    }

    /// The live rows above the status row at `millis`, after a frame then.
    fn live(&mut self, millis: i64) -> String {
        self.now = millis;
        self.frame();
        self.view.live_body(self.size, at(millis))
    }

    /// The glyphs of the view's options.
    fn view_glyphs(&self) -> efr_render::Glyphs {
        *self.view.options.glyphs()
    }

    /// The rows that the scrollback keeps once the view ends.
    fn committed(&mut self) -> String {
        self.view.close();
        self.frame();
        self.screen.raw()
    }
}

/// `text` with the glyph of the spinner that it shows replaced by `glyph`, the one that
/// a board draws.
fn spun(text: &str, glyph: &str) -> String {
    let glyphs = efr_render::Glyphs::UNICODE;
    let mut text = text.to_owned();
    for frame in glyphs.spinner.iter().chain([&glyphs.still]) {
        text = text.replace(frame, glyph);
    }
    text
}

/// Rows `range` of `text`, each with its newline.
fn rows(text: &str, range: Range<usize>) -> String {
    let all: Vec<&str> = text.split_inclusive('\n').collect();
    all.get(range.clone())
        .unwrap_or_else(|| panic!("rows {range:?} of {} rows:\n{text}", all.len()))
        .concat()
}

/// Checks rows `range` of `frame` in `depth` against `real`, every cell by its colour
/// and attributes but the cells that `text_only` names (row from the start of `range`,
/// column).
fn check(
    frame: &Frame,
    range: impl RangeBounds<usize>,
    real: &str,
    depth: Depth,
    text_only: impl Fn(usize, usize) -> bool,
) {
    if let Err(problem) = check_rows_where(frame, range, real, depth, text_only) {
        panic!("the output differs from the design: {problem}");
    }
}

/// Every cell by its colour and attributes.
fn all(_: usize, _: usize) -> bool {
    false
}

/// The cells of the code of a diff's rows: from `column` on, in the rows of `real` that
/// are rows of a diff (a sign at `column - 2`, or the indent of a line that stays or
/// goes on). The sign itself counts, with its colour, in colour; without colour the
/// render slice gives a removed row its weight, so the sign is text only there too.
fn diff_code(real: &str, column: usize, depth: Depth) -> impl Fn(usize, usize) -> bool {
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
        })
        .collect();
    let sign_too = depth == Depth::NoColor;
    move |row, col| {
        code.get(row).copied().unwrap_or(false)
            && (col >= column || (sign_too && col + 2 == column))
    }
}

/// The cells of [`diff_code`] that a test compares by their text alone: none in
/// truecolor, where the code theme of the design system colours the code
/// (`design_options`); all in 16 colours and without colour, where a changed row is
/// whole in its diff role or its weight (TM-Diffs 2 and 3, the note of TM-Depths),
/// which beats the syntax that a truecolor board draws.
fn code_cells(real: &str, column: usize, depth: Depth) -> impl Fn(usize, usize) -> bool {
    let code = diff_code(real, column, depth);
    let truecolor = depth == Depth::TrueColor;
    move |row, col| !truecolor && code(row, col)
}

// --- events ----------------------------------------------------------------------

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

fn refused(n: u8, reason: &str) -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: id(n),
        output: String::new(),
        truncated: false,
        is_error: true,
        exit_code: None,
        sandbox: None,
        refusal: Some(reason.to_owned()),
        changes: None,
        diff: None,
        left_running: false,
    }
}

fn output(n: u8, tail: &str) -> Event {
    Event::ToolCallOutputUpdated {
        turn_id: turn(),
        call_id: id(n),
        tail: tail.to_owned(),
        bytes: tail.len() as u64,
    }
}

fn read(n: u8, path: &str) -> Event {
    started(n, "read_file", json!({ "path": path }), None)
}

fn tool(n: u8, name: &str, input: Value) -> Event {
    started(n, name, input, None)
}

fn file(path: &str, kind: ChangeKind, added: u32, removed: u32) -> FileChange {
    FileChange { path: path.to_owned(), kind, from: None, added, removed, binary: false }
}

fn changes(files: Vec<FileChange>) -> FileChanges {
    let added = files.iter().map(|file| file.added).sum();
    let removed = files.iter().map(|file| file.removed).sum();
    FileChanges { files, more: 0, added, removed }
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

/// The edit of `crates/efr-daemon/src/turn.rs` of the Diffs board: two hunks, and 14
/// lines that the daemon cut.
const TURN_DIFF: &str = "\
--- a/crates/efr-daemon/src/turn.rs
+++ b/crates/efr-daemon/src/turn.rs
@@ -88,4 +88,8 @@ impl Turn {
     pub fn finish(&mut self) -> usize {
         let mut drained = 0;
-        self.state = State::Done;
+        while let Ok(msg) = self.rx.try_recv() {
+            self.pending.push(msg);
+            drained += 1;
+        }
+        self.state = State::Done;
         drained
@@ -131,1 +131,1 @@ fn steer
-        self.pending.push(msg);
+        self.tx.send(msg)?;
... 14 more lines
";

fn edit_turn(n: u8) -> Event {
    tool(
        n,
        "edit",
        json!({ "path": "crates/efr-daemon/src/turn.rs", "old_string": "a", "new_string": "b" }),
    )
}

// --- one row per call -------------------------------------------------------------

#[test]
fn a_running_call_is_the_board() {
    // TM-Calls 1, TM-Legend 14 and 15, TM-Sequence 5, TM-Depths 3, 7, 11: the spinner,
    // the command in text, the time after two spaces, the gauge after three, the tail 4
    // columns in, muted.
    let tail = "   Compiling efr-daemon v0.0.5 (crates/efr-daemon)\ntest turn::tests::steer_during_turn ... ok\ntest turn::tests::steer_after_finish ... ok\n";
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        play.context(38).event(10, shell(1, "cargo test -p efr-daemon steer"));
        play.event(500, output(1, tail));
        // NOTE: the spinner turns with the frames; the board draws one glyph of it.
        let live = spun(&play.live(12_500), "\u{283c}");
        check(&Frame::load("Calls", 1).unwrap(), 0..4, &live, depth, all);
        check(&Frame::load("Sequence", 5).unwrap(), 4..8, &live, depth, all);
        let depths = match depth {
            Depth::TrueColor => 3,
            Depth::Ansi16 => 7,
            Depth::NoColor => 11,
        };
        let two = format!("{}{}", rows(&live, 0..1), rows(&live, 2..4));
        check(&Frame::load("Depths", depths).unwrap(), 2..5, &two, depth, all);
    }
    let mut play = Play::new(Depth::TrueColor, 44);
    play.event(10, shell(1, "cargo test"));
    play.event(100, output(1, "test steer_after_finish ... ok\n"));
    let live = spun(&play.live(12_100), "\u{2839}");
    check(&Frame::load("Legend", 14).unwrap(), .., &rows(&live, 0..1), Depth::TrueColor, all);
    check(&Frame::load("Legend", 15).unwrap(), .., &rows(&live, 1..2), Depth::TrueColor, all);
}

#[test]
fn a_call_that_went_well_failed_or_was_refused_is_one_row() {
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        // TM-Calls 2, 3, 4, 10.
        play.event(10, shell(1, "cargo test -p efr-daemon steer"));
        play.event(38_010, ended(1, Some(0), ""));
        play.event(38_020, shell(2, "cargo clippy -p efr-daemon -- -D warnings"));
        play.event(
            47_020,
            ended(
                2,
                Some(101),
                "error: this `if` has identical blocks\n   --> crates/efr-daemon/src/turn.rs:97:9\nerror: could not compile `efr-daemon` (lib) due to 1 previous error\n[exit code 101]",
            ),
        );
        play.event(47_030, shell(3, "test -f target/release/efrd"));
        play.event(47_130, ended(3, Some(1), ""));
        play.event(47_140, shell(4, "curl -sI https://example.com"));
        play.event(47_150, refused(4, "no network in the sandbox"));
        let shown = play.committed();
        check(&Frame::load("Calls", 2).unwrap(), .., &rows(&shown, 0..1), depth, all);
        check(&Frame::load("Calls", 3).unwrap(), .., &rows(&shown, 1..5), depth, all);
        check(&Frame::load("Calls", 4).unwrap(), .., &rows(&shown, 5..6), depth, all);
        check(&Frame::load("Calls", 10).unwrap(), .., &rows(&shown, 6..7), depth, all);
    }
}

#[test]
fn the_legend_of_the_call_rows() {
    let depth = Depth::TrueColor;
    let mut play = Play::new(depth, 44);
    play.event(10, shell(1, "cargo test -p app"));
    play.event(38_010, ended(1, Some(0), ""));
    play.event(38_020, shell(2, "make"));
    play.event(39_520, ended(2, Some(2), "make: *** No rule to make target\n"));
    // NOTE: TM-Legend 10 leaves the reason out; the reason is the board's `curl …`.
    play.event(39_530, shell(3, "curl \u{2026}"));
    play.event(39_540, refused(3, ""));
    let shown = play.committed();
    check(&Frame::load("Legend", 7).unwrap(), .., &rows(&shown, 0..1), depth, all);
    check(&Frame::load("Legend", 8).unwrap(), .., &rows(&shown, 1..2), depth, all);
    check(&Frame::load("Legend", 9).unwrap(), .., &rows(&shown, 2..3), depth, all);
    check(&Frame::load("Legend", 10).unwrap(), .., &rows(&shown, 3..4), depth, all);
}

#[test]
fn a_command_wider_than_the_screen_goes_on_at_column_six() {
    // TM-Calls 16: the cut at the last space that fits, a muted `\`, the next row 4
    // columns after the `$`, and the time after the last row.
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 60);
        play.event(
            10,
            shell(
                1,
                "cargo run -p efr-store --bin migrate -- --dry-run ~/.local/share/efr/sessions",
            ),
        );
        play.event(4_110, ended(1, Some(0), ""));
        check(&Frame::load("Calls", 16).unwrap(), .., &play.committed(), depth, all);
    }
}

#[test]
fn a_call_that_changed_files_or_met_the_proxy_gets_a_muted_row_under_it() {
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        // TM-Calls 5.
        play.event(10, shell(1, "cargo add rusqlite -p efr-store --features bundled"));
        let cargo = changes(vec![
            file("crates/efr-store/Cargo.toml", ChangeKind::Modified, 1, 0),
            file("Cargo.lock", ChangeKind::Modified, 18, 0),
        ]);
        play.event(1_210, wrote(1, cargo, None));
        // TM-Calls 13.
        play.event(1_220, shell(2, "npm install"));
        let blocked = Blocked {
            host: "registry.npmjs.org".into(),
            port: 443,
            reason: BlockReason::NotAllowed,
            count: 3,
        };
        let summary = SandboxSummary { blocked: vec![blocked], ..SandboxSummary::default() };
        let mut end = ended(2, Some(0), "");
        if let Event::ToolCallCompleted { sandbox, .. } = &mut end {
            *sandbox = Some(summary);
        }
        play.event(9_420, end);
        let shown = play.committed();
        check(&Frame::load("Calls", 5).unwrap(), .., &rows(&shown, 0..2), depth, all);
        // NOTE: the end of the turn adds the line of where the sandbox wrote; the rows
        // of the calls come first.
        check(&Frame::load("Calls", 13).unwrap(), .., &rows(&shown, 2..4), depth, all);
    }
}

#[test]
fn a_denied_call_a_job_and_a_moved_call_are_muted_rows() {
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        // TM-Calls 12, row 2: the answer line before it is the live slice's.
        play.event(10, shell(1, "cargo update -p rusqlite"));
        play.event(20, super::asked(1, "network: index.crates.io", None));
        play.view.answered(id(1), efr_protocol::ApprovalDecision::Deny, play.size);
        play.event(30, refused_by_you(1));
        // TM-Calls 14, row 1: the line of the job is the commands slice's
        // (`format::jobs::started`).
        play.event(40, tool(2, "shell", json!({ "command": "npm run dev", "background": true })));
        play.event(440, ended(2, Some(0), ""));
        // TM-Calls 15, row 1: the line of the job is the commands slice's.
        play.event(450, shell(3, "cargo test --workspace"));
        let job: JobId = "j4".parse().unwrap();
        play.view.moved(id(3), job);
        play.event(42_450, ended(3, None, ""));
        let shown = play.committed();
        let all_rows: Vec<String> =
            cells(&shown).iter().map(efr_test_support::design::CellRow::text).collect();
        let at = |start: &str| all_rows.iter().position(|row| row.starts_with(start)).unwrap();
        let denied = at("\u{b7} $");
        check(
            &Frame::load("Calls", 12).unwrap(),
            1..2,
            &rows(&shown, denied..denied + 1),
            depth,
            all,
        );
        check(
            &Frame::load("Legend", 11).unwrap(),
            ..,
            &rows(&shown, denied..denied + 1).replace(" -p rusqlite", ""),
            depth,
            all,
        );
        let background = at("\u{2713} $ npm");
        check(
            &Frame::load("Calls", 14).unwrap(),
            0..1,
            &rows(&shown, background..background + 1),
            depth,
            all,
        );
        let moved = at("\u{2192} $");
        check(
            &Frame::load("Calls", 15).unwrap(),
            0..1,
            &rows(&shown, moved..moved + 1),
            depth,
            all,
        );
    }
}

fn refused_by_you(n: u8) -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: id(n),
        output: String::new(),
        truncated: false,
        is_error: true,
        exit_code: None,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: false,
    }
}

/// TM-Calls 6 and 11: a file tool's row names its path as the user knows it, also when
/// the model wrote the whole path: below the turn's directory without it, below the
/// home directory with `~`.
#[test]
fn a_whole_path_shows_from_the_project_or_from_home() {
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        play.event(10, read(1, "/home/user/project/crates/efr-daemon/src/turn.rs"));
        play.event(20, ended(1, None, ""));
        play.event(
            30,
            tool(2, "write_file", json!({ "path": "/home/user/.ssh/config", "content": "x" })),
        );
        play.event(40, refused(2, "efr's config (floor)"));
        let shown = play.committed();
        check(&Frame::load("Calls", 6).unwrap(), .., &rows(&shown, 0..1), depth, all);
        check(&Frame::load("Calls", 11).unwrap(), .., &rows(&shown, 1..2), depth, all);
    }
}

/// TM-Live 21 and 22: a command that asks. Its row has no gauge, its tail is muted, and
/// the answer line under it is the one row in the warning colour, then what is typed.
#[test]
fn a_command_that_asks_is_its_row_its_muted_tail_and_the_answer_line_live_21_22() {
    let cases = [
        (21, ":: Proceed with installation? [Y/n] ", InputWait::Visible, Some("y"), 14_000),
        (22, "[sudo] password for sam: ", InputWait::Hidden, None, 3_000),
    ];
    for (number, prompt, wait, typed, millis) in cases {
        for depth in Depth::ALL {
            let mut play = Play::new(depth, 100);
            play.context(38).event(10, shell(1, "sudo pacman -Syu"));
            play.event(500, output(1, prompt));
            play.event(
                600,
                Event::ToolCallInputChanged {
                    turn_id: turn(),
                    call_id: id(1),
                    input: wait,
                    looks_secret: false,
                },
            );
            if let Some(typed) = typed {
                play.view.typed(typed);
            }
            let live = spun(&play.live(millis + 10), "\u{283c}");
            check(&Frame::load("Live", number).unwrap(), .., &live, depth, all);
        }
    }
}

/// TM-Live 19 and the rule of the live zone while a call runs: the unread steers and the
/// queued prompts stand above the call row.
#[test]
fn the_waiting_rows_stand_above_a_running_call() {
    let mut view =
        TurnView::new(turn(), design_options(Depth::NoColor, 100)).with_look(LOOK).with_input();
    view.start();
    let size = Size { cols: 100, rows: 60 };
    let mut play = Play { view, screen: Screen::new(100), size, seq: 10, now: 0 };
    play.event(0, turn_started());
    play.event(10, shell(1, "cargo test"));
    let next = "019a9b1c-3d00-7a10-8b20-0000000000f1".parse().unwrap();
    play.view.queued_prompt(next, "then write the changelog entry".to_owned(), false);
    let live = play.live(5_000);
    let texts: Vec<String> =
        cells(&live).iter().map(efr_test_support::design::CellRow::text).collect();
    let queued = texts.iter().position(|row| row.starts_with("\u{21b3} queued")).unwrap();
    let call = texts.iter().position(|row| row.contains("$ cargo test")).unwrap();
    assert!(queued < call, "{texts:?}");
}

// --- the fold of reads ------------------------------------------------------------

#[test]
fn reads_that_follow_each_other_fold_into_one_growing_row() {
    let paths = [
        "crates/efr-store/src/lib.rs",
        "crates/efr-store/src/session.rs",
        "crates/efr-store/src/json.rs",
        "crates/efr-store/src/import.rs",
        "crates/efr-store/src/open.rs",
        "crates/efr-store/src/migrate.rs",
    ];
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        // TM-Calls 6: one read.
        play.event(10, read(1, "crates/efr-daemon/src/turn.rs"));
        play.event(20, ended(1, None, ""));
        check(&Frame::load("Calls", 6).unwrap(), .., &play.live(30), depth, all);
        // A message ends the fold; six more reads fold again (TM-Calls 7).
        play.event(40, message(0, "Next.\n"));
        for (n, path) in (2..).zip(paths) {
            let millis = i64::from(n) * 100;
            play.event(millis, read(n, path));
            play.event(millis + 50, ended(n, None, ""));
        }
        // NOTE: the blank row above the fold is the one between prose and a row.
        let live = play.live(1_000);
        check(&Frame::load("Calls", 7).unwrap(), .., &rows(&live, 1..2), depth, all);
        // Another call writes the fold once, as one row.
        play.event(1_010, shell(9, "cargo build"));
        play.event(1_020, ended(9, Some(0), ""));
        let shown = play.committed();
        let all_rows: Vec<String> =
            cells(&shown).iter().map(efr_test_support::design::CellRow::text).collect();
        assert_eq!(
            all_rows,
            [
                "\u{2713} read crates/efr-daemon/src/turn.rs",
                "",
                "Next.",
                "",
                "\u{2713} read 6 files  crates/efr-store/src/lib.rs, session.rs, json.rs (+3)",
                "\u{2713} $ cargo build",
            ]
        );
        check(&Frame::load("Calls", 7).unwrap(), .., &rows(&shown, 4..5), depth, all);
    }
}

#[test]
fn a_failed_read_ends_the_fold_and_is_loud() {
    let mut play = Play::new(Depth::NoColor, 80);
    play.event(10, read(1, "a.rs"));
    play.event(20, ended(1, None, ""));
    play.event(30, read(2, "b.rs"));
    let mut failed = ended(2, None, "no such file");
    if let Event::ToolCallCompleted { is_error, .. } = &mut failed {
        *is_error = true;
    }
    play.event(130, failed);
    let shown = play.committed();
    let texts: Vec<String> =
        cells(&shown).iter().map(efr_test_support::design::CellRow::text).collect();
    assert_eq!(
        texts,
        ["\u{2713} read a.rs", "\u{2717} read b.rs  failed \u{b7} 0.1s", "    no such file"]
    );
}

// --- file edits and their diffs --------------------------------------------------

/// The Diffs board: an edit with two hunks, a new file, a delete, a move and a patch
/// of three files, after a line of prose.
fn diffs_turn(depth: Depth, cols: u16) -> Play {
    let mut play = Play::new(depth, cols);
    play.event(5, message(0, "I drain the channel before the end check:\n"));
    play.event(10, edit_turn(1));
    let turn_rs = changes(vec![file("crates/efr-daemon/src/turn.rs", ChangeKind::Modified, 7, 2)]);
    play.event(20, wrote(1, turn_rs, Some(TURN_DIFF)));
    play.event(30, tool(2, "write_file", json!({ "path": "notes/steer-race.md", "content": "x" })));
    let notes = changes(vec![file("notes/steer-race.md", ChangeKind::Added, 12, 0)]);
    play.event(40, wrote(2, notes, Some(NOTES_DIFF)));
    play.event(50, tool(3, "apply_patch", json!(DELETE_PATCH)));
    let deleted = changes(vec![file("crates/efr-store/src/cache.rs", ChangeKind::Deleted, 0, 212)]);
    play.event(60, wrote(3, deleted, Some("--- a/crates/efr-store/src/cache.rs\n+++ /dev/null\n@@ -1,212 +0,0 @@\n-//! A cache.\n... 211 more lines\n")));
    play.event(70, tool(4, "apply_patch", json!(MOVE_PATCH)));
    let mut moved = file("crates/efr-tools/src/expression.rs", ChangeKind::Renamed, 1, 1);
    moved.from = Some("crates/efr-tools/src/expr.rs".to_owned());
    play.event(80, wrote(4, changes(vec![moved]), Some(MOVE_DIFF)));
    play.event(90, tool(5, "apply_patch", json!(THREE_PATCH)));
    let three = changes(vec![
        file("crates/efr-daemon/src/turn.rs", ChangeKind::Modified, 5, 1),
        file("crates/efr-daemon/tests/steer.rs", ChangeKind::Added, 3, 0),
        file("CHANGELOG.md", ChangeKind::Modified, 1, 1),
    ]);
    play.event(100, wrote(5, three, Some(THREE_DIFF)));
    play
}

const NOTES_DIFF: &str = "\
--- /dev/null
+++ b/notes/steer-race.md
@@ -0,0 +1,12 @@
+# A steer after the end
+
+The loop checked the channel only between calls. A steer that came
+during the last call stayed in pending until the next turn.
... 8 more lines
";

const DELETE_PATCH: &str =
    "*** Begin Patch\n*** Delete File: crates/efr-store/src/cache.rs\n*** End Patch\n";

const MOVE_PATCH: &str = "*** Begin Patch\n*** Update File: crates/efr-tools/src/expr.rs\n*** Move to: crates/efr-tools/src/expression.rs\n@@\n-//! Shell expressions.\n+//! Shell expressions: the words of a line, before efr judges it.\n*** End Patch\n";

const MOVE_DIFF: &str = "\
--- a/crates/efr-tools/src/expr.rs
+++ b/crates/efr-tools/src/expression.rs
@@ -1 +1 @@ expression
-//! Shell expressions.
+//! Shell expressions: the words of a line, before efr judges it.
";

const THREE_PATCH: &str = "*** Begin Patch\n*** Update File: crates/efr-daemon/src/turn.rs\n@@\n-a\n+b\n*** Add File: crates/efr-daemon/tests/steer.rs\n+#[test]\n*** Update File: CHANGELOG.md\n@@\n-x\n+y\n*** End Patch\n";

const THREE_DIFF: &str = "\
--- a/crates/efr-daemon/src/turn.rs
+++ b/crates/efr-daemon/src/turn.rs
@@ -131,1 +131,1 @@ fn steer
-        self.pending.push(msg);
+        self.tx.send(msg)?;
... 4 more lines
--- /dev/null
+++ b/crates/efr-daemon/tests/steer.rs
@@ -0,0 +1,3 @@
+#[test]
+fn steer_during_last_call() {
... 1 more line
--- a/CHANGELOG.md
+++ b/CHANGELOG.md
@@ -3,1 +3,1 @@
-- steers can land after a turn
+- a steer during the last call reaches the model
";

#[test]
fn a_file_edit_is_its_row_and_its_diff() {
    // TM-Diffs 1, 2, 3. The blank rows that the board draws between the edits (17,
    // 24, 27, 32) lose to DECISIONS.md 2 ("no blank line between calls"), as TM-Calls
    // 17 and TM-LongTurn 3 draw it: the blocks are compared one by one.
    for (depth, number) in [(Depth::TrueColor, 1), (Depth::Ansi16, 2), (Depth::NoColor, 3)] {
        let frame = Frame::load("Diffs", number).unwrap();
        let shown = diffs_turn(depth, 100).committed();
        let edit = rows(&shown, 0..17);
        check(&frame, 0..17, &edit, depth, code_cells(&edit, 4, depth));
        if depth != Depth::TrueColor {
            continue;
        }
        // NOTE: a first hunk with no function context has no header row (row 43, the
        // diff of `CHANGELOG.md`).
        let blocks = [(18..24, 17..23), (25..27, 23..25), (28..32, 25..29), (33..46, 29..42)];
        for (board, real) in blocks {
            let part = rows(&shown, real);
            let inner = board.start >= 33;
            let column = if inner { 6 } else { 4 };
            // NOTE: the board draws `pending` of the Markdown file as inline code, but
            // its text has no backticks (a diff shows the file's text as it is): the
            // code of that block is compared by its text.
            let code: Box<dyn Fn(usize, usize) -> bool> = if board.start == 18 {
                Box::new(diff_code(&part, column, depth))
            } else {
                Box::new(code_cells(&part, column, depth))
            };
            // NOTE: the rows of a patch of several files are at 4 columns: their code
            // starts at column 6, and the rows that name the files are no code.
            check(&frame, board, &part, depth, code);
        }
    }
    // The Legend: the edit row, the hunk header, an added row, the cut.
    let depth = Depth::TrueColor;
    let mut play = Play::new(depth, 44);
    play.event(
        10,
        tool(1, "edit", json!({ "path": "src/turn.rs", "old_string": "a", "new_string": "b" })),
    );
    let diff = "--- a/src/turn.rs\n+++ b/src/turn.rs\n@@ -88,1 +88,5 @@ impl Turn {\n+let x = 1;\n... 14 more lines\n";
    play.event(
        20,
        wrote(1, changes(vec![file("src/turn.rs", ChangeKind::Modified, 5, 1)]), Some(diff)),
    );
    let shown = play.committed();
    check(&Frame::load("Legend", 19).unwrap(), .., &rows(&shown, 0..1), depth, all);
    check(&Frame::load("Legend", 20).unwrap(), .., &rows(&shown, 1..2), depth, all);
    let added = rows(&shown, 2..3);
    check(&Frame::load("Legend", 21).unwrap(), .., &added, depth, code_cells(&added, 4, depth));
    check(&Frame::load("Legend", 22).unwrap(), .., &rows(&shown, 3..4), depth, all);
}

#[test]
fn a_row_of_a_diff_wider_than_the_screen_goes_on_after_a_muted_mark() {
    // TM-Diffs 4. NOTE: the board draws a frame 60 columns wide, but its rows fit in 60
    // columns: the cut that it draws, after `self.rx.`, falls at 41 columns. The rows
    // are the board's at that width.
    let depth = Depth::TrueColor;
    let frame = Frame::load("Diffs", 4).unwrap();
    let mut play = Play::new(depth, 41);
    play.event(10, edit_turn(1));
    let diff = "--- a/crates/efr-daemon/src/turn.rs\n+++ b/crates/efr-daemon/src/turn.rs\n@@ -88,1 +88,2 @@ impl Turn {\n-        self.state = State::Done;\n+        while let Ok(msg) = self.rx.try_recv() {\n+            self.pending.push(msg);\n";
    let turn_rs = changes(vec![file("crates/efr-daemon/src/turn.rs", ChangeKind::Modified, 7, 2)]);
    play.event(20, wrote(1, turn_rs, Some(diff)));
    let shown = play.committed();
    // NOTE: at 41 columns the edit's row goes on in a second row.
    let texts: Vec<String> =
        cells(&shown).iter().map(efr_test_support::design::CellRow::text).collect();
    let hunk = texts.iter().position(|row| row.starts_with("  @@")).unwrap();
    check(&frame, 1..2, &rows(&shown, hunk..hunk + 1), depth, all);
    let part = rows(&shown, hunk + 1..hunk + 5);
    // The code of the rows, and of the row that goes on (from column 10); the mark of
    // the cut at column 40 is muted.
    let code = code_cells(&part, 4, depth);
    check(&frame, 2..6, &part, depth, move |row, col| {
        (row == 2 && col >= 10) || (code(row, col) && !(row == 1 && col == 40))
    });
}

// --- whole runs of calls ----------------------------------------------------------

/// Call `n`, a shell call of `command` that started at `from` and ran `took`
/// milliseconds with exit code 0.
fn ran(play: &mut Play, n: u8, command: &str, from: i64, took: i64) {
    play.event(from, shell(n, command));
    play.event(from + took, ended(n, Some(0), ""));
}

/// Reads `paths` as calls `first`, `first + 1`, ..., from `from` on.
fn reads(play: &mut Play, first: u8, paths: &[&str], from: i64) {
    for (n, path) in (first..).zip(paths) {
        let at = from + i64::from(n - first) * 10;
        play.event(at, read(n, path));
        play.event(at + 5, ended(n, None, ""));
    }
}

/// Call `n` writes `path` with the edit tool, which changed `added` and `removed` lines
/// as `diff` shows.
fn edited(play: &mut Play, n: u8, path: &str, added: u32, removed: u32, diff: &str, at: i64) {
    play.event(at, tool(n, "edit", json!({ "path": path, "old_string": "a", "new_string": "b" })));
    let changed = changes(vec![file(path, ChangeKind::Modified, added, removed)]);
    let diff = format!("--- a/{path}\n+++ b/{path}\n{diff}");
    play.event(at + 5, wrote(n, changed, Some(&diff)));
}

#[test]
fn twelve_calls_in_a_row_in_three_depths() {
    // TM-Calls 17, 18, 19. Row 1 (`search`) waits for a search tool; row 17 (the line
    // of the job) is the commands slice's.
    let judge = "@@ -140,1 +140,1 @@ fn judge\n-        \"sudo\" => Verdict::Allow,\n+        \"sudo\" => Verdict::Ask,\n";
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        let paths = [
            "crates/efr-tools/src/expr.rs",
            "crates/efr-tools/src/words.rs",
            "crates/efr-tools/src/judge.rs",
            "crates/efr-tools/src/lib.rs",
        ];
        reads(&mut play, 1, &paths, 10);
        ran(&mut play, 5, "cargo build -p efr-tools", 100, 6_100);
        ran(&mut play, 6, "cargo test -p efr-tools expr", 6_300, 4_300);
        play.event(10_700, shell(7, "cargo test -p efr-tools judge"));
        let tail = "test judge::tests::sudo_asks ... FAILED\nassertion `left == right` failed: left: Ask, right: Allow\n  at crates/efr-tools/src/judge.rs:212:9\n";
        play.event(14_600, ended(7, Some(101), tail));
        edited(&mut play, 8, "crates/efr-tools/src/judge.rs", 2, 1, judge, 14_700);
        ran(&mut play, 9, "cargo test -p efr-tools judge", 14_800, 3_700);
        play.event(18_600, shell(10, "curl -sI https://example.com"));
        play.event(18_610, refused(10, "no network in the sandbox"));
        ran(&mut play, 11, "rg -c 'judge' crates", 18_700, 100);
        ran(&mut play, 12, "cargo fmt --all", 18_900, 800);
        play.event(19_800, shell(13, "cargo test --workspace"));
        play.view.moved(id(13), "j4".parse().unwrap());
        play.event(61_800, ended(13, None, ""));
        let shown = play.committed();
        let frame = Frame::load("Calls", 17).unwrap();
        check(&frame, 0..1, &rows(&shown, 0..1), depth, all);
        let rest = rows(&shown, 1..16);
        check(&frame, 2..17, &rest, depth, code_cells(&rest, 4, depth));
    }
}

/// Rows of the output and the rows of a frame that they match.
type Pair = (Range<usize>, Range<usize>);

/// The calls of TM-LongTurn 3 without its prose, its thought rows, its steer and its
/// `search` rows: the rows of the output and the rows of the frame that they match
/// (from, to), in order.
fn long_turn(depth: Depth) -> (String, Vec<Pair>) {
    let mut play = Play::new(depth, 100);
    let store = |name: &str| format!("crates/efr-store/src/{name}");
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
    reads(&mut play, 1, &eleven, 10);
    ran(&mut play, 12, "cargo tree -p efr-store -i rusqlite", 200, 400);
    play.event(700, shell(13, "cargo add rusqlite -p efr-store --features bundled"));
    let cargo = changes(vec![
        file("crates/efr-store/Cargo.toml", ChangeKind::Modified, 1, 0),
        file("Cargo.lock", ChangeKind::Modified, 18, 0),
    ]);
    play.event(1_900, wrote(13, cargo, None));
    play.event(
        2_000,
        tool(14, "write_file", json!({ "path": store("sqlite.rs"), "content": "x" })),
    );
    let mut sqlite = "--- /dev/null\n+++ b/crates/efr-store/src/sqlite.rs\n@@ -0,0 +1,142 @@\n+use rusqlite::{Connection, params};\n+\n+pub struct SqliteStore {\n+    conn: Connection,\n+}\n+\n+impl SqliteStore {\n+    // WAL keeps the turn loop's writes off the read path\n".to_owned();
    for n in 9..=142 {
        sqlite.push_str(&format!("+    // line {n}\n"));
    }
    let new = changes(vec![file(&store("sqlite.rs"), ChangeKind::Added, 142, 0)]);
    play.event(2_010, wrote(14, new, Some(&sqlite)));
    // NOTE: `\x66` is `f`: the text of a diff, not a `cfg` of this crate.
    let lib = "@@ -3,2 +3,4 @@ mod json\n mod session;\n-mod json;\n+#[cfg(\x66eature = \"legacy-store\")]\n+mod json;\n+mod sqlite;\n";
    edited(&mut play, 15, &store("lib.rs"), 3, 1, lib, 2_100);
    ran(&mut play, 16, "cargo build -p efr-store", 2_200, 18_000);
    play.event(20_300, shell(17, "cargo test -p efr-store"));
    let tail = "test sqlite::tests::import_missing_end ... FAILED\nthread 'sqlite::tests::import_missing_end' panicked at crates/efr-store/src/sqlite.rs:88:31:\ncalled `Option::unwrap()` on a `None` value\n";
    play.event(61_300, ended(17, Some(101), tail));
    reads(&mut play, 18, &[&store("sqlite.rs")], 61_400);
    let fix = "@@ -88,1 +88,1 @@ fn import\n-        let ended = row.ended_at.unwrap();\n+        let ended = row.ended_at.unwrap_or(row.started_at);\n";
    edited(&mut play, 19, &store("sqlite.rs"), 1, 1, fix, 61_500);
    ran(&mut play, 20, "cargo test -p efr-store", 61_600, 39_000);
    let four = [store("json.rs"), store("import.rs"), store("old.json"), store("migrate.rs")];
    let four: Vec<&str> = four.iter().map(String::as_str).collect();
    reads(&mut play, 21, &four, 100_700);
    ran(&mut play, 25, "cargo clippy -p efr-store --all-features -- -D warnings", 100_800, 22_000);
    ran(
        &mut play,
        26,
        "cargo run -p efr-store --bin migrate -- --dry-run ~/.local/share/efr/sessions",
        122_900,
        4_100,
    );
    ran(&mut play, 27, "ls ~/.local/share/efr/sessions | wc -l", 127_100, 100);
    let three =
        [store("migrate.rs"), store("open.rs"), "crates/efr-store/src/CHANGELOG.md".to_owned()];
    let three: Vec<&str> = three.iter().map(String::as_str).collect();
    reads(&mut play, 28, &three, 127_300);
    ran(&mut play, 31, "cargo fmt --all", 127_400, 900);
    ran(&mut play, 32, "cargo bench -p efr-store --bench open -- --quick", 128_400, 48_000);
    ran(&mut play, 33, "cargo test --workspace", 176_500, 132_000);
    play.event(308_600, shell(34, "cargo update -p rusqlite"));
    play.event(308_610, super::asked(34, "network: index.crates.io", None));
    play.view.answered(id(34), efr_protocol::ApprovalDecision::Deny, play.size);
    play.event(308_620, refused_by_you(34));
    ran(&mut play, 35, "git diff --stat", 308_700, 100);
    ran(&mut play, 36, "just lint", 308_900, 31_000);
    ran(&mut play, 37, "git add -A", 340_000, 100);
    ran(
        &mut play,
        38,
        "git commit -m 'store: sessions in SQLite, JSON reader behind legacy-store'",
        340_200,
        300,
    );
    let shown = play.committed();
    // NOTE: the frame's rows of a thought, of prose, of `search` and of the answer to
    // the question are other slices' (or wait for a search tool).
    let pairs = vec![
        (0..1, 3..4),
        (1..2, 5..6),
        (2..21, 12..31),
        (21..22, 31..32),
        (22..32, 33..43),
        (32..33, 44..45),
        (33..34, 46..47),
        (34..39, 52..57),
        (39..40, 58..59),
        (41..42, 60..61),
        (42..46, 64..68),
    ];
    (shown, pairs)
}

#[test]
fn the_long_turn_is_one_row_for_each_call() {
    // TM-LongTurn 3 and 4 (the same calls, one row higher in 4). The write of a new file
    // is the 14th call: its diff shows `render.diff_lines_long` (8) lines.
    let (shown, pairs) = long_turn(Depth::TrueColor);
    for (number, shift) in [(3, 0), (4, 1)] {
        let frame = Frame::load("LongTurn", number).unwrap();
        for (real, board) in &pairs {
            let part = rows(&shown, real.clone());
            let board = board.start - shift..board.end - shift;
            check(&frame, board, &part, Depth::TrueColor, code_cells(&part, 4, Depth::TrueColor));
        }
    }
}

/// The calls of the mid-size turn of TM-BeforeAfter 4 and TM-Depths 1, 5, 9, without
/// its prose, its steer and the answer of its question.
fn mid_size(depth: Depth) -> String {
    let mut play = Play::new(depth, 100);
    reads(&mut play, 1, &["crates/efr-daemon/src/turn.rs"], 10);
    ran(&mut play, 2, "rg -n 'fn steer|fn finish' crates/efr-daemon", 100, 30);
    let first = "@@ -88,2 +88,5 @@ impl Turn {\n     pub fn finish(&mut self) -> usize {\n-        self.state = State::Done;\n+        while let Ok(msg) = self.rx.try_recv() {\n+            self.pending.push(msg);\n+        }\n+        self.state = State::Done;\n";
    edited(&mut play, 3, "crates/efr-daemon/src/turn.rs", 5, 1, first, 200);
    ran(&mut play, 4, "cargo test -p efr-daemon steer", 300, 38_000);
    play.event(38_400, shell(5, "cargo clippy -p efr-daemon -- -D warnings"));
    let tail = "error: this `if` has identical blocks\n   --> crates/efr-daemon/src/turn.rs:97:9\nerror: could not compile `efr-daemon` (lib) due to 1 previous error\n";
    play.event(47_400, ended(5, Some(101), tail));
    let second =
        "@@ -95,2 +95,0 @@ impl Turn {\n-        } else {\n-            self.pending.clear();\n";
    edited(&mut play, 6, "crates/efr-daemon/src/turn.rs", 0, 2, second, 47_500);
    ran(&mut play, 7, "cargo clippy -p efr-daemon -- -D warnings", 47_600, 7_800);
    ran(&mut play, 8, "git push -u origin fix/steer-race", 55_500, 3_100);
    play.committed()
}

#[test]
fn the_mid_size_turn_in_three_depths() {
    // TM-BeforeAfter 4, TM-Depths 1, 5, 9: rows 2-3, 8-25 and 30. The edit of six
    // calls is the 6th call: from it on, a diff shows 8 lines.
    for (depth, board, number) in [
        (Depth::TrueColor, "BeforeAfter", 4),
        (Depth::TrueColor, "Depths", 1),
        (Depth::Ansi16, "Depths", 5),
        (Depth::NoColor, "Depths", 9),
    ] {
        let shown = mid_size(depth);
        let frame = Frame::load(board, number).unwrap();
        for (real, board) in [(0..2, 2..4), (2..20, 8..26), (20..21, 30..31)] {
            let part = rows(&shown, real);
            check(&frame, board, &part, depth, code_cells(&part, 4, depth));
        }
    }
}

#[test]
fn the_calls_of_the_sequence() {
    // TM-Sequence 4 (rows 1-2), 6 (row 4), 7 (rows 3-7), 8 (rows 0-7), 10 (row 0).
    let depth = Depth::TrueColor;
    let mut play = Play::new(depth, 100);
    reads(&mut play, 1, &["crates/efr-daemon/src/turn.rs"], 10);
    ran(&mut play, 2, "rg -n 'fn steer|fn finish' crates/efr-daemon", 100, 30);
    ran(&mut play, 3, "cargo test -p efr-daemon steer", 200, 38_000);
    play.event(38_300, shell(4, "cargo clippy -p efr-daemon -- -D warnings"));
    let tail = "error: this `if` has identical blocks\n   --> crates/efr-daemon/src/turn.rs:97:9\nerror: could not compile `efr-daemon` (lib) due to 1 previous error\n";
    play.event(47_300, ended(4, Some(101), tail));
    let edit = "@@ -95,4 +95,2 @@ impl Turn {\n             self.state = State::Draining;\n-        } else {\n-            self.pending.clear();\n         }\n";
    edited(&mut play, 5, "crates/efr-daemon/src/turn.rs", 0, 2, edit, 47_400);
    ran(&mut play, 6, "cargo clippy -p efr-daemon -- -D warnings", 47_500, 7_800);
    let shown = play.committed();
    let at = |number: u32, board: Range<usize>, real: Range<usize>| {
        let part = rows(&shown, real);
        check(
            &Frame::load("Sequence", number).unwrap(),
            board,
            &part,
            depth,
            code_cells(&part, 4, depth),
        );
    };
    at(4, 1..3, 0..2);
    at(6, 4..5, 2..3);
    at(7, 3..8, 2..7);
    // NOTE: frame 8 shows the last line of the failed call's tail only.
    at(8, 0..1, 3..4);
    at(8, 1..2, 6..7);
    at(8, 2..8, 7..13);
    at(10, 0..1, 13..14);
}

#[test]
fn the_legend_of_the_rows_under_a_call() {
    let depth = Depth::TrueColor;
    let mut play = Play::new(depth, 44);
    play.event(10, shell(1, "cargo add x"));
    play.event(20, wrote(1, changes(vec![file("Cargo.toml", ChangeKind::Modified, 1, 0)]), None));
    play.event(30, shell(2, "cargo test"));
    play.view.moved(id(2), "j4".parse().unwrap());
    // NOTE: TM-Legend 12 says nothing of how long the call ran: a call moved within a
    // tenth of a second says none.
    play.event(40, ended(2, None, ""));
    let shown = play.committed();
    check(&Frame::load("Legend", 18).unwrap(), .., &rows(&shown, 1..2), depth, all);
    check(&Frame::load("Legend", 12).unwrap(), .., &rows(&shown, 2..3), depth, all);
    // A file edit whose diff is not shown yet (TM-Calls 9, rows 0 and 1).
    let mut play = Play::new(depth, 100);
    let judge = "@@ -88,1 +88,5 @@ impl Turn {\n-a\n+b\n";
    edited(&mut play, 1, "crates/efr-daemon/src/turn.rs", 5, 1, judge, 10);
    let shown = play.committed();
    check(&Frame::load("Calls", 9).unwrap(), 0..2, &rows(&shown, 0..2), depth, all);
}

// --- the lines of the end of a turn and efr diff --stat -----------------------------

/// `pieces` painted with `options`, as one row.
fn painted(
    pieces: &[crate::format::changes::Piece],
    options: &efr_render::RenderOptions,
) -> String {
    let mut out: String =
        pieces.iter().map(|(text, tone)| crate::format::paint(text, *tone, options)).collect();
    out.push('\n');
    out
}

#[test]
fn the_changes_line_of_a_turn_is_the_board() {
    use crate::format::changes::{turn_line, undo_line};
    let three = changes(vec![
        file("crates/efr-daemon/src/turn.rs", ChangeKind::Modified, 20, 6),
        file("crates/efr-daemon/tests/steer.rs", ChangeKind::Added, 3, 0),
        file("CHANGELOG.md", ChangeKind::Modified, 1, 1),
    ]);
    let one = changes(vec![file("crates/efr-daemon/src/turn.rs", ChangeKind::Modified, 5, 3)]);
    let four = changes(vec![
        file("crates/efr-store/src/sqlite.rs", ChangeKind::Added, 142, 0),
        file("crates/efr-store/src/lib.rs", ChangeKind::Modified, 3, 1),
        file("crates/efr-store/Cargo.toml", ChangeKind::Modified, 1, 0),
        file("Cargo.lock", ChangeKind::Modified, 1, 2),
    ]);
    for depth in Depth::ALL {
        let options = design_options(depth, 100);
        // TM-Endings 15 (row 3), TM-Depths 1, 5, 9 and TM-BeforeAfter 4 (row 34).
        check(
            &Frame::load("Endings", 15).unwrap(),
            3..4,
            &painted(&turn_line(&three, &options), &options),
            depth,
            all,
        );
        let depths = match depth {
            Depth::TrueColor => 1,
            Depth::Ansi16 => 5,
            Depth::NoColor => 9,
        };
        let line = painted(&turn_line(&one, &options), &options);
        check(&Frame::load("Depths", depths).unwrap(), 34..35, &line, depth, all);
        check(&Frame::load("BeforeAfter", 4).unwrap(), 34..35, &line, depth, all);
        // TM-LongTurn 3, row 75: more than three files. NOTE: the board counts 3 files
        // and names 3 and one more (spec 2.2 item 8: mock data). The line counts the
        // files that it names: the count is compared as the frame draws 4.
        let line = painted(&turn_line(&four, &options), &options).replacen('4', "3", 1);
        check(&Frame::load("LongTurn", 3).unwrap(), 75..76, &line, depth, all);
        // TM-Diffs 1, row 47: the board draws this row in truecolor only.
        let nine = changes(vec![
            file("crates/efr-daemon/src/turn.rs", ChangeKind::Modified, 5, 1),
            file("crates/efr-daemon/tests/steer.rs", ChangeKind::Added, 3, 0),
            file("CHANGELOG.md", ChangeKind::Modified, 1, 1),
        ]);
        if depth == Depth::TrueColor {
            let line = painted(&turn_line(&nine, &options), &options);
            check(&Frame::load("Diffs", 1).unwrap(), 47..48, &line, depth, all);
        }
        // TM-Endings 17, row 3: the line of a turn that `efr undo` can take back.
        let undo = efr_protocol::TurnUndoInfo {
            undoable: true,
            roots: vec![efr_protocol::RootFiles { shown: "app".to_owned(), files: 4, refs: 0 }],
            home_files: 2,
            not_covered: Vec::new(),
            not_covered_more: 0,
        };
        let twelve = changes(vec![file("a.rs", ChangeKind::Modified, 12, 3)]);
        let twelve = FileChanges { more: 3, ..twelve };
        let line = painted(&undo_line(Some(&twelve), &undo, &options), &options);
        check(&Frame::load("Endings", 17).unwrap(), 3..4, &line, depth, all);
    }
}

#[test]
fn the_stat_of_efr_diff_is_the_board() {
    // TM-Commands 5, rows 1 to 9 (rows 0 and 10 are the user's prompt).
    let three = changes(vec![
        file("crates/efr-daemon/src/turn.rs", ChangeKind::Modified, 12, 3),
        file("crates/efr-daemon/tests/steer.rs", ChangeKind::Added, 11, 0),
        file("CHANGELOG.md", ChangeKind::Modified, 1, 1),
    ]);
    for depth in Depth::ALL {
        let options = design_options(depth, 100);
        let shown =
            crate::format::changes::stat(&three, &efr_config::DiffGroups::default(), &options);
        check(&Frame::load("Commands", 5).unwrap(), 1..10, &shown, depth, all);
    }
}

// --- the cards of questions, from real events --------------------------------------

/// The events of an exit of call `n` that `info` describes, with `record`, and its
/// question with `diff`.
fn asked_exit(
    n: u8,
    info: efr_protocol::ExitInfo,
    record: efr_protocol::ExitRecord,
    diff: Option<&str>,
) -> [Event; 2] {
    [
        Event::ExitRequested {
            turn_id: turn(),
            call_id: id(n),
            kinds: info.kinds.clone(),
            grants: info.grants.clone(),
            source: efr_protocol::ExitSource::Predicted,
            record: Box::new(record),
        },
        Event::ApprovalRequested {
            turn_id: turn(),
            call_id: id(n),
            summary: "a summary that the exit's card replaces".to_owned(),
            diff_preview: diff.map(str::to_owned),
            interactive: false,
            exit: Some(info),
            expires_in_secs: None,
            local_only: false,
            reason: None,
        },
    ]
}

/// A play up to the question of TM-Questions 3: `cargo update -p rusqlite` in the
/// sandbox needs `index.crates.io`, which the allow list does not hold, and the daemon
/// offers a rule for it.
fn network_question(depth: Depth) -> Play {
    let grants = vec![efr_protocol::Grant::Host { host: "index.crates.io".to_owned(), port: 443 }];
    let launch = efr_protocol::Launch::Contained { grants };
    let info = efr_protocol::ExitInfo {
        always: Some("network index.crates.io for cargo".to_owned()),
        ..crate::testing::exit_info(&[efr_protocol::ExitKind::Host], launch.clone())
    };
    let facts = efr_protocol::ExitFacts {
        hosts: vec![efr_protocol::HostFact {
            host: "index.crates.io".to_owned(),
            on_allow_list: false,
            named_in_user_messages: false,
            refused_by_proxy: true,
            project_remote: false,
        }],
        ..efr_protocol::ExitFacts::default()
    };
    let record = crate::testing::exit_record("cargo update -p rusqlite", facts);
    let mut play = Play::new(depth, 100);
    play.event(5, message(0, "Pushing the branch so CI runs.\n"));
    play.event(
        10,
        started(1, "shell", json!({ "command": "cargo update -p rusqlite" }), Some(launch)),
    );
    for event in asked_exit(1, info, record, None) {
        play.event(20, event);
    }
    play
}

/// TM-Questions 3 and 4 from real events: a question about network alone is `allow
/// network` with one row for its host and whether the allow list has it; after `a` the
/// rule it saves and the keys of the confirm. TM-Questions 7: its answer line names the
/// host.
#[test]
fn a_question_about_a_host_is_the_network_card_questions_3_4() {
    for depth in Depth::ALL {
        let mut play = network_question(depth);
        let live = play.live(30);
        check(&Frame::load("Questions", 3).unwrap(), 2..6, &rows(&live, 1..5), depth, all);
        play.view.confirm_always(id(1), play.size);
        let live = play.live(40);
        check(&Frame::load("Questions", 4).unwrap(), 2..7, &rows(&live, 1..6), depth, all);
    }
    let mut play = network_question(Depth::NoColor);
    play.view.answered(id(1), efr_protocol::ApprovalDecision::Deny, play.size);
    let shown = cells(&play.committed())
        .iter()
        .map(efr_test_support::design::CellRow::text)
        .collect::<Vec<_>>();
    assert!(
        shown.iter().any(|row| row == "\u{2717} denied \u{b7} network (index.crates.io)"),
        "{shown:?}"
    );
}

/// The diff of the write of TM-Questions 2, as the file tool's preview writes it.
const HYPR_DIFF: &str = "\
--- /home/user/.config/hypr/hyprland.conf
+++ /home/user/.config/hypr/hyprland.conf
@@ -41,1 +41,1 @@ input
-kb_options = caps:escape
+kb_options = caps:escape,compose:ralt
";

/// TM-Questions 2 from real events: the write of a file tool outside the project is
/// `allow this write`, the file with its counts and its diff, then why it asks, then the
/// keys with `a always…`.
#[test]
fn a_write_outside_the_project_is_the_write_card_questions_2() {
    for depth in Depth::ALL {
        let mut record = crate::testing::exit_record("", efr_protocol::ExitFacts::default());
        record.action.tool = "write_file".to_owned();
        record.action.scope =
            efr_protocol::Scope::Project("019a9b1c-3d00-7a10-8b20-0000000000e1".parse().unwrap());
        let info = efr_protocol::ExitInfo {
            always: Some("write under ~/.config/hypr".to_owned()),
            ..crate::testing::exit_info(
                &[efr_protocol::ExitKind::Write],
                efr_protocol::Launch::Direct,
            )
        };
        let path = "/home/user/.config/hypr/hyprland.conf";
        let mut play = Play::new(depth, 100);
        play.event(5, message(0, "Pushing the branch so CI runs.\n"));
        play.event(10, tool(1, "write_file", json!({ "path": path, "content": "x" })));
        for event in asked_exit(1, info, record, Some(HYPR_DIFF)) {
            play.event(20, event);
        }
        let live = play.live(30);
        // NOTE: the code of the diff rows is the render slice's; the rows, the signs and
        // the fills compare here.
        let frame = Frame::load("Questions", 2).unwrap();
        let card = rows(&live, 1..8);
        check(&frame, 2..9, &card, depth, |row, col| (3..5).contains(&row) && col >= 6);
    }
}

/// TM-Questions 6 from real events: a question that another client answers. The card is
/// the last thing on the screen: no status row stands under it.
// NOTE: the board's last row says `· the phone has it too`; no event tells efr which
// clients hold a question, so the row ends after `answer` (open, see repaired.md).
#[test]
fn a_question_that_another_client_answers_ends_the_screen_questions_6() {
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        play.event(5, message(0, "Pushing the branch so CI runs.\n"));
        play.event(10, shell(1, "git push -u origin fix/steer-race"));
        let approval = Event::ApprovalRequested {
            turn_id: turn(),
            call_id: id(1),
            summary: "shell: network (github.com); run \"git push -u origin fix/steer-race\""
                .to_owned(),
            diff_preview: None,
            interactive: false,
            exit: None,
            expires_in_secs: None,
            local_only: false,
            reason: None,
        };
        let envelope =
            EventEnvelope { seq: Seq::new(40), conversation_id: None, at: at(20), event: approval };
        play.view.envelope(&envelope, play.size, false);
        play.frame();
        let screen = play.screen.raw();
        let shown = cells(&screen);
        let frame = Frame::load("Questions", 6).unwrap();
        check(&frame, 2..5, &rows(&screen, 2..5), depth, all);
        assert_eq!(
            shown.len(),
            6,
            "nothing after the card: {:?}",
            shown.iter().map(efr_test_support::design::CellRow::text).collect::<Vec<_>>()
        );
        assert_eq!(shown[5].text().trim_start(), "waiting for another client to answer");
    }
}

/// TM-Questions 6 from real events, for a question whose summary names only the
/// command: the `why` row says why the mode asks, in the daemon's words (`reason`).
// NOTE: the command and the words of `why` are this test's: the board draws a question
// about a host, whose summary names the host itself. The last row ends after `answer`
// as in the test above.
#[test]
fn a_question_about_a_command_says_why_the_mode_asks_questions_6() {
    let why = "cautious asks before a command that is not on its list of read-only commands";
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        play.event(5, message(0, "Pushing the branch so CI runs.\n"));
        play.event(10, shell(1, "sleep 3"));
        let approval = Event::ApprovalRequested {
            turn_id: turn(),
            call_id: id(1),
            summary: "shell: run \"sleep 3\"".to_owned(),
            diff_preview: None,
            interactive: false,
            exit: None,
            expires_in_secs: None,
            local_only: false,
            reason: Some(format!("{why}.")),
        };
        let envelope =
            EventEnvelope { seq: Seq::new(40), conversation_id: None, at: at(20), event: approval };
        play.view.envelope(&envelope, play.size, false);
        play.frame();
        let screen = play.screen.raw();
        let frame = Frame::load("Questions", 6).unwrap();
        let fixes =
            [(3, "git push -u origin fix/steer-race", "sleep 3"), (4, "network (github.com)", why)];
        let fixed = design_fixed(&frame, depth, &fixes);
        check(&fixed, 2..5, &rows(&screen, 2..5), depth, all);
    }
}

/// TM-Endings 7 in a view: the `↪` line that the prompt's result gives, then the `⇄`
/// line of the turn's switch of account, in that order, before the reply.
#[test]
fn the_conversation_line_comes_before_the_model_line_endings_7() {
    let side = |provider: &str| efr_protocol::SwitchedModel {
        provider: Some(provider.to_owned()),
        model: "claude-opus-5-5".to_owned(),
        effort: Some("high".to_owned()),
        default_effort: None,
    };
    let scope = efr_protocol::RouteLabel {
        scope: efr_protocol::RouteScope::Project(
            "0192f0c1-7a00-7000-8000-0000000a4401".parse().unwrap(),
        ),
        name: Some("app".to_owned()),
        dir: PathBuf::from("/home/user/p/app"),
    };
    let back = efr_protocol::Switch::Back {
        title: Some("fix steer race".to_owned()),
        updated_at: crate::testing::now() - jiff::SignedDuration::from_mins(125),
    };
    let frame = Frame::load("Endings", 7).unwrap();
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        let glyphs = play.view_glyphs();
        let line = crate::format::route::switch_line(
            &back,
            &scope,
            crate::testing::now(),
            Some(std::path::Path::new("/home/user")),
            &glyphs,
        );
        play.view.event_row(&line, play.size);
        play.event(
            10,
            Event::ModelSwitched {
                turn_id: turn(),
                from: side("anthropic-api.work"),
                to: side("anthropic-api.personal"),
            },
        );
        play.event(20, message(0, "Done.\n"));
        let shown = play.committed();
        check(&frame, 1..3, &rows(&shown, 0..2), depth, all);
    }
}

/// TM-Live 16 rows 1 and 2: a running call with its gauge and the newest line of its
/// output, 4 columns in, muted.
#[test]
fn a_running_call_with_its_gauge_and_its_tail_live_16() {
    for depth in Depth::ALL {
        let mut play = Play::new(depth, 100);
        play.context(38).event(10, shell(1, "cargo test --workspace"));
        play.event(500, output(1, "test store::tests::open_twice ... ok\n"));
        let live = spun(&play.live(14_010), "\u{283c}");
        check(&Frame::load("Live", 16).unwrap(), 0..2, &live, depth, all);
    }
}
