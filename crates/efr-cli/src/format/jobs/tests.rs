use std::path::Path;

use efr_protocol::{
    BlockReason, Blocked, Event, ForwardOutcome, HeldReason, JobId, JobState, JobStopBy,
    PortForward, Seq, SurfaceChange,
};
use efr_render::RenderOptions;
use pretty_assertions::assert_eq;

use super::{
    Ended, ListRow, conversation_line, ended, held, link_of, linked, list, matched, minutes,
    move_exit, move_risk, moved_exits, not_moved, output_name, ports, started, tail_lines,
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
        blocked: Vec::new(),
        blocked_more: 0,
        bus_refused: Vec::new(),
        bus_refused_more: 0,
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
    assert_eq!(
        line(job_ended(JobState::Stopped, None, Some(JobStopBy::FullOff))),
        "job j3 stopped: full mode was turned off"
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

/// The `job_ended` of job 3 that exited 0, with these refusals of the proxy.
fn refused_job(blocked: Vec<Blocked>, more: u32) -> Event {
    let mut event = job_ended(JobState::Exited, Some(0), None);
    if let Event::JobEnded { blocked: refused, blocked_more, .. } = &mut event {
        *refused = blocked;
        *blocked_more = more;
    }
    event
}

#[test]
fn job_end_lists_blocked() {
    let refused =
        |host: &str, reason, count| Blocked { host: host.to_owned(), port: 443, reason, count };
    let blocked = vec![
        refused("other.test", BlockReason::NotAllowed, 3),
        refused("x.test", BlockReason::NotTls, 1),
    ];
    assert_eq!(
        conversation_line(&refused_job(blocked, 4)).unwrap(),
        "job j3 ended with exit 0 after 92 minutes; 1.2 MB output; network: blocked \
         other.test:443 (not on the allow list; 3 times), x.test:443 (not TLS) and 4 more"
    );
    assert_eq!(
        conversation_line(&refused_job(Vec::new(), 1)).unwrap(),
        "job j3 ended with exit 0 after 92 minutes; 1.2 MB output; network: 1 refused"
    );
    assert_eq!(
        conversation_line(&refused_job(Vec::new(), 0)).unwrap(),
        "job j3 ended with exit 0 after 92 minutes; 1.2 MB output"
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

/// The `job_port_forwarded` of job 1 for `listen`:`port`.
fn port_event(
    listen: &str,
    port: u16,
    outcome: ForwardOutcome,
    by_job: Option<u32>,
    reason: Option<&str>,
) -> Event {
    Event::JobPortForwarded {
        job_id: JobId::nth(1),
        listen: listen.to_owned(),
        port,
        outcome,
        by_job: by_job.map(JobId::nth),
        reason: reason.map(str::to_owned),
    }
}

#[test]
fn each_forward_outcome_has_its_line() {
    let events = [
        port_event("127.0.0.1", 8765, ForwardOutcome::Forwarded, None, None),
        port_event("::1", 5173, ForwardOutcome::Forwarded, None, None),
        port_event("127.0.0.1", 8765, ForwardOutcome::InUse, None, None),
        port_event("127.0.0.1", 8765, ForwardOutcome::InUse, Some(2), None),
        port_event("127.0.0.1", 6379, ForwardOutcome::Refused, None, Some("a service port")),
        port_event("127.0.0.1", 6379, ForwardOutcome::Refused, None, None),
        port_event(
            "127.0.0.1",
            9000,
            ForwardOutcome::Limit,
            None,
            Some("the job forwards 8 ports already"),
        ),
        port_event(
            "::1",
            9001,
            ForwardOutcome::Limit,
            None,
            Some("efrd forwards 32 ports already"),
        ),
        port_event("127.0.0.1", 9002, ForwardOutcome::Limit, None, None),
        port_event(
            "127.0.0.1",
            8765,
            ForwardOutcome::Stopped,
            None,
            Some("jobs.forward_ports is off"),
        ),
        port_event("::1", 5173, ForwardOutcome::Stopped, None, None),
        // An outcome that this efr does not know shows the plain listen, without its
        // reason.
        port_event("127.0.0.1", 8766, ForwardOutcome::Unknown, None, Some("a new reason")),
    ];
    let lines: Vec<String> = events.iter().map(|event| conversation_line(event).unwrap()).collect();
    insta::assert_snapshot!(lines.join("\n"));
}

#[test]
fn a_forward_line_is_safe_to_print() {
    let event = port_event(
        "127.0.0.1\u{1b}[2J",
        6379,
        ForwardOutcome::Refused,
        None,
        Some("a service\nport \u{202e}x"),
    );
    let line = conversation_line(&event).unwrap();
    assert_eq!(
        line,
        "job j1 listens on 127.0.0.1\u{241b}[2J:6379; not forwarded: efr does not forward port \
         6379 (a service port \u{fffd}x)"
    );
}

#[test]
fn only_a_forwarded_port_links_its_url() {
    let forwarded = port_event("::1", 5173, ForwardOutcome::Forwarded, None, None);
    assert_eq!(link_of(&forwarded).as_deref(), Some("http://localhost:5173"));
    for outcome in [ForwardOutcome::InUse, ForwardOutcome::Refused, ForwardOutcome::Stopped] {
        assert_eq!(link_of(&port_event("::1", 5173, outcome, None, None)), None, "{outcome:?}");
    }
    assert_eq!(link_of(&job_ended(JobState::Exited, Some(0), None)), None);
}

#[test]
fn the_url_is_an_osc8_link_on_a_terminal_only() {
    let line = "job j1 listens on 127.0.0.1:8765 -> http://localhost:8765";
    let url = Some("http://localhost:8765");
    let terminal = RenderOptions::new(80);
    assert_eq!(
        linked(line, url, &terminal),
        "job j1 listens on 127.0.0.1:8765 -> \u{1b}]8;;http://localhost:8765\u{1b}\\\
         http://localhost:8765\u{1b}]8;;\u{1b}\\"
    );
    // Not a terminal, links off, no URL, or a URL that the line does not hold whole:
    // the line stays as it is.
    assert_eq!(linked(line, url, &RenderOptions::new(80).with_terminal(false)), line);
    assert_eq!(linked(line, url, &RenderOptions::new(80).with_hyperlinks(false)), line);
    assert_eq!(linked(line, None, &terminal), line);
    let cut = "job j1 listens on 127.0.0.1:8765 -> http://local";
    assert_eq!(linked(cut, url, &terminal), cut);
}

#[test]
fn a_running_job_lists_its_forwarded_ports() {
    let mut running = job(2, "python3 -m http.server 8765");
    assert_eq!(ports(&running), None, "no forward, no ports");
    running.forwards = vec![
        PortForward { listen: "127.0.0.1".to_owned(), port: 8765 },
        PortForward { listen: "::1".to_owned(), port: 5173 },
    ];
    assert_eq!(ports(&running).as_deref(), Some("ports 8765, 5173"));
    let mut ended = running.clone();
    ended.id = JobId::nth(1);
    ended.state = JobState::Stopped;
    ended.stopped_by = Some(JobStopBy::User);
    ended.ended_at = Some(now() - jiff::SignedDuration::from_mins(5));
    assert_eq!(ports(&ended), None, "an ended job forwards nothing");
    let rows = [
        ListRow { job: &running, conversation: None, leftover: false },
        ListRow { job: &ended, conversation: None, leftover: false },
    ];
    assert_eq!(
        list(&rows, now()),
        "j2  running         ends in 3h 0m    0 B  python3 -m http.server 8765  (ports 8765, 5173)\n\
         j1  stopped by you  ended 5m 0s ago  0 B  python3 -m http.server 8765\n"
    );
}

/// Job 3, which the user moved from a call after `after_secs`, with `exits`.
fn moved(after_secs: u64, exits: Vec<efr_protocol::ExitInfo>) -> efr_protocol::JobInfo {
    let mut job = job(3, "make deploy");
    job.ends_by = job.started_at + jiff::SignedDuration::from_mins(60);
    job.moved_from = Some(efr_protocol::MovedCall {
        call_id: crate::testing::call(),
        turn_id: crate::testing::turn(),
        after_secs,
        exits,
        launcher: false,
    });
    job
}

#[test]
fn a_moved_job_starts_with_who_moved_it_and_when() {
    assert_eq!(
        started(&moved(42, Vec::new())),
        "moved to job j3 by you after 42 seconds; output $SCRATCH/jobs/j3.out; ends in 60 minutes"
    );
    assert!(started(&moved(150, Vec::new())).contains("after 2 minutes;"));
    assert_eq!(moved_exits(&moved(42, Vec::new()), None), Vec::<String>::new());
    assert_eq!(moved_exits(&job(3, "npm run dev"), None), Vec::<String>::new());
    let mut exit =
        crate::testing::exit_info(&[efr_protocol::ExitKind::Write], efr_protocol::Launch::Direct);
    exit.facts = vec!["a job asks in\u{1b}[2J cautious".to_owned()];
    assert_eq!(
        moved_exits(&moved(42, vec![exit]), None),
        ["your move covered: write; efr: a job asks in\u{241b}[2J cautious"],
        "efr's facts go through one_line"
    );
}

#[test]
fn the_texts_of_the_background_key() {
    assert_eq!(
        move_risk(60),
        "it keeps running with your full rights after the turn, for up to 60 minutes"
    );
    assert!(move_risk(1).ends_with("for up to 1 minute"));
    assert_eq!(
        not_moved("the command waits\u{1b}[2J for input"),
        "the command was not moved: the command waits\u{241b}[2J for input"
    );
    let exit =
        crate::testing::exit_info(&[efr_protocol::ExitKind::Host], efr_protocol::Launch::Direct);
    assert_eq!(move_exit(&exit, None), "as a job: network");
}

#[test]
fn the_list_marks_a_moved_job() {
    let job = moved(42, Vec::new());
    let rows = [ListRow { job: &job, conversation: None, leftover: false }];
    assert_eq!(
        list(&rows, now()),
        "j3  running  ends in 1h 0m  0 B  make deploy  (moved by you)\n"
    );
}

#[test]
fn the_list_marks_a_moved_job_that_waits_for_input() {
    let mut job = moved(42, Vec::new());
    job.waits_input = true;
    let rows = [ListRow { job: &job, conversation: None, leftover: false }];
    assert_eq!(
        list(&rows, now()),
        "j3  running  ends in 1h 0m  0 B  make deploy  (moved by you)  (waits for input; efr \
         jobs stop ends it)\n"
    );
    // NOTE: a job that ended waits for nothing.
    job.state = JobState::Exited;
    let rows = [ListRow { job: &job, conversation: None, leftover: false }];
    assert!(!list(&rows, now()).contains("waits for input"));
}
