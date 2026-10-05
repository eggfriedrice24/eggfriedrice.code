use std::path::PathBuf;

use efr_protocol::{
    ApprovalDecision, CallId, ErrorBody, ErrorCode, Event, InputWait, Origin, Scope, TurnId,
};
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Ask, Step, TurnEnd, TurnView, last_line};
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
    assert_eq!(step.ask, Some(Ask::Approval(call())));
    insta::assert_snapshot!(readable(&step.out));

    let step = view.answered(call(), ApprovalDecision::Deny, SIZE);
    insta::assert_snapshot!("answered", readable(&step.out));

    // The daemon's event for this client's own answer adds nothing.
    let step = view.event(&resolved(Origin::Shell), SIZE, true);
    assert_eq!(step, Step::default());
}

/// The end of the call of [`tool_started`] that never ran.
fn refused_call_completed() -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: "Error: The user denied the shell call; it did not run.".to_owned(),
        truncated: false,
        is_error: true,
        exit_code: None,
    }
}

#[test]
fn a_denied_call_is_not_reported_as_failed_too() {
    let mut view = terminal_view();
    view.event(&tool_started("touch note.txt"), SIZE, true);
    view.event(&approval(None), SIZE, true);
    view.answered(call(), ApprovalDecision::Deny, SIZE);
    assert_eq!(view.event(&refused_call_completed(), SIZE, true), Step::default());

    let mut view = terminal_view();
    view.event(&tool_started("touch note.txt"), SIZE, false);
    view.event(&approval(None), SIZE, false);
    let denied = Event::ApprovalResolved {
        turn_id: turn(),
        call_id: call(),
        decision: ApprovalDecision::Deny,
        origin: Origin::Phone,
    };
    assert!(readable(&view.event(&denied, SIZE, false).out).contains("denied from the phone"));
    assert_eq!(view.event(&refused_call_completed(), SIZE, false), Step::default());

    let mut view = terminal_view();
    view.event(&tool_started("touch note.txt"), SIZE, true);
    view.event(&approval(None), SIZE, true);
    view.event(&Event::ApprovalExpired { turn_id: turn(), call_id: call() }, SIZE, true);
    assert_eq!(view.event(&refused_call_completed(), SIZE, true), Step::default());
}

#[test]
fn an_allowed_call_that_fails_is_still_reported() {
    let mut view = terminal_view();
    view.event(&tool_started("touch /root/x"), SIZE, true);
    view.event(&approval(None), SIZE, true);
    view.answered(call(), ApprovalDecision::Allow, SIZE);
    let step = view.event(&refused_call_completed(), SIZE, true);
    assert!(readable(&step.out).contains("shell failed"), "{}", readable(&step.out));
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
    assert_eq!(step.ask, Some(Ask::Approval(call())));
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
    let started = Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/etc"),
        scope: Scope::Machine,
        settings: None,
    };
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

fn output(tail: &str) -> Event {
    Event::ToolCallOutputUpdated {
        turn_id: turn(),
        call_id: call(),
        tail: tail.to_owned(),
        bytes: tail.len() as u64,
    }
}

fn input(wait: InputWait) -> Event {
    Event::ToolCallInputChanged { turn_id: turn(), call_id: call(), input: wait }
}

fn call_completed(exit_code: i32) -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: "...".to_owned(),
        truncated: false,
        is_error: false,
        exit_code: Some(exit_code),
    }
}

/// Feeds events to a terminal view and joins the bytes of each write.
fn writes(view: &mut TurnView, events: &[Event], can_ask: bool) -> String {
    let writes: Vec<String> =
        events.iter().map(|event| readable(&view.event(event, SIZE, can_ask).out)).collect();
    writes.join("\n---\n")
}

#[test]
fn the_last_line_with_text_is_the_tail() {
    assert_eq!(last_line("one\ntwo\n\n  \n"), "two");
    assert_eq!(last_line("Proceed? [Y/n] "), "Proceed? [Y/n]");
    assert_eq!(last_line("\n \n"), "");
    assert_eq!(last_line(""), "");
}

#[test]
fn the_running_call_shows_its_last_line_live_until_it_completes() {
    let mut view = terminal_view();
    let shown = writes(
        &mut view,
        &[
            tool_started("sudo pacman -Syu"),
            output(":: Synchronizing package databases...\n core is up to date\n"),
            // A blank last line keeps the line before it, so nothing is redrawn.
            output(":: Synchronizing package databases...\n core is up to date\n\n"),
            // Two columns a character: cut at the width with an ellipsis.
            output(
                "\u{30d1}\u{30c3}\u{30b1}\u{30fc}\u{30b8}\u{3092}\u{53d6}\u{5f97}\u{3057}\u{3066}\u{3044}\u{307e}\u{3059}\u{3002}\u{304a}\u{5f85}\u{3061}\u{304f}\u{3060}\u{3055}\u{3044}\n",
            ),
            call_completed(0),
        ],
        false,
    );
    insta::assert_snapshot!(shown);
}

#[test]
fn a_wide_tail_takes_one_row_at_most() {
    let mut view = terminal_view();
    view.event(&tool_started("make"), SIZE, false);
    let line = "\u{6f22}".repeat(30);
    let out = view.event(&output(&line), SIZE, false).out;
    assert!(!out.contains(&line), "{}", readable(&out));
    assert!(out.contains('\u{2026}'), "{}", readable(&out));
    let live = out.rsplit('\n').nth(1).unwrap_or_default();
    assert!(crate::live::display_width(live) <= 40, "{}", readable(live));
}

#[test]
fn a_failed_call_commits_its_note_and_drops_the_tail() {
    let mut view = terminal_view();
    view.event(&tool_started("make"), SIZE, false);
    view.event(&output("error: no rule\n"), SIZE, false);
    let out = readable(&view.event(&call_completed(2), SIZE, false).out);
    assert_eq!(out, "\\e[?2026h\\r\\e[1A\\e[J\\e[2mshell exited with 2\\e[0m\n\\e[?2026l");
}

#[test]
fn a_tail_is_not_written_when_stdout_is_not_a_terminal() {
    let mut view = raw_view();
    view.event(&tool_started("make"), SIZE, false);
    assert_eq!(view.event(&output("building\n"), SIZE, false), Step::default());
    assert_eq!(view.event(&call_completed(0), SIZE, false), Step::default());
}

#[test]
fn a_hidden_input_asks_below_the_prompt_and_never_echoes() {
    let mut view = terminal_view();
    view.event(&tool_started("sudo pacman -Syu"), SIZE, true);
    view.event(&output("[sudo] password for egg: "), SIZE, true);
    let step = view.event(&input(InputWait::Hidden), SIZE, true);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), hidden: true }));
    insta::assert_snapshot!(readable(&step.out));
    // Text passed for a hidden answer would never be shown.
    assert_eq!(view.typed("hunter2", SIZE), Step::default());

    let out = readable(&view.answer_sent(SIZE).out);
    assert!(out.contains("answer sent"), "{out}");
    assert!(out.contains("it is not shown"), "the question stays for the next line: {out}");

    // sudo may ask again after a wrong password, so keys stay quiet until the call
    // completes.
    let step = view.event(&input(InputWait::None), SIZE, true);
    assert!(!step.settled);
    assert_eq!(step.ask, Some(Ask::Discard(call())));
    let out = readable(&step.out);
    assert!(!out.contains("type the answer"), "the question is gone: {out}");
    assert!(out.contains("[sudo] password for egg:"), "the tail stays: {out}");

    let step = view.event(&input(InputWait::Hidden), SIZE, true);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), hidden: true }));
    view.event(&input(InputWait::None), SIZE, true);
    let step = view.event(&call_completed(0), SIZE, true);
    assert!(step.settled, "the keys stop with the call");
    assert_eq!(step.ask, None);
}

#[test]
fn a_visible_input_that_ends_settles_at_once() {
    let mut view = terminal_view();
    view.event(&tool_started("pacman -Syu"), SIZE, true);
    view.event(&input(InputWait::Visible), SIZE, true);
    let step = view.event(&input(InputWait::None), SIZE, true);
    assert!(step.settled);
    assert_eq!(step.ask, None);
}

#[test]
fn a_visible_input_echoes_what_is_typed_until_it_is_sent() {
    let mut view = terminal_view();
    view.event(&tool_started("sudo pacman -Syu"), SIZE, true);
    view.event(&output(":: Proceed with installation? [Y/n] "), SIZE, true);
    let step = view.event(&input(InputWait::Visible), SIZE, true);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), hidden: false }));
    let typed = view.typed("y", SIZE);
    insta::assert_snapshot!(readable(&typed.out));

    let sent = readable(&view.answer_sent(SIZE).out);
    assert!(sent.contains("answer sent"), "{sent}");
    assert!(!sent.contains("> y"), "the echo goes with the send: {sent}");
}

#[test]
fn a_completed_call_settles_its_input_and_drops_the_question() {
    let mut view = terminal_view();
    view.event(&tool_started("sudo true"), SIZE, true);
    view.event(&output("[sudo] password for egg: "), SIZE, true);
    view.event(&input(InputWait::Hidden), SIZE, true);
    let step = view.event(&call_completed(1), SIZE, true);
    assert!(step.settled);
    let out = readable(&step.out);
    assert!(!out.contains("password") && !out.contains("type the answer"), "{out}");
    assert!(out.contains("shell exited with 1"), "{out}");
}

#[test]
fn a_refused_answer_is_a_note() {
    let mut view = terminal_view();
    view.event(&input(InputWait::Hidden), SIZE, true);
    let out = readable(&view.answer_refused(SIZE).out);
    assert!(out.contains("the command no longer waits"), "{out}");
    let out = readable(&view.answer_failed("boom \u{1b}[2J", SIZE).out);
    assert!(out.contains("the answer was not sent: boom"), "{out}");
    assert!(!out.contains("\\e[2J"), "{out}");
}

#[test]
fn without_keys_a_wait_is_one_note() {
    let mut view = raw_view();
    let step = view.event(&input(InputWait::Hidden), SIZE, false);
    assert_eq!(step.ask, None);
    assert_eq!(
        step.err,
        "the command waits for hidden input, such as a password; efr cannot ask for it here\n"
    );
    let step = view.event(&input(InputWait::None), SIZE, false);
    assert_eq!(step, Step::default());
    let step = view.event(&input(InputWait::Visible), SIZE, false);
    assert_eq!(step.err, "the command waits for input; efr cannot ask for it here\n");
}

#[test]
fn a_raw_view_asks_on_stderr() {
    let mut view = raw_view();
    view.event(&output("Password: "), SIZE, true);
    let step = view.event(&input(InputWait::Hidden), SIZE, true);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), hidden: true }));
    assert_eq!(step.out, "");
    assert_eq!(
        step.err,
        "Password:\ntype the answer and press Enter; it is not shown, and the agent sees it only if the \
         program prints it\n"
    );
    assert_eq!(view.typed("visible?", SIZE), Step::default());
}

#[test]
fn a_raw_view_echoes_a_visible_answer_on_stderr_where_backspace_erases() {
    let mut view = raw_view();
    view.event(&output("Proceed? [Y/n] "), SIZE, true);
    let step = view.event(&input(InputWait::Visible), SIZE, true);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), hidden: false }));
    assert_eq!(step.out, "");
    assert_eq!(step.err, "Proceed? [Y/n]\ntype the answer and press Enter; the agent sees it\n> ");
    assert_eq!(view.typed("y", SIZE).err, "y");
    assert_eq!(view.typed("ye", SIZE).err, "e");
    assert_eq!(view.typed("y", SIZE).err, "\u{8} \u{8}");
    // A wide character takes two columns, and Ctrl+U erases every column.
    assert_eq!(view.typed("y\u{6f22}", SIZE).err, "\u{6f22}");
    assert_eq!(view.typed("", SIZE).err, "\u{8} \u{8}".repeat(3));
    view.typed("n", SIZE);
    let sent = view.answer_sent(SIZE);
    assert_eq!((sent.out.as_str(), sent.err.as_str()), ("", "\nanswer sent\n"));
    // Another answer to the same question starts a line of its own.
    assert_eq!(view.typed("y", SIZE).err, "> y");
    // And a line left unsent ends before anything that comes after the view.
    assert_eq!(view.event(&turn_completed(), SIZE, true).err, "\n");
}

#[test]
fn an_input_does_not_ask_over_a_pending_approval() {
    let mut view = terminal_view();
    view.event(&approval(None), SIZE, true);
    let step = view.event(&input(InputWait::Hidden), SIZE, true);
    assert_eq!(step.ask, None);
    assert!(!step.settled, "the approval keeps its keys");
}

#[test]
fn the_end_of_the_turn_settles_an_input() {
    let mut view = terminal_view();
    view.event(&input(InputWait::Visible), SIZE, true);
    let step = view.event(&Event::TurnInterrupted { turn_id: turn() }, SIZE, true);
    assert!(step.settled);
    assert!(!readable(&step.out).contains("type the answer"));
}

#[test]
fn a_queued_view_asks_for_the_input_of_the_running_turn_until_its_own_turn_starts() {
    let mut view = terminal_view();
    view.queue();
    let running: TurnId = "0192f0c1-7a00-7000-8000-000000000077".parse().unwrap();
    let other: CallId = "0192f0c1-7a00-7000-8000-000000000078".parse().unwrap();
    let wait =
        Event::ToolCallInputChanged { turn_id: running, call_id: other, input: InputWait::Hidden };
    let step = view.event(&wait, SIZE, true);
    assert_eq!(step.ask, Some(Ask::Input { call_id: other, hidden: true }));

    let started = Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/home/u"),
        scope: Scope::Machine,
        settings: None,
    };
    let step = view.event(&started, SIZE, true);
    assert!(step.settled, "the question of the turn ahead goes once this one runs");
    assert!(!readable(&step.out).contains("type the answer"));

    // From now on another turn's waits are not this view's.
    assert_eq!(view.event(&wait, SIZE, true), Step::default());
}
