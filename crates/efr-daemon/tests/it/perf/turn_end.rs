//! `bench_perf_turn_end`: from the model's last event of a turn (the end of its
//! answer) until the client gets the turn's terminal event, which the daemon sends
//! after it committed it. That span holds the turn's end snapshot, `left_out`, the
//! surface report and the commit. The turn runs in `auto` (the fake launcher and a
//! probe that says ready) in a registered project, a fresh git repository of 100, 2,000,
//! 19,000 and 20,000 committed files, and it writes one file with `write_file`, so
//! efr's snapshots run. 19,000 is under the snapshot's limit of 20,000 files; at 20,000
//! files the written file puts the root over the limit.
//!
//! It counts the git runs with the counting git of [`GitCount`]: those after the
//! model's last event (`git at turn end`) and those of the whole turn, and it names the
//! git commands of one turn's end. The first turn of each project is reported apart: it
//! takes the first snapshot and the mirror of the project's git objects.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use efr_protocol::{Event, Mode};
use efr_provider::Provider;
use efr_test_daemon::FakeLauncher;
use efr_test_support::TestClock;
use efr_test_support::bench::Report;
use serde_json::json;

use super::{
    Follower, GitCount, PerfModel, Plan, RUNG_LIMIT, builder, git_fixture, median, prompt,
    register, run_turn, tree,
};

/// The terminal of the bench's prompts.
const TTY: &str = "/dev/pts/perf-turn-end";

/// The files of each project.
const FILES: [usize; 4] = [100, 2_000, 19_000, 20_000];

/// The measured turns of each project, after the first one.
const SAMPLES: usize = 6;

/// The plan of turn `n`: it writes the file `out-<n>.txt`, then answers.
fn plan(n: usize) -> Plan {
    let input = json!({ "path": format!("out-{n}.txt"), "content": "one line\n" });
    Plan::call("write_file", input, "I wrote the file.")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_turn_end() {
    let Some(git) = GitCount::child_or_run(module_path!(), "bench_perf_turn_end").await else {
        return;
    };
    let git = Arc::new(git);
    let mut report = Report::new("bench_perf_turn_end");
    let mut points = Vec::new();
    for files in FILES {
        let dirs = tree();
        let clock = TestClock::new();
        let _follower = Follower::start(&clock);
        let project = dirs.create_dir("home/project").unwrap();
        git_fixture(&project, files).await;
        let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
        let model = PerfModel::new(plan(0));
        let at_done = Arc::new(AtomicU64::new(0));
        {
            let (git, at_done) = (Arc::clone(&git), Arc::clone(&at_done));
            model.on_done(move || at_done.store(git.runs(), Ordering::SeqCst));
        }
        let daemon = Box::pin(
            builder(&dirs, &clock)
                .sandbox_launcher(fake.path())
                .probe_override(crate::sandbox::ready())
                .custom_provider(Arc::clone(&model) as Arc<dyn Provider>)
                .start(),
        )
        .await
        .unwrap();
        register(&daemon, &project).await;
        let client = daemon.client_for_tty(TTY).await.unwrap();
        let (mut ends, mut at_end, mut per_turn) = (Vec::new(), Vec::new(), Vec::new());
        let (mut snapshots, mut snapshot_ms) = (Vec::new(), Vec::new());
        let mut last_done = 0;
        for n in 0..=SAMPLES {
            model.set_plan(plan(n));
            let before = git.runs();
            let run = run_turn(&client, prompt(TTY, &project, "write it", Some(Mode::Auto))).await;
            assert_eq!(run.end_kind(), "turn_completed", "{:#?}", run.events);
            let wrote = run.events.iter().any(|(_, envelope)| {
                matches!(envelope.event, Event::ToolCallCompleted { is_error: false, .. })
            });
            assert!(wrote, "the turn wrote its file: {:#?}", run.events);
            let end = run.ended_at().saturating_duration_since(model.last_end().unwrap());
            last_done = at_done.load(Ordering::SeqCst);
            if n == 0 {
                report.time(&format!("{files} files first turn end"), &[end]);
                continue;
            }
            ends.push(end);
            at_end.push(git.runs() - last_done);
            per_turn.push(git.runs() - before);
            snapshots.push(run.count("snapshot_taken") as u64);
            let ms: u64 = run
                .events
                .iter()
                .filter_map(|(_, envelope)| match envelope.event {
                    Event::SnapshotTaken { ms, .. } => Some(ms),
                    _ => None,
                })
                .sum();
            snapshot_ms.push(ms);
            if end > RUNG_LIMIT {
                break;
            }
        }
        let stats = report.time(&format!("{files} files model end->terminal"), &ends);
        report.count(&format!("{files} files git at turn end"), median(&at_end));
        report.count(&format!("{files} files git per turn"), median(&per_turn));
        report.count(&format!("{files} files snapshot_taken events"), median(&snapshots));
        report.count(&format!("{files} files snapshot ms (sum)"), median(&snapshot_ms));
        for (command, count) in git.commands_after(last_done) {
            report.count(&format!("{files} files turn end: git {command}"), count);
        }
        points.push((files as u64, stats.p50));
        drop(client);
        daemon.stop().await.unwrap();
        if stats.p50 > RUNG_LIMIT {
            report.count("stopped_at_n (files)", files as u64);
            break;
        }
    }
    let under_limit: Vec<(u64, Duration)> =
        points.iter().copied().filter(|(files, _)| *files < 20_000).collect();
    report.slope("turn end over files (under the limit)", &under_limit);
    report.finish();
}
