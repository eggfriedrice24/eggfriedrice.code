use pretty_assertions::assert_eq;

use super::{ClickMode, PromptKind, SemanticPromptEvent, parse};

fn prompt(kind: PromptKind, fresh_line: bool) -> SemanticPromptEvent {
    SemanticPromptEvent::PromptStart { kind, aid: None, click: None, fresh_line }
}

fn end(exit_code: Option<i32>) -> SemanticPromptEvent {
    SemanticPromptEvent::CommandEnd { exit_code, error: None, aid: None }
}

fn output(command: Option<&str>) -> SemanticPromptEvent {
    SemanticPromptEvent::OutputStart { command: command.map(str::to_owned), aid: None }
}

#[test]
fn bare_actions() {
    assert_eq!(parse(b"A"), Some(prompt(PromptKind::Initial, true)));
    assert_eq!(parse(b"P"), Some(prompt(PromptKind::Initial, false)));
    assert_eq!(parse(b"B"), Some(SemanticPromptEvent::InputStart));
    assert_eq!(parse(b"C"), Some(output(None)));
    assert_eq!(parse(b"D"), Some(end(None)));
}

#[test]
fn the_ghostty_zsh_sequence_set() {
    assert_eq!(
        parse(b"A;cl=line"),
        Some(SemanticPromptEvent::PromptStart {
            kind: PromptKind::Initial,
            aid: None,
            click: Some(ClickMode::Line),
            fresh_line: true,
        })
    );
    assert_eq!(parse(b"P;k=s"), Some(prompt(PromptKind::Secondary, false)));
    assert_eq!(parse(b"D;0"), Some(end(Some(0))));
}

#[test]
fn exit_codes() {
    assert_eq!(parse(b"D;1"), Some(end(Some(1))));
    assert_eq!(parse(b"D;130"), Some(end(Some(130))));
    assert_eq!(parse(b"D;-1"), Some(end(Some(-1))));
}

#[test]
fn a_missing_or_malformed_exit_code_is_none() {
    assert_eq!(parse(b"D;"), Some(end(None)));
    assert_eq!(parse(b"D;;aid=1"), Some(end(None).with_aid("1")));
    assert_eq!(parse(b"D;ok"), Some(end(None)));
    assert_eq!(parse(b"D;99999999999"), Some(end(None)));
}

#[test]
fn the_exit_code_is_only_the_first_field() {
    assert_eq!(parse(b"D;aid=7;0"), Some(end(None).with_aid("7")));
}

#[test]
fn command_end_options() {
    assert_eq!(
        parse(b"D;2;err=not found;aid=shell-1"),
        Some(SemanticPromptEvent::CommandEnd {
            exit_code: Some(2),
            error: Some("not found".to_owned()),
            aid: Some("shell-1".to_owned()),
        })
    );
}

#[test]
fn aid_on_every_action_that_takes_it() {
    assert_eq!(parse(b"A;aid=42"), Some(prompt(PromptKind::Initial, true).with_aid("42")));
    assert_eq!(parse(b"C;aid=42"), Some(output(None).with_aid("42")));
    assert_eq!(parse(b"D;0;aid=42"), Some(end(Some(0)).with_aid("42")));
}

#[test]
fn the_first_occurrence_of_an_option_wins() {
    assert_eq!(parse(b"A;aid=1;aid=2"), Some(prompt(PromptKind::Initial, true).with_aid("1")));
}

#[test]
fn a_malformed_first_occurrence_is_not_replaced_by_a_later_one() {
    assert_eq!(parse(b"P;k=x;k=s"), Some(prompt(PromptKind::Initial, false)));
}

#[test]
fn every_prompt_kind() {
    for (value, kind) in [
        ("i", PromptKind::Initial),
        ("c", PromptKind::Continuation),
        ("s", PromptKind::Secondary),
        ("r", PromptKind::Right),
    ] {
        assert_eq!(parse(format!("P;k={value}").as_bytes()), Some(prompt(kind, false)));
    }
}

#[test]
fn every_click_mode() {
    for (value, click) in [
        ("line", ClickMode::Line),
        ("m", ClickMode::Multiple),
        ("v", ClickMode::ConservativeVertical),
        ("w", ClickMode::SmartVertical),
    ] {
        let parsed = parse(format!("A;cl={value}").as_bytes());
        assert_eq!(
            parsed,
            Some(SemanticPromptEvent::PromptStart {
                kind: PromptKind::Initial,
                aid: None,
                click: Some(click),
                fresh_line: true,
            })
        );
    }
}

#[test]
fn unknown_and_malformed_options_are_ignored() {
    assert_eq!(
        parse(b"A;redraw=1;cl=lines;k=;special_key=1"),
        Some(prompt(PromptKind::Initial, true))
    );
    assert_eq!(parse(b"B;k=i"), Some(SemanticPromptEvent::InputStart));
    assert_eq!(parse(b"A;;novalue;"), Some(prompt(PromptKind::Initial, true)));
}

#[test]
fn an_empty_aid_is_kept() {
    assert_eq!(parse(b"A;aid="), Some(prompt(PromptKind::Initial, true).with_aid("")));
}

#[test]
fn a_non_utf8_aid_is_ignored() {
    assert_eq!(parse(b"A;aid=\xff"), Some(prompt(PromptKind::Initial, true)));
}

#[test]
fn bodies_that_are_not_marks() {
    assert_eq!(parse(b""), None);
    assert_eq!(parse(b"X"), None);
    assert_eq!(parse(b"AB"), None);
    assert_eq!(parse(b"a"), None);
    assert_eq!(parse(b"D0"), None);
    // Actions that ghostty knows but the scanner does not report.
    assert_eq!(parse(b"L"), None);
    assert_eq!(parse(b"N"), None);
    assert_eq!(parse(b"I"), None);
}

#[test]
fn cmdline_is_printf_q_decoded() {
    assert_eq!(parse(b"C;cmdline=ls\\ -la"), Some(output(Some("ls -la"))));
    assert_eq!(parse(b"C;cmdline=$'echo\\ta\\nb'"), Some(output(Some("echo\ta\nb"))));
    assert_eq!(parse(b"C;cmdline='git status'"), Some(output(Some("git status"))));
    assert_eq!(parse(b"C;cmdline=a\\\\b\\\"c\\'d\\$e"), Some(output(Some("a\\b\"c'd$e"))));
    assert_eq!(parse(b"C;cmdline=\\e\\r\\v"), Some(output(Some("\x1b\r\x0b"))));
    assert_eq!(parse(b"C;cmdline=''"), Some(output(Some(""))));
}

#[test]
fn an_undecodable_cmdline_is_none() {
    assert_eq!(parse(b"C;cmdline=a\\d"), Some(output(None)));
    assert_eq!(parse(b"C;cmdline=trailing\\"), Some(output(None)));
    assert_eq!(parse(b"C;cmdline=$'open"), Some(output(None)));
    assert_eq!(parse(b"C;cmdline=$'"), Some(output(None)));
    assert_eq!(parse(b"C;cmdline='"), Some(output(None)));
}

#[test]
fn cmdline_url_is_percent_decoded() {
    assert_eq!(parse(b"C;cmdline_url=echo%20%3Bhi"), Some(output(Some("echo ;hi"))));
    assert_eq!(parse(b"C;cmdline_url=bad%2"), Some(output(None)));
}

#[test]
fn cmdline_wins_over_cmdline_url_even_when_it_does_not_decode() {
    assert_eq!(parse(b"C;cmdline_url=b;cmdline=a"), Some(output(Some("a"))));
    assert_eq!(parse(b"C;cmdline=a\\d;cmdline_url=b"), Some(output(None)));
}

trait WithAid {
    fn with_aid(self, aid: &str) -> Self;
}

impl WithAid for SemanticPromptEvent {
    fn with_aid(mut self, value: &str) -> Self {
        match &mut self {
            SemanticPromptEvent::PromptStart { aid, .. }
            | SemanticPromptEvent::OutputStart { aid, .. }
            | SemanticPromptEvent::CommandEnd { aid, .. } => *aid = Some(value.to_owned()),
            SemanticPromptEvent::InputStart => panic!("InputStart has no aid"),
        }
        self
    }
}
