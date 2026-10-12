use efr_protocol::{
    ClientFrame, ConversationHistoryResult, ConversationStatus, ConversationSummary,
    ConversationsListResult, ErrorBody, ErrorCode, Event, JobId, JobState, JobStopBy, JobsList,
    JobsListResult, JobsStop, JobsStopResult, Method, Seq,
};
use pretty_assertions::assert_eq;

use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command, conversation, envelope, job, now};

fn summary() -> ConversationSummary {
    ConversationSummary {
        id: conversation(),
        title: Some("run the dev server".to_owned()),
        status: ConversationStatus::Idle,
        created_at: now(),
        updated_at: now(),
        last_seq: Seq::new(9),
        cwd: None,
        scope: None,
        tty: None,
    }
}

/// The `job_ended` of job `n` after a stop by the user, with `tail` and `survivors`.
fn stopped_by_you(n: u32, tail: &str, survivors: &[&str]) -> Event {
    Event::JobEnded {
        job_id: JobId::nth(n),
        state: JobState::Stopped,
        exit_code: None,
        stopped_by: Some(JobStopBy::User),
        output_bytes: 2_000,
        ran_secs: 125,
        tail: tail.to_owned(),
        surface_changes: Vec::new(),
        quarantined: false,
        survivors: survivors.iter().map(|name| (*name).to_owned()).collect(),
        launcher_lost: false,
        output_truncated: false,
    }
}

/// What a run of `efr jobs <args>` did, with the `jobs.list` and `jobs.stop` it sent.
struct Ran {
    exit: Exit,
    stdout: String,
    stderr: String,
    listed: Vec<JobsList>,
    stops: Vec<JobsStop>,
}

/// What the fake daemon answers.
struct Daemon {
    jobs: JobsListResult,
    /// The answer to `jobs.stop`: the job after the stop, or the daemon's refusal.
    stop: Result<JobsStopResult, ErrorBody>,
    log: Vec<Event>,
}

/// Runs `efr jobs <args>` against a fake daemon that answers as `daemon` says.
async fn jobs_with(args: &[&str], daemon: Daemon) -> Ran {
    let env = TestEnv::new();
    let ctx = env.context();
    let listener = env.listen();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = listener.accept().await;
        let (mut listed, mut stops) = (Vec::new(), Vec::new());
        while let Some(frame) = conn.recv().await {
            let ClientFrame::Request { id, method } = frame else { continue };
            match method {
                Method::ConversationsList(_) => {
                    let list = ConversationsListResult {
                        conversations: vec![summary()],
                        next_cursor: None,
                    };
                    conn.reply(id, &list).await;
                }
                Method::JobsList(params) => {
                    listed.push(params);
                    conn.reply(id, &daemon.jobs).await;
                }
                Method::JobsStop(params) => {
                    stops.push(params);
                    match &daemon.stop {
                        Ok(result) => conn.reply(id, result).await,
                        Err(body) => conn.fail(id, body.clone()).await,
                    }
                }
                Method::ConversationHistory(_) => {
                    let events = daemon
                        .log
                        .iter()
                        .enumerate()
                        .map(|(at, event)| envelope(at as u64 + 1, event.clone()))
                        .collect();
                    let page = ConversationHistoryResult { events, next_cursor: None };
                    conn.reply(id, &page).await;
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        (listed, stops)
    };
    let mut line = vec!["jobs"];
    line.extend_from_slice(args);
    let line = command(&line);
    let (exit, (listed, stops)) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    Ran { exit, stdout: captured.stdout(), stderr: captured.stderr(), listed, stops }
}

fn no_stop() -> Result<JobsStopResult, ErrorBody> {
    Err(ErrorBody::new(ErrorCode::Internal, "no stop in this test"))
}

#[tokio::test]
async fn efr_jobs_list_rows() {
    let running = job(2, "npm run dev");
    let mut ended = job(1, "cargo build --release");
    ended.state = JobState::Stopped;
    ended.stopped_by = Some(JobStopBy::User);
    ended.ended_at = Some(now());
    ended.output_bytes = 2_000;
    let daemon = Daemon {
        jobs: JobsListResult { jobs: vec![running, ended] },
        stop: no_stop(),
        log: vec![stopped_by_you(1, "", &["node"])],
    };
    let ran = Box::pin(jobs_with(&[], daemon)).await;
    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(ran.listed, [JobsList { conversation_id: Some(conversation()) }]);
    assert_eq!(
        ran.stdout,
        "j2  running         ends in 3h 0m      0 B  npm run dev\n\
         j1  stopped by you  ended just now  2.0 KB  cargo build --release  (its processes may \
         still run)\n"
    );
    assert_eq!(
        ran.stderr,
        "the newest conversation; efr jobs list --conversation <id> lists another\n"
    );

    // `--all` names no conversation and puts the title first.
    let daemon = Daemon {
        jobs: JobsListResult { jobs: vec![job(2, "npm run dev")] },
        stop: no_stop(),
        log: Vec::new(),
    };
    let ran = Box::pin(jobs_with(&["list", "--all"], daemon)).await;
    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(ran.listed, [JobsList { conversation_id: None }]);
    assert!(ran.stdout.starts_with("run the dev server  j2  running"), "{}", ran.stdout);
    assert_eq!(ran.stderr, "");

    let daemon =
        Daemon { jobs: JobsListResult { jobs: Vec::new() }, stop: no_stop(), log: Vec::new() };
    let ran = Box::pin(jobs_with(&["list"], daemon)).await;
    assert_eq!(ran.stdout, "no jobs in this conversation\n");
}

#[tokio::test]
async fn efr_jobs_stop_shows_the_end_and_the_last_lines() {
    let mut stopped = job(3, "npm run dev");
    stopped.state = JobState::Stopped;
    stopped.stopped_by = Some(JobStopBy::User);
    stopped.ended_at = Some(now());
    let tail: String =
        (1..=12).map(|n| format!("line {n}\n")).collect::<String>() + "bad \u{202e}gnp.exe";
    let daemon = Daemon {
        jobs: JobsListResult { jobs: Vec::new() },
        stop: Ok(JobsStopResult { job: stopped }),
        log: vec![stopped_by_you(3, &tail, &[])],
    };
    let ran = Box::pin(jobs_with(&["stop", "j3"], daemon)).await;
    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let [stop] = ran.stops.try_into().unwrap();
    assert_eq!((stop.conversation_id, stop.job_id), (Some(conversation()), JobId::nth(3)));
    let lines: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(lines[0], "job j3 stopped by you after 2 minutes; 2.0 KB output");
    assert_eq!(lines.len(), 11, "the line and the last 10 lines: {}", ran.stdout);
    assert_eq!(lines[1], "  \u{2502} line 4");
    assert_eq!(
        lines[10], "  \u{2502} bad \u{fffd}gnp.exe",
        "a format character shows as a stand-in"
    );
}

#[tokio::test]
async fn efr_jobs_stop_unknown_job_fails() {
    let daemon = Daemon {
        jobs: JobsListResult { jobs: Vec::new() },
        stop: Err(ErrorBody::new(ErrorCode::NotFound, "the conversation has no job j9")),
        log: Vec::new(),
    };
    let ran = Box::pin(jobs_with(&["stop", "j9"], daemon)).await;
    assert_eq!(ran.exit, Exit::DaemonError);
    assert!(ran.stderr.ends_with("not_found: the conversation has no job j9\n"), "{}", ran.stderr);
    assert_eq!(ran.stdout, "");

    // A word that is not a job id is a usage error, before any connection.
    let parsed = <crate::cli::Cli as clap::Parser>::try_parse_from(["efr", "jobs", "stop", "3"]);
    assert!(parsed.is_err());
}
