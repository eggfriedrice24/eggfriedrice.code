use std::path::PathBuf;

use efr_screen::{PromptKind, SemanticPromptEvent, ShellMarkKind};
use libghostty_vt::screen::RowSemanticPrompt;
use pretty_assertions::assert_eq;

use super::{CrossCheck, Disagreement};
use crate::testing::terminal;

/// The zsh sequence set of one command, as the hidden shell emits it.
const PROMPT_CYCLE: &[u8] = b"\x1b]133;A;cl=line\x07\x1b]7;kitty-shell-cwd://arch/home/egg\x07$ \
    \x1b]133;B\x07ls\r\n\x1b]133;C\x07a  b\r\n\x1b]133;D;0\x07";

/// Feeds `chunks` in order through one cross-check and returns every disagreement.
fn run(chunks: &[&[u8]]) -> Vec<Disagreement> {
    let (mut terminal, effects) = terminal(40, 6);
    let mut check = CrossCheck::new(&terminal);
    chunks.iter().flat_map(|chunk| check.write(&mut terminal, &effects, chunk)).collect()
}

fn prompt(kind: PromptKind, fresh_line: bool) -> ShellMarkKind {
    ShellMarkKind::SemanticPrompt(SemanticPromptEvent::PromptStart {
        kind,
        aid: None,
        click: None,
        fresh_line,
    })
}

#[test]
fn a_prompt_cycle_raises_no_disagreement() {
    assert_eq!(run(&[PROMPT_CYCLE, PROMPT_CYCLE]), Vec::new());
}

#[test]
fn a_prompt_cycle_fed_one_byte_at_a_time_raises_none() {
    let chunks: Vec<&[u8]> = PROMPT_CYCLE.chunks(1).collect();
    assert_eq!(run(&chunks), Vec::new());
}

#[test]
fn every_prompt_kind_matches_ghosttys_row_state() {
    let found = run(&[
        b"output\x1b]133;A\x07$ \x1b]133;B\x07",
        b"\r\n\x1b]133;P;k=c\x07> ",
        b"\r\n\x1b]133;A;k=s\x07> ",
        b"\x1b]133;P;k=r\x07[right]",
        b"\r\n\x1b]133;P\x07$ ",
        b"\r\n\x1b]133;A;k=i;aid=7\x07$ ",
    ]);
    assert_eq!(found, Vec::new());
}

#[test]
fn both_osc_7_forms_agree_with_ghostty() {
    let found = run(&[
        b"\x1b]7;file://arch/home/egg/My%20Docs/caf%C3%A9\x07",
        b"\x1b]7;file:///tmp\x1b\\",
        b"\x1b]7;kitty-shell-cwd://arch/tmp/100% sure\x07",
        b"\x1b]7;file://h/srv/x?query#frag\x1b\\",
        b"\x1b]7;FILE://h/srv/y\x07",
    ]);
    assert_eq!(found, Vec::new());
}

#[test]
fn an_empty_osc_7_clears_ghosttys_directory_without_a_mark() {
    let found = run(&[b"\x1b]7;file:///tmp\x07", b"\x1b]7;\x07"]);
    assert_eq!(found, vec![Disagreement::UnmarkedCwd { backend: None }]);
}

#[test]
fn an_osc_7_scheme_the_scanner_refuses_is_an_unmarked_change() {
    let found = run(&[b"\x1b]7;http://arch/tmp\x07$ "]);
    assert_eq!(
        found,
        vec![Disagreement::UnmarkedCwd { backend: Some("http://arch/tmp".to_owned()) }]
    );
}

#[test]
fn an_unmarked_change_to_the_same_directory_is_not_reported_again() {
    let found = run(&[b"\x1b]7;http://arch/tmp\x07", b"\x1b]7;http://arch/tmp\x07"]);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn an_osc_7_cut_short_by_an_escape_is_dispatched_only_by_ghostty() {
    // The scanner drops an OSC that an ESC other than ESC \ interrupts; ghostty
    // dispatches it, so their working directories part.
    let found = run(&[b"\x1b]7;file:///srv\x1b[0m"]);
    assert_eq!(found, vec![Disagreement::UnmarkedCwd { backend: Some("file:///srv".to_owned()) }]);
}

#[test]
fn a_prompt_mark_ghostty_did_not_apply_is_reported() {
    let (mut terminal, effects) = terminal(10, 2);
    terminal.vt_write(b"ab");
    let mut check = CrossCheck::new(&terminal);
    let mut found = Vec::new();
    check.check(&terminal, &effects, Some(&prompt(PromptKind::Initial, true)), &mut found);
    assert_eq!(
        found,
        vec![
            Disagreement::FreshLine { col: Some(2) },
            Disagreement::PromptRow {
                kind: PromptKind::Initial,
                expected: RowSemanticPrompt::Prompt,
                actual: Some(RowSemanticPrompt::None),
            },
        ]
    );
}

#[test]
fn a_continuation_mark_on_a_prompt_row_is_reported() {
    let (mut terminal, effects) = terminal(10, 2);
    terminal.vt_write(b"\x1b]133;A\x07$ ");
    let mut check = CrossCheck::new(&terminal);
    let mut found = Vec::new();
    check.check(&terminal, &effects, Some(&prompt(PromptKind::Secondary, false)), &mut found);
    assert_eq!(
        found,
        vec![Disagreement::PromptRow {
            kind: PromptKind::Secondary,
            expected: RowSemanticPrompt::Continuation,
            actual: Some(RowSemanticPrompt::Prompt),
        }]
    );
}

#[test]
fn a_cwd_mark_ghostty_did_not_apply_is_reported() {
    let (mut terminal, effects) = terminal(10, 2);
    terminal.vt_write(b"\x1b]7;file:///tmp\x07");
    let mut check = CrossCheck::new(&terminal);
    let mark = ShellMarkKind::CwdChanged { host: None, path: PathBuf::from("/srv") };
    let mut found = Vec::new();
    check.check(&terminal, &effects, Some(&mark), &mut found);
    assert_eq!(
        found,
        vec![Disagreement::Cwd {
            host: None,
            path: PathBuf::from("/srv"),
            backend: Some("file:///tmp".to_owned()),
        }]
    );
}

#[test]
fn a_cwd_mark_for_another_host_is_reported() {
    let (mut terminal, effects) = terminal(10, 2);
    terminal.vt_write(b"\x1b]7;kitty-shell-cwd://arch/tmp\x07");
    let mut check = CrossCheck::new(&terminal);
    let mark =
        ShellMarkKind::CwdChanged { host: Some("other".to_owned()), path: PathBuf::from("/tmp") };
    let mut found = Vec::new();
    check.check(&terminal, &effects, Some(&mark), &mut found);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_restored_directory_is_not_taken_for_a_change() {
    let (mut terminal, effects) = terminal(10, 2);
    terminal.vt_write(b"\x1b]7;http://arch/tmp\x07");
    // A screen restored from a snapshot starts with the directory already set.
    let _ = effects.take_pwd_changed();
    let mut check = CrossCheck::new(&terminal);
    assert_eq!(check.write(&mut terminal, &effects, b"\x1b]7;http://arch/tmp\x07"), Vec::new());
}

#[test]
fn disagreements_read_as_one_sentence() {
    let cwd = Disagreement::Cwd {
        host: Some("arch".to_owned()),
        path: PathBuf::from("/srv"),
        backend: Some("file:///tmp".to_owned()),
    };
    assert_eq!(
        cwd.to_string(),
        "the scanner read OSC 7 for /srv on host arch, but libghostty-vt holds file:///tmp"
    );
    let unmarked = Disagreement::UnmarkedCwd { backend: None };
    assert_eq!(
        unmarked.to_string(),
        "libghostty-vt changed its working directory to nothing without an OSC 7 mark"
    );
    let row = Disagreement::PromptRow {
        kind: PromptKind::Right,
        expected: RowSemanticPrompt::Prompt,
        actual: None,
    };
    assert_eq!(
        row.to_string(),
        "a Right prompt mark left the cursor row in prompt state None, not Prompt"
    );
    let fresh = Disagreement::FreshLine { col: Some(3) };
    assert_eq!(
        fresh.to_string(),
        "OSC 133 A left the cursor in column Some(3), not at the start of a line"
    );
}
