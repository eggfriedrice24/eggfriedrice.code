use std::path::PathBuf;

use efr_protocol::Seq;
use efr_screen::ShellMarkScanner;
use pretty_assertions::assert_eq;

use super::{Phase, ShellState};

fn state() -> ShellState {
    let pty_id = "01920000-0000-7000-8000-000000000001".parse().unwrap();
    ShellState::new(pty_id, 4242, PathBuf::from("/home/u"), true)
}

/// Applies every mark in `bytes` and returns whether any changed the directory.
fn feed(state: &mut ShellState, bytes: &[u8]) -> bool {
    let mut changed = false;
    for mark in ShellMarkScanner::new().scan(bytes, Seq::ZERO) {
        changed |= state.apply(&mark.kind);
    }
    changed
}

#[test]
fn a_new_zsh_starts_and_a_new_other_shell_is_unmarked() {
    assert_eq!(state().phase, Phase::Starting);
    let pty_id = "01920000-0000-7000-8000-000000000001".parse().unwrap();
    let bash = ShellState::new(pty_id, 1, PathBuf::from("/"), false);
    assert_eq!(bash.phase, Phase::Unmarked);
    assert!(bash.accepts_command());
}

#[test]
fn a_primary_prompt_makes_the_shell_ready() {
    let mut state = state();
    feed(&mut state, b"\x1b]133;A;cl=line\x07% ");
    assert_eq!(state.phase, Phase::Prompting { continuation: false });
    assert!(!state.accepts_command());
    feed(&mut state, b"\x1b]133;B\x07");
    assert_eq!(state.phase, Phase::Ready);
    assert!(state.integration);
    assert!(state.accepts_command());
}

#[test]
fn a_command_runs_and_its_status_is_kept() {
    let mut state = state();
    feed(&mut state, b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07");
    assert_eq!(state.phase, Phase::Running);
    assert!(state.is_busy());
    feed(&mut state, b"\x1b]133;D;3\x07");
    assert_eq!(state.phase, Phase::Finished);
    assert_eq!(state.last_exit, Some(3));
}

#[test]
fn a_bare_end_after_an_empty_line_keeps_the_last_status() {
    let mut state = state();
    feed(&mut state, b"\x1b]133;C\x07\x1b]133;D;1\x07\x1b]133;A\x07\x1b]133;B\x07");
    feed(&mut state, b"\x1b]133;D\x07");
    assert_eq!(state.last_exit, Some(1));
    assert_eq!(state.phase, Phase::Finished);
}

#[test]
fn a_continuation_prompt_is_busy() {
    let mut state = state();
    feed(&mut state, b"\x1b]133;P;k=s\x07\x1b]133;B\x07");
    assert_eq!(state.phase, Phase::Continuation);
    assert!(state.is_busy());
    assert!(!state.accepts_command());
}

#[test]
fn an_input_mark_without_a_prompt_changes_nothing() {
    let mut state = state();
    feed(&mut state, b"\x1b]133;C\x07\x1b]133;B\x07");
    assert_eq!(state.phase, Phase::Running);
}

#[test]
fn a_cwd_report_changes_the_directory_once() {
    let mut state = state();
    assert!(feed(&mut state, b"\x1b]7;kitty-shell-cwd://box/tmp\x07"));
    assert_eq!(state.cwd, PathBuf::from("/tmp"));
    assert_eq!(state.host.as_deref(), Some("box"));
    assert!(!feed(&mut state, b"\x1b]7;kitty-shell-cwd://box/tmp\x07"));
}

#[test]
fn the_startup_timeout_only_moves_a_starting_shell() {
    let mut state = state();
    state.startup_expired();
    assert_eq!(state.phase, Phase::Unmarked);

    let mut ready = self::state();
    feed(&mut ready, b"\x1b]133;A\x07\x1b]133;B\x07");
    ready.startup_expired();
    assert_eq!(ready.phase, Phase::Ready);
}

#[test]
fn a_late_mark_brings_an_unmarked_shell_back_to_marks() {
    let mut state = state();
    state.startup_expired();
    feed(&mut state, b"\x1b]133;A\x07\x1b]133;B\x07");
    assert_eq!(state.phase, Phase::Ready);
}
