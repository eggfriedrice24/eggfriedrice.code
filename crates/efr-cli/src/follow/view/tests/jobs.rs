//! Background jobs in the view: the lines of a job's end, a matched line and a held
//! notice, which belong to the conversation and show while any turn of it is
//! followed; the mark of a turn that a job notice started.

use efr_protocol::{Event, HeldReason, JobId, JobState, JobStopBy, Seq, TurnId};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{feed, raw_view, terminal_view};
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
    };
    let (_, err, _) = feed(&mut view, &[started, job_started, completed], true);
    assert_eq!(
        err,
        "\u{b7} $ npm run dev (background)\n  \u{2713}\n  job j3 started; output \
         $SCRATCH/jobs/j3.out; ends in 180 minutes\n"
    );
}
