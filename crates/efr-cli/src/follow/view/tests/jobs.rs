//! Background jobs in the view: the lines of a job's end, a matched line and a held
//! notice and a port of a job, which belong to the conversation and show while any turn
//! of it is followed; the mark of a turn that a job notice started; the question of the stop
//! key; the call that the background key moves, its question and the block of a moved
//! call.

use efr_protocol::{Event, ForwardOutcome, HeldReason, JobId, JobState, JobStopBy, Seq, TurnId};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::{Ask, Preview, TurnView};
use super::{SIZE, feed, framed, raw_view, terminal_view};
use crate::testing::{bare, call, job, turn};

/// The `job_ended` of job 3 as `stopped_by` ended it, with `survivors`.
fn job_ended(stopped_by: Option<JobStopBy>, survivors: &[&str]) -> Event {
    Event::JobEnded {
        job_id: JobId::nth(3),
        state: if stopped_by.is_some() { JobState::Stopped } else { JobState::Exited },
        exit_code: if stopped_by.is_some() { None } else { Some(0) },
        stopped_by,
        output_bytes: 1_200_000,
        ran_secs: 92 * 60,
        tail: "listening on :3000\n".to_owned(),
        surface_changes: Vec::new(),
        quarantined: false,
        survivors: survivors.iter().map(|name| (*name).to_owned()).collect(),
        launcher_lost: false,
        output_truncated: false,
        blocked: Vec::new(),
        blocked_more: 0,
        bus_refused: Vec::new(),
        bus_refused_more: 0,
    }
}

/// A turn of the conversation that the view does not follow.
fn other_turn() -> TurnId {
    "019a9b1c-3d00-7a10-8b20-0000000000f1".parse().unwrap()
}

/// The notice of job 3 in turn `turn_id`; `woke` when the notice started the turn.
fn notice(turn_id: TurnId, woke: bool) -> Event {
    Event::JobNotice {
        conversation_id: crate::testing::conversation(),
        turn_id,
        job_id: JobId::nth(3),
        notice_kind: efr_protocol::NoticeKind::Exited,
        woke,
        source_seq: Seq::new(4),
        not_user_input: true,
        text: "<job id=\"j3\" state=\"exited\" exit=\"0\">".to_owned(),
    }
}

#[test]
fn view_job_end_outside_turn() {
    let mut view = terminal_view();
    let (out, _, end) = feed(&mut view, &[job_ended(Some(JobStopBy::User), &[])], true);
    assert_eq!(end, None, "a job's end ends no turn");
    // A line wider than the screen goes on in the next row, whole.
    assert!(
        bare(&out).contains("job j3 stopped by you after 92 minutes;\n  1.2 MB output\n"),
        "{out:?}"
    );

    // Off a terminal the line goes to stderr, and stdout keeps the reply alone.
    let mut view = raw_view();
    let (out, err, _) = feed(&mut view, &[job_ended(None, &[])], true);
    assert_eq!(out, "");
    assert_eq!(err, "job j3 ended with exit 0 after 92 minutes; 1.2 MB output\n");
}

#[test]
fn survivors_named_in_view() {
    let mut view = raw_view();
    let (_, err, _) = feed(&mut view, &[job_ended(Some(JobStopBy::Model), &["node", "sh"])], true);
    assert_eq!(
        err,
        "job j3 stopped by the model after 92 minutes; 1.2 MB output; 2 processes of the job \
         may still run\n"
    );
}

#[test]
fn held_notice_reason_in_view() {
    let mut view = raw_view();
    let held =
        |reason| Event::JobNoticeHeld { job_id: JobId::nth(3), source_seq: Seq::new(4), reason };
    let (_, err, _) = feed(
        &mut view,
        &[held(HeldReason::NotAuto), held(HeldReason::WakeLimit), held(HeldReason::WakeTurnJob)],
        true,
    );
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(
        lines,
        [
            "job j3: no wake turn (the conversation is not in auto); the notice waits for your \
             next prompt",
            "job j3: no wake turn (wake limit reached); the notice waits for your next prompt",
            "job j3: no wake turn (job of a wake turn); the notice waits for your next prompt",
        ]
    );
}

#[test]
fn watch_line_shows_on_one_line() {
    let mut view = raw_view();
    let matched = Event::JobWatchMatched {
        job_id: JobId::nth(3),
        line: "error\u{1b}[31m in \u{202e}main.rs".to_owned(),
        wakes_left: 0,
    };
    let (_, err, _) = feed(&mut view, &[matched], true);
    assert_eq!(err, "job j3 matched: error\u{241b}[31m in \u{fffd}main.rs\n");
}

/// The `job_port_forwarded` of job 1 for `127.0.0.1:<port>` with `outcome`.
fn port_event(port: u16, outcome: ForwardOutcome) -> Event {
    Event::JobPortForwarded {
        job_id: JobId::nth(1),
        listen: "127.0.0.1".to_owned(),
        port,
        outcome,
        by_job: None,
        reason: (outcome == ForwardOutcome::Refused).then(|| "a service port".to_owned()),
    }
}

#[test]
fn forward_line_outside_turn() {
    // Off a terminal the line goes to stderr, plain, with no link.
    let mut view = raw_view();
    let events =
        [port_event(8765, ForwardOutcome::Forwarded), port_event(6379, ForwardOutcome::Refused)];
    let (out, err, end) = feed(&mut view, &events, true);
    assert_eq!(end, None, "a forward ends no turn");
    assert_eq!(out, "");
    assert_eq!(
        err,
        "job j1 listens on 127.0.0.1:8765 -> http://localhost:8765\n\
         job j1 listens on 127.0.0.1:6379; not forwarded: efr does not forward port 6379 (a \
         service port)\n"
    );

    // On a terminal the URL is an OSC 8 link, also when the line goes on in the next
    // row; a refusal links nothing.
    let mut view = terminal_view();
    let (out, _, _) = feed(&mut view, &[port_event(8765, ForwardOutcome::Forwarded)], true);
    assert!(
        out.contains(
            "\u{1b}]8;;http://localhost:8765\u{1b}\\http://localhost:8765\u{1b}]8;;\u{1b}\\"
        ),
        "{out:?}"
    );
    assert!(
        bare(&out).contains("job j1 listens on 127.0.0.1:8765 ->\n  http://localhost:8765\n"),
        "{out:?}"
    );
    let mut view = terminal_view();
    let (out, _, _) = feed(&mut view, &[port_event(6379, ForwardOutcome::Refused)], true);
    assert!(!out.contains("\u{1b}]8;;"), "{out:?}");
    assert!(bare(&out).contains("6379"), "{out:?}");
}

#[test]
fn wake_turn_marked_in_view() {
    // A prompt that waits behind a turn that a job notice started says so, once.
    let mut view = raw_view();
    view.queue();
    let (_, err, _) =
        feed(&mut view, &[notice(other_turn(), true), notice(other_turn(), true)], true);
    assert_eq!(err, "\u{1f916} (job notice, not your input)\n");

    // A notice that rides in the followed turn is named, and is never the user's.
    let mut view = raw_view();
    let (_, err, _) = feed(&mut view, &[notice(turn(), false)], true);
    assert_eq!(err, "the model also reads a notice of job j3; it is not your input\n");
}

#[test]
fn a_background_call_says_so_and_its_block_names_the_job() {
    let mut view = raw_view();
    let started = Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input: json!({ "command": "npm run dev", "background": true }),
        manual_input: false,
        launch: None,
        freeform: false,
    };
    let job_started = Event::JobStarted {
        turn_id: turn(),
        call_id: call(),
        job: job(3, "npm run dev"),
        launcher_pid: 4242,
        launcher_start: 99,
    };
    let completed = Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: "job j3 started".to_owned(),
        truncated: false,
        is_error: false,
        exit_code: None,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: false,
    };
    let (_, err, _) = feed(&mut view, &[started, job_started, completed], true);
    assert_eq!(
        err,
        "\u{b7} $ npm run dev (background)\n  \u{2713}\n  job j3 started; output \
         $SCRATCH/jobs/j3.out; ends in 180 minutes\n"
    );
}

#[test]
fn stop_key_question_needs_yes() {
    let mut view = terminal_view();
    let step = framed(view.stop_question(&job(3, "npm run dev"), SIZE), &mut view);
    assert_eq!(step.ask, Some(Ask::StopJob));
    let shown = bare(&step.out);
    assert!(shown.contains("? stop job j3 (npm run dev)"), "{shown:?}");
    assert!(shown.contains("y yes \u{b7} n no"), "{shown:?}");

    let (step, stopped) = view.stop_answered(false, SIZE);
    assert_eq!(stopped, None, "n stops nothing");
    let step = framed(step, &mut view);
    assert!(bare(&step.out).contains("\u{2717} job j3 runs on"), "{:?}", step.out);
    assert_eq!(view.stop_answered(true, SIZE).1, None, "no question waits now");

    let mut view: TurnView = terminal_view();
    framed(view.stop_question(&job(3, "npm run dev"), SIZE), &mut view);
    let (step, stopped) = view.stop_answered(true, SIZE);
    assert_eq!(stopped, Some(JobId::nth(3)));
    assert!(bare(&framed(step, &mut view).out).contains("\u{2713} stopping job j3"));
}

/// The start of the test's shell call, with `input`.
fn shell_with(input: serde_json::Value) -> Event {
    Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input,
        manual_input: false,
        launch: None,
        freeform: false,
    }
}

/// The `job_started` of job 3, which the user moved from the test's call after 42 s,
/// with `exits` that the move covered.
fn moved_job_started(exits: Vec<efr_protocol::ExitInfo>) -> Event {
    let mut job = job(3, "make deploy");
    job.ends_by = job.started_at + jiff::SignedDuration::from_mins(60);
    job.moved_from =
        Some(efr_protocol::MovedCall { call_id: call(), turn_id: turn(), after_secs: 42, exits });
    Event::JobStarted {
        turn_id: turn(),
        call_id: call(),
        job,
        launcher_pid: 4242,
        launcher_start: 99,
    }
}

/// The end of the test's call with no exit code: a moved call ends so.
fn completed_without_code() -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: "[the user moved this command to the background after 42s: job j3.]".to_owned(),
        truncated: false,
        is_error: false,
        exit_code: None,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: false,
    }
}

#[test]
fn ctrl_b_moves_only_the_running_shell_call_of_the_followed_turn() {
    let mut view = terminal_view();
    assert_eq!(view.background_call(), None, "no call runs");
    feed(&mut view, &[shell_with(json!({ "command": "make deploy" }))], true);
    assert_eq!(view.background_call(), Some(call()));
    view.moved(call(), JobId::nth(3));
    assert_eq!(view.background_call(), None, "a moved call moves once");

    // A call that starts a job runs in the background already.
    let mut view = terminal_view();
    feed(&mut view, &[shell_with(json!({ "command": "npm run dev", "background": true }))], true);
    assert_eq!(view.background_call(), None);

    // A file tool has no command to move.
    let mut view = terminal_view();
    let read = Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "read_file".to_owned(),
        input: json!({ "path": "a.txt" }),
        manual_input: false,
        launch: None,
        freeform: false,
    };
    feed(&mut view, &[read], true);
    assert_eq!(view.background_call(), None);

    // NOTE: the call of a turn that the followed prompt waits behind, such as a turn
    // that a job notice started, is not the view's to move.
    let mut view = terminal_view();
    view.queue();
    let ahead = Event::ToolCallStarted {
        turn_id: other_turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input: json!({ "command": "sleep 600" }),
        manual_input: false,
        launch: None,
        freeform: false,
    };
    feed(&mut view, &[notice(other_turn(), true), ahead], true);
    assert_eq!(view.background_call(), None);
}

#[test]
fn move_question_needs_yes() {
    let mut view = terminal_view();
    feed(&mut view, &[shell_with(json!({ "command": "make deploy" }))], true);
    let mut exit =
        crate::testing::exit_info(&[efr_protocol::ExitKind::Write], efr_protocol::Launch::Direct);
    exit.facts = vec!["a job asks in cautious".to_owned()];
    let exits = [exit];
    let preview = Preview { token: 7, command: "make deploy", max_minutes: 60, exits: &exits };
    let step = framed(view.move_question(call(), &preview, SIZE), &mut view);
    assert_eq!(step.ask, Some(Ask::Move { call_id: call(), token: 7 }));
    let shown = bare(&step.out);
    assert!(shown.contains("? move this command to the background"), "{shown:?}");
    assert!(shown.contains("make deploy"), "{shown:?}");
    assert!(shown.contains("it keeps running with your full rights"), "{shown:?}");
    assert!(shown.contains("as a job: write; efr: a job asks in"), "{shown:?}");
    assert!(shown.contains("y move \u{b7} n no"), "{shown:?}");

    let (step, moved) = view.move_answered(false, SIZE);
    assert_eq!(moved, None, "n moves nothing");
    let step = framed(step, &mut view);
    assert!(bare(&step.out).contains("\u{2717} the command runs on here"), "{:?}", step.out);
    assert_eq!(view.move_answered(true, SIZE).1, None, "no question waits now");

    framed(view.move_question(call(), &preview, SIZE), &mut view);
    let (step, moved) = view.move_answered(true, SIZE);
    assert_eq!(moved, Some(call()));
    let shown = bare(&framed(step, &mut view).out);
    assert!(shown.contains("\u{2713} moving the command to the background"), "{shown:?}");
}

#[test]
fn a_moved_call_ends_with_its_job_and_no_check_mark() {
    let mut view = raw_view();
    let events = [
        shell_with(json!({ "command": "make deploy" })),
        moved_job_started(Vec::new()),
        completed_without_code(),
    ];
    let (_, err, _) = feed(&mut view, &events, true);
    assert_eq!(
        err,
        "\u{b7} $ make deploy\n  (moved to job j3)\n  moved to job j3 by you after 42 seconds; \
         output $SCRATCH/jobs/j3.out; ends in 60 minutes\n"
    );
}
