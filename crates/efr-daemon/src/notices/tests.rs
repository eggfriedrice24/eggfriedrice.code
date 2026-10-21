use std::os::unix::fs::PermissionsExt as _;

use efr_protocol::{
    CallId, ConversationId, ErrorBody, ErrorCode, Event, EventEnvelope, ForwardOutcome, HeldReason,
    JobId, JobState, JobStopBy, Seq, TurnId,
};
use pretty_assertions::assert_eq;

use crate::notices::{Wake, append, file_name, line, not_run, wake_of};

fn turn() -> TurnId {
    TurnId::from_uuid(uuid::Uuid::from_u128(1))
}

#[test]
fn the_file_name_is_the_tty_without_dev_and_with_dashes() {
    assert_eq!(file_name("/dev/pts/3").as_deref(), Some("pts-3"));
    assert_eq!(file_name("/dev/tty1").as_deref(), Some("tty1"));
    assert_eq!(file_name("pts/12").as_deref(), Some("pts-12"));
}

#[test]
fn a_tty_that_could_escape_the_directory_gets_no_file() {
    assert_eq!(file_name(""), None);
    assert_eq!(file_name("/dev/"), None);
    assert_eq!(file_name(".."), None);
    assert_eq!(file_name("/dev/.."), None);
    assert_eq!(file_name("pts 3"), None);
    assert_eq!(file_name("pts\n3"), None);
}

#[test]
fn finished_and_failed_turns_and_waiting_approvals_get_a_line() {
    let completed = Event::TurnCompleted {
        turn_id: turn(),
        usage: None,
        changes: None,
        context: None,
        stop: None,
        undo: None,
    };
    let failed = Event::TurnFailed {
        turn_id: turn(),
        error: ErrorBody::new(ErrorCode::Internal, "the provider stream broke"),
        usage: None,
        context: None,
        changes: None,
        undo: None,
    };
    let approval = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: CallId::from_uuid(uuid::Uuid::from_u128(2)),
        summary: "write /etc/hosts".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
        expires_in_secs: None,
    };

    assert_eq!(
        line(&completed, Some("fix nginx"), None).as_deref(),
        Some("efr: turn finished: fix nginx")
    );
    assert_eq!(
        line(&failed, Some("fix nginx"), None).as_deref(),
        Some("efr: turn failed: fix nginx: the provider stream broke")
    );
    assert_eq!(
        line(&approval, None, None).as_deref(),
        Some("efr: approval waiting: a conversation: write /etc/hosts")
    );
}

#[test]
fn a_turn_that_failed_for_want_of_a_login_says_how_to_log_in() {
    let failed = Event::TurnFailed {
        turn_id: turn(),
        error: ErrorBody::new(ErrorCode::Unauthorized, "no provider credentials are stored"),
        usage: None,
        context: None,
        changes: None,
        undo: None,
    };

    assert_eq!(
        line(&failed, Some("fix nginx"), None).as_deref(),
        Some(
            "efr: turn failed: fix nginx: no provider credentials are stored; run efr login openai"
        )
    );
}

#[test]
fn the_parts_that_ask_follow_the_summary_on_the_same_line() {
    let approval = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: CallId::from_uuid(uuid::Uuid::from_u128(2)),
        summary: "shell: run \"uptime; hostnamectl\"\nasks for: hostnamectl".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
        expires_in_secs: None,
    };
    assert_eq!(
        line(&approval, Some("fix nginx"), None).as_deref(),
        Some(
            "efr: approval waiting: fix nginx: shell: run \"uptime; hostnamectl\"; asks for: \
             hostnamectl"
        )
    );
}

#[test]
fn the_login_hint_survives_a_long_title() {
    let failed = Event::TurnFailed {
        turn_id: turn(),
        error: ErrorBody::new(ErrorCode::Unauthorized, "the token was refused"),
        usage: None,
        context: None,
        changes: None,
        undo: None,
    };
    let title = "x".repeat(400);

    let text = line(&failed, Some(&title), None).unwrap();

    assert_eq!(text.chars().count(), 200);
    assert!(text.ends_with("...; run efr login openai"), "{text}");
}

#[test]
fn other_events_get_no_line() {
    assert_eq!(
        line(
            &Event::TurnInterrupted {
                turn_id: turn(),
                usage: None,
                context: None,
                changes: None,
                undo: None
            },
            Some("x"),
            None
        ),
        None
    );
    assert_eq!(line(&Event::TurnCancelled { turn_id: turn() }, Some("x"), None), None);
}

#[test]
fn a_notice_stays_on_one_short_line() {
    let event = Event::TurnCompleted {
        turn_id: turn(),
        usage: None,
        changes: None,
        context: None,
        stop: None,
        undo: None,
    };
    let title = format!("two\nlines\x1b[31m{}", "x".repeat(400));

    let text = line(&event, Some(&title), None).unwrap();

    assert!(!text.contains('\n') && !text.contains('\x1b'), "{text:?}");
    assert_eq!(text.chars().count(), 200);
    assert!(text.ends_with("..."));
}

#[test]
fn notices_append_to_a_private_file_per_tty() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("notices");

    let path = append(&dir, "/dev/pts/3", "efr: one").unwrap().unwrap();
    append(&dir, "/dev/pts/3", "efr: two").unwrap();
    append(&dir, "/dev/pts/4", "efr: other").unwrap();

    assert_eq!(path, dir.join("pts-3"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "efr: one\nefr: two\n");
    assert_eq!(std::fs::read_to_string(dir.join("pts-4")).unwrap(), "efr: other\n");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn a_tty_without_a_safe_name_writes_nothing() {
    let root = tempfile::tempdir().unwrap();

    assert_eq!(append(&root.path().join("notices"), "..", "efr: x").unwrap(), None);
    assert!(!root.path().join("notices").exists());
}

#[test]
fn a_prompt_that_did_not_run_is_named_by_its_conversation_and_never_quoted() {
    let conversation: ConversationId = "0192f0c1-7a00-7000-8000-000000000003".parse().unwrap();
    assert_eq!(
        not_run(conversation, 1),
        format!(
            "efr restarted; a queued prompt did not run; see it with efr history {conversation} and send it again"
        )
    );
    assert!(not_run(conversation, 3).starts_with("efr restarted; 3 queued prompts did not run;"));
}

#[test]
fn a_notice_shows_a_stand_in_for_each_format_character() {
    // The approval's summary names a path whose name turns around on the screen.
    let approval = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: CallId::from_uuid(uuid::Uuid::from_u128(2)),
        summary: "write /home/u/\u{202e}fdp.sh\u{200b}".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
        expires_in_secs: None,
    };

    let text = line(&approval, Some("title\u{2066}"), None).unwrap();

    assert!(!text.contains(['\u{202e}', '\u{200b}', '\u{2066}']), "{text:?}");
    assert!(text.ends_with("write /home/u/\u{fffd}fdp.sh\u{fffd}"), "{text:?}");
}

/// A job's end with how it ended.
fn job_ended(state: JobState, exit_code: Option<i32>, stopped_by: Option<JobStopBy>) -> Event {
    Event::JobEnded {
        job_id: JobId::nth(3),
        state,
        exit_code,
        stopped_by,
        output_bytes: 2048,
        ran_secs: 60,
        tail: "the last line".to_owned(),
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
fn job_ended_notice_line() {
    let title = Some("run the dev server");
    let cases = [
        (job_ended(JobState::Exited, Some(0), None), "ended (exit 0)"),
        (job_ended(JobState::Exited, Some(3), None), "ended (exit 3)"),
        (job_ended(JobState::TimedOut, None, Some(JobStopBy::TimeLimit)), "timed out"),
        (job_ended(JobState::Stopped, None, Some(JobStopBy::User)), "stopped by you"),
        (job_ended(JobState::Stopped, None, Some(JobStopBy::Model)), "stopped by the model"),
        (
            job_ended(JobState::Stopped, None, Some(JobStopBy::DaemonRestart)),
            "stopped because efrd restarted",
        ),
        (job_ended(JobState::Failed, None, None), "failed to start or run"),
    ];
    for (event, what) in cases {
        assert_eq!(
            line(&event, title, None).as_deref(),
            Some(format!("efr: job j3 {what}: run the dev server").as_str())
        );
    }
    let Event::JobEnded { job_id, state, exit_code, output_bytes, ran_secs, tail, .. } =
        job_ended(JobState::Stopped, None, Some(JobStopBy::Guard))
    else {
        unreachable!()
    };
    let lost = Event::JobEnded {
        job_id,
        state,
        exit_code,
        stopped_by: Some(JobStopBy::Guard),
        output_bytes,
        ran_secs,
        tail,
        surface_changes: Vec::new(),
        quarantined: true,
        survivors: vec!["sleep".to_owned(), "node".to_owned()],
        launcher_lost: true,
        output_truncated: false,
        blocked: Vec::new(),
        blocked_more: 0,
        bus_refused: Vec::new(),
        bus_refused_more: 0,
    };
    assert_eq!(
        line(&lost, title, None).as_deref(),
        Some(
            "efr: job j3 stopped: it changed git settings (quarantined): run the dev server; its \
             launcher was lost, and its processes may still run; 2 processes of the job may \
             still run"
        )
    );
    // The end of the line survives a long title.
    let text = line(&lost, Some(&"x".repeat(300)), None).unwrap();
    assert!(text.ends_with("2 processes of the job may still run"), "{text}");
    assert!(text.chars().count() <= 200, "{text}");
}

#[test]
fn held_notice_line_names_reason() {
    let held =
        |reason| Event::JobNoticeHeld { job_id: JobId::nth(2), source_seq: Seq::new(7), reason };
    let cases = [
        (HeldReason::NotAuto, "the conversation is not in auto"),
        (HeldReason::WakeLimit, "wake limit reached"),
        (HeldReason::WakeTurnJob, "job of a wake turn"),
        (HeldReason::DaemonRestart, "efrd restarted"),
        (HeldReason::WakesOff, "jobs.max_wakes_per_hour is 0"),
    ];
    for (reason, why) in cases {
        assert_eq!(
            line(&held(reason), Some("fix nginx"), None).as_deref(),
            Some(
                format!("efr: job j2 waits for your next prompt: fix nginx; no wake turn ({why})")
                    .as_str()
            )
        );
    }
}

#[test]
fn port_forward_notice_line_keeps_the_url_whole() {
    let forwarded = |outcome, reason: Option<&str>| Event::JobPortForwarded {
        job_id: JobId::nth(1),
        listen: "127.0.0.1".to_owned(),
        port: 8765,
        outcome,
        by_job: None,
        reason: reason.map(str::to_owned),
    };
    assert_eq!(
        line(&forwarded(ForwardOutcome::Forwarded, None), Some("dev server"), None).as_deref(),
        Some("efr: dev server: job j1 listens on 127.0.0.1:8765 -> http://localhost:8765")
    );
    assert_eq!(
        line(&forwarded(ForwardOutcome::InUse, None), None, None).as_deref(),
        Some(
            "efr: a conversation: job j1 listens on 127.0.0.1:8765; not forwarded: \
             127.0.0.1:8765 is in use on this machine"
        )
    );
    let off = forwarded(ForwardOutcome::Stopped, Some("jobs.forward_ports is off"));
    assert_eq!(
        line(&off, Some("dev"), None).as_deref(),
        Some("efr: dev: job j1: port 8765 is no longer forwarded (jobs.forward_ports is off)")
    );
    let long = "t".repeat(400);
    let text = line(&forwarded(ForwardOutcome::Forwarded, None), Some(&long), None).unwrap();
    assert!(
        text.ends_with(": job j1 listens on 127.0.0.1:8765 -> http://localhost:8765"),
        "{text}"
    );
    assert!(text.chars().count() <= 200, "{text}");
}

fn completed() -> Event {
    Event::TurnCompleted {
        turn_id: turn(),
        usage: None,
        changes: None,
        context: None,
        stop: None,
        undo: None,
    }
}

/// An event of the page of a turn.
fn envelope(seq: u64, event: Event) -> EventEnvelope {
    EventEnvelope {
        seq: Seq::new(seq),
        conversation_id: None,
        at: jiff::Timestamp::UNIX_EPOCH,
        event,
    }
}

/// An answer of the model in the turn `turn_id`.
fn answer(seq: u64, turn_id: TurnId, index: u32, text: &str) -> EventEnvelope {
    envelope(seq, Event::AssistantMessageCompleted { turn_id, index, text: text.to_owned() })
}

#[test]
fn wake_turn_finished_names_the_job() {
    let wake = wake_of(turn(), "job notice: j2", &[]);
    assert_eq!(wake, Wake { jobs: "j2".to_owned(), answer: None });
    assert_eq!(
        line(&completed(), Some("fix nginx"), Some(&wake)).as_deref(),
        Some("efr: job notice turn finished (job j2): fix nginx")
    );
    let two = Wake { jobs: "j2, j3".to_owned(), answer: None };
    assert_eq!(
        line(&completed(), Some("fix nginx"), Some(&two)).as_deref(),
        Some("efr: job notice turn finished (jobs j2, j3): fix nginx")
    );
    let failed = Event::TurnFailed {
        turn_id: turn(),
        error: ErrorBody::new(ErrorCode::Internal, "the provider stream broke"),
        usage: None,
        context: None,
        changes: None,
        undo: None,
    };
    assert_eq!(
        line(&failed, Some("fix nginx"), Some(&wake)).as_deref(),
        Some("efr: job notice turn failed (job j2): fix nginx: the provider stream broke")
    );
}

#[test]
fn wake_turn_notice_shows_first_answer_line() {
    let other = TurnId::from_uuid(uuid::Uuid::from_u128(9));
    let page = [
        answer(10, turn(), 0, "I look at the log."),
        answer(
            11,
            turn(),
            1,
            "\n  The build passed:\x1b[31m 12 tests\u{202e} ran.\nDetails follow.",
        ),
        answer(12, other, 0, "another turn's answer"),
    ];

    let wake = wake_of(turn(), "job notice: j2", &page);
    let text = line(&completed(), Some("fix nginx"), Some(&wake)).unwrap();

    assert_eq!(
        text,
        "efr: job notice turn finished (job j2): fix nginx: The build passed: [31m 12 tests\u{fffd} \
         ran.; efr history shows all"
    );
    // A long answer is cut, and the hint stays at the end of the one line.
    let long = Wake { jobs: "j2".to_owned(), answer: Some("y".repeat(400)) };
    let text = line(&completed(), Some("fix nginx"), Some(&long)).unwrap();
    assert_eq!(text.chars().count(), 200);
    assert!(text.ends_with("...; efr history shows all"), "{text}");
    assert!(!text.contains('\n'), "{text:?}");
}

#[tokio::test]
async fn the_store_names_a_job_notice_turn_and_its_answer() {
    let clock = efr_test_support::TestClock::new();
    let store = efr_test_support::TestStore::open(clock.shared()).await.unwrap();
    let conversation: ConversationId = "0192f0c1-7a00-7000-8000-000000000003".parse().unwrap();
    let notice = |job: u32, woke: bool| Event::JobNotice {
        conversation_id: conversation,
        turn_id: turn(),
        job_id: JobId::nth(job),
        notice_kind: efr_protocol::NoticeKind::Exited,
        woke,
        source_seq: Seq::new(1),
        not_user_input: true,
        text: "<job/>".to_owned(),
    };
    let created =
        Event::ConversationCreated { origin: efr_protocol::Origin::Shell, tty: None, route: None };
    let said = Event::AssistantMessageCompleted {
        turn_id: turn(),
        index: 0,
        text: "Both jobs passed.".to_owned(),
    };
    let committed = store
        .writer()
        .append(
            efr_store::Batch::new()
                .event(conversation, created)
                .event(conversation, notice(2, true))
                .event(conversation, notice(3, true))
                .event(conversation, said)
                .event(conversation, completed()),
        )
        .await
        .unwrap();
    let end = committed.events().last().unwrap().clone();

    let wake = crate::notices::wake_turn(store.readers(), conversation, &end).await;

    let jobs = "j2, j3".to_owned();
    assert_eq!(wake, Some(Wake { jobs, answer: Some("Both jobs passed.".to_owned()) }));
    let other = envelope(end.seq.get(), Event::TurnCancelled { turn_id: turn() });
    assert_eq!(crate::notices::wake_turn(store.readers(), conversation, &other).await, None);
}
