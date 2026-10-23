//! `bench_perf_shell_call_auto`: one `shell` call in `auto`, from the client's
//! `tool_call_started` until its `tool_call_completed` (`call`), and from the end of
//! the model's answer that asked for the call until `tool_call_started` (`decide`: the
//! requirements, the facts and the permission engine run before the start). It runs a
//! real hidden zsh (so it needs `EFR_TEST_ZSH=1`, as the `shell_` tests do) with the
//! fake launcher of `efr-test-daemon` in place of `efr-sbx` (a `/bin/sh` script that
//! runs the line with no sandbox) and a probe that says ready, in a registered project
//! with 50 committed files. The real sandbox has a bench of its own under
//! `just test-sandbox`.
//!
//! The lines are `true`, a line with 10 `cd`s, and `rm -f` lines of 100 B, 10 KB and
//! 100 KB of names that do not exist. `true` and the `cd`s run while 0, 1, 4 and 16
//! contained background jobs (`sleep`) run, four conversations of four jobs each, in the
//! same project, since a conversation runs at most 4. The `rm` lines run with no job,
//! and the one of 100 KB again with 16 jobs; a call of it takes seconds, so it runs
//! fewer times.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{Event, Mode};
use efr_provider::Provider;
use efr_test_daemon::{Client, FakeLauncher};
use efr_test_support::TestClock;
use efr_test_support::bench::Report;
use serde_json::{Value, json};

use super::{
    Follower, PerfModel, Plan, RUNG_LIMIT, TurnRun, builder, git_fixture, prompt, register,
    run_turn, tree,
};

/// The terminal of the measured calls.
const TTY: &str = "/dev/pts/perf-shell";

/// The running jobs of each rung.
const JOBS: [usize; 4] = [0, 1, 4, 16];

/// The jobs that one conversation may run (`jobs.max_running`).
const JOBS_PER_CONVERSATION: usize = 4;

/// A line that changes into 10 nested dirs, reads a file there and goes back.
fn cds_line(project: &Path) -> String {
    let cds: Vec<String> = (1..=10).map(|n| format!("cd c{n}")).collect();
    format!("{} && cat x; cd {}", cds.join(" && "), project.display())
}

/// An `rm -f` line of about `bytes` of names that do not exist.
fn rm_line(bytes: usize) -> String {
    let mut line = String::from("rm -f");
    let mut n = 0;
    while line.len() < bytes {
        line.push_str(&format!(" gone-{n:06}.txt"));
        n += 1;
    }
    line
}

/// The plan of a turn that calls `shell` with each of `inputs`, then answers.
fn calls(inputs: Vec<Value>) -> Plan {
    let tools = inputs.into_iter().map(|input| ("shell".to_owned(), input)).collect();
    Plan { tools, answer: "done".to_owned() }
}

/// One turn from `tty` in `auto` that follows `plan`, checked to end well.
async fn auto_turn(
    client: &Client,
    model: &PerfModel,
    tty: &str,
    project: &Path,
    plan: Plan,
) -> TurnRun {
    model.set_plan(plan);
    let run = run_turn(client, prompt(tty, project, "run it", Some(Mode::Auto))).await;
    assert_eq!(run.end_kind(), "turn_completed", "{:#?}", run.events);
    run
}

/// The `decide` span of each call of `run`: from the end of the model's answer that
/// asked for it to its `tool_call_started`.
fn decide_times(model: &PerfModel, run: &TurnRun) -> Vec<Duration> {
    let ends = model.ends_between(run.sent_at, run.ended_at());
    run.started_at()
        .iter()
        .zip(ends)
        .map(|(started, end)| started.saturating_duration_since(end))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_shell_call_auto() {
    let mut report = Report::new("bench_perf_shell_call_auto");
    if !crate::support::zsh_enabled("bench_perf_shell_call_auto") {
        report.count("skipped: EFR_TEST_ZSH is not 1", 1);
        report.finish();
        return;
    }
    let dirs = tree();
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let project = dirs.create_dir("home/project").unwrap();
    git_fixture(&project, 50).await;
    let mut deep = project.clone();
    for n in 1..=10 {
        deep = deep.join(format!("c{n}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("x"), "x\n").unwrap();
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    let model = PerfModel::new(Plan::answer("ok"));
    let daemon = Box::pin(
        builder(&dirs, &clock)
            .local_pty()
            .sandbox_launcher(fake.path())
            .probe_override(crate::sandbox::ready())
            .custom_provider(Arc::clone(&model) as Arc<dyn Provider>)
            .start(),
    )
    .await
    .unwrap();
    register(&daemon, &project).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    // The first turn starts the hidden shell; it does not count.
    auto_turn(&client, &model, TTY, &project, calls(vec![json!({ "command": "true" })])).await;
    let cds = cds_line(&project);
    let mut running = 0;
    let mut rm_points = Vec::new();
    let mut rm_stopped = false;
    'rungs: for jobs in JOBS {
        while running < jobs {
            let tty = format!("/dev/pts/perf-jobs-{}", running / JOBS_PER_CONVERSATION);
            let starter = daemon.client_for_tty(&tty).await.unwrap();
            let count =
                (jobs - running).min(JOBS_PER_CONVERSATION - running % JOBS_PER_CONVERSATION);
            let start = json!({ "command": "sleep 900", "background": true });
            let run = auto_turn(&starter, &model, &tty, &project, calls(vec![start; count])).await;
            assert_eq!(run.count("job_started"), count, "{:#?}", run.events);
            running += count;
        }
        let mut lines = vec![("true", "true".to_owned(), 5), ("10 cds", cds.clone(), 5)];
        if jobs == 0 {
            for (name, bytes, times) in
                [("rm 100 B", 100, 3), ("rm 10 KB", 10 * 1024, 3), ("rm 100 KB", 100 * 1024, 2)]
            {
                lines.push((name, rm_line(bytes), times));
            }
        }
        if jobs == 16 {
            lines.push(("rm 100 KB", rm_line(100 * 1024), 2));
        }
        for (name, line, times) in lines {
            let rm = name.starts_with("rm ");
            if rm && rm_stopped {
                continue;
            }
            let run = auto_turn(
                &client,
                &model,
                TTY,
                &project,
                calls(vec![json!({ "command": line }); times]),
            )
            .await;
            let failed = run.events.iter().any(|(_, envelope)| {
                matches!(envelope.event,
                    Event::ToolCallCompleted { exit_code, .. } if exit_code != Some(0))
            });
            assert!(!failed, "every call ran: {:#?}", run.events);
            let call = run.call_times();
            let decide = decide_times(&model, &run);
            report.time(&format!("jobs {jobs} {name} call"), &call);
            let stats = report.time(&format!("jobs {jobs} {name} decide"), &decide);
            if call.len() != times {
                report.count(&format!("jobs {jobs} {name} calls (not {times})"), call.len() as u64);
            }
            if run.approvals > 0 {
                report.count(&format!("jobs {jobs} {name} approvals"), run.approvals as u64);
            }
            if jobs == 0 && rm {
                rm_points.push((line.len() as u64, stats.p50 + super::median_time(&call)));
            }
            let longest = call.iter().chain(&decide).max().copied().unwrap_or(Duration::ZERO);
            if longest > RUNG_LIMIT {
                report.count(&format!("stopped_at_n (jobs {jobs}, line bytes)"), line.len() as u64);
                // NOTE: the `rm` ladder ends here; the other lines go on with more jobs.
                if !rm {
                    break 'rungs;
                }
                rm_stopped = true;
            }
        }
    }
    report.slope("rm decide+call over line bytes", &rm_points);
    drop(client);
    daemon.stop().await.unwrap();
    report.finish();
}
