//! A turn in full mode: the line in the error role at its start, the question of a
//! delete that full mode guards with its time limit, and the line at its end that says
//! what full mode let through. The events are built by hand, as efrd sends them.

use std::path::PathBuf;

use efr_protocol::{
    CallId, EffectiveSettings, Event, ExitFacts, ExitKind, ExitSource, Grant, JudgeKind, Launch,
    Mode, OverriddenSettings, Scope, Verdict,
};
use efr_render::{Colour, ColourMode, Palette, RenderOptions, Role};
use pretty_assertions::assert_eq;

use super::super::{TurnEnd, TurnView};
use super::{WIDE, sandbox_view, turn_completed};
use crate::testing::{bare, call, exit_info, exit_record, now, readable, turn};

/// The red of the error role in these tests' palette, as its SGR.
const RED: &str = "\x1b[38;2;224;108;117m";

/// Feeds `events` at the width of [`WIDE`] and joins what they wrote, each step with
/// the frame that shows it, and the end of the turn.
fn wide(view: &mut TurnView, events: &[Event], can_ask: bool) -> (String, String, Option<TurnEnd>) {
    let (mut out, mut err, mut end) = (String::new(), String::new(), None);
    for event in events {
        let step = view.event(event, WIDE, can_ask);
        out.push_str(&step.out);
        out.push_str(&view.frame(WIDE, now()));
        err.push_str(&step.err);
        end = end.or(step.end);
    }
    (out, err, end)
}

fn started(mode: Mode) -> Event {
    Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/home/user/project"),
        scope: Scope::Machine,
        settings: Some(EffectiveSettings {
            mode,
            model: "gpt-5.5".to_owned(),
            effort: None,
            overridden: OverriddenSettings::default(),
            fallback: None,
        }),
    }
}

/// A second call of the turn.
fn other_call() -> CallId {
    "019a9b1c-3d00-7a10-8b20-000000000013".parse().unwrap()
}

/// The record of an exit of call `call_id` with `kinds` and `grants`, for line `line`.
fn requested(call_id: CallId, line: &str, kinds: &[ExitKind], grants: Vec<Grant>) -> Event {
    Event::ExitRequested {
        turn_id: turn(),
        call_id,
        kinds: kinds.to_vec(),
        grants,
        source: ExitSource::Predicted,
        record: Box::new(exit_record(line, ExitFacts::default())),
    }
}

/// The verdict of `judge` on the exit of call `call_id`.
fn judged(call_id: CallId, judge: JudgeKind, verdict: Verdict) -> Event {
    Event::ExitJudged {
        turn_id: turn(),
        call_id,
        judge,
        verdict,
        model: None,
        latency_ms: None,
        risk: None,
        user_authorization: None,
        category: None,
        rationale: None,
        record_sha256: None,
        cached: false,
        chain_step: None,
        skipped: Vec::new(),
    }
}

/// A write of `~/<name>` that full mode let through, by call `call_id`.
fn write_let_through(call_id: CallId, name: &str) -> [Event; 2] {
    let path = PathBuf::from(format!("/home/user/{name}"));
    let line = format!("echo x > ~/{name}");
    [
        requested(call_id, &line, &[ExitKind::Write], vec![Grant::Write { path }]),
        judged(call_id, JudgeKind::Full, Verdict::Allow),
    ]
}

#[test]
fn a_full_turn_says_so_at_the_start_in_the_error_role() {
    let palette = Palette::new().with(Role::Error, Colour::Rgb(0xe0, 0x6c, 0x75));
    let options = RenderOptions::new(100).with_colour(ColourMode::TrueColor).with_palette(palette);
    let mut view = TurnView::new(turn(), options);
    let (out, err, _) = wide(&mut view, &[started(Mode::Full)], true);
    assert_eq!(err, "");
    let line = "full mode: no sandbox, no questions; efr undo reverts the project and the \
                files efr saw";
    assert!(out.contains(&format!("{RED}{line}\x1b[0m\n")), "{}", readable(&out));
    // Without a terminal the line goes to stderr, plain.
    let mut view = sandbox_view(false);
    let (out, err, _) = wide(&mut view, &[started(Mode::Full)], true);
    assert_eq!((out.as_str(), err.as_str()), ("", format!("{line}\n").as_str()));
    // Another mode has no such line.
    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        let mut view = sandbox_view(false);
        let (out, err, _) = wide(&mut view, &[started(mode)], true);
        assert_eq!((out.as_str(), err.as_str()), ("", ""), "{mode}");
    }
}

#[test]
fn the_turn_end_counts_what_full_mode_let_through() {
    let mut view = sandbox_view(false);
    let mut events = vec![started(Mode::Full)];
    events.extend(write_let_through(call(), "notes.txt"));
    events.push(requested(other_call(), "git push", &[ExitKind::Upload], Vec::new()));
    events.push(judged(other_call(), JudgeKind::Full, Verdict::Allow));
    // The same action again counts once.
    events.extend(write_let_through(other_call(), "notes.txt"));
    events.push(turn_completed());
    let (_, err, _) = wide(&mut view, &events, true);
    assert!(
        err.contains("efr saw 2 actions that full mode let through: write ~/notes.txt, upload\n"),
        "{err}"
    );

    // Past three, the line ends with `...`.
    let mut view = sandbox_view(true);
    let mut events = vec![started(Mode::Full)];
    for name in ["a", "b", "c", "d"] {
        events.extend(write_let_through(call(), name));
    }
    events.push(turn_completed());
    let (out, _, _) = wide(&mut view, &events, true);
    assert!(
        bare(&out).contains(
            "efr saw 4 actions that full mode let through: write ~/a, write ~/b, write ~/c, ...\n"
        ),
        "{}",
        readable(&out)
    );
}

#[test]
fn no_turn_end_line_without_a_full_exit() {
    // A full turn whose calls named no exit, an exit that another judge allowed, and a
    // full verdict that is no allow: no line, and never `0 actions`.
    let mut view = sandbox_view(false);
    let mut events = vec![started(Mode::Full)];
    events.push(requested(call(), "git push", &[ExitKind::Upload], Vec::new()));
    events.push(judged(call(), JudgeKind::User, Verdict::Allow));
    events.push(requested(other_call(), "rm -rf ~", &[ExitKind::Destructive], Vec::new()));
    events.push(judged(other_call(), JudgeKind::Full, Verdict::Deny));
    events.push(turn_completed());
    let (out, err, _) = wide(&mut view, &events, true);
    assert!(!out.contains("full mode let through"), "{out}");
    assert!(!err.contains("full mode let through"), "{err}");
    assert!(!err.contains("0 actions"), "{err}");
}

#[test]
fn the_full_question_card_shows_the_summary_and_expires() {
    // efrd writes the guard's line first in the summary; the card shows the summary and
    // how long the question waits.
    let mut view = sandbox_view(false);
    let question = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "full mode still asks for this: delete of ~ (your home directory)\n\
                  $ rm -rf ~"
            .to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
        expires_in_secs: Some(120),
    };
    let (_, err, _) = wide(&mut view, &[started(Mode::Full), question], true);
    insta::assert_snapshot!(err);
    assert!(err.contains("full mode still asks for this: delete of ~ (your home directory)"));
    assert!(err.contains("\u{2502} expires in 2 min\n"), "{err}");
    // A question with no fixed limit has no such row.
    let mut view = sandbox_view(false);
    let question = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "write ~/.zshrc".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
        expires_in_secs: None,
    };
    let (_, err, _) = wide(&mut view, &[started(Mode::Full), question], true);
    assert!(!err.contains("expires in"), "{err}");
}

#[test]
fn a_guard_question_with_an_exit_keeps_its_line_first() {
    let mut view = sandbox_view(false);
    let line = "false && rm -rf ~";
    let info = exit_info(&[ExitKind::Destructive], Launch::Unsandboxed);
    let events = [
        started(Mode::Full),
        requested(call(), line, &[ExitKind::Destructive], Vec::new()),
        Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "full mode still asks for this: delete of ~ (your home directory)\n\
                      shell: run \"false && rm -rf ~\""
                .to_owned(),
            diff_preview: None,
            interactive: false,
            exit: Some(info),
            expires_in_secs: Some(120),
        },
    ];
    let (_, err, _) = wide(&mut view, &events, true);
    let card = &err[err.find("? ").unwrap()..];
    let rows: Vec<&str> = card.lines().collect();
    assert_eq!(
        rows[1], "\u{2502} full mode still asks for this: delete of ~ (your home directory)",
        "{card}"
    );
    assert_eq!(card.matches("full mode still asks").count(), 1, "{card}");
    assert!(card.contains(line), "{card}");
    assert!(card.contains("\u{2502} expires in 2 min\n"), "{card}");
}

#[test]
fn the_status_row_of_a_full_turn_shows_full() {
    let mut view = TurnView::new(turn(), RenderOptions::new(100));
    view.start();
    let (out, _, _) = wide(&mut view, &[started(Mode::Full)], true);
    let rows: Vec<String> = bare(&out).lines().map(str::to_owned).collect();
    assert!(rows.iter().any(|row| row.ends_with("  full")), "{rows:?}");
    // A turn in another mode has no tag.
    let mut view = TurnView::new(turn(), RenderOptions::new(100));
    view.start();
    let (out, _, _) = wide(&mut view, &[started(Mode::Auto)], true);
    assert!(!bare(&out).contains("full"), "{}", readable(&out));
}
