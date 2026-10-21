use std::time::Duration;

use efr_protocol::{
    ConversationId, Event, ForwardOutcome, HeldReason, JobId, JobInfo, JobState, JobStopBy, Launch,
    NoticeKind, PortForward, Seq,
};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use super::*;
use crate::conversations::{self, TurnStatus};
use crate::testing::{self, TestClock, on_writer};
use crate::{Batch, WriterHandle};

/// The job `n` of `conversation`, started by turn 1, running.
fn job(conversation: ConversationId, n: u32) -> JobInfo {
    let started = testing::start();
    JobInfo {
        id: JobId::nth(n),
        conversation_id: conversation,
        turn_id: testing::turn(1),
        call_id: testing::call(u64::from(n)),
        command: format!("sleep {n}"),
        launch: Launch::contained(),
        state: JobState::Running,
        started_at: sql::truncate_to_micros(started),
        ends_by: sql::truncate_to_micros(started) + jiff::SignedDuration::from_mins(60),
        ended_at: None,
        exit_code: None,
        stopped_by: None,
        output: format!("/d/scratch/x/jobs/j{n}.out").into(),
        output_bytes: 0,
        watch: None,
        wakes: 0,
        forwards: Vec::new(),
        moved_from: None,
    }
}

fn job_started(job: JobInfo, pid: u32) -> Event {
    Event::JobStarted {
        turn_id: job.turn_id,
        call_id: job.call_id,
        job,
        launcher_pid: pid,
        launcher_start: 7_000 + u64::from(pid),
    }
}

fn job_ended(n: u32, state: JobState, stopped_by: Option<JobStopBy>) -> Event {
    Event::JobEnded {
        job_id: JobId::nth(n),
        state,
        exit_code: (state == JobState::Exited).then_some(0),
        stopped_by,
        output_bytes: 512,
        ran_secs: 30,
        tail: "done\n".to_owned(),
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

fn notice(turn: u64, n: u32, source: u64, woke: bool) -> Event {
    Event::JobNotice {
        conversation_id: testing::conversation(1),
        turn_id: testing::turn(turn),
        job_id: JobId::nth(n),
        notice_kind: NoticeKind::Exited,
        woke,
        source_seq: Seq::new(source),
        not_user_input: true,
        text: format!("<job id=\"j{n}\" state=\"exited\" exit=\"0\" command=\"sleep\">\n</job>"),
    }
}

/// A writer with conversation 1 and its turn 1, which ran.
async fn with_turn() -> (WriterHandle, crate::StoreWriter, std::sync::Arc<TestClock>) {
    let clock = TestClock::new();
    let (writer, thread) = testing::memory_writer(clock.clone());
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "run the dev server"))
                .event(id, testing::started(1, "/home/u/p/app")),
        )
        .await
        .unwrap();
    (writer, thread, clock)
}

#[tokio::test]
async fn job_started_inserts_a_row() {
    let (writer, _thread, _clock) = with_turn().await;
    let id = testing::conversation(1);
    let committed =
        writer.append(Batch::new().event(id, job_started(job(id, 1), 4242))).await.unwrap();

    let rows = on_writer(&writer, move |conn| of_conversation(conn, id, 10)).await.unwrap();

    assert_eq!(
        rows,
        [JobRow {
            info: job(id, 1),
            start_seq: committed.last_seq(),
            launcher_pid: 4242,
            launcher_start: 11_242,
        }]
    );
    let running = on_writer(&writer, running).await.unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].launcher_pid, 4242, "the pid comes from the event");
    let other = testing::conversation(2);
    let none = on_writer(&writer, move |conn| of_conversation(conn, other, 10)).await.unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn job_ended_updates_the_row() {
    let (writer, _thread, clock) = with_turn().await;
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, job_started(job(id, 1), 10))
                .event(id, job_started(job(id, 2), 11)),
        )
        .await
        .unwrap();
    clock.advance(Duration::from_secs(30));
    let ended = writer
        .append(Batch::new().event(id, job_ended(2, JobState::Stopped, Some(JobStopBy::User))))
        .await
        .unwrap();

    let rows = on_writer(&writer, move |conn| of_conversation(conn, id, 10)).await.unwrap();
    assert_eq!(rows.iter().map(|row| row.info.id.as_str()).collect::<Vec<_>>(), ["j2", "j1"]);
    let stopped = &rows[0].info;
    assert_eq!(stopped.state, JobState::Stopped);
    assert_eq!(stopped.stopped_by, Some(JobStopBy::User));
    assert_eq!(stopped.output_bytes, 512);
    assert_eq!(stopped.ended_at, Some(sql::truncate_to_micros(ended.events()[0].at)));
    assert_eq!(rows[1].info, job(id, 1), "the other job is untouched");

    let running = on_writer(&writer, running).await.unwrap();
    assert_eq!(running.iter().map(|row| row.info.id.number()).collect::<Vec<_>>(), [Some(1)]);
    let all = on_writer(&writer, |conn| all(conn, 1)).await.unwrap();
    assert_eq!(all.len(), 1, "the limit holds");

    let before = on_writer(&writer, |conn| Ok(testing::dump(conn, "jobs"))).await.unwrap();
    writer.rebuild_projections().await.unwrap();
    let after = on_writer(&writer, |conn| Ok(testing::dump(conn, "jobs"))).await.unwrap();
    assert_eq!(after, before, "a rebuild gives the same rows");
}

/// The `job_port_forwarded` of job `n` for `listen`:`port` with `outcome`.
fn port_event(n: u32, listen: &str, port: u16, outcome: ForwardOutcome) -> Event {
    Event::JobPortForwarded {
        job_id: JobId::nth(n),
        listen: listen.to_owned(),
        port,
        outcome,
        by_job: (outcome == ForwardOutcome::InUse).then(|| JobId::nth(9)),
        reason: matches!(outcome, ForwardOutcome::Refused | ForwardOutcome::Limit)
            .then(|| "a service port".to_owned()),
    }
}

fn forward(listen: &str, port: u16) -> PortForward {
    PortForward { listen: listen.to_owned(), port }
}

/// The forwarded ports of job `n` of conversation 1, as the projection holds them.
async fn forwards_of(writer: &WriterHandle, n: u32) -> Vec<PortForward> {
    let id = testing::conversation(1);
    let rows = on_writer(writer, move |conn| of_conversation(conn, id, 10)).await.unwrap();
    rows.into_iter().find(|row| row.info.id == JobId::nth(n)).map(|row| row.info.forwards).unwrap()
}

#[tokio::test]
async fn forwarded_ports_follow_the_port_events() {
    let (writer, _thread, _clock) = with_turn().await;
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, job_started(job(id, 1), 10))
                .event(id, job_started(job(id, 2), 11))
                .event(id, port_event(1, "127.0.0.1", 8765, ForwardOutcome::Forwarded))
                .event(id, port_event(1, "::1", 5173, ForwardOutcome::Forwarded))
                .event(id, port_event(1, "127.0.0.1", 3000, ForwardOutcome::InUse))
                .event(id, port_event(1, "127.0.0.1", 6379, ForwardOutcome::Refused))
                .event(id, port_event(1, "127.0.0.1", 9000, ForwardOutcome::Limit))
                .event(id, port_event(1, "127.0.0.1", 9001, ForwardOutcome::Unknown))
                .event(id, port_event(2, "127.0.0.1", 4000, ForwardOutcome::Forwarded)),
        )
        .await
        .unwrap();
    assert_eq!(
        forwards_of(&writer, 1).await,
        [forward("127.0.0.1", 8765), forward("::1", 5173)],
        "only a forwarded port counts, in the order of the events"
    );
    assert_eq!(forwards_of(&writer, 2).await, [forward("127.0.0.1", 4000)]);

    // A port that the job forwards already keeps one entry, with the newest address.
    writer
        .append(Batch::new().event(id, port_event(1, "::1", 8765, ForwardOutcome::Forwarded)))
        .await
        .unwrap();
    assert_eq!(forwards_of(&writer, 1).await, [forward("::1", 8765), forward("::1", 5173)]);

    // `stopped` removes its port; a stop of a port that the job does not forward
    // changes nothing.
    writer
        .append(
            Batch::new()
                .event(id, port_event(1, "::1", 5173, ForwardOutcome::Stopped))
                .event(id, port_event(1, "127.0.0.1", 7000, ForwardOutcome::Stopped)),
        )
        .await
        .unwrap();
    assert_eq!(forwards_of(&writer, 1).await, [forward("::1", 8765)]);

    let notices = on_writer(&writer, |conn| Ok(testing::dump(conn, "job_notices"))).await.unwrap();
    assert!(notices.is_empty(), "a forward is no notice: {notices:?}");
    let before = on_writer(&writer, |conn| Ok(testing::dump(conn, "jobs"))).await.unwrap();
    writer.rebuild_projections().await.unwrap();
    let after = on_writer(&writer, |conn| Ok(testing::dump(conn, "jobs"))).await.unwrap();
    assert_eq!(after, before, "a rebuild gives the same forwards");
}

#[tokio::test]
async fn the_end_of_a_job_ends_its_forwards() {
    let (writer, _thread, _clock) = with_turn().await;
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, job_started(job(id, 1), 10))
                .event(id, job_started(job(id, 2), 11))
                .event(id, port_event(1, "127.0.0.1", 8765, ForwardOutcome::Forwarded))
                .event(id, port_event(2, "127.0.0.1", 5173, ForwardOutcome::Forwarded))
                .event(id, job_ended(1, JobState::Stopped, Some(JobStopBy::DaemonRestart))),
        )
        .await
        .unwrap();
    assert_eq!(forwards_of(&writer, 1).await, []);
    assert_eq!(forwards_of(&writer, 2).await, [forward("127.0.0.1", 5173)], "job 2 still runs");

    // An event of a job that the projection does not hold, such as a job of the exit
    // child, adds no row.
    writer
        .append(Batch::new().event(id, port_event(7, "127.0.0.1", 8000, ForwardOutcome::Forwarded)))
        .await
        .unwrap();
    let rows = on_writer(&writer, move |conn| of_conversation(conn, id, 10)).await.unwrap();
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn next_number_follows_the_highest() {
    let (writer, _thread, _clock) = with_turn().await;
    let id = testing::conversation(1);
    let other = testing::conversation(2);
    assert_eq!(on_writer(&writer, move |conn| next_number(conn, id)).await.unwrap(), 1);
    writer
        .append(
            Batch::new()
                .event(id, job_started(job(id, 1), 10))
                .event(id, job_started(job(id, 7), 11))
                .event(id, job_started(job(id, 3), 12))
                .event(other, testing::created(None)),
        )
        .await
        .unwrap();

    assert_eq!(on_writer(&writer, move |conn| next_number(conn, id)).await.unwrap(), 8);
    assert_eq!(on_writer(&writer, move |conn| next_number(conn, other)).await.unwrap(), 1);
}

#[tokio::test]
async fn undelivered_notices_skip_delivered_ones() {
    let (writer, _thread, clock) = with_turn().await;
    let id = testing::conversation(1);
    let watched = Event::JobWatchMatched {
        job_id: JobId::nth(1),
        line: "error: port in use".to_owned(),
        wakes_left: 2,
    };
    let committed = writer
        .append(
            Batch::new()
                .event(id, job_started(job(id, 1), 10))
                .event(id, job_started(job(id, 2), 11))
                .event(id, watched.clone())
                .event(id, job_ended(2, JobState::Exited, None))
                .event(id, job_ended(1, JobState::TimedOut, Some(JobStopBy::TimeLimit))),
        )
        .await
        .unwrap();
    let seqs: Vec<u64> = committed.events().iter().map(|envelope| envelope.seq.get()).collect();
    let (watch_seq, end_two, end_one) = (seqs[2], seqs[3], seqs[4]);
    let held = Event::JobNoticeHeld {
        job_id: JobId::nth(2),
        source_seq: Seq::new(end_two),
        reason: HeldReason::WakeLimit,
    };
    writer
        .append(
            Batch::new()
                .event(id, notice(2, 1, watch_seq, true))
                .event(id, testing::started(2, "/home/u/p/app"))
                .event(id, held),
        )
        .await
        .unwrap();

    let since = testing::start();
    let waiting = on_writer(&writer, move |conn| undelivered_notices(conn, since)).await.unwrap();

    assert_eq!(
        waiting.iter().map(|notice| notice.source_seq.get()).collect::<Vec<_>>(),
        [end_two, end_one],
        "the taken watch notice is gone; a held one still waits"
    );
    assert_eq!(waiting[0].conversation_id, id);
    assert_eq!(waiting[0].job.id, JobId::nth(2));
    assert_eq!(waiting[0].job.state, JobState::Exited);
    assert!(matches!(waiting[1].event, Event::JobEnded { state: JobState::TimedOut, .. }));

    let held = on_writer(&writer, move |conn| held_notices(conn, id)).await.unwrap();
    assert_eq!(held, [Seq::new(end_two)], "only the waiting notice that was held");
    let other = testing::conversation(2);
    let none = on_writer(&writer, move |conn| held_notices(conn, other)).await.unwrap();
    assert!(none.is_empty());

    clock.advance(Duration::from_secs(60));
    let later = Timestamp::from_second(testing::start().as_second() + 30).unwrap();
    let none = on_writer(&writer, move |conn| undelivered_notices(conn, later)).await.unwrap();
    assert!(none.is_empty(), "a notice older than the window is left out");
}

#[tokio::test]
async fn wake_notice_inserts_one_turn_row() {
    let (writer, _thread, _clock) = with_turn().await;
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, job_started(job(id, 3), 10))
                .event(id, job_started(job(id, 4), 11))
                .event(id, job_ended(3, JobState::Exited, None))
                .event(id, job_ended(4, JobState::Exited, None)),
        )
        .await
        .unwrap();
    writer
        .append(
            Batch::new()
                .event(id, notice(2, 3, 6, true))
                .event(id, notice(2, 4, 7, true))
                .event(id, testing::started(2, "/home/u/p/app")),
        )
        .await
        .unwrap();

    let turns = on_writer(&writer, move |conn| conversations::turns(conn, id)).await.unwrap();

    assert_eq!(turns.len(), 2, "one row for the turn that two notices started");
    let wake = &turns[1];
    assert!(wake.wake && !turns[0].wake);
    assert_eq!(wake.id, testing::turn(2));
    assert_eq!(wake.command_id, None);
    assert_eq!(wake.prompt, "job notice: j3, j4");
    assert_eq!(wake.status, TurnStatus::Running);
    let summary = on_writer(&writer, move |conn| conversations::get(conn, id)).await.unwrap();
    assert_eq!(
        summary.and_then(|summary| summary.title).as_deref(),
        Some("run the dev server"),
        "a notice never names the conversation"
    );

    let before = on_writer(&writer, |conn| Ok(testing::dump(conn, "turns"))).await.unwrap();
    writer.rebuild_projections().await.unwrap();
    let after = on_writer(&writer, |conn| Ok(testing::dump(conn, "turns"))).await.unwrap();
    assert_eq!(after, before, "a rebuild gives the same rows");
}

#[tokio::test]
async fn wake_turn_row_gets_started_at() {
    let (writer, _thread, clock) = with_turn().await;
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, job_started(job(id, 1), 10))
                .event(id, job_ended(1, JobState::Exited, None)),
        )
        .await
        .unwrap();
    clock.advance(Duration::from_secs(5));
    let committed = writer
        .append(
            Batch::new()
                .event(id, notice(2, 1, 5, true))
                .event(id, testing::started(2, "/home/u/p/app"))
                .event(
                    id,
                    Event::TurnCompleted {
                        turn_id: testing::turn(2),
                        usage: None,
                        context: None,
                        changes: None,
                        stop: None,
                        undo: None,
                    },
                ),
        )
        .await
        .unwrap();

    let turn = on_writer(&writer, |conn| conversations::turn(conn, testing::turn(2)))
        .await
        .unwrap()
        .unwrap();

    let at = sql::truncate_to_micros(committed.events()[1].at);
    assert_eq!(turn.started_at, Some(at), "an undo candidate needs started_at");
    assert_eq!(turn.status, TurnStatus::Completed);
    assert_eq!(turn.last_seq, committed.last_seq());
}

#[tokio::test]
async fn wake_starts_since_counts_wake_rows() {
    let (writer, _thread, clock) = with_turn().await;
    let id = testing::conversation(1);
    let other = testing::conversation(2);
    writer
        .append(
            Batch::new()
                .event(id, job_started(job(id, 1), 10))
                .event(id, job_ended(1, JobState::Exited, None))
                .event(other, testing::created(None)),
        )
        .await
        .unwrap();
    clock.advance(Duration::from_secs(60));
    writer
        .append(
            Batch::new()
                .event(id, notice(2, 1, 5, true))
                .event(id, testing::started(2, "/home/u/p/app")),
        )
        .await
        .unwrap();
    clock.advance(Duration::from_secs(60));
    let other_wake = Event::JobNotice {
        conversation_id: other,
        turn_id: testing::turn(3),
        job_id: JobId::nth(9),
        notice_kind: NoticeKind::Watch,
        woke: true,
        source_seq: Seq::new(99),
        not_user_input: true,
        text: "<job id=\"j9\" state=\"running\" watch=\"matched\" command=\"x\">\n</job>"
            .to_owned(),
    };
    writer
        .append(
            Batch::new()
                .event(other, other_wake)
                .event(other, testing::started(3, "/tmp"))
                .event(id, testing::queued(4, "a prompt"))
                .event(id, testing::started(4, "/tmp")),
        )
        .await
        .unwrap();

    let since = testing::start();
    let ours =
        on_writer(&writer, move |conn| wake_starts_since(conn, Some(id), since)).await.unwrap();
    let every = on_writer(&writer, move |conn| wake_starts_since(conn, None, since)).await.unwrap();
    let first = sql::truncate_to_micros(testing::start() + jiff::SignedDuration::from_secs(60));
    let second = sql::truncate_to_micros(testing::start() + jiff::SignedDuration::from_secs(120));
    assert_eq!(ours, [first], "a prompt turn and another conversation do not count");
    assert_eq!(every, [first, second]);
    let late = on_writer(&writer, move |conn| wake_starts_since(conn, None, second)).await.unwrap();
    assert_eq!(late, [second], "the window starts at since");
}
