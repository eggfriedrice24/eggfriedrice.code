//! The layout of the interactive part of a turn, case by case: tool calls, their
//! results, refusals, questions and their answers, the spacing between blocks, the
//! sandbox note and the compactions of the model's context. Each case plays on a terminal 40 and 80 columns wide, with colour and
//! with `NO_COLOR`, and through a pipe. A snapshot shows the screen that a terminal
//! keeps after each step that the case names, and what the pipe got.

use std::path::PathBuf;

use efr_protocol::{
    ApprovalDecision, CallId, ChainStep, ChangeKind, CompactionTrigger, ContextUse, Draft,
    DraftPart, Event, EventEnvelope, ExitFacts, ExitInfo, ExitKind, ExitSource, FileChange,
    FileChanges, Grant, JudgeKind, Launch, Origin, PathClassName, PauseReason, Presence,
    PresenceSource, ProgramFact, Risk, RootFiles, Scope, Seq, TargetFact, TurnUndoInfo, Usage,
    Verdict,
};
use efr_render::{ColourMode, RenderOptions, WidthMethod, display_width};
use serde_json::{Value, json};

use super::super::{Look, TurnView};
use super::at;
use crate::terminal::Size;
use crate::testing::{FROM_SRC, compaction, exit_info, exit_record, program_fact, readable, turn};

/// What a case does, step by step.
#[expect(clippy::large_enum_variant, reason = "a short list of steps in a test")]
#[derive(Debug, Clone)]
enum Act {
    /// The daemon sends this event, `millis` after the prompt.
    Event(i64, Event),
    /// The user answers the question about call `n` with a key.
    Key(u8, ApprovalDecision),
    /// The daemon sends this draft of the turn, after the last event.
    Drafted(DraftPart),
    /// The snapshot shows the screen now, under this name.
    Look(&'static str),
}

use Act::{Drafted, Event as Sent, Key, Look as Shot};

fn id(n: u8) -> CallId {
    format!("019a9b1c-3d00-7a10-8b20-0000000000{n:02x}").parse().unwrap()
}

fn started(n: u8, tool: &str, input: Value, launch: Option<Launch>) -> Event {
    Event::ToolCallStarted {
        turn_id: turn(),
        call_id: id(n),
        tool: tool.to_owned(),
        input,
        manual_input: false,
        launch,
        freeform: false,
    }
}

fn shell(n: u8, command: &str) -> Event {
    started(n, "shell", json!({ "command": command }), None)
}

fn output(n: u8, tail: &str) -> Event {
    Event::ToolCallOutputUpdated {
        turn_id: turn(),
        call_id: id(n),
        tail: tail.to_owned(),
        bytes: tail.len() as u64,
    }
}

fn ended(n: u8, exit_code: Option<i32>, refusal: Option<&str>) -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: id(n),
        output: String::new(),
        truncated: false,
        is_error: refusal.is_some() || exit_code.is_some_and(|code| code != 0),
        exit_code,
        sandbox: None,
        refusal: refusal.map(str::to_owned),
        changes: None,
        diff: None,
    }
}

fn asked(n: u8, summary: &str, exit: Option<ExitInfo>) -> Event {
    Event::ApprovalRequested {
        turn_id: turn(),
        call_id: id(n),
        summary: summary.to_owned(),
        diff_preview: None,
        interactive: false,
        exit,
    }
}

fn exit_requested(n: u8, line: &str, facts: ExitFacts, info: &ExitInfo) -> Event {
    Event::ExitRequested {
        turn_id: turn(),
        call_id: id(n),
        kinds: info.kinds.clone(),
        grants: info.grants.clone(),
        source: ExitSource::Predicted,
        record: Box::new(exit_record(line, facts)),
    }
}

fn message(index: u32, text: &str) -> Event {
    Event::AssistantMessageCompleted { turn_id: turn(), index, text: text.to_owned() }
}

fn turn_started() -> Event {
    Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/home/user/project"),
        scope: Scope::Project("019a9b1c-3d00-7a10-8b20-0000000000e1".parse().unwrap()),
        settings: None,
    }
}

fn turn_completed() -> Event {
    let usage = Usage::new(18_200, 1_100);
    Event::TurnCompleted {
        turn_id: turn(),
        usage: Some(usage),
        changes: None,
        context: None,
        stop: None,
        undo: None,
    }
}

/// A terminal's screen: the rows that the writes of a view leave on it. The escape
/// sequences that move the cursor and erase rows act; the ones that paint stay in the
/// text.
#[derive(Debug, Default)]
struct Screen {
    rows: Vec<String>,
    current: String,
    cols: usize,
}

impl Screen {
    fn new(cols: u16) -> Screen {
        Screen { cols: usize::from(cols), ..Screen::default() }
    }

    fn write(&mut self, text: &str) {
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => match chars.next() {
                    Some('[') => {
                        let mut sequence = String::from("\x1b[");
                        for c in chars.by_ref() {
                            sequence.push(c);
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                        self.sequence(&sequence);
                    }
                    Some(']') => {
                        // An OSC, such as the progress bar: it draws nothing.
                        while let Some(c) = chars.next() {
                            if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                '\r' => {}
                '\n' => self.rows.push(std::mem::take(&mut self.current)),
                c => self.current.push(c),
            }
        }
    }

    fn sequence(&mut self, sequence: &str) {
        let Some(last) = sequence.chars().last() else { return };
        match last {
            'm' => self.current.push_str(sequence),
            'A' => {
                let count: usize = sequence[2..sequence.len() - 1].parse().unwrap_or(1);
                let mut gone = 0;
                while gone < count {
                    let Some(row) = self.rows.pop() else { break };
                    gone += display_width(&bare(&row), WidthMethod::CodePoint)
                        .div_ceil(self.cols)
                        .max(1);
                }
            }
            _ => {}
        }
    }

    /// The lines, with their paint as `\e` sequences. A line of efr's own never takes
    /// more than the width; prose may, and the terminal wraps it.
    fn shown(&self) -> String {
        let mut out = String::new();
        for row in &self.rows {
            let width = display_width(&bare(row), WidthMethod::CodePoint);
            let own = ["\u{b7} ", "  ", "\u{2502} ", "? ", "\u{2713} ", "\u{2717} "]
                .iter()
                .any(|start| bare(row).starts_with(start));
            assert!(!own || width <= self.cols, "wider than {} columns: {row:?}", self.cols);
            out.push_str(&readable(row));
            out.push('\n');
        }
        out
    }
}

/// `text` without its escape sequences.
fn bare(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Plays `acts` on a view with `options` and a screen of `size`: the screen at each
/// look and at the end on a terminal, or what stdout and stderr got through a pipe.
fn play(options: RenderOptions, size: Size, acts: &[Act]) -> String {
    play_with(LOOK, options, size, acts)
}

/// The look of the cases: no motion, the end-of-turn line, and 20 lines of a diff.
const LOOK: Look = Look { motion: false, summary: true, progress: false, diff_lines: 20 };

/// [`play`] with `look`.
fn play_with(look: Look, options: RenderOptions, size: Size, acts: &[Act]) -> String {
    let terminal = options.is_terminal();
    let mut view =
        TurnView::new(turn(), options).with_look(look).with_home(Some(PathBuf::from("/home/user")));
    view.start();
    let mut screen = Screen::new(size.cols);
    let (mut out, mut err) = (String::new(), String::new());
    let mut shots = Vec::new();
    let mut now = 0;
    let mut seq = 10;
    for act in acts {
        let step = match act {
            Sent(millis, event) => {
                now = *millis;
                seq += 1;
                let envelope = EventEnvelope {
                    seq: Seq::new(seq),
                    conversation_id: None,
                    at: at(now),
                    event: event.clone(),
                };
                view.envelope(&envelope, size, true)
            }
            Key(n, decision) => view.answered(id(*n), *decision, size),
            Drafted(part) => {
                let draft =
                    Draft { turn_id: turn(), after_seq: Seq::new(seq), draft: part.clone() };
                view.draft(&draft, size)
            }
            Shot(name) => {
                if terminal {
                    shots.push(format!("[{name}]\n{}", screen.shown()));
                } else {
                    shots.push(format!("[{name}]\nstdout:\n{out}stderr:\n{err}"));
                }
                continue;
            }
        };
        out.push_str(&step.out);
        err.push_str(&step.err);
        screen.write(&view.frame(size, at(now)));
    }
    if terminal {
        shots.push(format!("[end]\n{}", screen.shown()));
    } else {
        shots.push(format!("[end]\nstdout:\n{out}stderr:\n{err}"));
    }
    shots.join("\n")
}

/// `acts` played every way: 40 and 80 columns, with colour and with `NO_COLOR`, and
/// through a pipe.
fn every_way(acts: &[Act]) -> String {
    every_way_with(LOOK, acts)
}

/// [`every_way`] with `look`.
fn every_way_with(look: Look, acts: &[Act]) -> String {
    let mut shown = Vec::new();
    for cols in [40_u16, 80] {
        let size = Size { cols, rows: 40 };
        for (name, colour) in [("colour", ColourMode::Ansi16), ("NO_COLOR", ColourMode::None)] {
            let options = RenderOptions::new(cols).with_colour(colour);
            shown.push(format!(
                "=== {cols} columns, {name}\n{}",
                play_with(look, options, size, acts)
            ));
        }
    }
    let piped = RenderOptions::new(80).with_terminal(false);
    shown.push(format!("=== not a terminal\n{}", play_with(look, piped, Size::default(), acts)));
    shown.join("\n")
}

#[test]
fn a_call_that_went_well() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, "cargo metadata --no-deps | jq -r '.packages[].name'")),
        Sent(300, output(1, "efr-cli\nefr-render\n")),
        Shot("running"),
        Sent(1_400, ended(1, Some(0), None)),
    ]));
}

#[test]
fn a_failed_call_keeps_the_tail_of_its_output() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, "make")),
        Sent(100, output(1, "cc -c a.c\nmake: *** No rule to make target 'all'.  Stop.\n")),
        Sent(210, ended(1, Some(2), None)),
        Sent(220, shell(2, "find / -name libghostty.so 2>/dev/null")),
        Sent(4_000, ended(2, Some(1), None)),
    ]));
}

#[test]
fn a_refused_call_says_why_on_a_row_of_its_own() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(
            10,
            shell(1, "printf 'theme = \"x\"' >> ~/.config/efr/config.toml && efr config reload")
        ),
        Sent(20, ended(1, None, Some("efr's config (floor)"))),
    ]));
}

#[test]
fn a_long_reason_of_a_refusal_goes_on_in_the_next_row() {
    // The screen of the test fails on a row of efr's own that is wider than the screen:
    // the terminal would wrap it to the first column.
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, "cd ~/proj && find . -type f | sort")),
        Sent(20, ended(1, None, Some("an approved command outside the sandbox must run alone"))),
    ]));
}

#[test]
fn a_command_of_several_lines_shows_each_line() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, FROM_SRC)),
        Shot("running"),
        Sent(2_510, ended(1, Some(0), None)),
    ]));
}

#[test]
fn a_long_command_goes_on_in_the_next_row() {
    let command = "cd ~/p/eggfriedrice.code && find target/debug/build -path '*libghostty*' -type f | awk -F/ '{print $4}' | sort -u";
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, command)),
        Shot("running"),
        Sent(640, ended(1, Some(0), None)),
    ]));
}

#[test]
fn a_long_word_goes_on_in_the_next_row_with_no_space() {
    // The path is wider than a row at 40 columns: it is cut inside it, and its rows
    // show no space that the command does not have.
    let command = "echo \"built $HOME/p/eggfriedrice.code/target/debug/build/libghostty-vt-1a2b3c4d5e6f/out/lib/libghostty-vt.so\"";
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, command)),
        Sent(20, asked(1, &format!("shell: run {command:?}"), None)),
        Shot("asked"),
        Key(1, ApprovalDecision::Allow),
        Sent(900, ended(1, Some(0), None)),
    ]));
}

#[test]
fn a_file_tool_names_what_it_reads_and_writes() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, started(1, "read_file", json!({ "path": "crates/efr-cli/README.md" }), None)),
        Sent(40, ended(1, None, None)),
        Sent(
            50,
            started(2, "write_file", json!({ "path": "notes/plan.md", "content": "x" }), None)
        ),
        Sent(70, ended(2, None, None)),
    ]));
}

/// The facts of a line whose second program the sandbox wrote this turn.
fn untrusted_facts() -> ExitFacts {
    let written = ProgramFact {
        in_write_root: true,
        changed_this_turn: true,
        ..program_fact("./scripts/setup.sh", "/home/user/project/scripts/setup.sh")
    };
    ExitFacts {
        programs: vec![program_fact("sudo", "/usr/bin/sudo"), written],
        ..ExitFacts::default()
    }
}

#[test]
fn a_question_for_a_grant_in_the_sandbox() {
    let line = "cp report.pdf ~/Documents/";
    let mut info = exit_info(
        &[ExitKind::Write],
        Launch::Contained { grants: vec![Grant::Write { path: "/home/user/Documents".into() }] },
    );
    info.model_reason = Some("you asked for the report in Documents".to_owned());
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, started(1, "shell", json!({ "command": line }), Some(Launch::contained()))),
        Sent(20, exit_requested(1, line, ExitFacts::default(), &info)),
        Sent(20, asked(1, "shell: run \"cp report.pdf ~/Documents/\"", Some(info.clone()))),
        Shot("asked"),
        Key(1, ApprovalDecision::Allow),
        Sent(
            3_000,
            Event::ApprovalResolved {
                turn_id: turn(),
                call_id: id(1),
                decision: ApprovalDecision::Allow,
                origin: Origin::Shell,
            }
        ),
        Shot("allowed"),
        Sent(3_200, ended(1, Some(0), None)),
    ]));
}

#[test]
fn a_question_for_a_run_with_full_rights_and_an_untrusted_program() {
    let line = "sudo ./scripts/setup.sh";
    let mut info = exit_info(&[ExitKind::Privilege], Launch::Unsandboxed);
    info.user_only = true;
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, line)),
        Sent(20, exit_requested(1, line, untrusted_facts(), &info)),
        Sent(20, asked(1, "shell: run \"sudo ./scripts/setup.sh\"", Some(info.clone()))),
        Shot("asked"),
        Key(1, ApprovalDecision::Deny),
        Shot("denied"),
        Sent(
            5_000,
            Event::ApprovalResolved {
                turn_id: turn(),
                call_id: id(1),
                decision: ApprovalDecision::Deny,
                origin: Origin::Shell,
            }
        ),
        Sent(5_010, ended(1, None, None)),
    ]));
}

#[test]
fn a_question_gives_its_place_to_one_line_when_it_is_answered() {
    let resolved = |n: u8, decision, origin| Event::ApprovalResolved {
        turn_id: turn(),
        call_id: id(n),
        decision,
        origin,
    };
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        // Allowed here: the call follows at once.
        Sent(10, shell(1, "make install")),
        Sent(20, asked(1, "shell: run \"make install\"", None)),
        Key(1, ApprovalDecision::Allow),
        Sent(1_000, resolved(1, ApprovalDecision::Allow, Origin::Shell)),
        Sent(2_500, ended(1, Some(0), None)),
        // Denied here.
        Sent(2_600, shell(2, "rm -rf build")),
        Sent(2_610, asked(2, "shell: run \"rm -rf build\"", None)),
        Key(2, ApprovalDecision::Deny),
        Sent(3_000, resolved(2, ApprovalDecision::Deny, Origin::Shell)),
        Sent(3_010, ended(2, None, None)),
        // Allowed on the phone.
        Sent(3_100, shell(3, "git push")),
        Sent(3_110, asked(3, "shell: run \"git push\"", None)),
        Sent(4_000, resolved(3, ApprovalDecision::Allow, Origin::Phone)),
        Sent(4_400, ended(3, Some(0), None)),
        // Nobody answered.
        Sent(4_500, shell(4, "systemctl --user restart app")),
        Sent(4_510, asked(4, "shell: run \"systemctl --user restart app\"", None)),
        Sent(9_000, Event::ApprovalExpired { turn_id: turn(), call_id: id(4), away: false }),
        Sent(9_010, ended(4, None, None)),
    ]));
}

#[test]
fn a_whole_turn_with_prose_two_calls_and_a_question() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(900, message(0, "I will look for the **libghostty** build first.")),
        Sent(
            1_000,
            started(
                1,
                "shell",
                json!({ "command": "ls target/debug/build | grep ghostty" }),
                Some(Launch::contained())
            )
        ),
        Sent(1_200, output(1, "libghostty-vt-1a2b3c\n")),
        Sent(1_300, ended(1, Some(0), None)),
        Sent(2_000, message(1, "It is there. Now I copy it out of the sandbox.")),
        Sent(
            2_100,
            shell(2, "cp target/debug/build/libghostty-vt-1a2b3c/out/libghostty.so ~/lib/")
        ),
        Sent(
            2_110,
            asked(
                2,
                "shell: run \"cp target/debug/build/libghostty-vt-1a2b3c/out/libghostty.so ~/lib/\"; write ~/lib (user data)",
                None
            )
        ),
        Shot("asked"),
        Key(2, ApprovalDecision::Allow),
        Sent(
            4_000,
            Event::ApprovalResolved {
                turn_id: turn(),
                call_id: id(2),
                decision: ApprovalDecision::Allow,
                origin: Origin::Shell,
            }
        ),
        Sent(4_300, ended(2, Some(0), None)),
        Sent(5_000, message(2, "Done: `~/lib/libghostty.so` is in place.")),
        Sent(5_100, turn_completed()),
    ]));
}

#[test]
fn a_card_taller_than_the_screen_goes_to_the_scrollback_and_its_keys_stay() {
    let command = (1..=12).map(|n| format!("echo step {n}")).collect::<Vec<_>>().join("\n");
    let size = Size { cols: 40, rows: 10 };
    let shown = play(
        RenderOptions::new(40).with_colour(ColourMode::None),
        size,
        &[
            Sent(0, turn_started()),
            Sent(10, shell(1, &command)),
            Sent(20, asked(1, &format!("shell: run {command:?}"), None)),
            Shot("asked"),
            Key(1, ApprovalDecision::Allow),
        ],
    );
    // Every line of the command stays on the screen, and the answer follows the card.
    for n in 1..=12 {
        assert!(shown.contains(&format!("echo step {n}\n")), "{shown}");
    }
    insta::assert_snapshot!(shown);
}

fn file(path: &str, kind: ChangeKind, added: u32, removed: u32) -> FileChange {
    FileChange { path: path.to_owned(), kind, from: None, added, removed, binary: false }
}

fn file_changes(files: Vec<FileChange>, more: u32) -> FileChanges {
    let added = files.iter().map(|file| file.added).sum();
    let removed = files.iter().map(|file| file.removed).sum();
    FileChanges { files, more, added, removed }
}

/// The end of call `n` that went well and changed `changes`, with the diff of a write.
fn changed(n: u8, changes: FileChanges, diff: Option<&str>) -> Event {
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
    }
}

fn write_file(n: u8, path: &str) -> Event {
    started(n, "write_file", json!({ "path": path, "content": "..." }), None)
}

/// A diff of `src/main.rs` with its headers, 26 lines from its first hunk on and one
/// line wider than 40 columns.
fn main_rs_diff() -> String {
    let mut diff = "diff --git a/src/main.rs b/src/main.rs\nindex 1111111..2222222 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,4 +1,27 @@\n fn main() {\n-    println!(\"old\");\n+    println!(\"a new greeting that is much wider than forty columns\");\n".to_owned();
    for n in 1..=22 {
        diff.push_str(&format!("+    step({n});\n"));
    }
    diff.push_str(" }\n");
    diff
}

fn turn_changed(changes: FileChanges) -> Event {
    let usage = Usage::new(18_200, 1_100);
    Event::TurnCompleted {
        turn_id: turn(),
        usage: Some(usage),
        changes: Some(changes),
        context: None,
        stop: None,
        undo: None,
    }
}

#[test]
fn a_file_write_shows_the_first_lines_of_its_diff() {
    let changes = file_changes(vec![file("src/main.rs", ChangeKind::Modified, 23, 1)], 0);
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, write_file(1, "src/main.rs")),
        Sent(40, changed(1, changes, Some(&main_rs_diff()))),
    ]));
}

#[test]
fn a_diff_that_the_daemon_cut_counts_the_lines_it_left_out() {
    // A new file without headers: the call's path gives the syntax colours, and the
    // lines that the daemon cut join the lines that the view leaves out.
    let diff = "@@ -0,0 +1,2000 @@\n+# Notes\n+\n+- one\n... 1997 more lines\n";
    let changes = file_changes(vec![file("notes.md", ChangeKind::Added, 2000, 0)], 0);
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, write_file(1, "notes.md")),
        Sent(40, changed(1, changes, Some(diff))),
    ]));
}

#[test]
fn without_diff_lines_a_file_write_shows_the_row_of_its_files() {
    let changes = file_changes(vec![file("src/main.rs", ChangeKind::Modified, 23, 1)], 0);
    let look = Look { diff_lines: 0, ..LOOK };
    insta::assert_snapshot!(every_way_with(
        look,
        &[
            Sent(0, turn_started()),
            Sent(10, write_file(1, "src/main.rs")),
            Sent(40, changed(1, changes, Some(&main_rs_diff()))),
        ]
    ));
}

#[test]
fn a_shell_call_that_changed_files_gets_one_row_under_its_result() {
    let mut renamed = file("src/parse/expression.rs", ChangeKind::Renamed, 2, 2);
    renamed.from = Some("src/parse/expr.rs".to_owned());
    let mut logo = file("assets/logo.png", ChangeKind::Modified, 0, 0);
    logo.binary = true;
    let many = file_changes(
        vec![
            file("src/a.rs", ChangeKind::Modified, 3, 1),
            file("old.rs", ChangeKind::Deleted, 0, 40),
            file("notes.md", ChangeKind::Added, 12, 0),
            file("src/b.rs", ChangeKind::Modified, 1, 0),
        ],
        1,
    );
    let moved = file_changes(vec![renamed, logo], 0);
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, "cargo fmt && rm old.rs && touch notes.md")),
        Sent(1_500, changed(1, many, None)),
        Sent(1_600, shell(2, "git mv src/parse/expr.rs src/parse/expression.rs")),
        Sent(1_700, changed(2, moved, None)),
        Sent(1_800, shell(3, "cargo test")),
        Sent(1_900, changed(3, FileChanges::default(), None)),
    ]));
}

#[test]
fn a_turn_that_changed_files_says_so_before_its_end() {
    let changes = file_changes(
        vec![
            file("src/a.rs", ChangeKind::Modified, 20, 7),
            file("notes.md", ChangeKind::Added, 4, 0),
            file("old.rs", ChangeKind::Deleted, 0, 0),
        ],
        0,
    );
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(900, message(0, "Done.")),
        Sent(1_000, turn_changed(changes)),
    ]));
}

#[test]
fn a_whole_turn_that_writes_a_file_and_runs_a_command() {
    let written = file_changes(vec![file("src/main.rs", ChangeKind::Modified, 23, 1)], 0);
    let formatted = file_changes(vec![file("src/lib.rs", ChangeKind::Modified, 2, 2)], 0);
    let all = file_changes(
        vec![
            file("src/main.rs", ChangeKind::Modified, 23, 1),
            file("src/lib.rs", ChangeKind::Modified, 2, 2),
        ],
        0,
    );
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(900, message(0, "I will greet the user and run the formatter.")),
        Sent(1_000, write_file(1, "src/main.rs")),
        Sent(1_100, changed(1, written, Some(&main_rs_diff()))),
        Sent(1_200, shell(2, "cargo fmt")),
        Shot("running"),
        Sent(2_500, changed(2, formatted, None)),
        Sent(3_000, message(1, "The greeting is in `src/main.rs`.")),
        Sent(3_100, turn_changed(all)),
    ]));
}

/// A patch that updates two files, adds one, deletes one and moves one, as the model
/// writes it for `apply_patch`.
const PATCH: &str = "*** Begin Patch
*** Update File: src/main.rs
@@ fn main() {
-    println!(\"old\");
+    println!(\"a new greeting that is much wider than forty columns\");
*** Update File: src/lib.rs
@@
+pub mod greet;
*** Add File: notes.md
+# Notes
+
*** Delete File: old.rs
*** Update File: src/expr.rs
*** Move to: src/expression.rs
@@ pub fn parse
-    let a = 1;
+    let a = 2;
*** End Patch
";

/// The start of call `n` of `apply_patch` with `patch`, in the freeform form: the
/// input is a JSON string with the text.
fn apply_patch(n: u8, patch: &str) -> Event {
    let mut event = started(n, "apply_patch", Value::String(patch.to_owned()), None);
    if let Event::ToolCallStarted { freeform, .. } = &mut event {
        *freeform = true;
    }
    event
}

/// The diff of `src/main.rs` that [`PATCH`] makes, 25 lines from its hunk on, as the
/// daemon sends it for a file of a patch.
fn patched_main_rs() -> String {
    let mut diff = "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,24 @@\n fn main() {\n-    println!(\"old\");\n+    println!(\"a new greeting that is much wider than forty columns\");\n".to_owned();
    for n in 1..=21 {
        diff.push_str(&format!("+    step({n});\n"));
    }
    diff.push_str(" }\n");
    diff
}

/// The diff of every file of [`PATCH`], in its order, with the line of the preview
/// that marks a delete and a move when `preview`.
fn patch_diff(preview: bool) -> String {
    let mut diff = patched_main_rs();
    diff.push_str(
        "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,2 @@\n pub mod parse;\n+pub mod greet;\n",
    );
    diff.push_str("--- /dev/null\n+++ b/notes.md\n@@ -0,0 +1,2 @@\n+# Notes\n+\n");
    if preview {
        diff.push_str("delete old.rs\n");
    }
    diff.push_str("--- a/old.rs\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-fn old() {}\n");
    if preview {
        diff.push_str("move src/expr.rs -> src/expression.rs\n");
    }
    diff.push_str("--- a/src/expr.rs\n+++ b/src/expression.rs\n@@ -1,3 +1,3 @@\n pub fn parse() {\n-    let a = 1;\n+    let a = 2;\n }\n");
    diff
}

fn patch_changes() -> FileChanges {
    let mut moved = file("src/expression.rs", ChangeKind::Renamed, 1, 1);
    moved.from = Some("src/expr.rs".to_owned());
    file_changes(
        vec![
            file("notes.md", ChangeKind::Added, 2, 0),
            file("old.rs", ChangeKind::Deleted, 0, 1),
            moved,
            file("src/lib.rs", ChangeKind::Modified, 1, 0),
            file("src/main.rs", ChangeKind::Modified, 22, 1),
        ],
        0,
    )
}

fn patch_asked(n: u8) -> Event {
    Event::ApprovalRequested {
        turn_id: turn(),
        call_id: id(n),
        summary: "apply_patch: edit src/main.rs, src/lib.rs, notes.md; delete old.rs; move \
                  src/expr.rs"
            .to_owned(),
        diff_preview: Some(patch_diff(true)),
        interactive: false,
        exit: None,
    }
}

#[test]
fn a_patch_names_its_files_and_shows_one_diff_per_file() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, apply_patch(1, PATCH)),
        Shot("running"),
        Sent(40, changed(1, patch_changes(), Some(&patch_diff(false)))),
    ]));
}

#[test]
fn a_question_about_a_patch_shows_every_file_and_marks_a_delete_and_a_move() {
    let resolved = Event::ApprovalResolved {
        turn_id: turn(),
        call_id: id(1),
        decision: ApprovalDecision::Allow,
        origin: Origin::Shell,
    };
    let shown = every_way(&[
        Sent(0, turn_started()),
        Sent(10, apply_patch(1, PATCH)),
        Sent(20, patch_asked(1)),
        Shot("asked"),
        Key(1, ApprovalDecision::Allow),
        Sent(1_000, resolved),
        Sent(1_100, changed(1, patch_changes(), Some(&patch_diff(false)))),
    ]);
    // Every line of every file shows in the question, the last step of `src/main.rs`
    // too, and a delete and a move are in the `warning` role.
    assert!(shown.contains("step(21);"), "{shown}");
    assert!(shown.contains("\\e[1;33mdelete old.rs\\e[0m"), "{shown}");
    assert!(
        shown.contains("\\e[1;33mmove src/expr.rs \u{2192} src/expression.rs\\e[0m"),
        "{shown}"
    );
    insta::assert_snapshot!(shown);
}

#[test]
fn a_patch_of_one_file_shows_its_diff_as_a_write_does() {
    let patch =
        "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n+pub mod greet;\n*** End Patch\n";
    let diff =
        "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,2 @@\n pub mod parse;\n+pub mod greet;\n";
    let changes = file_changes(vec![file("src/lib.rs", ChangeKind::Modified, 1, 0)], 0);
    let shown = play(
        RenderOptions::new(80).with_colour(ColourMode::None),
        Size { cols: 80, rows: 40 },
        &[
            Sent(0, turn_started()),
            Sent(10, apply_patch(1, patch)),
            Sent(40, changed(1, changes, Some(diff))),
        ],
    );
    let block: Vec<&str> = shown.lines().skip(1).take(5).collect();
    assert_eq!(
        block,
        [
            "\\e[1m·\\e[0m apply_patch src/lib.rs +1",
            "\\e[2m  │ \\e[0m@@ -1,1 +1,2 @@",
            "\\e[2m  │ \\e[0m pub mod parse;",
            "\\e[2m  │ \\e[0m\\e[1m+\\e[0mpub mod greet;",
            "  ✓",
        ],
        "{shown}"
    );
}

/// The diff of an `edit` call that changes one line of `~/.zshrc`, with the shown
/// path, as the daemon sends it for the question and for the call that ran.
const ZSHRC_EDIT: &str = "--- a/~/.zshrc\n+++ b/~/.zshrc\n@@ -1,2 +1,2 @@\n export EDITOR=nvim\n-alias ll='ls -l'\n+alias ll='ls -la'\n";

/// The start of call `n` of `edit`, which a Claude model calls to change one line of
/// `~/.zshrc`.
fn edit(n: u8) -> Event {
    let input = json!({
        "path": "~/.zshrc",
        "old_string": "alias ll='ls -l'",
        "new_string": "alias ll='ls -la'",
    });
    started(n, "edit", input, None)
}

#[test]
fn an_edit_names_its_file_and_its_question_shows_the_diff() {
    let asked = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: id(1),
        summary: "edit: write ~/.zshrc (user config)".to_owned(),
        diff_preview: Some(ZSHRC_EDIT.to_owned()),
        interactive: false,
        exit: None,
    };
    let resolved = Event::ApprovalResolved {
        turn_id: turn(),
        call_id: id(1),
        decision: ApprovalDecision::Allow,
        origin: Origin::Shell,
    };
    let changes = file_changes(vec![file("~/.zshrc", ChangeKind::Modified, 1, 1)], 0);
    let shown = every_way(&[
        Sent(0, turn_started()),
        Sent(10, edit(1)),
        Sent(20, asked),
        Shot("asked"),
        Key(1, ApprovalDecision::Allow),
        Sent(1_000, resolved),
        Sent(1_100, changed(1, changes, Some(ZSHRC_EDIT))),
    ]);
    assert!(shown.contains("? allow this edit"), "{shown}");
    assert!(shown.contains("edit ~/.zshrc"), "{shown}");
    insta::assert_snapshot!(shown);
}

fn counted(tokens: u64) -> DraftPart {
    DraftPart::Context(ContextUse { tokens, limit: 206_720, window: 272_000 })
}

#[test]
fn a_turn_that_compacts_its_context() {
    let end = Event::TurnCompleted {
        turn_id: turn(),
        usage: Some(Usage::new(918_200, 1_100)),
        changes: None,
        context: Some(ContextUse { tokens: 196_000, limit: 206_720, window: 272_000 }),
        stop: None,
        undo: None,
    };
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Drafted(counted(98_400)),
        Shot("counted"),
        Sent(10, shell(1, "cargo test --workspace")),
        Shot("running"),
        Sent(1_400, ended(1, Some(0), None)),
        Drafted(counted(205_000)),
        Drafted(DraftPart::Compacting { trigger: CompactionTrigger::Auto }),
        Shot("compacting"),
        Sent(6_000, Event::ConversationCompacted(compaction(CompactionTrigger::Auto, 24_100))),
        Drafted(counted(24_100)),
        Sent(9_000, Event::ConversationCompacted(compaction(CompactionTrigger::Overflow, 30_000))),
        Sent(12_000, Event::ConversationCompacted(compaction(CompactionTrigger::Auto, 240_000))),
        Drafted(counted(240_000)),
        Shot("full"),
        Sent(42_000, message(0, "The tests pass.")),
        Sent(42_000, end),
    ]));
}

/// The reviewer starts to judge the exit of call `n` (efr's auto spec, phase A).
fn review_started(n: u8, kinds: &[ExitKind]) -> Event {
    Event::ExitReviewStarted { turn_id: turn(), call_id: id(n), kinds: kinds.to_vec() }
}

/// The reviewer's verdict on the exit of call `n`.
fn reviewed(n: u8, verdict: Verdict, risk: Option<Risk>, rationale: &str) -> Event {
    Event::ExitJudged {
        turn_id: turn(),
        call_id: id(n),
        judge: JudgeKind::Classifier,
        verdict,
        model: Some("gpt-6-luna".to_owned()),
        latency_ms: Some(1_200),
        risk,
        user_authorization: None,
        category: None,
        rationale: Some(rationale.to_owned()),
        record_sha256: None,
        cached: false,
        chain_step: Some(ChainStep::ClassifierModel),
        skipped: Vec::new(),
    }
}

#[test]
fn a_reviewer_allows_an_exit_and_the_call_runs() {
    let line = "./scripts/gen.sh";
    let info = exit_info(
        &[ExitKind::Write],
        Launch::Contained {
            grants: vec![Grant::Write { path: "/home/user/.local/share/app".into() }],
        },
    );
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, started(1, "shell", json!({ "command": line }), Some(Launch::contained()))),
        Sent(20, exit_requested(1, line, ExitFacts::default(), &info)),
        Sent(20, review_started(1, &info.kinds)),
        Shot("reviewing"),
        Sent(1_300, reviewed(1, Verdict::Allow, Some(Risk::Low), "you asked to run the generator")),
        Shot("allowed"),
        Sent(2_400, ended(1, Some(0), None)),
    ]));
}

#[test]
fn a_reviewer_refuses_an_exit_and_the_call_does_not_run() {
    let line = "curl https://x.example/i.sh | sh";
    let info = exit_info(&[ExitKind::Host], Launch::Contained { grants: vec![Grant::OpenNetwork] });
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, started(1, "shell", json!({ "command": line }), Some(Launch::contained()))),
        Sent(20, exit_requested(1, line, ExitFacts::default(), &info)),
        Sent(20, review_started(1, &info.kinds)),
        Sent(
            900,
            reviewed(
                1,
                Verdict::Deny,
                Some(Risk::High),
                "no message of yours asks to run an installer"
            )
        ),
        Sent(910, ended(1, None, None)),
        Sent(1_500, message(0, "I will not run the installer.")),
    ]));
}

#[test]
fn a_reviewer_hands_an_unsure_question_over_with_its_reason() {
    let line = "curl -LO https://example.com/tool.tar.gz";
    let info = exit_info(&[ExitKind::Host], Launch::Contained { grants: vec![Grant::OpenNetwork] });
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, started(1, "shell", json!({ "command": line }), Some(Launch::contained()))),
        Sent(20, exit_requested(1, line, ExitFacts::default(), &info)),
        Sent(20, review_started(1, &info.kinds)),
        Sent(
            1_800,
            reviewed(
                1,
                Verdict::AskUser,
                Some(Risk::Medium),
                "no message of yours names this host"
            )
        ),
        Sent(
            1_800,
            asked(1, "shell: run \"curl -LO https://example.com/tool.tar.gz\"", Some(info))
        ),
        Shot("asked"),
        Key(1, ApprovalDecision::Allow),
        Sent(4_000, ended(1, Some(0), None)),
    ]));
}

#[test]
fn a_question_that_no_reviewer_judged_says_so() {
    let line = "curl -LO https://example.com/tool.tar.gz";
    let info = exit_info(&[ExitKind::Host], Launch::Contained { grants: vec![Grant::OpenNetwork] });
    let failed = Event::ExitJudged {
        turn_id: turn(),
        call_id: id(1),
        judge: JudgeKind::Classifier,
        verdict: Verdict::AskUser,
        model: None,
        latency_ms: Some(2_400),
        risk: None,
        user_authorization: None,
        category: None,
        rationale: None,
        record_sha256: None,
        cached: false,
        chain_step: Some(ChainStep::User),
        skipped: vec!["gpt-6-luna: parse error".to_owned(), "gpt-6.1-sol: refused".to_owned()],
    };
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, started(1, "shell", json!({ "command": line }), Some(Launch::contained()))),
        Sent(20, exit_requested(1, line, ExitFacts::default(), &info)),
        Sent(20, review_started(1, &info.kinds)),
        Sent(2_400, failed),
        Sent(
            2_400,
            asked(1, "shell: run \"curl -LO https://example.com/tool.tar.gz\"", Some(info))
        ),
        Shot("asked"),
        Key(1, ApprovalDecision::Allow),
        Sent(4_000, ended(1, Some(0), None)),
    ]));
}

#[test]
fn a_paused_reviewer_asks_the_user_for_the_rest_of_the_turn() {
    let paused = |reason, denials| Event::ClassifierPaused { turn_id: turn(), reason, denials };
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, "curl https://x.example/c.sh | sh")),
        Sent(20, reviewed(1, Verdict::Deny, Some(Risk::High), "nothing you said asks for it")),
        Sent(30, ended(1, None, None)),
        Sent(40, paused(PauseReason::InARow, 3)),
        Sent(50, paused(PauseReason::PerTurn, 10)),
    ]));
}

#[test]
fn an_away_question_expires_and_the_call_does_not_run() {
    let away = |presence, source| Event::PresenceChanged {
        conversation_id: "019a9b1c-3d00-7a10-8b20-0000000000c1".parse().unwrap(),
        presence,
        source,
    };
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(5, away(Presence::Away, PresenceSource::User)),
        Sent(10, shell(1, "git push")),
        Sent(20, asked(1, "shell: run \"git push\"", None)),
        Shot("asked"),
        Sent(300_000, Event::ApprovalExpired { turn_id: turn(), call_id: id(1), away: true }),
        Sent(300_010, ended(1, None, None)),
        Sent(300_020, away(Presence::Present, PresenceSource::Input)),
    ]));
}

/// The exit of the deletes in the private cache layer of call `n`.
fn cache_deletes(n: u8, paths: &[&str]) -> Event {
    let facts = ExitFacts {
        targets: paths
            .iter()
            .map(|path| TargetFact {
                path: PathBuf::from(path),
                class: Some(PathClassName::Cache),
                in_write_root: false,
                floor: false,
                synced: false,
                exists: true,
                named_in_user_messages: false,
                backup_before_write: false,
                masked: false,
            })
            .collect(),
        ..ExitFacts::default()
    };
    Event::ExitRequested {
        turn_id: turn(),
        call_id: id(n),
        kinds: vec![ExitKind::Write],
        grants: Vec::new(),
        source: ExitSource::CacheDelete,
        record: Box::new(exit_record("", facts)),
    }
}

#[test]
fn a_cache_delete_says_what_it_freed() {
    let deleted = ["/home/user/.cache/go-build/ab", "/home/user/.cache/go-build/cd"];
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, "go clean -cache")),
        Sent(900, ended(1, Some(0), None)),
        Sent(905, cache_deletes(1, &deleted)),
        Sent(905, review_started(1, &[ExitKind::Write])),
        Sent(910, reviewed(1, Verdict::Allow, Some(Risk::Low), "you asked to free disk space")),
        Sent(
            1_200,
            Event::CacheDeleteApplied {
                turn_id: turn(),
                call_id: id(1),
                paths: deleted.iter().map(PathBuf::from).collect(),
                freed_bytes: 1_234_000_000,
            }
        ),
    ]));
}

/// Call `n` changed `path`, a file that runs code later, and efrd kept `backup`.
fn home_changed(n: u8, path: &str, backup: &str) -> Event {
    Event::HomeFileChanged {
        turn_id: turn(),
        call_id: id(n),
        path: PathBuf::from(path),
        floor: true,
        covered: false,
        backup: Some(PathBuf::from(backup)),
        real: None,
    }
}

/// An event from before the home store names the copy in the phase A backup store, and
/// keeps that line.
#[test]
fn old_backup_event_keeps_its_line() {
    let line = "echo 'alias k=kubectl' >> ~/.zshrc";
    let info = exit_info(&[ExitKind::Persistence], Launch::Unsandboxed);
    let backup = "/home/user/.local/share/efr/file-backups/c1/t1/1/.zshrc";
    let edit = [
        Sent(0, turn_started()),
        Sent(10, shell(1, line)),
        Sent(20, exit_requested(1, line, ExitFacts::default(), &info)),
        Sent(20, review_started(1, &info.kinds)),
        Sent(900, reviewed(1, Verdict::Allow, Some(Risk::Low), "you named your zshrc")),
        Sent(1_000, ended(1, Some(0), None)),
        Sent(1_010, home_changed(1, "/home/user/.zshrc", backup)),
        Sent(
            1_020,
            home_changed(
                1,
                "/home/user/.config/hypr/new.conf",
                "/home/user/.local/share/efr/file-backups/c1/t1/2/meta.json",
            ),
        ),
    ];
    let mut completed = edit.to_vec();
    completed.push(Sent(1_500, message(0, "I added the alias.")));
    completed.push(Sent(1_500, turn_completed()));
    let mut failed = edit.to_vec();
    failed.push(Sent(
        1_500,
        Event::TurnFailed {
            turn_id: turn(),
            error: efr_protocol::ErrorBody {
                code: efr_protocol::ErrorCode::Forbidden,
                message: "auto stopped this turn: the reviewer refused 3 actions in a row and nobody could decide. Read the answers, then send a new prompt."
                    .to_owned(),
                data: None,
            },
            usage: None,
            context: None, changes: None, undo: None,
        },
    ));
    insta::assert_snapshot!(format!(
        "### completed\n{}\n### failed\n{}",
        every_way(&completed),
        every_way(&failed)
    ));
}

/// Call `n` changed `path` outside the roots; from phase B efr's home store holds the
/// copy, so the event names no backup.
fn home_copied(n: u8, path: &str, floor: bool, covered: bool) -> Event {
    Event::HomeFileChanged {
        turn_id: turn(),
        call_id: id(n),
        path: PathBuf::from(path),
        floor,
        covered,
        backup: None,
        real: None,
    }
}

/// What `efr undo` can take back of the turn: `files` in `roots` and `home` files
/// outside them, and `not_covered`.
fn undo_info(roots: &[(&str, u32)], home: u32, not_covered: &[&str], more: u32) -> TurnUndoInfo {
    TurnUndoInfo {
        undoable: true,
        roots: roots
            .iter()
            .map(|(shown, files)| RootFiles { shown: (*shown).to_owned(), files: *files, refs: 0 })
            .collect(),
        home_files: home,
        not_covered: not_covered.iter().map(|name| (*name).to_owned()).collect(),
        not_covered_more: more,
    }
}

fn turn_undone(changes: Option<FileChanges>, undo: TurnUndoInfo) -> Event {
    Event::TurnCompleted {
        turn_id: turn(),
        usage: Some(Usage::new(18_200, 1_100)),
        changes,
        context: None,
        stop: None,
        undo: Some(undo),
    }
}

fn three_files() -> FileChanges {
    file_changes(
        vec![
            file("src/a.rs", ChangeKind::Modified, 20, 7),
            file("notes.md", ChangeKind::Added, 4, 0),
            file("old.rs", ChangeKind::Deleted, 0, 0),
        ],
        0,
    )
}

/// What a pipe's stderr got of `acts`: the muted lines of the turn.
fn piped(acts: &[Act]) -> String {
    let shown = play(RenderOptions::new(80).with_terminal(false), Size::default(), acts);
    shown.split_once("stderr:\n").map(|(_, err)| err.to_owned()).unwrap_or_default()
}

#[test]
fn floor_edit_turn_end_line_names_path() {
    let line = "echo 'alias k=kubectl' >> ~/.zshrc";
    let info = exit_info(&[ExitKind::Persistence], Launch::Unsandboxed);
    let edit = [
        Sent(0, turn_started()),
        Sent(10, shell(1, line)),
        Sent(20, exit_requested(1, line, ExitFacts::default(), &info)),
        Sent(20, review_started(1, &info.kinds)),
        Sent(900, reviewed(1, Verdict::Allow, Some(Risk::Low), "you named your zshrc")),
        Sent(1_000, ended(1, Some(0), None)),
        Sent(1_010, home_copied(1, "/home/user/.zshrc", true, true)),
        Sent(1_020, home_copied(1, "/home/user/.aws/config", false, false)),
        Sent(1_030, home_copied(1, "/home/user/Documents/notes.md", false, true)),
    ];
    let undo = undo_info(&[], 2, &["~/.aws/config (masked)"], 0);
    let mut completed = edit.to_vec();
    completed.push(Sent(1_500, message(0, "I added the alias.")));
    completed.push(Sent(1_500, turn_undone(None, undo.clone())));
    let mut failed = edit.to_vec();
    failed.push(Sent(
        1_500,
        Event::TurnFailed {
            turn_id: turn(),
            error: efr_protocol::ErrorBody {
                code: efr_protocol::ErrorCode::Internal,
                message: "the provider is down".to_owned(),
                data: None,
            },
            usage: None,
            context: None,
            changes: None,
            undo: Some(undo),
        },
    ));
    insta::assert_snapshot!(format!(
        "### completed\n{}\n### failed\n{}",
        every_way(&completed),
        every_way(&failed)
    ));
}

#[test]
fn turn_line_counts_home_files_and_names_undo() {
    let undo = undo_info(&[("eggfriedrice.code", 3)], 2, &[], 0);
    let err = piped(&[
        Sent(0, turn_started()),
        Sent(900, message(0, "Done.")),
        Sent(1_000, turn_undone(Some(three_files()), undo)),
    ]);
    assert_eq!(
        err,
        "changed 3 files (+24 \u{2212}7) in eggfriedrice.code and 2 in ~; efr undo reverts this \
         turn\n"
    );
    let undo = undo_info(&[("app", 2), ("$SCRATCH", 1), ("lib", 1)], 0, &[], 0);
    let err =
        piped(&[Sent(0, turn_started()), Sent(1_000, turn_undone(Some(three_files()), undo))]);
    assert!(
        err.starts_with(
            "changed 3 files (+24 \u{2212}7) in app, $SCRATCH and lib; efr undo reverts this turn\n"
        ),
        "{err}"
    );
    let undo = undo_info(&[], 1, &[], 0);
    let err = piped(&[Sent(0, turn_started()), Sent(1_000, turn_undone(None, undo))]);
    assert!(err.starts_with("changed 1 file in ~; efr undo reverts this turn\n"), "{err}");
}

#[test]
fn a_home_file_with_its_own_line_is_not_counted_again() {
    let line = "echo 'alias k=kubectl' >> ~/.zshrc";
    let info = exit_info(&[ExitKind::Persistence], Launch::Unsandboxed);
    let undo = undo_info(&[], 1, &[], 0);
    let err = piped(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, line)),
        Sent(20, exit_requested(1, line, ExitFacts::default(), &info)),
        Sent(1_000, ended(1, Some(0), None)),
        Sent(1_010, home_copied(1, "/home/user/.zshrc", true, true)),
        Sent(1_500, turn_undone(None, undo)),
    ]);
    assert!(err.contains("changed ~/.zshrc; efr undo reverts it\n"), "{err}");
    assert!(!err.contains("in ~; efr undo reverts this turn"), "{err}");
}

#[test]
fn turn_line_lists_not_covered_paths() {
    let names = ["target/ (ignored, large)", ".env.large (over 10 MiB)", "a", "b", "c", "d", "e"];
    let undo = undo_info(&[("app", 3)], 0, &names, 4);
    let err =
        piped(&[Sent(0, turn_started()), Sent(1_000, turn_undone(Some(three_files()), undo))]);
    assert!(
        err.starts_with(
            "changed 3 files (+24 \u{2212}7) in app; efr undo reverts this turn\nnot covered: \
             target/ (ignored, large), .env.large (over 10 MiB), a, b, c and 6 more\n"
        ),
        "{err}"
    );
}

#[test]
fn turn_line_names_refs_not_covered() {
    let names = ["refs of app (shallow)", "~/big (over 2000 files)"];
    let undo = undo_info(&[("app", 3)], 0, &names, 0);
    let err =
        piped(&[Sent(0, turn_started()), Sent(1_000, turn_undone(Some(three_files()), undo))]);
    assert!(
        err.contains("\nnot covered: refs of app (shallow), ~/big (over 2000 files)\n"),
        "{err}"
    );
}

#[test]
fn turn_line_of_manual_turn_has_no_undo_hint() {
    let undo = TurnUndoInfo { undoable: false, ..undo_info(&[("app", 3)], 0, &["x (big)"], 0) };
    let err =
        piped(&[Sent(0, turn_started()), Sent(1_000, turn_undone(Some(three_files()), undo))]);
    assert_eq!(err, "3 files changed, +24 \u{2212}7\n");
}

#[test]
fn interrupted_turn_shows_its_undo_line() {
    let undo = undo_info(&[("app", 3)], 0, &[], 0);
    let interrupted = Event::TurnInterrupted {
        turn_id: turn(),
        usage: None,
        context: None,
        changes: Some(three_files()),
        undo: Some(undo),
    };
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, shell(1, "cargo fmt")),
        Sent(900, ended(1, Some(0), None)),
        Sent(1_000, interrupted),
    ]));
}

/// Job `number`, which call `n` started.
fn job_of(n: u8, number: u32, command: &str) -> efr_protocol::JobInfo {
    efr_protocol::JobInfo { call_id: id(n), ..crate::testing::job(number, command) }
}

#[test]
fn view_shows_job_start_and_end() {
    let input = json!({ "command": "npm run dev -- --port 3000", "background": true });
    let job = job_of(1, 3, "npm run dev -- --port 3000");
    let job_ended = Event::JobEnded {
        job_id: job.id.clone(),
        state: efr_protocol::JobState::Stopped,
        exit_code: None,
        stopped_by: Some(efr_protocol::JobStopBy::User),
        output_bytes: 1_200_000,
        ran_secs: 92 * 60,
        tail: "ready on :3000\n".to_owned(),
        surface_changes: Vec::new(),
        quarantined: false,
        survivors: vec!["node".to_owned()],
        launcher_lost: false,
        output_truncated: false,
        blocked: Vec::new(),
        blocked_more: 0,
    };
    let matched = Event::JobWatchMatched {
        job_id: job.id.clone(),
        line: "error: port 3000 in use".to_owned(),
        wakes_left: 2,
    };
    let held = Event::JobNoticeHeld {
        job_id: job.id.clone(),
        source_seq: Seq::new(14),
        reason: efr_protocol::HeldReason::NotAuto,
    };
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(10, started(1, "shell", input, Some(Launch::contained()))),
        Sent(
            40,
            Event::JobStarted {
                turn_id: turn(),
                call_id: id(1),
                job,
                launcher_pid: 4242,
                launcher_start: 99,
            }
        ),
        Sent(50, ended(1, None, None)),
        Shot("started"),
        Sent(60, matched),
        Sent(70, message(0, "The dev server runs as job j3.")),
        Sent(80, job_ended),
        Sent(90, held),
        Sent(100, turn_completed()),
    ]));
}
