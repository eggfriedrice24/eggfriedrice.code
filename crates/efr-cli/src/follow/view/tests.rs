use std::path::PathBuf;

use efr_protocol::{
    ApprovalDecision, BlockReason, Blocked, CallId, Draft, DraftPart, EffectiveSettings, ErrorBody,
    ErrorCode, Event, EventEnvelope, ExitFacts, ExitInfo, ExitKind, ExitSource, Grant, InputWait,
    Launch, Mode, ModeFallback, Origin, OverriddenSettings, PathClassName, QuestionId,
    ReportedFile, SandboxSummary, Scope, Seq, SurfaceChange, TargetFact, TurnId, Usage,
};
use efr_render::{ColourMode, RenderOptions};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{AnswerKind, Ask, ECHO_PREFIX, Look, Step, TurnEnd, TurnView, last_line};
use crate::progress;
use crate::terminal::Size;
use crate::testing::{
    FAILED_UNITS, FROM_SRC, call, exit_info, exit_record, now, program_fact, readable, turn,
};

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
        manual_input: true,
        launch: None,
    }
}

fn approval(diff: Option<&str>) -> Event {
    Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "write ~/.zshrc".to_owned(),
        diff_preview: diff.map(str::to_owned),
        interactive: false,
        exit: None,
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

/// `step` with the frame that shows it, at a time that never moves, in its `out`.
fn framed(mut step: Step, view: &mut TurnView) -> Step {
    step.out.push_str(&view.frame(SIZE, now()));
    step
}

/// Feeds events and joins what they wrote, with the end of the turn.
fn feed(view: &mut TurnView, events: &[Event], can_ask: bool) -> (String, String, Option<TurnEnd>) {
    let (mut out, mut err, mut end) = (String::new(), String::new(), None);
    for event in events {
        let step = framed(view.event(event, SIZE, can_ask), view);
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
    assert_eq!(err, "$ free -h\n");
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
        writes.push(readable(&framed(view.event(&event, SIZE, false), &mut view).out));
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
                sandbox: None,
                refusal: None,
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
    let step = framed(view.event(&tool_started(&"x".repeat(100)), SIZE, false), &mut view);
    assert!(step.out.contains('\u{2026}'), "{}", readable(&step.out));
}

#[test]
fn an_approval_asks_below_the_live_zone_when_keys_can_be_read() {
    let mut view = terminal_view();
    let step = framed(view.event(&approval(Some("-a\n+b\n")), SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Approval(call())));
    insta::assert_snapshot!(readable(&step.out));

    let step = framed(view.answered(call(), ApprovalDecision::Deny, SIZE), &mut view);
    insta::assert_snapshot!("answered", readable(&step.out));

    // The daemon's event for this client's own answer adds nothing.
    let step = framed(view.event(&resolved(Origin::Shell), SIZE, true), &mut view);
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
        sandbox: None,
        refusal: None,
    }
}

#[test]
fn a_denied_call_is_not_reported_as_failed_too() {
    let mut view = terminal_view();
    framed(view.event(&tool_started("touch note.txt"), SIZE, true), &mut view);
    framed(view.event(&approval(None), SIZE, true), &mut view);
    framed(view.answered(call(), ApprovalDecision::Deny, SIZE), &mut view);
    assert_eq!(
        framed(view.event(&refused_call_completed(), SIZE, true), &mut view),
        Step::default()
    );

    let mut view = terminal_view();
    framed(view.event(&tool_started("touch note.txt"), SIZE, false), &mut view);
    framed(view.event(&approval(None), SIZE, false), &mut view);
    let denied = Event::ApprovalResolved {
        turn_id: turn(),
        call_id: call(),
        decision: ApprovalDecision::Deny,
        origin: Origin::Phone,
    };
    assert!(
        readable(&framed(view.event(&denied, SIZE, false), &mut view).out)
            .contains("denied from the phone")
    );
    assert_eq!(
        framed(view.event(&refused_call_completed(), SIZE, false), &mut view),
        Step::default()
    );

    let mut view = terminal_view();
    framed(view.event(&tool_started("touch note.txt"), SIZE, true), &mut view);
    framed(view.event(&approval(None), SIZE, true), &mut view);
    framed(
        view.event(&Event::ApprovalExpired { turn_id: turn(), call_id: call() }, SIZE, true),
        &mut view,
    );
    assert_eq!(
        framed(view.event(&refused_call_completed(), SIZE, true), &mut view),
        Step::default()
    );
}

#[test]
fn a_call_that_efr_refused_says_why() {
    let refused = Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: "Permission denied for the shell call: ...".to_owned(),
        truncated: false,
        is_error: true,
        exit_code: None,
        sandbox: None,
        refusal: Some("efr's config (floor)".to_owned()),
    };
    let contained = Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input: json!({ "command": "echo x >> ~/.config/efr/config.toml" }),
        manual_input: true,
        launch: Some(Launch::contained()),
    };
    let mut view = raw_view();
    let (_, err, _) = feed(&mut view, &[contained, refused], false);
    assert_eq!(
        err,
        "$ echo x >> ~/.config/efr/config.toml\n\
         sandbox: writes in $SCRATCH, private /tmp; no network\n\
         shell refused: efr's config (floor)\n"
    );
}

#[test]
fn an_allowed_call_that_fails_is_still_reported() {
    let mut view = terminal_view();
    framed(view.event(&tool_started("touch /root/x"), SIZE, true), &mut view);
    framed(view.event(&approval(None), SIZE, true), &mut view);
    framed(view.answered(call(), ApprovalDecision::Allow, SIZE), &mut view);
    let step = framed(view.event(&refused_call_completed(), SIZE, true), &mut view);
    let out = readable(&step.out);
    assert!(out.contains("$ touch /root/x\\e[0m  \\e[31mfailed"), "{out}");
}

#[test]
fn without_keys_an_approval_waits_for_another_client() {
    let mut view = terminal_view();
    let step = framed(view.event(&approval(None), SIZE, false), &mut view);
    assert_eq!(step.ask, None);
    assert!(readable(&step.out).contains("waiting for another client to answer"));
    let step = framed(view.event(&resolved(Origin::Phone), SIZE, false), &mut view);
    assert!(!step.settled);
    assert!(readable(&step.out).contains("allowed from the phone"));
}

#[test]
fn an_answer_from_elsewhere_settles_the_question() {
    let mut view = terminal_view();
    framed(view.event(&approval(None), SIZE, true), &mut view);
    let step = framed(view.event(&resolved(Origin::Phone), SIZE, true), &mut view);
    assert!(step.settled);
    let out = readable(&step.out);
    assert!(out.contains("allowed from the phone"), "{out}");
    assert!(!out.contains("allow? y"), "the question is gone: {out}");
}

#[test]
fn an_expired_approval_settles_the_question() {
    let mut view = terminal_view();
    framed(view.event(&approval(None), SIZE, true), &mut view);
    let step = framed(
        view.event(&Event::ApprovalExpired { turn_id: turn(), call_id: call() }, SIZE, true),
        &mut view,
    );
    assert!(step.settled);
    assert!(readable(&step.out).contains("the approval expired"));
}

#[test]
fn a_raw_approval_goes_to_stderr_with_the_question() {
    let mut view = raw_view();
    let step = framed(view.event(&approval(Some("-a\n+b")), SIZE, true), &mut view);
    assert_eq!(step.out, "");
    assert_eq!(step.err, "approval needed: write ~/.zshrc\n-a\n+b\nallow? y = yes, n = no\n");
    assert_eq!(step.ask, Some(Ask::Approval(call())));
}

/// An approval of a long line in which two parts ask, as the daemon summarises it.
fn approval_of_parts() -> Event {
    Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "shell: run \"printf x; hostnamectl; uptime; systemctl --failed\"\n\
                  asks for: hostnamectl, systemctl --failed"
            .to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
    }
}

#[test]
fn an_approval_names_the_parts_that_ask_on_a_line_of_their_own() {
    let mut view = terminal_view();
    let step = framed(view.event(&approval_of_parts(), SIZE, true), &mut view);
    insta::assert_snapshot!(readable(&step.out));
}

#[test]
fn a_raw_approval_names_the_parts_that_ask_on_a_line_of_their_own() {
    let mut view = raw_view();
    let step = framed(view.event(&approval_of_parts(), SIZE, true), &mut view);
    assert_eq!(
        step.err,
        "approval needed: shell: run \"printf x; hostnamectl; uptime; systemctl --failed\"\n\
         asks for: hostnamectl, systemctl --failed\nallow? y = yes, n = no\n"
    );
}

#[test]
fn only_a_line_of_plain_names_passes_for_the_parts_that_ask() {
    let mut view = raw_view();
    let event = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "write_file: write /home/u/a\nasks for: ls (user data)".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
    };
    let step = framed(view.event(&event, SIZE, true), &mut view);
    assert_eq!(
        step.err,
        "approval needed: write_file: write /home/u/a asks for: ls (user data)\n\
         allow? y = yes, n = no\n"
    );
}

#[test]
fn approval_summaries_cannot_drive_the_terminal() {
    let mut view = terminal_view();
    let event = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "run \u{1b}]52;c;cGF5bG9hZA==\u{7}".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
    };
    let out = framed(view.event(&event, SIZE, false), &mut view).out;
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
    assert_eq!(framed(view.event(&event, SIZE, true), &mut view), Step::default());
    let started = Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/etc"),
        scope: Scope::Machine,
        settings: None,
    };
    assert_eq!(framed(view.event(&started, SIZE, true), &mut view), Step::default());
}

#[test]
fn a_failed_turn_commits_what_arrived_and_ends() {
    let mut view = terminal_view();
    framed(view.event(&updated(0, "Half a line"), SIZE, false), &mut view);
    let error = ErrorBody::new(ErrorCode::Internal, "the provider is down");
    let step = framed(
        view.event(&Event::TurnFailed { turn_id: turn(), error: error.clone() }, SIZE, false),
        &mut view,
    );
    assert_eq!(step.end, Some(TurnEnd::Failed(error)));
    assert_eq!(readable(&step.out), "\\e[?2026h\\r\\e[1A\\e[JHalf a line\n\\e[?2026l");
}

#[test]
fn an_interrupted_turn_says_so() {
    let mut view = raw_view();
    let step =
        framed(view.event(&Event::TurnInterrupted { turn_id: turn() }, SIZE, false), &mut view);
    assert_eq!(step.end, Some(TurnEnd::Interrupted));
    assert_eq!(step.err, "interrupted\n");
}

#[test]
fn a_late_update_of_a_completed_message_is_ignored() {
    let mut view = raw_view();
    feed(&mut view, &[completed(0, "done")], false);
    assert_eq!(
        framed(view.event(&updated(0, "done and more"), SIZE, false), &mut view),
        Step::default()
    );
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
    framed(view.event(&updated(0, "partial"), SIZE, false), &mut view);
    let step = framed(view.close(), &mut view);
    assert_eq!(readable(&step.out), "\\e[?2026h\\r\\e[1A\\e[Jpartial\n\\e[?2026l");
    assert_eq!(framed(view.close(), &mut view), Step::default());
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
    assert_eq!(err, "$ df -h\n");
}

#[test]
fn on_a_terminal_the_message_is_committed_above_the_tool_call() {
    let mut view = terminal_view();
    framed(view.event(&updated(0, "Let me check"), SIZE, false), &mut view);
    let out = readable(&framed(view.event(&tool_started("df -h"), SIZE, false), &mut view).out);
    // The call's line shows live below a blank line, with the spinner.
    assert_eq!(
        out,
        "\\e[?2026h\\r\\e[1A\\e[JLet me check\n\n\\e[33m\u{2022}\\e[0m \\e[2m$ df -h\\e[0m\n\\e[?2026l"
    );
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
    Event::ToolCallInputChanged {
        turn_id: turn(),
        call_id: call(),
        input: wait,
        looks_secret: false,
    }
}

fn call_completed(exit_code: i32) -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: "...".to_owned(),
        truncated: false,
        is_error: false,
        exit_code: Some(exit_code),
        sandbox: None,
        refusal: None,
    }
}

/// Feeds events to a terminal view and joins the bytes of each write.
fn writes(view: &mut TurnView, events: &[Event], can_ask: bool) -> String {
    let writes: Vec<String> = events
        .iter()
        .map(|event| readable(&framed(view.event(event, SIZE, can_ask), view).out))
        .collect();
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
    framed(view.event(&tool_started("make"), SIZE, false), &mut view);
    let line = "\u{6f22}".repeat(30);
    let out = framed(view.event(&output(&line), SIZE, false), &mut view).out;
    assert!(!out.contains(&line), "{}", readable(&out));
    assert!(out.contains('\u{2026}'), "{}", readable(&out));
    let live = out.rsplit('\n').nth(1).unwrap_or_default();
    assert!(
        efr_render::display_width(live, efr_render::WidthMethod::CodePoint) <= 40,
        "{}",
        readable(live)
    );
}

#[test]
fn a_failed_call_commits_one_line_and_the_last_lines_of_its_output() {
    let mut view = terminal_view();
    framed(view.event(&tool_started("make"), SIZE, false), &mut view);
    framed(view.event(&output("cc -c a.c\nerror: no rule\n"), SIZE, false), &mut view);
    let out = readable(&framed(view.event(&call_completed(2), SIZE, false), &mut view).out);
    assert_eq!(
        out,
        "\\e[?2026h\\r\\e[3A\\e[J\\e[2m$ make\\e[0m  \\e[31mexit 2\\e[0m\n\\e[2m  \u{2502} cc -c a.c\\e[0m\n\\e[2m  \u{2502} error: no rule\\e[0m\n\\e[?2026l"
    );
}

#[test]
fn a_call_that_went_well_commits_one_line_and_none_of_its_output() {
    let mut view = terminal_view();
    framed(view.envelope(&sent(11, 0, tool_started("cargo build")), SIZE, false), &mut view);
    framed(view.event(&output("Compiling app\nFinished\n"), SIZE, false), &mut view);
    let out = readable(
        &framed(view.envelope(&sent(12, 6_200, call_completed(0)), SIZE, false), &mut view).out,
    );
    assert_eq!(
        out,
        "\\e[?2026h\\r\\e[3A\\e[J\\e[2m$ cargo build\\e[0m  \\e[2m6.2s\\e[0m\n\\e[?2026l"
    );
    // A call under a second shows no time.
    let mut view = terminal_view();
    view.envelope(&sent(11, 0, tool_started("true")), SIZE, false);
    let out = readable(
        &framed(view.envelope(&sent(12, 900, call_completed(0)), SIZE, false), &mut view).out,
    );
    assert!(out.ends_with("\\e[2m$ true\\e[0m\n\\e[?2026l"), "{out}");
}

#[test]
fn consecutive_call_lines_have_no_blank_line_between_them() {
    let mut view = terminal_view();
    let shown = writes(
        &mut view,
        &[
            completed(0, "Checking."),
            tool_started("uptime"),
            call_completed(0),
            tool_started("df -h"),
            call_completed(1),
            completed(1, "Done."),
        ],
        false,
    );
    insta::assert_snapshot!(shown);
}

#[test]
fn a_running_call_shows_its_line_and_three_lines_of_output_at_40_and_80_columns() {
    let mut frames = Vec::new();
    for cols in [40, 80] {
        let size = Size { cols, rows: 20 };
        let mut view = started_view(Look { motion: true, ..Look::default() });
        view.envelope(&sent(11, 0, turn_started()), size, false);
        view.envelope(&sent(12, 0, tool_started("cargo test -p app --no-fail-fast")), size, false);
        view.event(
            &output("   Compiling app v0.1.0 (/home/me/p/app)\n     Running unittests src/lib.rs (target/debug/deps/app-1234)\ntest parse::tests::parse_empty ... FAILED\ntest parse::tests::parse_one ... ok\n"),
            size,
            false,
        );
        frames.push(readable(&view.frame(size, at(0))));
        frames.push(readable(&view.tick(size, at(12_300))));
    }
    insta::assert_snapshot!(frames.join("\n---\n"));
}

#[test]
fn a_tail_is_not_written_when_stdout_is_not_a_terminal() {
    let mut view = raw_view();
    framed(view.event(&tool_started("make"), SIZE, false), &mut view);
    assert_eq!(framed(view.event(&output("building\n"), SIZE, false), &mut view), Step::default());
    assert_eq!(framed(view.event(&call_completed(0), SIZE, false), &mut view), Step::default());
}

#[test]
fn a_hidden_input_asks_below_the_prompt_and_never_echoes() {
    let mut view = terminal_view();
    framed(view.event(&tool_started("sudo pacman -Syu"), SIZE, true), &mut view);
    framed(view.event(&output("[sudo] password for egg: "), SIZE, true), &mut view);
    let step = framed(view.event(&input(InputWait::Hidden), SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Hidden }));
    insta::assert_snapshot!(readable(&step.out));
    // Text passed for a hidden answer would never be shown.
    assert_eq!(framed(view.typed("hunter2"), &mut view), Step::default());

    let out = readable(&framed(view.answer_sent(SIZE), &mut view).out);
    assert!(out.contains("answer sent"), "{out}");
    assert!(out.contains("it is not shown"), "the question stays for the next line: {out}");

    // sudo may ask again after a wrong password, so keys stay quiet until the call
    // completes.
    let step = framed(view.event(&input(InputWait::None), SIZE, true), &mut view);
    assert!(!step.settled);
    assert_eq!(step.ask, Some(Ask::Discard(call())));
    let out = readable(&step.out);
    assert!(!out.contains("type the answer"), "the question is gone: {out}");
    assert!(out.contains("[sudo] password for egg:"), "the tail stays: {out}");

    let step = framed(view.event(&input(InputWait::Hidden), SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Hidden }));
    framed(view.event(&input(InputWait::None), SIZE, true), &mut view);
    let step = framed(view.event(&call_completed(0), SIZE, true), &mut view);
    assert!(step.settled, "the keys stop with the call");
    assert_eq!(step.ask, None);
}

#[test]
fn a_visible_input_that_ends_settles_at_once() {
    let mut view = terminal_view();
    framed(view.event(&tool_started("pacman -Syu"), SIZE, true), &mut view);
    framed(view.event(&input(InputWait::Visible), SIZE, true), &mut view);
    let step = framed(view.event(&input(InputWait::None), SIZE, true), &mut view);
    assert!(step.settled);
    assert_eq!(step.ask, None);
}

#[test]
fn a_visible_input_echoes_what_is_typed_until_it_is_sent() {
    let mut view = terminal_view();
    framed(view.event(&tool_started("sudo pacman -Syu"), SIZE, true), &mut view);
    framed(view.event(&output(":: Proceed with installation? [Y/n] "), SIZE, true), &mut view);
    let step = framed(view.event(&input(InputWait::Visible), SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Visible }));
    let typed = framed(view.typed("y"), &mut view);
    insta::assert_snapshot!(readable(&typed.out));

    let sent = readable(&framed(view.answer_sent(SIZE), &mut view).out);
    assert!(sent.contains("answer sent"), "{sent}");
    assert!(!sent.contains("> y"), "the echo goes with the send: {sent}");
}

#[test]
fn a_completed_call_settles_its_input_and_drops_the_question() {
    let mut view = terminal_view();
    framed(view.event(&tool_started("sudo true"), SIZE, true), &mut view);
    framed(view.event(&output("[sudo] password for egg: "), SIZE, true), &mut view);
    framed(view.event(&input(InputWait::Hidden), SIZE, true), &mut view);
    let step = framed(view.event(&call_completed(1), SIZE, true), &mut view);
    assert!(step.settled);
    let out = readable(&step.out);
    // The question goes; the call's line and the last line of its output stay.
    assert!(!out.contains("type the answer"), "{out}");
    assert!(out.contains("$ sudo true\\e[0m  \\e[31mexit 1"), "{out}");
}

#[test]
fn a_refused_answer_is_a_note() {
    let mut view = terminal_view();
    framed(view.event(&input(InputWait::Hidden), SIZE, true), &mut view);
    let out = readable(&framed(view.answer_refused(SIZE), &mut view).out);
    assert!(out.contains("the command no longer waits"), "{out}");
    let out = readable(&framed(view.answer_failed("boom \u{1b}[2J", SIZE), &mut view).out);
    assert!(out.contains("the answer was not sent: boom"), "{out}");
    assert!(!out.contains("\\e[2J"), "{out}");
}

#[test]
fn without_keys_a_wait_is_one_note() {
    let mut view = raw_view();
    let step = framed(view.event(&input(InputWait::Hidden), SIZE, false), &mut view);
    assert_eq!(step.ask, None);
    assert_eq!(
        step.err,
        "the command waits for hidden input, such as a password; efr cannot ask for it here\n"
    );
    let step = framed(view.event(&input(InputWait::None), SIZE, false), &mut view);
    assert_eq!(step, Step::default());
    let step = framed(view.event(&input(InputWait::Visible), SIZE, false), &mut view);
    assert_eq!(step.err, "the command waits for input; efr cannot ask for it here\n");
}

#[test]
fn a_raw_view_asks_on_stderr() {
    let mut view = raw_view();
    framed(view.event(&output("Password: "), SIZE, true), &mut view);
    let step = framed(view.event(&input(InputWait::Hidden), SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Hidden }));
    assert_eq!(step.out, "");
    assert_eq!(
        step.err,
        "Password:\ntype the answer and press Enter; it is not shown, and the agent sees it only if the \
         program prints it\n"
    );
    assert_eq!(framed(view.typed("visible?"), &mut view), Step::default());
}

#[test]
fn a_raw_view_echoes_a_visible_answer_on_stderr_where_backspace_erases() {
    let mut view = raw_view();
    framed(view.event(&output("Proceed? [Y/n] "), SIZE, true), &mut view);
    let step = framed(view.event(&input(InputWait::Visible), SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Visible }));
    assert_eq!(step.out, "");
    assert_eq!(
        step.err,
        "Proceed? [Y/n]\ntype the answer and press Enter; the agent sees it if the program shows \
         it\n> "
    );
    assert_eq!(framed(view.typed("y"), &mut view).err, "y");
    assert_eq!(framed(view.typed("ye"), &mut view).err, "e");
    assert_eq!(framed(view.typed("y"), &mut view).err, "\u{8} \u{8}");
    // A wide character takes two columns, and Ctrl+U erases every column.
    assert_eq!(framed(view.typed("y\u{6f22}"), &mut view).err, "\u{6f22}");
    assert_eq!(framed(view.typed(""), &mut view).err, "\u{8} \u{8}".repeat(3));
    framed(view.typed("n"), &mut view);
    let sent = framed(view.answer_sent(SIZE), &mut view);
    assert_eq!((sent.out.as_str(), sent.err.as_str()), ("", "\nanswer sent\n"));
    // Another answer to the same question starts a line of its own.
    assert_eq!(framed(view.typed("y"), &mut view).err, "> y");
    // And a line left unsent ends before anything that comes after the view.
    assert_eq!(framed(view.event(&turn_completed(), SIZE, true), &mut view).err, "\n");
}

#[test]
fn an_input_does_not_ask_over_a_pending_approval() {
    let mut view = terminal_view();
    framed(view.event(&approval(None), SIZE, true), &mut view);
    let step = framed(view.event(&input(InputWait::Hidden), SIZE, true), &mut view);
    assert_eq!(step.ask, None);
    assert!(!step.settled, "the approval keeps its keys");
}

#[test]
fn the_end_of_the_turn_settles_an_input() {
    let mut view = terminal_view();
    framed(view.event(&input(InputWait::Visible), SIZE, true), &mut view);
    let step =
        framed(view.event(&Event::TurnInterrupted { turn_id: turn() }, SIZE, true), &mut view);
    assert!(step.settled);
    assert!(!readable(&step.out).contains("type the answer"));
}

#[test]
fn a_queued_view_asks_for_the_input_of_the_running_turn_until_its_own_turn_starts() {
    let mut view = terminal_view();
    view.queue();
    let running: TurnId = "0192f0c1-7a00-7000-8000-000000000077".parse().unwrap();
    let other: CallId = "0192f0c1-7a00-7000-8000-000000000078".parse().unwrap();
    let wait = Event::ToolCallInputChanged {
        turn_id: running,
        call_id: other,
        input: InputWait::Hidden,
        looks_secret: false,
    };
    let step = framed(view.event(&wait, SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: other, kind: AnswerKind::Hidden }));

    let started = Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/home/u"),
        scope: Scope::Machine,
        settings: None,
    };
    let step = framed(view.event(&started, SIZE, true), &mut view);
    assert!(step.settled, "the question of the turn ahead goes once this one runs");
    assert!(!readable(&step.out).contains("type the answer"));

    // From now on another turn's waits are not this view's.
    assert_eq!(framed(view.event(&wait, SIZE, true), &mut view), Step::default());
}

fn secret_input() -> Event {
    Event::ToolCallInputChanged {
        turn_id: turn(),
        call_id: call(),
        input: InputWait::Visible,
        looks_secret: true,
    }
}

const SECRET_NOTE: &str = "this looks like a password prompt behind another program: your typing is not shown here, and the agent sees it only if that program shows it";

#[test]
fn a_visible_wait_that_looks_secret_hides_what_is_typed_and_says_why() {
    let mut view = TurnView::new(turn(), RenderOptions::new(400));
    let size = Size { cols: 400, rows: 20 };
    framed(view.event(&tool_started("sudo -u build passwd"), size, true), &mut view);
    framed(view.event(&output("Current password: "), size, true), &mut view);
    let step = framed(view.event(&secret_input(), size, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Masked }));
    let shown = readable(&step.out);
    assert!(shown.contains("Current password:"), "{shown}");
    assert!(shown.contains(SECRET_NOTE), "{shown}");
    assert!(!shown.contains("> "), "no echo line: {shown}");
    // Text handed to the view by mistake is dropped, never drawn.
    assert_eq!(framed(view.typed("hunter2"), &mut view), Step::default());

    // Asked again later, keys typed in between are thrown away, as for a hidden wait.
    let step = framed(view.event(&input(InputWait::None), size, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Discard(call())));
}

#[test]
fn a_raw_view_of_a_secret_looking_wait_opens_no_echo_line() {
    let mut view = TurnView::new(turn(), RenderOptions::new(400).with_terminal(false));
    let size = Size { cols: 400, rows: 20 };
    framed(view.event(&output("Enter PIN: "), size, true), &mut view);
    let step = framed(view.event(&secret_input(), size, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Masked }));
    assert!(step.err.contains(SECRET_NOTE), "{:?}", step.err);
    assert!(!step.err.contains("> "), "{:?}", step.err);
}

#[test]
fn an_answer_kind_says_whether_it_is_shown_and_how_it_is_sent() {
    assert!(AnswerKind::Visible.shown() && !AnswerKind::Visible.hidden());
    assert!(!AnswerKind::Hidden.shown() && AnswerKind::Hidden.hidden());
    // A secret-looking visible wait hides the typing but answers as visible.
    assert!(!AnswerKind::Masked.shown() && !AnswerKind::Masked.hidden());
}

const HINT: &str = "no output for 10 s; press Ctrl+\\ to type an input for the command";

fn shell_started() -> Event {
    Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input: json!({ "command": "./deploy" }),
        manual_input: true,
        launch: None,
    }
}

/// A terminal view wide enough for the hint, with a shell call of this turn running.
fn silent_shell() -> (TurnView, Size) {
    let mut view = TurnView::new(turn(), RenderOptions::new(400));
    let size = Size { cols: 400, rows: 20 };
    framed(view.event(&shell_started(), size, true), &mut view);
    (view, size)
}

#[test]
fn a_running_shell_call_of_the_turn_is_silent_until_it_shows_a_sign_of_life() {
    let (mut view, size) = silent_shell();
    let (silent, first) = view.silence().unwrap();
    assert_eq!(silent, call());
    assert_eq!(view.manual_offer(), None, "nothing is offered before the time is up");
    framed(view.event(&output("deploying"), size, true), &mut view);
    let (_, second) = view.silence().unwrap();
    assert_ne!(first, second, "output starts the silence again");
    // A reported wait asks for itself, so the call is not silent.
    framed(view.event(&input(InputWait::Visible), size, true), &mut view);
    assert_eq!(view.silence(), None);
}

#[test]
fn only_calls_of_the_followed_turn_that_take_a_manual_input_can_be_silent() {
    let mut view = TurnView::new(turn(), RenderOptions::new(80));
    let read = Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "read_file".to_owned(),
        input: json!({ "path": "/etc/hosts" }),
        manual_input: false,
        launch: None,
    };
    framed(view.event(&read, SIZE, true), &mut view);
    assert_eq!(view.silence(), None);
    framed(view.event(&output("127.0.0.1 localhost"), SIZE, true), &mut view);
    assert_eq!(view.silence(), None, "output alone does not make a call take a manual input");
}

#[test]
fn a_shell_call_into_a_shell_that_reads_command_lines_is_never_silent() {
    // The daemon refuses a manual answer there: it would run as a command line.
    let mut view = TurnView::new(turn(), RenderOptions::new(80));
    let nested = Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input: json!({ "command": "sleep 60", "nested_shell": true }),
        manual_input: false,
        launch: None,
    };
    framed(view.event(&nested, SIZE, true), &mut view);
    assert_eq!(view.silence(), None);
    assert_eq!(view.manual_offer(), None);
}

#[test]
fn a_silent_call_offers_ctrl_backslash_on_one_dim_line_until_it_prints() {
    let (mut view, size) = silent_shell();
    let step = framed(view.silent(call(), size), &mut view);
    assert_eq!(step.ask, None, "no key is read for the hint");
    assert!(readable(&step.out).contains(HINT), "{}", readable(&step.out));
    assert_eq!(view.manual_offer(), Some(call()));
    assert_eq!(view.silence(), None, "the hint shows once");

    let step = framed(view.event(&output("deploying"), size, true), &mut view);
    assert!(!readable(&step.out).contains(HINT), "{}", readable(&step.out));
    assert_eq!(view.manual_offer(), None);
    assert!(view.silence().is_some(), "a new silence starts");
}

#[test]
fn ctrl_backslash_asks_for_a_manual_line_that_is_not_shown_and_one_answer_ends_it() {
    let (mut view, size) = silent_shell();
    framed(view.silent(call(), size), &mut view);
    let step = framed(view.manual(call(), size), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Manual }));
    let shown = readable(&step.out);
    assert!(shown.contains("your typing is not shown here"), "{shown}");
    assert!(!shown.contains(HINT), "{shown}");
    assert!(!shown.contains(ECHO_PREFIX), "{shown}");
    assert_eq!(framed(view.typed("yes"), &mut view), Step::default(), "nothing typed is shown");
    assert_eq!(view.manual_offer(), None);

    let step = framed(view.answer_sent(size), &mut view);
    assert!(step.settled, "the keys stop after a manual answer");
    assert!(view.silence().is_some(), "the silence starts again");
}

#[test]
fn a_manual_line_under_a_password_prompt_echoes_nothing() {
    // A prompt behind a relay in a raw-mode terminal reports no wait, so only a manual
    // line can answer it; the password must not show.
    let mut view = TurnView::new(turn(), RenderOptions::new(400).with_terminal(false));
    let size = Size { cols: 400, rows: 20 };
    framed(view.event(&shell_started(), size, true), &mut view);
    framed(view.event(&output("[sudo] password for u:"), size, true), &mut view);
    framed(view.silent(call(), size), &mut view);
    let step = framed(view.manual(call(), size), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Manual }));
    assert!(!step.err.contains(ECHO_PREFIX), "{:?}", step.err);
    assert!(!AnswerKind::Manual.shown());
    assert_eq!(framed(view.typed("hunter2"), &mut view), Step::default());
    let sent = framed(view.answer_sent(size), &mut view);
    assert!(!sent.err.contains("hunter2"), "{:?}", sent.err);
}

#[test]
fn ctrl_backslash_again_closes_the_manual_line_and_offers_it_again() {
    let (mut view, size) = silent_shell();
    assert_eq!(framed(view.manual_cancelled(size), &mut view), Step::default(), "no line is open");
    framed(view.silent(call(), size), &mut view);
    framed(view.manual(call(), size), &mut view);
    assert!(view.manual_open());
    let step = framed(view.manual_cancelled(size), &mut view);
    assert!(step.settled, "the keys stop");
    assert_eq!(step.ask, None);
    let shown = readable(&step.out);
    assert!(shown.contains("the input was not sent"), "{shown}");
    assert!(shown.contains(HINT), "the offer comes back: {shown}");
    assert!(!view.manual_open());
    assert_eq!(view.manual_offer(), Some(call()));
}

#[test]
fn ctrl_backslash_for_a_call_that_no_longer_offers_it_does_nothing() {
    let (mut view, size) = silent_shell();
    assert_eq!(
        framed(view.manual(call(), size), &mut view),
        Step::default(),
        "the hint was not shown"
    );
    framed(view.silent(call(), size), &mut view);
    framed(view.event(&output("deploying"), size, true), &mut view);
    assert_eq!(
        framed(view.manual(call(), size), &mut view),
        Step::default(),
        "the call printed since"
    );
    let other: CallId = "0192f0c1-7a00-7000-8000-0000000000ff".parse().unwrap();
    assert_eq!(framed(view.silent(other, size), &mut view), Step::default());
}

#[test]
fn an_approval_hides_the_offer() {
    let (mut view, size) = silent_shell();
    framed(view.silent(call(), size), &mut view);
    framed(view.event(&approval(None), size, true), &mut view);
    assert_eq!(view.manual_offer(), None);
    assert_eq!(view.silence(), None);
}

#[test]
fn without_a_terminal_the_offer_is_a_note_on_stderr() {
    let mut view = TurnView::new(turn(), RenderOptions::new(400).with_terminal(false));
    let size = Size { cols: 400, rows: 20 };
    framed(view.event(&shell_started(), size, true), &mut view);
    let step = framed(view.silent(call(), size), &mut view);
    assert!(step.err.contains(HINT), "{:?}", step.err);
    assert_eq!(step.out, "");
}

#[test]
fn the_hint_names_the_silence_that_the_follow_loop_waits_for() {
    let seconds = format!("no output for {} s;", crate::follow::SILENCE.as_secs());
    assert!(super::SILENCE_HINT.starts_with(&seconds), "{}", super::SILENCE_HINT);
}

/// An approval of the shell call that says whether it may wait for input.
fn shell_approval(interactive: bool) -> Event {
    Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "shell: sudo pacman -Syu".to_owned(),
        diff_preview: None,
        interactive,
        exit: None,
    }
}

/// A terminal view of a shell call whose interactive approval the user allowed here.
fn kept_shell() -> TurnView {
    let mut view = terminal_view();
    framed(view.event(&tool_started("sudo pacman -Syu"), SIZE, true), &mut view);
    framed(view.event(&shell_approval(true), SIZE, true), &mut view);
    let step = framed(view.answered(call(), ApprovalDecision::Allow, SIZE), &mut view);
    assert_eq!(step.ask, Some(Ask::Retain(call())), "the keys go on for the call");
    assert!(!step.settled);
    view
}

#[test]
fn allowing_a_call_that_may_wait_for_input_keeps_the_keys_until_it_completes() {
    let mut view = kept_shell();
    // The approval resolved by this view changes nothing, and neither does output.
    let step = framed(view.event(&resolved(Origin::Shell), SIZE, true), &mut view);
    assert_eq!((step.ask, step.settled), (None, false));
    let step = framed(view.event(&output("resolving dependencies..."), SIZE, true), &mut view);
    assert_eq!((step.ask, step.settled), (None, false));
    let step = framed(view.event(&call_completed(0), SIZE, true), &mut view);
    assert!(step.settled, "the call's end stops the keys");
    assert_eq!(step.ask, None);
}

#[test]
fn a_visible_wait_of_a_kept_call_asks_and_its_end_keeps_the_keys_again() {
    let mut view = kept_shell();
    framed(view.event(&output(":: Proceed with installation? [Y/n] "), SIZE, true), &mut view);
    let step = framed(view.event(&input(InputWait::Visible), SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Visible }));
    framed(view.answer_sent(SIZE), &mut view);
    let step = framed(view.event(&input(InputWait::None), SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Retain(call())), "the next prompt of the call gets them too");
    assert!(!step.settled);
}

#[test]
fn after_a_password_wait_a_kept_call_throws_its_keys_away() {
    let mut view = kept_shell();
    let step = framed(view.event(&input(InputWait::Hidden), SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Input { call_id: call(), kind: AnswerKind::Hidden }));
    // A password typed again meanwhile must never start a shown answer line.
    let step = framed(view.event(&input(InputWait::None), SIZE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Discard(call())));
}

#[test]
fn only_an_interactive_approval_allowed_here_keeps_the_keys() {
    // Allowed, but the call waits for nothing.
    let mut view = terminal_view();
    framed(view.event(&tool_started("make clean"), SIZE, true), &mut view);
    framed(view.event(&shell_approval(false), SIZE, true), &mut view);
    assert_eq!(framed(view.answered(call(), ApprovalDecision::Allow, SIZE), &mut view).ask, None);
    // Denied.
    let mut view = terminal_view();
    framed(view.event(&tool_started("sudo pacman -Syu"), SIZE, true), &mut view);
    framed(view.event(&shell_approval(true), SIZE, true), &mut view);
    assert_eq!(framed(view.answered(call(), ApprovalDecision::Deny, SIZE), &mut view).ask, None);
    // Allowed elsewhere: the keys belong to the user's shell here.
    let mut view = terminal_view();
    framed(view.event(&tool_started("sudo pacman -Syu"), SIZE, true), &mut view);
    framed(view.event(&shell_approval(true), SIZE, true), &mut view);
    let step = framed(view.event(&resolved(Origin::Phone), SIZE, true), &mut view);
    assert_eq!(step.ask, None);
    assert!(step.settled, "the question here is settled");
    let step = framed(view.event(&input(InputWait::None), SIZE, true), &mut view);
    assert_eq!(step.ask, None);
}

#[test]
fn a_manual_line_of_a_kept_call_throws_the_keys_after_it_away() {
    let mut view = TurnView::new(turn(), RenderOptions::new(400));
    let size = Size { cols: 400, rows: 20 };
    framed(view.event(&shell_started(), size, true), &mut view);
    framed(view.event(&shell_approval(true), size, true), &mut view);
    framed(view.answered(call(), ApprovalDecision::Allow, size), &mut view);
    // A kept call still offers `Ctrl+\` when it is silent.
    framed(view.silent(call(), size), &mut view);
    assert_eq!(view.manual_offer(), Some(call()));
    framed(view.manual(call(), size), &mut view);
    let step = framed(view.manual_cancelled(size), &mut view);
    assert_eq!((step.ask, step.settled), (Some(Ask::Discard(call())), false));
    // The line may have held a password, so the keys are not kept for the call, but
    // `Ctrl+\` still opens the next one.
    assert_eq!(view.manual_offer(), Some(call()));
    framed(view.manual(call(), size), &mut view);
    let step = framed(view.answer_sent(size), &mut view);
    assert_eq!((step.ask, step.settled), (Some(Ask::Discard(call())), false));
    framed(view.silent(call(), size), &mut view);
    assert_eq!(view.manual_offer(), Some(call()));
}

#[test]
fn the_end_of_the_turn_or_a_new_approval_ends_the_kept_keys() {
    let mut view = kept_shell();
    let step = framed(view.event(&turn_completed(), SIZE, true), &mut view);
    assert!(step.settled);
    let mut view = kept_shell();
    let next: CallId = "0192f0c1-7a00-7000-8000-0000000000fe".parse().unwrap();
    let approval = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: next,
        summary: "write ~/.zshrc".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
    };
    assert_eq!(framed(view.event(&approval, SIZE, true), &mut view).ask, Some(Ask::Approval(next)));
    // The call that kept keys is over; a late wait of it asks nothing more of the keys.
    framed(view.answered(next, ApprovalDecision::Allow, SIZE), &mut view);
    let step = framed(view.event(&input(InputWait::None), SIZE, true), &mut view);
    assert_eq!(step.ask, None);
}

/// A view that prints `/home/user` as `~`, raw or on a terminal.
fn sandbox_view(terminal: bool) -> TurnView {
    TurnView::new(turn(), RenderOptions::new(100).with_terminal(terminal))
        .with_home(Some(PathBuf::from("/home/user")))
}

const WIDE: Size = Size { cols: 100, rows: 20 };

fn started_in(scope: Scope, fallback: Option<ModeFallback>) -> Event {
    Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/home/user/project"),
        scope,
        settings: Some(EffectiveSettings {
            mode: if fallback.is_some() { Mode::Cautious } else { Mode::Auto },
            model: "gpt-5.5".to_owned(),
            effort: None,
            overridden: OverriddenSettings::default(),
            fallback,
        }),
    }
}

fn contained_started(command: &str) -> Event {
    Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input: json!({ "command": command }),
        manual_input: true,
        launch: Some(Launch::contained()),
    }
}

fn contained_completed(exit_code: i32, sandbox: Option<SandboxSummary>) -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: String::new(),
        truncated: false,
        is_error: false,
        exit_code: Some(exit_code),
        sandbox,
        refusal: None,
    }
}

#[test]
fn a_routine_command_shows_the_sandbox_once_and_a_failure_says_sandbox() {
    let mut view = sandbox_view(false);
    let project = Scope::Project("019a9b1c-3d00-7a10-8b20-0000000000e1".parse().unwrap());
    let (_, err, _) = feed(
        &mut view,
        &[
            started_in(project, None),
            contained_started("cargo nextest run -p efr-cli"),
            contained_completed(
                0,
                Some(SandboxSummary { confined: true, ..SandboxSummary::default() }),
            ),
            contained_started("cargo add serde"),
            contained_completed(
                101,
                Some(SandboxSummary {
                    confined: true,
                    blocked: vec![Blocked {
                        host: "evil.example".to_owned(),
                        port: 443,
                        reason: BlockReason::NotAllowed,
                    }],
                    background_stopped: vec!["vite".to_owned()],
                    ..SandboxSummary::default()
                }),
            ),
        ],
        true,
    );
    insta::assert_snapshot!(err);
}

#[test]
fn a_setup_failure_says_why_on_its_own_line() {
    let mut view = sandbox_view(false);
    let project = Scope::Project("019a9b1c-3d00-7a10-8b20-0000000000e1".parse().unwrap());
    let failed = SandboxSummary {
        setup_error: Some("bwrap: Can't mount proc on /newroot/proc".to_owned()),
        ..SandboxSummary::default()
    };
    let (_, err, _) = feed(
        &mut view,
        &[
            started_in(project, None),
            contained_started("cargo test"),
            contained_completed(125, Some(failed)),
        ],
        true,
    );
    insta::assert_snapshot!(err);
}

#[test]
fn a_hidden_prompt_in_a_contained_call_is_a_note_not_a_question() {
    let mut view = sandbox_view(false);
    let project = Scope::Project("019a9b1c-3d00-7a10-8b20-0000000000e1".parse().unwrap());
    framed(view.event(&started_in(project, None), WIDE, true), &mut view);
    framed(view.event(&contained_started("ssh-add"), WIDE, true), &mut view);
    let step = framed(view.event(&input(InputWait::Hidden), WIDE, true), &mut view);
    assert_eq!(step.ask, None, "efr never asks for a secret for the sandbox");
    assert_eq!(
        step.err,
        "sandbox: the command asked for a password; efr does not type secrets into the \
         sandbox\n"
    );
}

#[test]
fn a_turn_outside_a_project_writes_only_in_scratch_and_tmp() {
    let mut view = sandbox_view(false);
    let (_, err, _) =
        feed(&mut view, &[started_in(Scope::Machine, None), contained_started("ls")], true);
    assert_eq!(err, "$ ls\nsandbox: writes in $SCRATCH, private /tmp; no network\n");
}

#[test]
fn a_fallback_says_so_at_the_start_of_the_turn() {
    let mut view = sandbox_view(true);
    let fallback = ModeFallback { asked: Mode::Auto, reason: "Landlock ABI 6".to_owned() };
    let step =
        framed(view.event(&started_in(Scope::Machine, Some(fallback)), WIDE, true), &mut view);
    insta::assert_snapshot!(readable(&step.out));
}

/// The record and the approval of an exit of `line`.
fn exit_events(line: &str, facts: ExitFacts, info: ExitInfo) -> [Event; 2] {
    [
        Event::ExitRequested {
            turn_id: turn(),
            call_id: call(),
            kinds: info.kinds.clone(),
            grants: info.grants.clone(),
            source: ExitSource::Predicted,
            record: Box::new(exit_record(line, facts)),
        },
        Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "a summary that the exit's whole line replaces".to_owned(),
            diff_preview: None,
            interactive: true,
            exit: Some(info),
        },
    ]
}

#[test]
fn an_exit_question_shows_the_whole_line_what_leaves_and_the_models_reason() {
    let info = ExitInfo {
        facts: vec!["the file exists".to_owned(), "persistence".to_owned()],
        model_reason: Some("you asked me to add the alias".to_owned()),
        user_only: true,
        ..exit_info(&[ExitKind::Persistence], Launch::Unsandboxed)
    };
    let facts = ExitFacts {
        targets: vec![TargetFact {
            path: PathBuf::from("/home/user/.zshrc"),
            class: Some(PathClassName::UserConfig),
            in_write_root: false,
            floor: true,
            synced: false,
            exists: true,
            named_in_user_messages: false,
        }],
        programs: vec![program_fact("echo", "shell builtin")],
        ..ExitFacts::default()
    };
    let mut view = sandbox_view(false);
    let (_, err, _) =
        feed(&mut view, &exit_events("echo 'alias k=kubectl' >> ~/.zshrc", facts, info), true);
    insta::assert_snapshot!(err);
}

#[test]
fn an_exit_question_on_a_terminal_paints_the_untrusted_program_yellow() {
    let info =
        ExitInfo { user_only: true, ..exit_info(&[ExitKind::Privilege], Launch::Unsandboxed) };
    let mut setup = program_fact("./scripts/setup.sh", "/home/user/project/scripts/setup.sh");
    setup.in_write_root = true;
    setup.changed_this_turn = true;
    let facts = ExitFacts {
        programs: vec![program_fact("sudo", "/usr/bin/sudo"), setup],
        ..ExitFacts::default()
    };
    let mut view = sandbox_view(true);
    let [record, approval] = exit_events("sudo ./scripts/setup.sh", facts, info);
    assert_eq!(framed(view.event(&record, WIDE, true), &mut view), Step::default());
    let step = framed(view.event(&approval, WIDE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Approval(call())));
    insta::assert_snapshot!(readable(&step.out));
}

#[test]
fn a_network_exit_question_runs_in_the_sandbox_with_full_network() {
    let info = exit_info(&[ExitKind::Host], Launch::Contained { grants: vec![Grant::OpenNetwork] });
    let mut view = sandbox_view(false);
    let (_, err, _) = feed(&mut view, &exit_events("npm ci", ExitFacts::default(), info), true);
    assert_eq!(
        err,
        "\
approval needed: shell: run \"npm ci\"
leaves the sandbox: network; runs in the sandbox with full network for this call
allow? y = yes, n = no
"
    );
}

#[test]
fn an_exit_without_its_record_keeps_the_summary() {
    let info = exit_info(&[ExitKind::Host], Launch::Contained { grants: vec![Grant::OpenNetwork] });
    let approval = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "run `npm ci`".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: Some(info),
    };
    let mut view = sandbox_view(false);
    let (_, err, _) = feed(&mut view, &[approval], false);
    assert!(
        err.starts_with("approval needed: run `npm ci`\nleaves the sandbox: network;"),
        "{err}"
    );
}

#[test]
fn the_progress_line_of_a_command_of_several_lines_says_how_many_follow() {
    let mut view = terminal_view();
    let step = framed(view.event(&tool_started(FAILED_UNITS), SIZE, false), &mut view);
    insta::assert_snapshot!(readable(&step.out));

    let mut view = raw_view();
    let (_, err, _) = feed(&mut view, &[tool_started(FROM_SRC)], false);
    assert_eq!(err, "$ cd src (and 3 more lines)\n");
}

#[test]
fn an_exit_question_shows_each_line_of_a_command_of_several() {
    let grants = vec![Grant::Bus { bus: efr_protocol::BusKind::System }];
    let info = exit_info(&[ExitKind::Bus], Launch::Contained { grants });
    let mut view = sandbox_view(false);
    let (_, err, _) =
        feed(&mut view, &exit_events(FAILED_UNITS, ExitFacts::default(), info.clone()), true);
    insta::assert_snapshot!(err);

    let mut view = sandbox_view(true);
    let [record, approval] = exit_events(FAILED_UNITS, ExitFacts::default(), info);
    framed(view.event(&record, WIDE, true), &mut view);
    insta::assert_snapshot!(
        "several_lines_on_a_terminal",
        readable(&framed(view.event(&approval, WIDE, true), &mut view).out)
    );
}

#[test]
fn a_question_without_an_exit_shows_each_line_of_a_command_of_several() {
    let approval = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: format!("shell: run {FROM_SRC:?}"),
        diff_preview: None,
        interactive: false,
        exit: None,
    };
    let mut view = raw_view();
    let (_, err, _) = feed(&mut view, &[tool_started(FROM_SRC), approval.clone()], true);
    insta::assert_snapshot!(err);

    // A view that missed the call's start shows the daemon's summary, which quotes the
    // line with its newlines escaped.
    let mut view = raw_view();
    let (_, err, _) = feed(&mut view, &[approval], true);
    assert!(err.starts_with("approval needed: shell: run \"cd src\\nexport"), "{err}");
}

fn question_id() -> QuestionId {
    "019a9b1c-3d00-7a10-8b20-0000000000d1".parse().unwrap()
}

fn surface_requested() -> Event {
    Event::SurfaceQuestionRequested {
        turn_id: turn(),
        call_id: call(),
        question_id: question_id(),
        changes: vec![SurfaceChange {
            path: PathBuf::from("/home/user/project/.git/commondir"),
            rule: "commondir_in_main_git_dir".to_owned(),
            key: Some("core.fsmonitor".to_owned()),
            quarantined: true,
        }],
    }
}

fn surface_answered(keep: bool, origin: Option<Origin>) -> Event {
    Event::SurfaceQuestionAnswered { turn_id: turn(), question_id: question_id(), keep, origin }
}

#[test]
fn the_quarantine_question_asks_for_one_key_below_the_live_zone() {
    let mut view = sandbox_view(true);
    let step = framed(view.event(&surface_requested(), WIDE, true), &mut view);
    assert_eq!(step.ask, Some(Ask::Surface(question_id())));
    insta::assert_snapshot!(readable(&step.out));

    let step = framed(view.surface_answered(question_id(), false, WIDE), &mut view);
    insta::assert_snapshot!("surface_answered", readable(&step.out));
    // The daemon's event for this client's own answer adds nothing.
    assert_eq!(
        framed(view.event(&surface_answered(false, Some(Origin::Shell)), WIDE, true), &mut view),
        Step::default()
    );
}

#[test]
fn a_quarantine_question_answered_elsewhere_or_expired_settles_with_a_note() {
    let mut view = sandbox_view(false);
    let (_, err, _) = feed(&mut view, &[surface_requested()], true);
    assert_eq!(
        err,
        "\
question: the last command changed git settings that run programs
  ~/project/.git/commondir (core.fsmonitor); moved to quarantine
keep it? y = yes, n = no
"
    );
    let step =
        framed(view.event(&surface_answered(true, Some(Origin::Phone)), WIDE, true), &mut view);
    assert!(step.settled);
    assert_eq!(step.err, "the git change was kept, from the phone\n");

    let mut view = sandbox_view(false);
    framed(view.event(&surface_requested(), WIDE, true), &mut view);
    let step = framed(view.event(&surface_answered(false, None), WIDE, true), &mut view);
    assert!(step.settled);
    assert_eq!(step.err, "no answer; the git change stays in quarantine\n");

    // Without keys nobody here answers, and the end of the turn needs no keys stopped.
    let mut view = sandbox_view(false);
    let step = framed(view.event(&surface_requested(), WIDE, false), &mut view);
    assert_eq!(step.ask, None);
    assert!(step.err.ends_with("waiting for another client to answer\n"), "{}", step.err);
    assert!(!framed(view.event(&turn_completed(), WIDE, false), &mut view).settled);
}

#[test]
fn the_end_of_the_turn_stops_the_keys_of_a_quarantine_question() {
    let mut view = sandbox_view(false);
    framed(view.event(&surface_requested(), WIDE, true), &mut view);
    assert!(framed(view.event(&turn_completed(), WIDE, true), &mut view).settled);
}

#[test]
fn the_turn_end_report_lists_the_files_that_run_code() {
    let mut view = sandbox_view(false);
    let report = Event::TurnSurfaceReport {
        turn_id: turn(),
        files: vec![
            ReportedFile { path: PathBuf::from("build.rs"), detail: None },
            ReportedFile {
                path: PathBuf::from(".cargo/config.toml"),
                detail: Some("build.rustc-wrapper".to_owned()),
            },
        ],
    };
    let (_, err, _) = feed(&mut view, &[report], true);
    assert_eq!(
        err,
        "\
efr: this turn changed files that run code later outside the sandbox:
  build.rs, .cargo/config.toml (build.rustc-wrapper)
  check them before you run the project yourself
"
    );
}

// --- frames, the status row, drafts and the end of a turn ---------------------------

/// The time `millis` after the prompt was sent.
fn at(millis: i64) -> Timestamp {
    now() + SignedDuration::from_millis(millis)
}

/// Every switch of `config.toml` on.
const ALL: Look = Look { motion: true, summary: true, progress: true };

/// A terminal view with `look` whose status row runs.
fn started_view(look: Look) -> TurnView {
    let mut view = TurnView::new(turn(), RenderOptions::new(40)).with_look(look);
    view.start();
    view
}

fn turn_started() -> Event {
    Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/home/user/project"),
        scope: Scope::Machine,
        settings: None,
    }
}

/// `event` as the daemon sends it: number `seq`, recorded `millis` after the prompt.
fn sent(seq: u64, millis: i64, event: Event) -> EventEnvelope {
    EventEnvelope { seq: Seq::new(seq), conversation_id: None, at: at(millis), event }
}

/// A draft made after event `after` of the turn.
fn draft(after: u64, part: DraftPart) -> Draft {
    Draft { turn_id: turn(), after_seq: Seq::new(after), draft: part }
}

fn text_draft(after: u64, offset: usize, delta: &str) -> Draft {
    let part = DraftPart::Text { index: 0, offset: offset as u64, delta: delta.to_owned() };
    draft(after, part)
}

#[test]
fn the_status_row_shows_from_the_first_frame_and_hides_the_cursor() {
    let mut view = started_view(Look { motion: true, ..Look::default() });
    let first = view.frame(SIZE, at(0));
    assert_eq!(
        readable(&first),
        "\\e[?25l\\e[?2026h\\e[33m\u{280b}\\e[0m \\e[2mwaiting for the model\\e[0m\n\\e[?2026l"
    );
    assert!(view.ticks());
}

#[test]
fn a_tick_that_changes_only_the_status_row_writes_only_that_row() {
    let mut view = started_view(Look { motion: true, ..Look::default() });
    view.event(&turn_started(), SIZE, false);
    view.event(&updated(0, "I will run the test first"), SIZE, false);
    view.frame(SIZE, at(0));
    let tick = view.tick(SIZE, at(100));
    assert_eq!(
        readable(&tick),
        "\\e[?2026h\\r\\e[1A\\e[2K\\e[33m\u{2819}\\e[0m \\e[2m\\e[0mw\\e[0;2mriting\\e[0m\n\\e[?2026l"
    );
    // A row that does not change writes nothing.
    let mut still = started_view(Look::default());
    still.frame(SIZE, at(0));
    assert_eq!(still.tick(SIZE, at(100)), "");
}

#[test]
fn the_frames_of_a_turn_at_its_ticks() {
    let mut view = started_view(Look { motion: true, ..Look::default() });
    let mut frames = vec![view.frame(SIZE, at(0))];
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    frames.push(view.frame(SIZE, at(10)));
    frames.push(view.tick(SIZE, at(110)));
    view.envelope(&sent(12, 1_500, updated(0, "Reading the logs.\n\nThe disk")), SIZE, false);
    frames.push(view.frame(SIZE, at(1_510)));
    frames.push(view.tick(SIZE, at(1_610)));
    view.envelope(
        &sent(13, 2_000, completed(0, "Reading the logs.\n\nThe disk is full.\n")),
        SIZE,
        false,
    );
    frames.push(view.frame(SIZE, at(2_010)));
    frames.push(view.tick(SIZE, at(2_110)));
    let frames: Vec<String> = frames.iter().map(|frame| readable(frame)).collect();
    insta::assert_snapshot!(frames.join("\n---\n"));
}

#[test]
fn a_question_writes_without_the_status_row_shows_the_cursor_and_stops_the_time() {
    let mut view = started_view(ALL);
    view.envelope(&sent(11, 0, turn_started()), SIZE, true);
    view.frame(SIZE, at(0));
    view.tick(SIZE, at(5_000));
    let step = view.envelope(&sent(12, 5_000, approval(None)), SIZE, true);
    assert_eq!(step.ask, Some(Ask::Approval(call())));
    let asked = view.frame(SIZE, at(5_000));
    assert!(!asked.contains("waiting for"), "{}", readable(&asked));
    assert!(asked.contains("\x1b[?25h"), "the user types here: {}", readable(&asked));
    assert!(asked.contains(progress::PAUSED), "{}", readable(&asked));
    assert!(!view.tick(SIZE, at(60_000)).contains("waiting for"));
    view.answered(call(), ApprovalDecision::Allow, SIZE);
    let back = view.frame(SIZE, at(65_000));
    assert!(back.contains("\x1b[?25l"), "{}", readable(&back));
    assert!(back.contains(progress::RUNNING), "{}", readable(&back));
    // Five seconds before the question and none of the minute it waited. The model
    // could send nothing while the user was asked, so that is no stall either.
    let back = readable(&back);
    assert!(back.contains("for the model\\e[0m  \\e[2m5s\\e[0m\n"), "{back}");
    let tick = readable(&view.tick(SIZE, at(66_000)));
    assert!(tick.ends_with("model\\e[0m  \\e[2m6s\\e[0m\n\\e[?2026l\\e]9;4;3\\e\\"), "{tick}");
    assert!(!back.contains("1m") && !tick.contains("1m"), "{back} {tick}");
}

#[test]
fn twenty_seconds_without_data_says_so() {
    let mut view = started_view(Look::default());
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    let wide = Size { cols: 80, rows: 20 };
    view.frame(wide, at(0));
    let stalled = view.tick(wide, at(41_000));
    assert!(
        readable(&stalled).contains("waiting for the model, no data for 41s"),
        "{}",
        readable(&stalled)
    );
}

#[test]
fn drafts_then_an_overlapping_update_show_no_text_twice() {
    let mut view = started_view(Look::default());
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    let mut out = view.frame(SIZE, at(0));
    for (millis, (offset, delta)) in
        [(0, "The disk "), (9, "is full"), (18, ".\n\nFree")].into_iter().enumerate()
    {
        let _ = millis;
        view.draft(&text_draft(11, offset, delta), SIZE);
        out.push_str(&view.frame(SIZE, at(16 * (millis as i64 + 1))));
    }
    // The persisted update every 200 ms ends inside what the drafts showed.
    let step = view.envelope(&sent(12, 200, updated(0, "The disk is full")), SIZE, false);
    assert_eq!(step, Step::default());
    out.push_str(&view.frame(SIZE, at(200)));
    view.envelope(
        &sent(13, 300, completed(0, "The disk is full.\n\nFree some space.\n")),
        SIZE,
        false,
    );
    out.push_str(&view.frame(SIZE, at(300)));
    let shown = out.matches("The disk is full.").count();
    assert_eq!(shown, 1, "{}", readable(&out));
    assert!(out.contains("Free some space."), "{}", readable(&out));
}

#[test]
fn a_dropped_draft_heals_at_the_next_persisted_update() {
    let mut view = started_view(Look::default());
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    view.draft(&text_draft(11, 0, "one "), SIZE);
    view.frame(SIZE, at(16));
    // The draft of "two " was dropped: the next one starts past the text held.
    view.draft(&text_draft(11, 8, "three "), SIZE);
    let gap = view.frame(SIZE, at(32));
    assert!(!gap.contains("three"), "{}", readable(&gap));
    view.envelope(&sent(12, 200, updated(0, "one two three ")), SIZE, false);
    // The update shows whole in the next frame.
    let healed = view.frame(SIZE, at(200));
    assert!(
        healed
            .ends_with("one two three\n\x1b[33m\u{2022}\x1b[0m \x1b[2mwriting\x1b[0m\n\x1b[?2026l"),
        "{}",
        readable(&healed)
    );
}

#[test]
fn a_draft_of_another_turn_an_older_one_and_one_of_a_completed_message_change_nothing() {
    let mut view = started_view(Look::default());
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    view.envelope(&sent(12, 10, completed(0, "Done.")), SIZE, false);
    view.frame(SIZE, at(10));
    let other: TurnId = "019a9b1c-3d00-7a10-8b20-0000000000ff".parse().unwrap();
    let foreign = Draft { turn_id: other, ..text_draft(12, 0, "x") };
    assert_eq!(view.draft(&foreign, SIZE), Step::default());
    assert_eq!(view.draft(&text_draft(12, 0, "Done. Again"), SIZE), Step::default());
    let thinking = DraftPart::Reasoning { offset: 0, delta: "x".to_owned(), title: None };
    view.draft(&draft(11, thinking), SIZE);
    assert!(!view.frame(SIZE, at(20)).contains("thinking"), "older than event 12");
}

#[test]
fn drafts_after_a_steer_still_show() {
    let mut view = started_view(Look::default());
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    // The conversation records the steer, so the turn's drafts still come after 11.
    let steered = Event::TurnSteered { turn_id: turn(), text: "also the docs".to_owned() };
    view.envelope(&sent(12, 5, steered), SIZE, false);
    view.frame(SIZE, at(5));
    let thinking = DraftPart::Reasoning {
        offset: 0,
        delta: "x".to_owned(),
        title: Some("Reading the docs".to_owned()),
    };
    view.draft(&draft(11, thinking), SIZE);
    assert!(view.frame(SIZE, at(10)).contains("thinking: Reading the docs"));
    view.draft(&text_draft(11, 0, "Hello"), SIZE);
    assert!(view.frame(SIZE, at(20)).contains("Hello"));
}

#[test]
fn drafts_show_thinking_and_preparing_until_the_call_starts_and_completes() {
    let mut view = started_view(Look::default());
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    view.frame(SIZE, at(0));
    let reasoning = |title: Option<&str>| DraftPart::Reasoning {
        offset: 0,
        delta: "**Reading the test output**\n".to_owned(),
        title: title.map(str::to_owned),
    };
    view.draft(&draft(11, reasoning(None)), SIZE);
    assert!(view.frame(SIZE, at(10)).contains("thinking\x1b[0m"));
    view.draft(&draft(11, reasoning(Some("Reading the \x1b[2Jtest output"))), SIZE);
    let titled = view.frame(SIZE, at(20));
    assert!(
        titled.contains("thinking: Reading the \u{241b}[2Jtest output"),
        "{}",
        readable(&titled)
    );
    let input =
        |call: u32, bytes: u64| DraftPart::ToolInput { call, tool: "write_file".to_owned(), bytes };
    view.draft(&draft(11, input(1, 3_250)), SIZE);
    view.draft(&draft(11, input(0, 9_000)), SIZE);
    let preparing = view.frame(SIZE, at(30));
    assert!(preparing.contains("preparing write_file, 3.2 KB"), "{}", readable(&preparing));
    view.envelope(&sent(12, 40, tool_started("ls")), SIZE, false);
    // The call's line carries the spinner, so the status row hides while it runs.
    let running = view.frame(SIZE, at(40));
    assert!(running.contains("$ ls"), "{}", readable(&running));
    assert!(
        !running.contains("preparing") && !running.contains("waiting"),
        "{}",
        readable(&running)
    );
    let done = Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: String::new(),
        truncated: false,
        is_error: false,
        exit_code: Some(0),
        sandbox: None,
        refusal: None,
    };
    view.envelope(&sent(13, 50, done), SIZE, false);
    assert!(view.frame(SIZE, at(50)).contains("waiting for the model"));
}

/// The end of a turn on a terminal with `look`, `millis` after its start.
fn ended(look: Look, end: Event, millis: i64) -> String {
    let mut view = started_view(look);
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    view.envelope(&sent(12, 100, completed(0, "Done.")), SIZE, false);
    view.frame(SIZE, at(100));
    let step = view.envelope(&sent(13, millis, end), SIZE, false);
    assert!(step.end.is_some());
    readable(&view.frame(SIZE, at(millis)))
}

#[test]
fn a_completed_turn_ends_with_its_time_and_tokens() {
    let usage = Usage { input_tokens: 18_250, output_tokens: 1_100 };
    let end = Event::TurnCompleted { turn_id: turn(), usage: Some(usage) };
    insta::assert_snapshot!(ended(ALL, end, 42_000));
}

#[test]
fn an_interrupted_turn_ends_with_its_time() {
    let end = Event::TurnInterrupted { turn_id: turn() };
    let frame = ended(ALL, end, 12_400);
    assert!(frame.contains("interrupted after 12s"), "{frame}");
    assert!(frame.contains("\\e]9;4;0\\e\\"), "{frame}");
}

#[test]
fn a_failed_turn_has_no_end_line_and_marks_the_bar_failed() {
    let error = ErrorBody::new(ErrorCode::Internal, "boom");
    let frame = ended(ALL, Event::TurnFailed { turn_id: turn(), error }, 3_000);
    assert!(!frame.contains("done"), "{frame}");
    assert!(frame.contains("\\e]9;4;2;100\\e\\"), "{frame}");
    assert!(frame.ends_with("\\e[?25h\\e]9;4;2;100\\e\\"), "{frame}");
}

#[test]
fn without_the_summary_a_turn_ends_as_before() {
    let end = Event::TurnCompleted { turn_id: turn(), usage: None };
    let frame = ended(Look::default(), end, 5_000);
    assert!(!frame.contains("done"), "{frame}");
    let frame = ended(Look::default(), Event::TurnInterrupted { turn_id: turn() }, 5_000);
    assert!(frame.contains("interrupted\\e[0m"), "{frame}");
}

#[test]
fn the_progress_bar_runs_on_every_tick_and_clears_at_the_end() {
    let mut view = started_view(ALL);
    assert!(view.frame(SIZE, at(0)).ends_with(progress::RUNNING));
    assert_eq!(view.restore(), format!("\x1b[?25h{}", progress::CLEAR));
    assert!(view.tick(SIZE, at(100)).ends_with(progress::RUNNING), "sent again on a tick");
    assert!(!view.frame(SIZE, at(110)).contains(progress::RUNNING), "not on a plain frame");
    view.close();
    let last = view.frame(SIZE, at(120));
    assert!(last.ends_with(&format!("\x1b[?25h{}", progress::CLEAR)), "{}", readable(&last));
    assert_eq!(view.restore(), "");
    assert!(!view.ticks());
    assert_eq!(view.tick(SIZE, at(200)), "");
}

#[test]
fn without_the_bar_no_osc_9_goes_out() {
    let mut view = started_view(Look { progress: false, ..ALL });
    let mut out = view.frame(SIZE, at(0));
    out.push_str(&view.tick(SIZE, at(100)));
    view.close();
    out.push_str(&view.frame(SIZE, at(200)));
    assert!(!out.contains("\x1b]9;"), "{}", readable(&out));
}

#[test]
fn without_colour_the_status_row_keeps_bold_and_dim_only() {
    let mut view = TurnView::new(turn(), RenderOptions::new(40).with_colour(ColourMode::None))
        .with_look(Look { motion: true, ..Look::default() });
    view.start();
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    view.frame(SIZE, at(0));
    let frames = [view.tick(SIZE, at(200)), view.tick(SIZE, at(2_300))].join("\n---\n");
    insta::assert_snapshot!(readable(&frames));
}

#[test]
fn piped_output_has_no_status_row_and_no_escape_sequences() {
    let mut view =
        TurnView::new(turn(), RenderOptions::new(40).with_terminal(false)).with_look(ALL);
    view.start();
    assert!(!view.ticks());
    let mut out = view.frame(SIZE, at(0));
    for (seq, event) in (11..).zip([
        turn_started(),
        updated(0, "The disk"),
        completed(0, "The disk is full."),
        Event::TurnCompleted {
            turn_id: turn(),
            usage: Some(Usage { input_tokens: 5, output_tokens: 1 }),
        },
    ]) {
        let step = view.envelope(&sent(seq, 0, event), SIZE, false);
        out.push_str(&step.out);
        out.push_str(&step.err);
        out.push_str(&view.tick(SIZE, at(100)));
    }
    view.draft(&text_draft(12, 8, " is"), SIZE);
    assert_eq!(out, "The disk is full.\n");
    assert_eq!(view.restore(), "");
}

#[test]
fn a_call_that_waits_for_its_approval_shows_no_line_and_its_time_counts_from_the_answer() {
    let mut view = started_view(Look { motion: true, ..Look::default() });
    view.envelope(&sent(11, 0, turn_started()), SIZE, true);
    view.envelope(&sent(12, 0, tool_started("make install")), SIZE, true);
    view.envelope(&sent(13, 0, approval(None)), SIZE, true);
    let asked = view.frame(SIZE, at(0));
    assert!(!asked.contains("$ make install"), "{}", readable(&asked));
    view.answered(call(), ApprovalDecision::Allow, SIZE);
    view.envelope(&sent(14, 30_000, resolved(Origin::Shell)), SIZE, true);
    let running = view.frame(SIZE, at(30_000));
    assert!(running.contains("$ make install"), "{}", readable(&running));
    assert!(!running.contains("waiting for"), "the call's line replaces the row");
    let done = view.envelope(&sent(15, 32_500, call_completed(0)), SIZE, true);
    let out = readable(&framed(done, &mut view).out);
    assert!(out.contains("$ make install\\e[0m  \\e[2m2.5s"), "{out}");
}

#[test]
fn a_denied_call_writes_no_line_of_its_own() {
    let mut view = terminal_view();
    framed(view.event(&tool_started("rm -rf build"), SIZE, true), &mut view);
    framed(view.event(&approval(None), SIZE, true), &mut view);
    let denied = framed(view.answered(call(), ApprovalDecision::Deny, SIZE), &mut view);
    assert!(!readable(&denied.out).contains("$ rm"), "{}", readable(&denied.out));
    let out = framed(view.event(&refused_call_completed(), SIZE, true), &mut view).out;
    assert!(!out.contains("$ rm"), "{}", readable(&out));
}

#[test]
fn the_cli_lines_take_their_colours_from_the_palette() {
    use efr_render::{Colour, Palette, Role};
    let palette = Palette::new()
        .with(Role::Accent, Colour::Rgb(0xf2, 0xc1, 0x4e))
        .with(Role::Muted, Colour::Palette(8))
        .with(Role::Warning, Colour::Palette(5))
        .with(Role::Error, Colour::Rgb(0xe0, 0x6c, 0x75));
    let mut shown = Vec::new();
    for (name, colour, palette) in [
        ("16 colours, default palette", ColourMode::Ansi16, Palette::new()),
        ("16 colours, palette", ColourMode::Ansi16, palette.clone()),
        ("truecolor, palette", ColourMode::TrueColor, palette.clone()),
        ("NO_COLOR, palette", ColourMode::None, palette),
    ] {
        let options = RenderOptions::new(40).with_colour(colour).with_palette(palette);
        let mut view =
            TurnView::new(turn(), options).with_look(Look { motion: true, ..Look::default() });
        view.start();
        let mut frames = vec![view.frame(SIZE, at(0))];
        view.envelope(&sent(11, 0, tool_started("make")), SIZE, true);
        view.event(&output("error: no rule\n"), SIZE, true);
        frames.push(view.frame(SIZE, at(1_000)));
        view.envelope(&sent(12, 2_000, call_completed(2)), SIZE, true);
        view.event(&approval(None), SIZE, true);
        frames.push(view.frame(SIZE, at(2_000)));
        let frames: Vec<String> = frames.iter().map(|frame| readable(frame)).collect();
        shown.push(format!("{name}:\n{}", frames.join("\n---\n")));
    }
    insta::assert_snapshot!(shown.join("\n===\n"));
}
