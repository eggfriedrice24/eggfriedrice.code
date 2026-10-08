//! The layout of the interactive part of a turn, case by case: tool calls, their
//! results, refusals, questions and their answers, the spacing between blocks and the
//! sandbox note. Each case plays on a terminal 40 and 80 columns wide, with colour and
//! with `NO_COLOR`, and through a pipe. A snapshot shows the screen that a terminal
//! keeps after each step that the case names, and what the pipe got.

use std::path::PathBuf;

use efr_protocol::{
    ApprovalDecision, CallId, ChangeKind, Event, EventEnvelope, ExitFacts, ExitInfo, ExitKind,
    ExitSource, FileChange, FileChanges, Grant, Launch, Origin, ProgramFact, Scope, Seq, Usage,
};
use efr_render::{ColourMode, RenderOptions, WidthMethod, display_width};
use serde_json::{Value, json};

use super::super::{Look, TurnView};
use super::at;
use crate::terminal::Size;
use crate::testing::{FROM_SRC, exit_info, exit_record, program_fact, readable, turn};

/// What a case does, step by step.
#[expect(clippy::large_enum_variant, reason = "a short list of steps in a test")]
#[derive(Debug, Clone)]
enum Act {
    /// The daemon sends this event, `millis` after the prompt.
    Event(i64, Event),
    /// The user answers the question about call `n` with a key.
    Key(u8, ApprovalDecision),
    /// The snapshot shows the screen now, under this name.
    Look(&'static str),
}

use Act::{Event as Sent, Key, Look as Shot};

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
    let usage = Usage { input_tokens: 18_200, output_tokens: 1_100 };
    Event::TurnCompleted { turn_id: turn(), usage: Some(usage), changes: None }
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
        Sent(9_000, Event::ApprovalExpired { turn_id: turn(), call_id: id(4) }),
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
    let usage = Usage { input_tokens: 18_200, output_tokens: 1_100 };
    Event::TurnCompleted { turn_id: turn(), usage: Some(usage), changes: Some(changes) }
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
