//! The layout of the interactive part of a turn, case by case: tool calls, their
//! results, refusals, questions and their answers, the spacing between blocks and the
//! sandbox note. Each case plays on a terminal 40 and 80 columns wide, with colour and
//! with `NO_COLOR`, and through a pipe. A snapshot shows the screen that a terminal
//! keeps after each step that the case names, and what the pipe got.

use std::path::PathBuf;

use efr_protocol::{
    ApprovalDecision, CallId, Event, EventEnvelope, ExitFacts, ExitInfo, ExitKind, ExitSource,
    Grant, Launch, Origin, ProgramFact, Scope, Seq, Usage,
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
    Event::TurnCompleted { turn_id: turn(), usage: Some(usage) }
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
    let terminal = options.is_terminal();
    let look = Look { motion: false, summary: true, progress: false };
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
    let mut shown = Vec::new();
    for cols in [40_u16, 80] {
        let size = Size { cols, rows: 40 };
        for (name, colour) in [("colour", ColourMode::Ansi16), ("NO_COLOR", ColourMode::None)] {
            let options = RenderOptions::new(cols).with_colour(colour);
            shown.push(format!("=== {cols} columns, {name}\n{}", play(options, size, acts)));
        }
    }
    let piped = RenderOptions::new(80).with_terminal(false);
    shown.push(format!("=== not a terminal\n{}", play(piped, Size::default(), acts)));
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
