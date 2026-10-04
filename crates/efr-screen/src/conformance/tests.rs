use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use efr_protocol::{Cursor, RowCells, ScreenSnapshot, Seq, Size};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::cases::{self, Case};
use super::{FIXTURES, drive, mark_json, run};
use crate::fake::FakeScreen;
use crate::{
    ClickMode, PromptKind, Screen, ScreenSink, SemanticPromptEvent, ShellMark, ShellMarkKind,
};

fn case(text: &str) -> Case {
    cases::parse(Path::new("/fixtures/inline.ndjson"), text).unwrap()
}

/// A backend that renders like the fake screen but never answers a query.
struct Mute(FakeScreen);

struct DropReplies<'a>(&'a mut dyn ScreenSink);

impl ScreenSink for DropReplies<'_> {
    fn pty_reply(&mut self, _bytes: &[u8]) {}

    fn bell(&mut self) {
        self.0.bell();
    }

    fn title_changed(&mut self, title: &str) {
        self.0.title_changed(title);
    }
}

impl Screen for Mute {
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink) {
        self.0.feed(bytes, &mut DropReplies(sink));
    }

    fn resize(&mut self, cols: u16, rows: u16, sink: &mut dyn ScreenSink) {
        self.0.resize(cols, rows, sink);
    }

    fn snapshot(&mut self, scrollback_rows: usize) -> ScreenSnapshot {
        self.0.snapshot(scrollback_rows)
    }

    fn row(&self, index: usize) -> RowCells {
        self.0.row(index)
    }

    fn cursor(&self) -> Cursor {
        self.0.cursor()
    }

    fn title(&self) -> Option<&str> {
        self.0.title()
    }

    fn pwd(&self) -> Option<&str> {
        None
    }
}

fn mute(size: Size) -> impl FnOnce() -> Mute + Send + 'static {
    move || Mute(FakeScreen::new(size))
}

/// A backend that dies on its first feed.
struct Broken;

impl Screen for Broken {
    fn feed(&mut self, _bytes: &[u8], _sink: &mut dyn ScreenSink) {
        panic!("the broken backend failed on purpose");
    }

    fn resize(&mut self, _cols: u16, _rows: u16, _sink: &mut dyn ScreenSink) {}

    fn snapshot(&mut self, _scrollback_rows: usize) -> ScreenSnapshot {
        ScreenSnapshot::default()
    }

    fn row(&self, _index: usize) -> RowCells {
        RowCells::default()
    }

    fn cursor(&self) -> Cursor {
        Cursor::default()
    }

    fn title(&self) -> Option<&str> {
        None
    }

    fn pwd(&self) -> Option<&str> {
        None
    }
}

#[test]
fn the_fake_screen_passes_the_suite() {
    run("fake", FakeScreen::factory);
}

#[test]
fn the_fixtures_load() {
    for dir in ["vt", "shell_marks"] {
        let loaded = cases::load_dir(&Path::new(FIXTURES).join(dir)).unwrap();
        assert!(loaded.len() >= 10, "{dir} holds only {} fixtures", loaded.len());
    }
}

const DA1_CASE: &str = r#"{"kind":"case","size":{"cols":10,"rows":1},"differs":["vt100"]}
{"kind":"feed","text":"\u001b[c"}
{"kind":"expect_replies","text":"\u001b[?62;22c"}"#;

#[test]
fn a_backend_that_misses_a_reply_fails() {
    let outcome = drive("mute", &mute, &case(DA1_CASE));
    assert_eq!(outcome.failures.len(), 1);
    let failure = &outcome.failures[0];
    assert_eq!((failure.line, failure.check), (3, "replies"));
    assert_eq!(failure.expected, "\"\\x1b[?62;22c\"");
    assert_eq!(failure.actual, "\"\"");
}

#[test]
fn a_backend_in_differs_skips_the_backend_checks() {
    let outcome = drive("vt100", &mute, &case(DA1_CASE));
    assert_eq!(outcome.failures, vec![]);
}

#[test]
fn differs_never_skips_the_mark_checks() {
    let text = r#"{"kind":"case","size":{"cols":10,"rows":1},"differs":["fake"]}
{"kind":"feed","text":"\u001b]133;B\u0007"}
{"kind":"expect_marks","marks":[]}"#;
    let outcome = drive("fake", &FakeScreen::factory, &case(text));
    assert_eq!(outcome.failures.len(), 1);
    assert_eq!(outcome.failures[0].check, "marks");
}

#[test]
fn unexpected_marks_at_the_end_fail() {
    let text = r#"{"kind":"case","size":{"cols":10,"rows":1}}
{"kind":"feed","text":"\u001b]133;C\u0007"}"#;
    let outcome = drive("fake", &FakeScreen::factory, &case(text));
    assert_eq!(outcome.failures.len(), 1);
    assert_eq!(outcome.failures[0].check, "unexpected marks at the end");
}

#[test]
fn unexpected_replies_and_bells_at_the_end_fail() {
    let text = r#"{"kind":"case","size":{"cols":10,"rows":1}}
{"kind":"feed","text":"\u001b[c\u0007"}"#;
    let outcome = drive("fake", &FakeScreen::factory, &case(text));
    let checks: Vec<&str> = outcome.failures.iter().map(|failure| failure.check).collect();
    assert_eq!(checks, vec!["unexpected replies at the end", "unexpected bells at the end"]);
}

#[test]
fn a_backend_that_dies_is_a_failure_not_a_hang() {
    let text = r#"{"kind":"case","size":{"cols":10,"rows":1}}
{"kind":"feed","text":"x"}
{"kind":"expect_rows","rows":["x"]}"#;
    let outcome = drive("broken", &|_size: Size| || Broken, &case(text));
    assert_eq!(outcome.failures.len(), 1);
    assert_eq!((outcome.failures[0].line, outcome.failures[0].check), (3, "screen"));
}

#[test]
fn run_reports_every_failure_and_panics() {
    let result = catch_unwind(AssertUnwindSafe(|| run("mute", mute)));
    let payload = result.unwrap_err();
    let report = payload.downcast_ref::<String>().unwrap();
    assert!(report.starts_with("3 conformance failures for backend mute:\n"), "{report}");
    assert!(report.contains("vt/da1_reply.ndjson:3 (replies)"), "{report}");
    assert!(report.contains("vt/dsr_reply.ndjson:3 (replies)"), "{report}");
    assert!(report.contains("vt/dsr_reply.ndjson:5 (replies)"), "{report}");
}

#[test]
fn the_rendering_shows_the_final_screen_and_what_was_reported() {
    let text = r#"{"kind":"case","size":{"cols":6,"rows":2}}
{"kind":"feed","text":"\u001b]2;t\u0007ok\u0007\u001b[c"}
{"kind":"expect_bells","count":1}
{"kind":"expect_replies","text":"\u001b[?62;22c"}"#;
    let outcome = drive("fake", &FakeScreen::factory, &case(text));
    assert_eq!(outcome.failures, vec![]);
    assert_eq!(
        outcome.rendering,
        "case: inline\nsize: 6x2\ncursor: row 0, col 2\ntitle: t\nrows:\n  0 |ok|\n  1 ||\n\
         title changes: [\"t\"]\nbells: 1\nreplies: \"\\x1b[?62;22c\""
    );
}

#[test]
fn mark_json_spells_out_every_kind() {
    let mark = |kind| ShellMark { start: Seq::new(3), end: Seq::new(9), kind };
    let prompt = ShellMarkKind::SemanticPrompt(SemanticPromptEvent::PromptStart {
        kind: PromptKind::Continuation,
        aid: Some("a".to_owned()),
        click: Some(ClickMode::SmartVertical),
        fresh_line: false,
    });
    assert_eq!(
        mark_json(&mark(prompt)),
        json!({"start": 3, "end": 9, "prompt_start": {
            "kind": "continuation", "fresh_line": false, "aid": "a", "click": "smart_vertical"
        }})
    );
    let input = ShellMarkKind::SemanticPrompt(SemanticPromptEvent::InputStart);
    assert_eq!(mark_json(&mark(input)), json!({"start": 3, "end": 9, "input_start": {}}));
    let output = ShellMarkKind::SemanticPrompt(SemanticPromptEvent::OutputStart {
        command: Some("ls".to_owned()),
        aid: None,
    });
    assert_eq!(
        mark_json(&mark(output)),
        json!({"start": 3, "end": 9, "output_start": {"command": "ls"}})
    );
    let end = ShellMarkKind::SemanticPrompt(SemanticPromptEvent::CommandEnd {
        exit_code: Some(-1),
        error: Some("e".to_owned()),
        aid: None,
    });
    assert_eq!(
        mark_json(&mark(end)),
        json!({"start": 3, "end": 9, "command_end": {"exit_code": -1, "error": "e"}})
    );
    let cwd = ShellMarkKind::CwdChanged { host: None, path: PathBuf::from("/tmp") };
    assert_eq!(
        mark_json(&mark(cwd)),
        json!({"start": 3, "end": 9, "cwd_changed": {"path": "/tmp"}})
    );
}
