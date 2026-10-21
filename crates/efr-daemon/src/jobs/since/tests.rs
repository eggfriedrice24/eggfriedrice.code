use std::path::PathBuf;

use efr_protocol::{CallId, ConversationId, JobId, JobInfo, JobState, Launch, TurnId};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::earliest;

fn at(minutes: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000).unwrap() + SignedDuration::from_mins(minutes)
}

fn job(started: i64, ended: Option<i64>) -> JobInfo {
    JobInfo {
        id: JobId::nth(1),
        conversation_id: ConversationId::from_uuid(uuid::Uuid::from_u128(1)),
        turn_id: TurnId::from_uuid(uuid::Uuid::from_u128(2)),
        call_id: CallId::from_uuid(uuid::Uuid::from_u128(3)),
        command: "make".to_owned(),
        launch: Launch::contained(),
        state: if ended.is_some() { JobState::Exited } else { JobState::Running },
        started_at: at(started),
        ends_by: at(started + 60),
        ended_at: ended.map(at),
        exit_code: None,
        stopped_by: None,
        output: PathBuf::from("/d/scratch/x/jobs/j1.out"),
        output_bytes: 0,
        watch: None,
        wakes: 0,
        forwards: Vec::new(),
        moved_from: None,
        waits_input: false,
    }
}

/// A job that ran since the previous turn started moves the time back to its start;
/// a job that ended before it does not.
#[test]
fn program_written_by_job_counts_as_changed() {
    let (turn_start, previous) = (at(0), Some(at(-30)));
    assert_eq!(earliest(turn_start, previous, &[]), at(0));
    assert_eq!(earliest(turn_start, previous, &[job(-40, None)]), at(-40), "it still runs");
    assert_eq!(earliest(turn_start, previous, &[job(-50, Some(-20))]), at(-50), "it ended late");
    assert_eq!(earliest(turn_start, previous, &[job(-90, Some(-60))]), at(0), "it ended before");
    assert_eq!(earliest(turn_start, None, &[job(-90, Some(-60))]), at(-90), "no turn before");
}

/// A program that a job wrote between two turns counts as changed in the next one,
/// from the job's start.
#[test]
fn a_program_of_a_job_between_turns_changed_since_its_start() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("tool");
    let now = Timestamp::now();
    std::fs::write(&program, "#!/bin/sh\n").unwrap();
    // NOTE: the next turn starts an hour later; the job started a minute before the
    // write, after the previous turn.
    let turn_start = now + SignedDuration::from_hours(1);
    let ran = JobInfo { started_at: now - SignedDuration::from_mins(1), ..job(0, None) };
    let previous = Some(now - SignedDuration::from_mins(5));
    assert!(!crate::sandbox::facts::changed_since(&program, turn_start));
    let since = earliest(turn_start, previous, &[ran]);
    assert!(crate::sandbox::facts::changed_since(&program, since));
}
