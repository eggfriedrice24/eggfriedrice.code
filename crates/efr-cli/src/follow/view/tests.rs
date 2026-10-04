use std::path::PathBuf;

use efr_protocol::{ApprovalDecision, ErrorBody, ErrorCode, Event, Origin, Scope, TurnId};
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Step, TurnEnd, TurnView};
use crate::terminal::Size;
use crate::testing::{call, readable, turn};

const SIZE: Size = Size { cols: 40, rows: 20 };

fn terminal_view() -> TurnView {
    TurnView::new(turn(), RenderOptions::new(40))
}

fn raw_view() -> TurnView {
    TurnView::new(turn(), RenderOptions::new(40).with_terminal(false))
}

/// The first update of message `index`.
fn updated(index: u32, text: &str) -> Event {
    Event::AssistantMessageUpdated { turn_id: turn(), index, offset: 0, delta: text.to_owned() }
}

/// An update of message `index` that grows its text from `before` to `after`.
fn grown(index: u32, before: &str, after: &str) -> Event {
    Event::AssistantMessageUpdated {
        turn_id: turn(),
        index,
        offset: before.len() as u64,
        delta: after.strip_prefix(before).unwrap().to_owned(),
    }
}

fn completed(index: u32, text: &str) -> Event {
    Event::AssistantMessageCompleted { turn_id: turn(), index, text: text.to_owned() }
}

fn tool_started(command: &str) -> Event {
    Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input: json!({ "command": command }),
    }
}

fn approval(diff: Option<&str>) -> Event {
    Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "write ~/.zshrc".to_owned(),
        diff_preview: diff.map(str::to_owned),
    }
}

fn resolved(origin: Origin) -> Event {
    Event::ApprovalResolved {
        turn_id: turn(),
        call_id: call(),
        decision: ApprovalDecision::Allow,
        origin,
    }
}

fn turn_completed() -> Event {
    Event::TurnCompleted { turn_id: turn(), usage: None }
}

/// Feeds events and joins what they wrote, with the end of the turn.
fn feed(view: &mut TurnView, events: &[Event], can_ask: bool) -> (String, String, Option<TurnEnd>) {
    let (mut out, mut err, mut end) = (String::new(), String::new(), None);
    for event in events {
        let step = view.event(event, SIZE, can_ask);
        out.push_str(&step.out);
        err.push_str(&step.err);
        end = end.or(step.end);
    }
    (out, err, end)
}

#[test]
fn raw_output_is_the_markdown_as_it_streams() {
    let mut view = raw_view();
    let (out, err, end) = feed(
        &mut view,
        &[
            updated(0, "It failed "),
            grown(0, "It failed ", "It failed because **make**"),
            completed(0, "It failed because **make** ran out of memory."),
            tool_started("free -h"),
            updated(1, "Add swap:\n"),
            completed(1, "Add swap:\n\n```sh\nswapon -a\n```\n"),
            turn_completed(),
        ],
        false,
    );
    assert_eq!(
        out,
        "It failed because **make** ran out of memory.\n\nAdd swap:\n\n```sh\nswapon -a\n```\n"
    );
    assert_eq!(err, "shell: free -h\n");
    assert_eq!(end, Some(TurnEnd::Completed));
}

#[test]
fn a_terminal_reply_commits_complete_blocks_and_redraws_the_live_zone() {
    let mut view = terminal_view();
    let mut writes = Vec::new();
    for event in [
        updated(0, "# Plan"),
        grown(0, "# Plan", "# Plan\n\nFirst we"),
        grown(0, "# Plan\n\nFirst we", "# Plan\n\nFirst we check the logs.\n\n- one"),
        completed(0, "# Plan\n\nFirst we check the logs.\n\n- one\n- two\n"),
        turn_completed(),
    ] {
        writes.push(readable(&view.event(&event, SIZE, false).out));
    }
    insta::assert_snapshot!(writes.join("\n---\n"));
}

#[test]
fn notes_sit_between_messages_and_dim() {
    let mut view = terminal_view();
    let (out, err, _) = feed(
        &mut view,
        &[
            completed(0, "Checking."),
            tool_started("journalctl -u nginx --since today"),
            Event::ToolCallCompleted {
                turn_id: turn(),
                call_id: call(),
                output: "...".to_owned(),
                truncated: false,
                is_error: false,
                exit_code: Some(3),
            },
            completed(1, "It exited with 3."),
        ],
        false,
    );
    assert_eq!(err, "");
    insta::assert_snapshot!(readable(&out));
}

#[test]
fn a_long_trace_is_cut_to_the_screen_width() {
    let mut view = terminal_view();
    let step = view.event(&tool_started(&"x".repeat(100)), SIZE, false);
    assert!(step.out.contains('\u{2026}'), "{}", readable(&step.out));
}

#[test]
fn an_approval_asks_below_the_live_zone_when_keys_can_be_read() {
    let mut view = terminal_view();
    let step = view.event(&approval(Some("-a\n+b\n")), SIZE, true);
    assert_eq!(step.ask, Some(call()));
    insta::assert_snapshot!(readable(&step.out));

    let step = view.answered(call(), ApprovalDecision::Deny, SIZE);
    insta::assert_snapshot!("answered", readable(&step.out));

    // The daemon's event for this client's own answer adds nothing.
    let step = view.event(&resolved(Origin::Shell), SIZE, true);
    assert_eq!(step, Step::default());
}

#[test]
fn without_keys_an_approval_waits_for_another_client() {
    let mut view = terminal_view();
    let step = view.event(&approval(None), SIZE, false);
    assert_eq!(step.ask, None);
    assert!(readable(&step.out).contains("waiting for another client to answer"));
    let step = view.event(&resolved(Origin::Phone), SIZE, false);
    assert!(!step.settled);
    assert!(readable(&step.out).contains("allowed from the phone"));
}

#[test]
fn an_answer_from_elsewhere_settles_the_question() {
    let mut view = terminal_view();
    view.event(&approval(None), SIZE, true);
    let step = view.event(&resolved(Origin::Phone), SIZE, true);
    assert!(step.settled);
    let out = readable(&step.out);
    assert!(out.contains("allowed from the phone"), "{out}");
    assert!(!out.contains("allow? y"), "the question is gone: {out}");
}

#[test]
fn an_expired_approval_settles_the_question() {
    let mut view = terminal_view();
    view.event(&approval(None), SIZE, true);
    let step = view.event(&Event::ApprovalExpired { turn_id: turn(), call_id: call() }, SIZE, true);
    assert!(step.settled);
    assert!(readable(&step.out).contains("the approval expired"));
}

#[test]
fn a_raw_approval_goes_to_stderr_with_the_question() {
    let mut view = raw_view();
    let step = view.event(&approval(Some("-a\n+b")), SIZE, true);
    assert_eq!(step.out, "");
    assert_eq!(step.err, "approval needed: write ~/.zshrc\n-a\n+b\nallow? y = yes, n = no\n");
    assert_eq!(step.ask, Some(call()));
}

#[test]
fn approval_summaries_cannot_drive_the_terminal() {
    let mut view = terminal_view();
    let event = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "run \u{1b}]52;c;cGF5bG9hZA==\u{7}".to_owned(),
        diff_preview: None,
    };
    let out = view.event(&event, SIZE, false).out;
    assert!(!out.contains("\u{1b}]52"), "{}", readable(&out));
}

#[test]
fn events_of_other_turns_change_nothing() {
    let mut view = terminal_view();
    let other: TurnId = "019a9b1c-3d00-7a10-8b20-0000000000ff".parse().unwrap();
    let event = Event::AssistantMessageUpdated {
        turn_id: other,
        index: 0,
        offset: 0,
        delta: "x".to_owned(),
    };
    assert_eq!(view.event(&event, SIZE, true), Step::default());
    let started =
        Event::TurnStarted { turn_id: turn(), cwd: PathBuf::from("/etc"), scope: Scope::Machine };
    assert_eq!(view.event(&started, SIZE, true), Step::default());
}

#[test]
fn a_failed_turn_commits_what_arrived_and_ends() {
    let mut view = terminal_view();
    view.event(&updated(0, "Half a line"), SIZE, false);
    let error = ErrorBody::new(ErrorCode::Internal, "the provider is down");
    let step =
        view.event(&Event::TurnFailed { turn_id: turn(), error: error.clone() }, SIZE, false);
    assert_eq!(step.end, Some(TurnEnd::Failed(error)));
    assert_eq!(readable(&step.out), "\\e[?2026h\\r\\e[1A\\e[JHalf a line\n\\e[?2026l");
}

#[test]
fn an_interrupted_turn_says_so() {
    let mut view = raw_view();
    let step = view.event(&Event::TurnInterrupted { turn_id: turn() }, SIZE, false);
    assert_eq!(step.end, Some(TurnEnd::Interrupted));
    assert_eq!(step.err, "interrupted\n");
}

#[test]
fn a_late_update_of_a_completed_message_is_ignored() {
    let mut view = raw_view();
    feed(&mut view, &[completed(0, "done")], false);
    assert_eq!(view.event(&updated(0, "done and more"), SIZE, false), Step::default());
}

#[test]
fn a_message_that_rewrites_sent_text_keeps_what_was_shown() {
    let mut view = raw_view();
    let (out, _, _) = feed(
        &mut view,
        &[updated(0, "Hello wor"), updated(0, "Goodbye"), completed(0, "Goodbye")],
        false,
    );
    assert_eq!(out, "Hello wor\n");
}

#[test]
fn a_new_index_finishes_the_previous_message() {
    let mut view = raw_view();
    let (out, _, _) = feed(&mut view, &[updated(0, "first"), updated(1, "second")], false);
    assert_eq!(out, "first\n\nsecond");
}

#[test]
fn close_commits_the_live_text() {
    let mut view = terminal_view();
    view.event(&updated(0, "partial"), SIZE, false);
    let step = view.close(SIZE);
    assert_eq!(readable(&step.out), "\\e[?2026h\\r\\e[1A\\e[Jpartial\n\\e[?2026l");
    assert_eq!(view.close(SIZE), Step::default());
}

#[test]
fn without_colour_the_reply_keeps_its_styles_but_no_colours() {
    let mut view = TurnView::new(turn(), RenderOptions::new(40).with_colour(ColourMode::None));
    let (out, _, _) = feed(&mut view, &[completed(0, "# Title"), approval(None)], false);
    assert!(!out.contains("\x1b[1;33m") && !out.contains("\x1b[1;35m"), "{}", readable(&out));
}

#[test]
fn an_update_past_what_the_view_holds_waits_for_the_completed_text() {
    let mut view = raw_view();
    // The view joined after the message's first update.
    let (out, _, _) = feed(
        &mut view,
        &[
            grown(0, "Half ", "Half a line"),
            grown(0, "Half a line", "Half a line and more"),
            completed(0, "Half a line and more."),
        ],
        false,
    );
    assert_eq!(out, "Half a line and more.\n");
}

#[test]
fn a_tool_call_completes_the_message_before_it() {
    let mut view = raw_view();
    let (out, err, _) = feed(
        &mut view,
        &[
            updated(0, "Let me check"),
            tool_started("df -h"),
            grown(0, "Let me check", "Let me check more"),
        ],
        false,
    );
    assert_eq!(out, "Let me check\n");
    assert_eq!(err, "shell: df -h\n");
}

#[test]
fn on_a_terminal_the_message_is_committed_above_the_tool_call() {
    let mut view = terminal_view();
    view.event(&updated(0, "Let me check"), SIZE, false);
    let out = readable(&view.event(&tool_started("df -h"), SIZE, false).out);
    assert_eq!(out, "\\e[?2026h\\r\\e[1A\\e[JLet me check\n\n\\e[2mshell: df -h\\e[0m\n\\e[?2026l");
}

#[test]
fn an_approval_completes_the_message_before_it() {
    let mut view = raw_view();
    let (out, err, _) = feed(&mut view, &[updated(0, "I will edit it."), approval(None)], false);
    assert_eq!(out, "I will edit it.\n");
    assert!(err.starts_with("approval needed: write ~/.zshrc\n"), "{err}");
}
