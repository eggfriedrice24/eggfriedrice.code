use std::path::{Path, PathBuf};
use std::time::Duration;

use efr_config::{AutoOnOff, NotifySettings};
use efr_protocol::{RouteLabel, RouteScope};
use efr_test_support::TestClock;
use futures::FutureExt as _;
use pretty_assertions::assert_eq;

use super::{Notices, WAIT_AFTER, WaitNotice, place, sequence, wanted};
use crate::terminal::TermFacts;
use crate::testing::terminal_facts;

/// A terminal with this `TERM_PROGRAM`, behind a multiplexer or not.
fn facts(program: Option<&str>, multiplexer: bool) -> TermFacts {
    TermFacts {
        term_program: program.map(str::to_owned),
        term_program_version: Some("1.4.0".to_owned()),
        multiplexer,
        ..terminal_facts()
    }
}

/// `[notify]` with `terminal` and `after_secs`.
fn settings(terminal: AutoOnOff, after_secs: u32) -> NotifySettings {
    let mut settings = NotifySettings::default();
    settings.terminal = terminal;
    settings.after_secs = after_secs;
    settings
}

fn notices(after_secs: u32) -> Notices {
    let settings = settings(AutoOnOff::On, after_secs);
    Notices::of(&settings, &terminal_facts(), "app".to_owned()).unwrap()
}

#[test]
fn auto_sends_the_notice_only_in_ghostty_and_never_through_a_multiplexer() {
    let cases = [
        (Some("ghostty"), false, true),
        (Some("Ghostty"), false, true),
        // tmux drops OSC 777, and GNU screen and zellij keep the outer TERM_PROGRAM.
        (Some("ghostty"), true, false),
        (Some("tmux"), true, false),
        (Some("kitty"), false, false),
        (Some("WezTerm"), false, false),
        (None, false, false),
    ];
    for (program, multiplexer, expected) in cases {
        let facts = facts(program, multiplexer);
        assert_eq!(wanted(AutoOnOff::Auto, &facts), expected, "{facts:?}");
        assert!(wanted(AutoOnOff::On, &facts), "on sends it everywhere: {facts:?}");
        assert!(!wanted(AutoOnOff::Off, &facts), "off never sends it: {facts:?}");
    }
}

#[test]
fn nothing_goes_out_when_stdout_is_not_a_terminal() {
    let ghostty = facts(Some("ghostty"), false);
    let piped = TermFacts { stdout_tty: false, ..ghostty.clone() };
    let dumb = TermFacts { term: Some("dumb".to_owned()), ..ghostty };
    for facts in [piped, dumb] {
        assert!(!wanted(AutoOnOff::Auto, &facts));
        assert!(!wanted(AutoOnOff::On, &facts));
    }
}

#[test]
fn the_notices_follow_the_setting() {
    let off = settings(AutoOnOff::Off, 30);
    assert_eq!(Notices::of(&off, &facts(Some("ghostty"), false), "app".to_owned()), None);
    let auto = NotifySettings::default();
    assert!(Notices::of(&auto, &facts(Some("ghostty"), false), "app".to_owned()).is_some());
}

#[test]
fn the_sequence_has_the_title_efr_and_the_body() {
    assert_eq!(
        sequence("app: a question waits"),
        "\x1b]777;notify;efr;app: a question waits\x1b\\"
    );
}

#[test]
fn a_wait_names_a_question_or_a_command_that_waits_for_input() {
    let notices = notices(30);
    assert_eq!(notices.wait(true), "app: a question waits");
    assert_eq!(notices.wait(false), "app: a command waits for input");
}

#[test]
fn the_end_of_a_turn_sends_a_notice_only_from_after_secs_on() {
    let half = notices(30);
    assert_eq!(half.ended(Duration::from_secs(29), false), None);
    assert_eq!(half.ended(Duration::from_secs(30), false).as_deref(), Some("app: done in 30s"));
    assert_eq!(half.ended(Duration::from_secs(252), false).as_deref(), Some("app: done in 4m 12s"));
    assert_eq!(
        half.ended(Duration::from_secs(63), true).as_deref(),
        Some("app: failed after 1m 3s")
    );
    // 0 sends one for each turn.
    assert_eq!(notices(0).ended(Duration::ZERO, false).as_deref(), Some("app: done in 0s"));
}

#[test]
fn a_body_carries_no_control_character_of_the_place() {
    let on = settings(AutoOnOff::On, 0);
    let place = "a\x1b\\\x07pp".to_owned();
    let body = Notices::of(&on, &terminal_facts(), place).unwrap().wait(true);
    assert!(!body.chars().any(char::is_control), "{body:?}");
}

#[tokio::test]
async fn a_wait_sends_its_notice_once_after_its_time_with_no_key() {
    let clock = TestClock::new();
    let mut notice = WaitNotice::default();
    notice.watch(None, &clock);
    assert!(!notice.runs(), "no wait, no time");
    notice.watch(Some(1), &clock);
    assert!(notice.runs());
    clock.advance(WAIT_AFTER - Duration::from_millis(1));
    assert!(notice.due().now_or_never().is_none(), "not yet");
    clock.advance(Duration::from_millis(1));
    assert!(notice.due().now_or_never().is_some());
    // The same wait goes on: no second notice.
    notice.watch(Some(1), &clock);
    assert!(!notice.runs());
    assert!(notice.due().now_or_never().is_none());
    // The next wait has its own time.
    notice.watch(None, &clock);
    notice.watch(Some(1), &clock);
    assert!(notice.runs());
}

#[tokio::test]
async fn a_key_or_the_end_of_the_wait_stops_its_time() {
    let clock = TestClock::new();
    let mut notice = WaitNotice::default();
    notice.watch(Some(1), &clock);
    notice.key();
    notice.watch(Some(1), &clock);
    clock.advance(WAIT_AFTER);
    assert!(notice.due().now_or_never().is_none(), "a key came in this wait");
    let mut ended = WaitNotice::default();
    ended.watch(Some(1), &clock);
    ended.watch(None, &clock);
    clock.advance(WAIT_AFTER);
    assert!(ended.due().now_or_never().is_none(), "the wait ended");
}

#[tokio::test]
async fn a_wait_that_follows_another_at_once_has_its_own_time() {
    let clock = TestClock::new();
    let mut notice = WaitNotice::default();
    // A card, answered with a key, while the answer line of a command still waits.
    notice.watch(Some(1), &clock);
    notice.key();
    notice.watch(Some(2), &clock);
    assert!(notice.runs());
    clock.advance(WAIT_AFTER);
    assert!(notice.due().now_or_never().is_some(), "the answer line sends its notice");
}

#[test]
fn the_place_is_the_project_else_the_last_part_of_the_directory_else_home() {
    let id = "019a9b1c-3d00-7a10-8b20-0000000000a1".parse().unwrap();
    let project = |name: Option<&str>| RouteLabel {
        scope: RouteScope::Project(id),
        name: name.map(str::to_owned),
        dir: PathBuf::from("/home/u/p/app"),
    };
    assert_eq!(place(Some(&project(Some("efr"))), None), "efr");
    // A project that is no longer registered has no name.
    assert_eq!(place(Some(&project(None)), None), "app");
    let outside =
        RouteLabel { scope: RouteScope::Outside, name: None, dir: PathBuf::from("/home/u/notes") };
    assert_eq!(place(Some(&outside), Some(Path::new("/srv"))), "notes");
    assert_eq!(place(None, Some(Path::new("/srv/www"))), "www");
    assert_eq!(place(None, Some(Path::new("/"))), "~");
    assert_eq!(place(None, None), "~");
}
