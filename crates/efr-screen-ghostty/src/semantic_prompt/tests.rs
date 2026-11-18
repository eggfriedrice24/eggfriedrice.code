use std::path::PathBuf;

use efr_screen::{
    PromptKind, SemanticPromptEvent, Seq, ShellMark, ShellMarkKind, ShellMarkScanner,
};
use libghostty_vt::Terminal;
use libghostty_vt::screen::RowSemanticPrompt;
use pretty_assertions::assert_eq;

use super::{CrossCheck, Disagreement, Step};
use crate::effects::Effects;
use crate::testing::terminal;

/// The zsh sequence set of one command, as the hidden shell emits it.
const PROMPT_CYCLE: &[u8] = b"\x1b]133;A;cl=line\x07\x1b]7;kitty-shell-cwd://arch/home/egg\x07$ \
    \x1b]133;B\x07ls\r\n\x1b]133;C\x07a  b\r\n\x1b]133;D;0\x07";

/// The actor's part: scans each chunk once and hands the bytes and the marks over.
struct Actor {
    scanner: ShellMarkScanner,
    next: u64,
}

impl Actor {
    fn new() -> Self {
        Actor { scanner: ShellMarkScanner::new(), next: 0 }
    }

    fn write(
        &mut self,
        check: &mut CrossCheck,
        terminal: &mut Terminal<'static, 'static>,
        effects: &Effects,
        chunk: &[u8],
    ) -> Vec<Disagreement> {
        let base = Seq::new(self.next);
        self.next += chunk.len() as u64;
        let marks = self.scanner.scan(chunk, base);
        check.write(terminal, effects, chunk, base, &marks)
    }
}

/// Feeds `chunks` in order through one cross-check and returns every disagreement.
fn run(chunks: &[&[u8]]) -> Vec<Disagreement> {
    let (mut terminal, effects) = terminal(40, 6);
    let mut check = CrossCheck::new(&terminal);
    let mut actor = Actor::new();
    chunks
        .iter()
        .flat_map(|chunk| actor.write(&mut check, &mut terminal, &effects, chunk))
        .collect()
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
fn a_full_reset_raises_no_disagreement() {
    assert_eq!(run(&[PROMPT_CYCLE, b"\x1bc", PROMPT_CYCLE]), Vec::new());
    assert_eq!(run(&[PROMPT_CYCLE, b"\x1bcok"]), Vec::new());
}

#[test]
fn an_unmarked_directory_after_a_full_reset_is_reported() {
    // The same directory as before the reset: the reset cleared it, so it changed.
    let found = run(&[PROMPT_CYCLE, b"\x1bc\x1b]7;kitty-shell-cwd://arch/home/egg\x1b[0m"]);
    let backend = Some("kitty-shell-cwd://arch/home/egg".to_owned());
    assert_eq!(found, vec![Disagreement::UnmarkedCwd { backend }]);
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
            Disagreement::Step { expected: Step::PromptStart(PromptKind::Initial), actual: None },
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
        vec![
            Disagreement::Step {
                expected: Step::PromptStart(PromptKind::Secondary),
                actual: Some(Some(Step::PromptStart(PromptKind::Initial))),
            },
            Disagreement::PromptRow {
                kind: PromptKind::Secondary,
                expected: RowSemanticPrompt::Continuation,
                actual: Some(RowSemanticPrompt::Prompt),
            },
        ]
    );
}

/// The marks of `bytes` from one scan, as the actor gives them.
fn marks(bytes: &[u8]) -> Vec<ShellMark> {
    ShellMarkScanner::new().scan(bytes, Seq::ZERO)
}

#[test]
fn every_mark_lines_up_with_the_end_of_an_osc_in_every_split() {
    // Both terminators, so that `ESC \` is also cut between its two bytes.
    let mut stream = PROMPT_CYCLE.to_vec();
    stream.extend_from_slice(
        b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\x\r\n\x1b]133;C\x1b\\\x1b]133;D;3\x1b\\",
    );
    stream.extend_from_slice(b"\x1b]7;file://arch/srv\x1b\\");
    for at in 0..=stream.len() {
        let (head, tail) = stream.split_at(at);
        assert_eq!(run(&[head, tail]), Vec::new(), "split at {at}");
    }
}

#[test]
fn the_steps_that_libghostty_reports_are_compared_with_the_marks() {
    let bytes = b"\x1b]133;D;3\x07";
    let (mut terminal, effects) = terminal(20, 2);
    let mut check = CrossCheck::new(&terminal);
    let mut wrong = marks(bytes);
    let ShellMarkKind::SemanticPrompt(SemanticPromptEvent::CommandEnd { exit_code, .. }) =
        &mut wrong[0].kind
    else {
        panic!("{wrong:?}");
    };
    *exit_code = Some(1);
    let found = check.write(&mut terminal, &effects, bytes, Seq::ZERO, &wrong);
    assert_eq!(
        found,
        vec![Disagreement::Step {
            expected: Step::CommandEnd(Some(1)),
            actual: Some(Some(Step::CommandEnd(Some(3)))),
        }]
    );
}

#[test]
fn a_mark_that_ends_where_no_osc_ends_is_unread() {
    let bytes = b"ab\x1b]133;B\x07cd";
    let (mut terminal, effects) = terminal(20, 2);
    let mut check = CrossCheck::new(&terminal);
    let mut late = marks(bytes);
    late[0].end = Seq::new(late[0].end.get() + 1);
    let found = check.write(&mut terminal, &effects, bytes, Seq::ZERO, &late);
    assert_eq!(found, vec![Disagreement::Unread { end: 11 }]);
}

#[test]
fn a_step_without_a_mark_is_not_a_disagreement() {
    // The scanner skips the marks of a tmux in the hidden shell on purpose, and
    // libghostty-vt reads them.
    let found = run(&[b"\x1bPtmux;\x1b\x1b]133;A\x07\x1b\\$ "]);
    assert_eq!(found, Vec::new());
}

#[test]
fn a_feed_without_marks_checks_nothing_and_keeps_the_directory() {
    let (mut terminal, effects) = terminal(20, 2);
    let mut check = CrossCheck::new(&terminal);
    check.write_unchecked(&mut terminal, &effects, b"\x1b]7;http://arch/tmp\x07\x1b]133;A\x07");
    assert_eq!(check.pwd.as_deref(), Some("http://arch/tmp"));
    // The next checked feed starts from that directory and those flags.
    let found = Actor::new().write(&mut check, &mut terminal, &effects, b"$ ");
    assert_eq!(found, Vec::new());
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
    let found =
        Actor::new().write(&mut check, &mut terminal, &effects, b"\x1b]7;http://arch/tmp\x07");
    assert_eq!(found, Vec::new());
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
