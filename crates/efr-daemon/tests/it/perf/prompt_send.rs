//! `bench_perf_prompt_send`: `prompt.send` until its ack, which covers the routing, the
//! peer walk, the repository hint and the commit of the prompt. It runs with 1, 5 and
//! 20 open conversations (each from a terminal of its own), from the working directory
//! of a registered project (a git repository), from a git repository outside every
//! project, and from a plain directory outside every project; then from the repository
//! outside again with `conversation.repo_hint` off. The terminal that measures has one
//! conversation per scope of its own besides the open ones.
//!
//! It counts the git runs of each prompt with the counting git of [`GitCount`]: those
//! that ended before the ack (`git by ack`; the turn's own can start before the ack),
//! and those of the whole turn up to its end (`git per turn`). The model answers at
//! once and calls no tool.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use efr_protocol::{Method, PromptSendResult};
use efr_provider::Provider;
use efr_test_support::TestClock;
use efr_test_support::bench::Report;

use super::{
    Follower, GitCount, PerfModel, Plan, builder, follow_turn, git_fixture, median, prompt,
    register, tree,
};

/// The terminal that measures.
const TTY: &str = "/dev/pts/perf-send";

/// The open conversations of each rung.
const OPEN: [usize; 3] = [1, 5, 20];

/// The measured prompts of each place, after [`WARMUP`] that are not.
const SAMPLES: usize = 15;

/// The prompts of each place before the measured ones: the first one starts the
/// terminal's conversation of the scope.
const WARMUP: usize = 2;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_prompt_send() {
    let Some(git) = GitCount::child_or_run(module_path!(), "bench_perf_prompt_send").await else {
        return;
    };
    let mut report = Report::new("bench_perf_prompt_send");
    for repo_hint in [true, false] {
        let dirs = tree();
        let clock = TestClock::new();
        let _follower = Follower::start(&clock);
        let project = dirs.create_dir("home/project").unwrap();
        git_fixture(&project, 20).await;
        let repo = dirs.create_dir("home/outside-repo").unwrap();
        git_fixture(&repo, 20).await;
        let plain = dirs.create_dir("home/plain").unwrap();
        let model = PerfModel::new(Plan::answer("ok"));
        let daemon = Box::pin(
            builder(&dirs, &clock)
                .custom_provider(Arc::clone(&model) as Arc<dyn Provider>)
                .config(|config| {
                    config.conversation.repo_hint = repo_hint;
                    // NOTE: a source of its own, so the reload of the project's
                    // registration keeps it.
                    config.set_source("conversation.repo_hint", efr_daemon::Source::Flag("bench"));
                })
                .start(),
        )
        .await
        .unwrap();
        register(&daemon, &project).await;
        let places: Vec<(&str, PathBuf)> = if repo_hint {
            vec![("project", project.clone()), ("outside repo", repo), ("outside plain", plain)]
        } else {
            vec![("outside repo no hint", repo)]
        };
        let mut open = 0;
        for target in OPEN {
            while open < target {
                let tty = format!("/dev/pts/perf-open-{open}");
                let client = daemon.client_for_tty(&tty).await.unwrap();
                let run = super::run_turn(&client, prompt(&tty, &project, "hello", None)).await;
                assert_eq!(run.end_kind(), "turn_completed");
                open += 1;
            }
            for (place, cwd) in &places {
                let client = daemon.client_for_tty(TTY).await.unwrap();
                let (mut acks, mut by_ack, mut per_turn) = (Vec::new(), Vec::new(), Vec::new());
                let mut last = 0;
                for n in 0..WARMUP + SAMPLES {
                    let before = git.runs();
                    last = before;
                    let params = prompt(TTY, cwd, "hello", None);
                    let sent_at = Instant::now();
                    let sent: PromptSendResult =
                        client.call(Method::PromptSend(params)).await.unwrap();
                    let acked_at = Instant::now();
                    let at_ack = git.runs();
                    let run = follow_turn(&client, sent, sent_at, acked_at).await;
                    assert_eq!(run.end_kind(), "turn_completed");
                    if n < WARMUP {
                        continue;
                    }
                    acks.push(acked_at - sent_at);
                    by_ack.push(at_ack - before);
                    per_turn.push(git.runs() - before);
                }
                report.time(&format!("open {open} {place} send->ack"), &acks);
                report.count(&format!("open {open} {place} git by ack"), median(&by_ack));
                report.count(&format!("open {open} {place} git per turn"), median(&per_turn));
                if target == OPEN[0] {
                    for (command, count) in git.commands_after(last) {
                        report.count(&format!("{place} turn: git {command}"), count);
                    }
                }
            }
        }
        daemon.stop().await.unwrap();
    }
    report.finish();
}
