//! A command that a shell call left running in the hidden shell: the call's block, the
//! answer line of its waits with the prompt row, the notes of its end, and the line at
//! the end of the turn.

use efr_protocol::{ApprovedBy, Event, InputWait, Launch, ShellCommandEnd};
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::{AnswerKind, Ask, Step, TurnView};
use super::{feed, raw_view, turn_completed};
use crate::terminal::Size;
use crate::testing::{bare, call, now, readable, turn};

/// The line of the incident: an upload that asks for a password on the terminal.
const RSYNC: &str =
    "rsync -a -e 'ssh -o PreferredAuthentications=password' u@h:.config/Slack ./slack";

const WIDE: Size = Size { cols: 160, rows: 30 };

fn wide_terminal() -> TurnView {
    TurnView::new(turn(), RenderOptions::new(WIDE.cols).with_colour(ColourMode::None))
}

fn wide_raw() -> TurnView {
    TurnView::new(turn(), RenderOptions::new(WIDE.cols).with_terminal(false))
}

fn started(command: &str, contained: bool) -> Event {
    Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input: json!({ "command": command }),
        manual_input: true,
        launch: contained.then(Launch::contained),
        freeform: false,
    }
}

fn output(tail: &str) -> Event {
    Event::ToolCallOutputUpdated {
        turn_id: turn(),
        call_id: call(),
        tail: tail.to_owned(),
        bytes: tail.len() as u64,
    }
}

/// The call answered the model while its command still runs.
fn left_completed() -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: "receiving file list ...\nu@h's password: ".to_owned(),
        truncated: false,
        is_error: false,
        exit_code: None,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: true,
    }
}

fn detached(command: &str, input: InputWait, contained: bool, prompt: &str) -> Event {
    Event::ShellCommandDetached {
        call_id: call(),
        command: command.to_owned(),
        since: now(),
        contained,
        input,
        looks_secret: false,
        prompt: prompt.to_owned(),
        approved_by: (!contained).then_some(ApprovedBy::You),
    }
}

fn input_changed(input: InputWait, contained: bool, looks_secret: bool, prompt: &str) -> Event {
    Event::ShellCommandInputChanged {
        call_id: call(),
        input,
        contained,
        looks_secret,
        prompt: prompt.to_owned(),
    }
}

fn ended(how: ShellCommandEnd, exit_code: Option<i32>) -> Event {
    Event::ShellCommandEnded { call_id: call(), exit_code, how }
}

/// `step` with the frame of a wide screen that shows it in its `out`.
fn wide(mut step: Step, view: &mut TurnView) -> Step {
    step.out.push_str(&view.frame(WIDE, now()));
    step
}

/// Feeds `events` to a wide view and returns the last step, framed.
fn last_step(view: &mut TurnView, events: &[Event], can_ask: bool) -> Step {
    let mut last = Step::default();
    for event in events {
        last = wide(view.event(event, WIDE, can_ask), view);
    }
    last
}

#[test]
fn a_timed_out_call_with_left_running_says_it_still_runs() {
    let mut view = wide_raw();
    let (_, err, _) = feed(
        &mut view,
        &[started(RSYNC, false), output("u@h's password: "), left_completed()],
        false,
    );
    assert!(!err.contains('\u{2713}'), "no check mark: the command did not end: {err}");
    insta::assert_snapshot!(err);

    // On a terminal the result is in the warning role.
    let mut view = TurnView::new(turn(), RenderOptions::new(WIDE.cols));
    let (out, _, _) = feed(&mut view, &[started(RSYNC, false), left_completed()], false);
    assert!(readable(&out).contains("33mstill runs in the hidden shell"), "{}", readable(&out));
}

#[test]
fn a_hidden_wait_of_a_left_command_asks_below_its_prompt_row() {
    let mut view = wide_terminal();
    let step = last_step(
        &mut view,
        &[
            started(RSYNC, false),
            left_completed(),
            detached(RSYNC, InputWait::Hidden, false, "u@h's password: "),
        ],
        true,
    );
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Hidden }));
    assert_eq!(view.left_keys(), Some(call()));
    // A hidden answer handed to the view by mistake is dropped, never drawn.
    assert_eq!(wide(view.typed("hunter2"), &mut view), Step::default());
    insta::assert_snapshot!(bare(&step.out));
}

#[test]
fn a_contained_left_command_asks_only_for_a_plain_visible_wait() {
    let mut view = wide_terminal();
    let step = last_step(
        &mut view,
        &[
            started("./setup.sh", true),
            detached("./setup.sh", InputWait::Visible, true, "Go on? [y/N] "),
        ],
        true,
    );
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Visible }));
    let typed = wide(view.typed("y"), &mut view);
    insta::assert_snapshot!(bare(&format!("{}{}", step.out, typed.out)));

    // A wait that may be for a secret is a note, and never asks.
    for (input, looks_secret) in [(InputWait::Hidden, false), (InputWait::Visible, true)] {
        let step =
            last_step(&mut view, &[input_changed(input, true, looks_secret, "Password: ")], true);
        assert_eq!(step.ask, None, "{input:?}");
        if input == InputWait::Hidden {
            assert!(step.settled, "the line of the plain wait goes");
        }
        assert!(
            bare(&step.out).contains("efr does not type secrets into the sandbox"),
            "{}",
            bare(&step.out)
        );
        assert_eq!(view.left_keys(), None);
    }
}

#[test]
fn the_end_line_says_password_only_outside_the_sandbox() {
    let cases = [
        ("outside, hidden, keys", InputWait::Hidden, false, true),
        ("outside, hidden, no keys", InputWait::Hidden, false, false),
        ("sandbox, visible, no keys", InputWait::Visible, true, false),
        ("sandbox, visible, keys", InputWait::Visible, true, true),
        ("outside, no wait", InputWait::None, false, true),
    ];
    let mut shots = Vec::new();
    for (name, input, contained, can_ask) in cases {
        let mut view = wide_raw();
        let command = if contained { "./setup.sh" } else { RSYNC };
        let (_, err, end) = feed(
            &mut view,
            &[
                started(command, contained),
                detached(command, input, contained, "u@h's password: "),
                left_completed(),
                turn_completed(),
            ],
            can_ask,
        );
        assert!(end.is_some(), "{name}");
        let kept = view.lingers();
        let tail = err.rsplit_once("\n\n").map_or(err.as_str(), |(_, tail)| tail);
        shots.push(format!("[{name}; efr stays: {kept}]\n{tail}"));
    }
    insta::assert_snapshot!(shots.join("\n"));
}

#[test]
fn a_kept_wait_at_the_end_keeps_the_answer_line_until_esc() {
    let mut view = wide_terminal();
    let events = [
        started(RSYNC, false),
        detached(RSYNC, InputWait::Hidden, false, "u@h's password: "),
        left_completed(),
    ];
    last_step(&mut view, &events, true);
    let step = wide(view.event(&turn_completed(), WIDE, true), &mut view);
    assert!(step.end.is_some());
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Hidden }));
    assert!(view.lingers());
    let shown = bare(&step.out);
    assert!(shown.contains("the hidden shell still runs: rsync"), "{shown}");
    assert!(shown.contains("(it waits for a password)"), "{shown}");
    assert!(shown.contains("enter answers \u{b7} ctrl+c stops it \u{b7} esc leaves it"), "{shown}");

    let left = wide(view.left_leave(call(), WIDE).unwrap(), &mut view);
    assert!(left.settled);
    assert!(!view.lingers());
    assert!(bare(&left.out).contains(",shell answers or stops it"), "{}", bare(&left.out));
    assert_eq!(view.left_leave(call(), WIDE), None, "once");
}

#[test]
fn a_kept_wait_that_ends_lets_efr_end() {
    let mut view = wide_terminal();
    let events = [
        started(RSYNC, false),
        left_completed(),
        detached(RSYNC, InputWait::Hidden, false, "u@h's password: "),
        turn_completed(),
    ];
    last_step(&mut view, &events, true);
    assert!(view.lingers());
    let step = last_step(&mut view, &[input_changed(InputWait::None, false, false, "")], true);
    assert!(!view.lingers());
    // It asked for a password: the keys typed after it are thrown away until efr ends.
    assert_eq!(step.ask, Some(Ask::Discard(call())));
}

#[test]
fn each_way_of_an_end_is_a_note_and_the_line_goes() {
    let mut shots = Vec::new();
    for how in ShellCommandEnd::ALL {
        let mut view = wide_raw();
        let events =
            [started(RSYNC, false), detached(RSYNC, InputWait::Visible, false, "Continue? [y/N] ")];
        let asked = last_step(&mut view, &events, true);
        assert!(asked.ask.is_some());
        let exit = (how == ShellCommandEnd::Ended).then_some(23);
        let step = last_step(&mut view, &[ended(how, exit)], true);
        assert!(step.settled, "{how:?}: the keys stop");
        assert_eq!(view.left_keys(), None);
        shots.push(format!("[{}] {}", how.as_str(), step.err.trim()));
    }
    // A way that this build does not know still frees the line.
    let unknown: Event = serde_json::from_value(json!({
        "kind": "shell_command_ended",
        "call_id": call(),
        "how": "teleported",
    }))
    .unwrap();
    let mut view = wide_raw();
    last_step(&mut view, &[detached(RSYNC, InputWait::Visible, false, "? ")], true);
    let step = last_step(&mut view, &[unknown], true);
    assert!(step.settled);
    shots.push(format!("[teleported] {}", step.err.trim()));
    insta::assert_snapshot!(shots.join("\n"));
}

#[test]
fn the_detach_and_the_completion_come_in_either_order() {
    let wait = || detached(RSYNC, InputWait::Hidden, false, "u@h's password: ");
    let mut first = wide_raw();
    let (_, detach_first, _) = feed(
        &mut first,
        &[started(RSYNC, false), output("u@h's password: "), wait(), left_completed()],
        true,
    );
    let mut second = wide_raw();
    let (_, complete_first, _) = feed(
        &mut second,
        &[started(RSYNC, false), output("u@h's password: "), left_completed(), wait()],
        true,
    );
    for (name, err, view) in
        [("detach first", &detach_first, &first), ("completion first", &complete_first, &second)]
    {
        assert!(err.contains("still runs in the hidden shell"), "{name}: {err}");
        assert!(err.contains("the hidden shell: rsync"), "{name}: {err}");
        assert!(err.contains("u@h's password:"), "{name}: {err}");
        assert_eq!(view.left_keys(), Some(call()), "{name}");
    }
}

#[test]
fn a_completion_first_holds_the_keys_of_a_call_that_may_ask_for_a_password() {
    let mut view = wide_terminal();
    let approval = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: RSYNC.to_owned(),
        diff_preview: None,
        interactive: true,
        exit: None,
        expires_in_secs: None,
    };
    last_step(&mut view, &[started(RSYNC, false), approval], true);
    view.answered(call(), efr_protocol::ApprovalDecision::Allow, WIDE);
    let step = last_step(&mut view, &[left_completed()], true);
    assert!(step.settled, "the keys kept for the call go");
    // The follow loop asks the view what takes the keys next: they wait for the wait.
    let offer = view.left_offer().unwrap();
    assert_eq!(offer.ask, Some(Ask::Discard(call())));
    let step = last_step(&mut view, &[detached(RSYNC, InputWait::Hidden, false, "pw: ")], true);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Hidden }));
}

#[test]
fn a_command_and_a_prompt_with_escape_sequences_show_flat() {
    let command = "printf '\x1b]52;c;aGVsbG8=\x07'; read -r x";
    let prompt = "\x1b]52;c;aGVsbG8=\x07ok? \x1b[2J";
    let mut view = wide_raw();
    let (_, err, _) = feed(
        &mut view,
        &[detached(command, InputWait::Visible, false, prompt), turn_completed()],
        false,
    );
    assert!(!err.contains('\x1b') && !err.contains('\x07'), "{err:?}");
    assert!(err.contains("52;c;aGVsbG8="), "{err:?}");

    let mut view = wide_terminal();
    let step = last_step(&mut view, &[detached(command, InputWait::Visible, false, prompt)], true);
    assert!(step.ask.is_some());
    let shown = step.out.replace("\x1b[", "");
    assert!(!shown.contains("\x1b]") && !shown.contains('\x07'), "{:?}", step.out);
}

#[test]
fn without_keys_a_wait_of_a_left_command_is_a_note() {
    let mut view = raw_view();
    let (_, err, _) = feed(&mut view, &[detached(RSYNC, InputWait::Hidden, false, "pw: ")], false);
    assert!(err.contains("efr cannot ask for it here"), "{err}");
    assert!(err.contains("waits for a password"), "{err}");
    assert_eq!(view.left_keys(), None);
}

/// With no call running, Ctrl+B moves the command that a call left running in the
/// hidden shell, by its call's id, while that command waits for nothing (OQS16).
#[test]
fn ctrl_b_moves_a_left_command_that_waits_for_nothing() {
    let mut view = wide_terminal();
    let events = [
        started("cargo build --release", false),
        left_completed(),
        detached("cargo build --release", InputWait::None, false, ""),
    ];
    last_step(&mut view, &events, true);
    assert_eq!(view.background_call(), Some(call()));
    // A wait must be answered first; the move would be refused.
    last_step(&mut view, &[input_changed(InputWait::Visible, false, false, "[y/N] ")], true);
    assert_eq!(view.background_call(), None);
    last_step(&mut view, &[input_changed(InputWait::None, false, false, "")], true);
    assert_eq!(view.background_call(), Some(call()));
    // A command that moved, or ended, moves no more.
    view.moved(call(), efr_protocol::JobId::nth(1));
    assert_eq!(view.background_call(), None);
    let mut view = wide_terminal();
    last_step(&mut view, &events, true);
    last_step(&mut view, &[ended(ShellCommandEnd::Moved, None)], true);
    assert_eq!(view.background_call(), None);
}
