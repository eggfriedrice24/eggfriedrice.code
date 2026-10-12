use std::path::Path;

use efr_protocol::{Event, HeldReason, JobId, JobState, JobStopBy, Seq, SurfaceChange};
use pretty_assertions::assert_eq;

use super::{
    Ended, ListRow, conversation_line, ended, held, list, matched, minutes, output_name, started,
    tail_lines,
};
use crate::testing::{job, now};

fn j3() -> JobId {
    JobId::nth(3)
}

/// The `job_ended` of job 3 with these facts; the rest empty.
fn job_ended(state: JobState, exit_code: Option<i32>, stopped_by: Option<JobStopBy>) -> Event {
    Event::JobEnded {
        job_id: j3(),
        state,
        exit_code,
        stopped_by,
        output_bytes: 1_200_000,
        ran_secs: 92 * 60 + 5,
        tail: String::new(),
        surface_changes: Vec::new(),
        quarantined: false,
        survivors: Vec::new(),
        launcher_lost: false,
        output_truncated: false,
    }
}

#[test]
fn a_time_reads_in_seconds_then_in_minutes() {
    assert_eq!(minutes(0), "0 seconds");
    assert_eq!(minutes(1), "1 second");
    assert_eq!(minutes(59), "59 seconds");
    assert_eq!(minutes(60), "1 minute");
    assert_eq!(minutes(119), "1 minute");
    assert_eq!(minutes(92 * 60 + 59), "92 minutes");
}

#[test]
fn the_output_file_is_named_as_the_model_names_it() {
    let path = Path::new("/home/user/.local/share/efr/scratch/jobs/j3.out");
    assert_eq!(output_name(path), "$SCRATCH/jobs/j3.out");
    assert_eq!(output_name(Path::new("/tmp/x\u{1b}.out")), "/tmp/x\u{241b}.out");
}

#[test]
fn a_start_names_the_job_its_file_and_its_time_limit() {
    assert_eq!(
        started(&job(3, "npm run dev")),
        "job j3 started; output $SCRATCH/jobs/j3.out; ends in 180 minutes"
    );
}

#[test]
fn each_end_has_its_line() {
    let line = |event: Event| conversation_line(&event).unwrap();
    assert_eq!(
        line(job_ended(JobState::Stopped, None, Some(JobStopBy::User))),
        "job j3 stopped by you after 92 minutes; 1.2 MB output"
    );
    assert_eq!(
        line(job_ended(JobState::Stopped, None, Some(JobStopBy::Model))),
        "job j3 stopped by the model after 92 minutes; 1.2 MB output"
    );
    assert_eq!(
        line(job_ended(JobState::Exited, Some(3), None)),
        "job j3 ended with exit 3 after 92 minutes; 1.2 MB output"
    );
    assert_eq!(
        line(job_ended(JobState::TimedOut, None, Some(JobStopBy::TimeLimit))),
        "job j3 timed out after 92 minutes; 1.2 MB output"
    );
    assert_eq!(
        line(job_ended(JobState::Stopped, None, Some(JobStopBy::DaemonRestart))),
        "job j3 stopped because efrd restarted"
    );
    assert_eq!(
        line(job_ended(JobState::Stopped, None, Some(JobStopBy::DaemonStop))),
        "job j3 stopped because efrd stopped"
    );
    assert_eq!(
        line(job_ended(JobState::Failed, None, None)),
        "job j3 failed after 92 minutes; 1.2 MB output"
    );
}

#[test]
fn a_guard_stop_a_lost_launcher_survivors_and_a_cut_output_are_named() {
    let change = SurfaceChange {
        path: "sub/.git/hooks/pre-commit".into(),
        rule: "hook_in_new_repository".to_owned(),
        key: None,
        quarantined: true,
    };
    let survivors = vec!["sleep".to_owned(), "node".to_owned()];
    let end = Ended {
        state: Some(JobState::Stopped),
        stopped_by: Some(JobStopBy::Guard),
        surface_changes: std::slice::from_ref(&change),
        quarantined: true,
        survivors: &survivors,
        launcher_lost: true,
        output_truncated: true,
        ..Ended::default()
    };
    assert_eq!(
        ended(&j3(), &end),
        "job j3 stopped: it changed git settings (quarantined); its launcher was lost, and \
         its processes may still run; 2 processes of the job may still run; efr could not \
         write all of its output to the file"
    );
    let one = ["sleep".to_owned()];
    let end = Ended {
        state: Some(JobState::Exited),
        exit_code: Some(0),
        survivors: &one,
        surface_changes: std::slice::from_ref(&change),
        ..Ended::default()
    };
    assert_eq!(
        ended(&j3(), &end),
        "job j3 ended with exit 0 after 0 seconds; 0 B output; it changed git settings; 1 \
         process of the job may still run"
    );
}

#[test]
fn watch_line_goes_through_one_line() {
    let line = "ready\u{1b}[2J on \u{202e}port 3000\n\tnext";
    let shown = matched(&j3(), line);
    assert_eq!(shown, "job j3 matched: ready\u{241b}[2J on \u{fffd}port 3000  next");
    let event = Event::JobWatchMatched { job_id: j3(), line: line.to_owned(), wakes_left: 1 };
    assert_eq!(conversation_line(&event).unwrap(), shown);
}

#[test]
fn a_held_notice_says_why_no_turn_started() {
    assert_eq!(
        held(&j3(), HeldReason::NotAuto),
        "job j3: no wake turn (the conversation is not in auto); the notice waits for your \
         next prompt"
    );
    let event = Event::JobNoticeHeld {
        job_id: j3(),
        source_seq: Seq::new(9),
        reason: HeldReason::WakeLimit,
    };
    assert!(conversation_line(&event).unwrap().contains("(wake limit reached)"));
    assert!(held(&j3(), HeldReason::WakeTurnJob).contains("(job of a wake turn)"));
}

#[test]
fn job_tail_drops_format_characters() {
    let tail = "one\n\u{1b}]52;c;bad\u{7}\n\nok \u{202e}txt.exe\u{200b}\nlast";
    let lines = tail_lines(tail, 3);
    assert_eq!(lines, ["\u{241b}]52;c;bad\u{2407}", "ok \u{fffd}txt.exe\u{fffd}", "last"]);
    for line in &lines {
        assert!(!line.chars().any(|c| c.is_control() || efr_stdx::text::is_format(c)), "{line:?}");
    }
}

#[test]
fn the_list_has_a_row_per_job_in_columns() {
    let running = job(2, "npm run dev\nnpm run lint");
    let mut ended = job(1, "cargo build");
    ended.state = JobState::Exited;
    ended.exit_code = Some(0);
    ended.ended_at = Some(now() - jiff::SignedDuration::from_mins(5));
    ended.output_bytes = 34_000;
    let rows = [
        ListRow { job: &running, conversation: None, leftover: false },
        ListRow { job: &ended, conversation: None, leftover: true },
    ];
    assert_eq!(
        list(&rows, now()),
        "j2  running  ends in 3h 0m        0 B  npm run dev \u{2026}\n\
         j1  exit 0   ended 5m 0s ago  34.0 KB  cargo build  (its processes may still run)\n"
    );
    let rows = [ListRow {
        job: &running,
        conversation: Some("fix the build".to_owned()),
        leftover: false,
    }];
    assert!(list(&rows, now()).starts_with("fix the build  j2  running"));
}
