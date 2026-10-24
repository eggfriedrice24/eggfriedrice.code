use std::path::{Path, PathBuf};

use efr_protocol::{CallId, ConversationId, JobId, JobInfo, JobState, Launch, TurnId};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{doomed, remove};

fn at(hours: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000).unwrap() + SignedDuration::from_hours(hours)
}

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

/// Job `number` of conversation `c`, ended at `ended` hours, or running for `None`.
fn job(c: u128, number: u32, ended: Option<i64>) -> JobInfo {
    JobInfo {
        id: JobId::nth(number),
        conversation_id: conversation(c),
        turn_id: TurnId::from_uuid(uuid::Uuid::from_u128(9)),
        call_id: CallId::from_uuid(uuid::Uuid::from_u128(u128::from(number))),
        command: "make".to_owned(),
        launch: Launch::contained(),
        state: if ended.is_some() { JobState::Exited } else { JobState::Running },
        started_at: at(-1000),
        ends_by: at(1000),
        ended_at: ended.map(at),
        exit_code: None,
        stopped_by: None,
        output: PathBuf::from(format!("/d/scratch/c{c}/jobs/j{number}.out")),
        output_bytes: 0,
        watch: None,
        wakes: 0,
        forwards: Vec::new(),
        moved_from: None,
        waits_input: false,
        cgroup: None,
    }
}

fn output(c: u128, number: u32) -> PathBuf {
    PathBuf::from(format!("/d/scratch/c{c}/jobs/j{number}.out"))
}

#[test]
fn job_outputs_removed_after_seven_days() {
    let now = at(0);
    let jobs = [
        job(1, 1, Some(-7 * 24)),
        job(1, 2, Some(-7 * 24 + 1)),
        job(1, 3, None),
        job(2, 1, Some(-8 * 24)),
    ];
    assert_eq!(doomed(&jobs, now), [output(1, 1), output(2, 1)]);
}

#[test]
fn job_outputs_kept_at_most_twenty() {
    let now = at(0);
    let mut jobs: Vec<JobInfo> = (1..=23).map(|n| job(1, n, Some(-30 + i64::from(n)))).collect();
    jobs.push(job(1, 24, None));
    jobs.extend((1..=3).map(|n| job(2, n, Some(-1))));
    // NOTE: the three oldest ends of conversation 1 go; a running job and another
    // conversation's outputs do not count.
    assert_eq!(doomed(&jobs, now), [output(1, 1), output(1, 2), output(1, 3)]);
}

/// Only a file named like an output, in a `jobs` dir, goes; a link goes as a link.
#[test]
fn only_a_job_output_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = dir.path().join("jobs");
    std::fs::create_dir(&jobs).unwrap();
    let kept = dir.path().join("kept.txt");
    std::fs::write(&kept, "the user's").unwrap();
    std::fs::write(jobs.join("j1.out"), "out").unwrap();
    std::os::unix::fs::symlink(&kept, jobs.join("j2.out")).unwrap();
    std::fs::write(jobs.join("notes.txt"), "x").unwrap();
    std::fs::write(dir.path().join("j3.out"), "x").unwrap();
    assert!(remove(&jobs.join("j1.out")));
    assert!(remove(&jobs.join("j2.out")));
    assert!(kept.exists(), "the link went, not its target");
    assert!(!remove(&jobs.join("notes.txt")));
    assert!(!remove(&dir.path().join("j3.out")));
    assert!(!remove(&jobs.join("j9.out")), "a missing output");
    assert!(!remove(Path::new("/")));
}
