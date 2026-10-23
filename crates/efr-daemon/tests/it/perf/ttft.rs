//! `bench_perf_ttft`: the time to the first token. From `prompt.send` until the client
//! holds the first text of the answer (a draft or `assistant_message_updated`), the way
//! `efr send` follows a turn: the prompt, its ack, then a subscription with drafts. It
//! runs on the HTTP Responses path (the real `openai-api` provider against
//! `ResponsesServer`) and on the Messages path (the real `anthropic-api` provider
//! against `MessagesServer`). The fake servers answer at once, so the time is efr's own
//! chain. The fake has no WebSocket, so the WebSocket path of Responses has no line.
//!
//! The conversation grows with real turns through [`PerfModel`]: each turn reads a
//! file of 3 KiB with `read_file` and answers, so each turn adds a tool output of 3 KiB
//! to the history. The rungs are 20, 200 and 2,000 turns; the build stops after 60 s
//! and the bench then reports the turns that it reached. A line `send->request` is the
//! time until the fake server got the model request: the daemon's work before the model
//! call. The rest of `send->first text` is the stream and the fan-out.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use efr_test_daemon::{MessagesAnswer, MessagesServer, ResponsesAnswer, ResponsesServer};
use efr_test_support::TestClock;
use efr_test_support::bench::Report;
use serde_json::json;

use super::{Follower, PerfModel, Plan, builder, prompt, run_turn, tree};

/// The terminal of the bench's prompts.
const TTY: &str = "/dev/pts/perf-ttft";

/// The rungs: a label and the turns of the conversation.
const RUNGS: [(&str, usize); 3] = [("s", 20), ("m", 200), ("l", 2_000)];

/// The most real time that the build of the history may take in all.
const BUILD_BUDGET: Duration = Duration::from_secs(60);

/// The measured prompts of each path at each rung, after one that is not measured.
const SAMPLES: usize = 8;

/// The size of the file that each turn of the history reads.
const OUTPUT_BYTES: usize = 3 * 1024;

/// The answer of each turn of the history.
const ANSWER: &str = "I read the notes. They list the steps of the build and the tests.";

/// The model of the Messages path.
const CLAUDE: &str = "claude-opus-5-5";

#[derive(Debug, Clone, Copy)]
enum Path {
    Http,
    Messages,
}

impl Path {
    fn name(self) -> &'static str {
        match self {
            Path::Http => "http",
            Path::Messages => "messages",
        }
    }
}

/// The servers of both paths.
struct Servers {
    responses: ResponsesServer,
    messages: MessagesServer,
}

impl Servers {
    /// Queues answers until two wait, so a turn that compacts first gets one for its
    /// summary and one for its call.
    fn fill(&self, path: Path) {
        let text = "Hello. The build is fine.";
        match path {
            Path::Http => {
                while self.responses.remaining() < 2 {
                    self.responses.push(ResponsesAnswer::text(text));
                }
            }
            Path::Messages => {
                while self.messages.remaining() < 2 {
                    self.messages.push(MessagesAnswer::text(text));
                }
            }
        }
    }

    fn received_times(&self, path: Path) -> Vec<Instant> {
        match path {
            Path::Http => self.responses.received_times(),
            Path::Messages => self.messages.received_times(),
        }
    }

    /// The size of the latest request body of `path`, as JSON text.
    fn last_body(&self, path: Path) -> u64 {
        let body = match path {
            Path::Http => self.responses.received().last().map(|request| request.body.to_string()),
            Path::Messages => {
                self.messages.received().last().map(|request| request.body.to_string())
            }
        };
        body.map_or(0, |body| body.len() as u64)
    }
}

/// The text of the file that each turn of the history reads.
fn output_text() -> String {
    let line = "step: build the crate, run the tests, read the report, fix the warnings\n";
    line.repeat(OUTPUT_BYTES / line.len() + 1)[..OUTPUT_BYTES].to_owned()
}

/// The size of the database files under `dirs`.
fn database_bytes(dirs: &efr_test_support::TestDirs) -> u64 {
    let base = dirs.dirs().data().join(efr_store::DATABASE_FILE);
    ["", "-wal"]
        .iter()
        .map(|suffix| {
            let mut path = base.clone().into_os_string();
            path.push(suffix);
            std::fs::metadata(path).map_or(0, |meta| meta.len())
        })
        .sum()
}

/// What one path measured at one rung.
#[derive(Debug, Default)]
struct Measured {
    first_text: Vec<Duration>,
    to_request: Vec<Duration>,
    to_ack: Vec<Duration>,
    first_after_start: Vec<Duration>,
    compactions: usize,
}

/// Runs `SAMPLES + 1` prompts on `path` in the conversation of [`TTY`] on a daemon of
/// its own; the first one after the start is reported apart.
async fn measure(
    dirs: &std::sync::Arc<efr_test_support::TestDirs>,
    clock: &TestClock,
    servers: &Servers,
    path: Path,
    cwd: &std::path::Path,
) -> Measured {
    let base = builder(dirs, clock);
    let base = match path {
        Path::Http => base.responses(&servers.responses),
        Path::Messages => base.messages(&servers.messages),
    };
    let daemon = Box::pin(base.start()).await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let mut measured = Measured::default();
    for n in 0..=SAMPLES {
        servers.fill(path);
        let run = run_turn(&client, prompt(TTY, cwd, "how is the build?", None)).await;
        assert_eq!(run.end_kind(), "turn_completed", "{:#?}", run.events);
        measured.compactions += run.count("conversation_compacted");
        // NOTE: the turn's own call is the last request; a compaction's comes before it.
        let received = *servers.received_times(path).last().unwrap();
        let first = run.first_text_at.expect("the answer had text") - run.sent_at;
        if n == 0 {
            measured.first_after_start.push(first);
            continue;
        }
        measured.first_text.push(first);
        measured.to_request.push(received.saturating_duration_since(run.sent_at));
        measured.to_ack.push(run.acked_at - run.sent_at);
    }
    drop(client);
    daemon.stop().await.unwrap();
    measured
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_ttft() {
    let dirs = tree();
    let cwd = dirs.create_dir("home/project").unwrap();
    std::fs::write(cwd.join("notes.txt"), output_text()).unwrap();
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let servers = Servers {
        responses: ResponsesServer::start().await,
        messages: MessagesServer::start().await,
    };
    servers.messages.set_models(vec![MessagesServer::model(CLAUDE)]);
    let model = PerfModel::new(Plan::call("read_file", json!({ "path": "notes.txt" }), ANSWER));
    let mut report = Report::new("bench_perf_ttft");
    report.count("websocket path in the fake (0: none)", 0);
    let mut turns = 0_usize;
    let mut build_time = Duration::ZERO;
    let mut conversation = None;
    let mut compactions = 0;
    let mut points: BTreeMap<&str, Vec<(u64, Duration)>> = BTreeMap::new();
    for (label, target) in RUNGS {
        let daemon = Box::pin(
            builder(&dirs, &clock).custom_provider(model.clone() as std::sync::Arc<_>).start(),
        )
        .await
        .unwrap();
        let client = daemon.client_for_tty(TTY).await.unwrap();
        let begin = Instant::now();
        let mut stopped = false;
        while turns < target {
            let run =
                run_turn(&client, prompt(TTY, &cwd, &format!("read the notes {turns}"), None))
                    .await;
            assert_eq!(run.end_kind(), "turn_completed", "{:#?}", run.events);
            assert_eq!(run.count("tool_call_completed"), 1, "the turn read the file");
            compactions += run.count("conversation_compacted");
            let id = *conversation.get_or_insert(run.sent.conversation_id);
            assert_eq!(run.sent.conversation_id, id, "one conversation");
            turns += 1;
            if build_time + begin.elapsed() > BUILD_BUDGET {
                stopped = true;
                break;
            }
        }
        build_time += begin.elapsed();
        drop(client);
        daemon.stop().await.unwrap();
        report.count(&format!("{label} turns before the samples"), turns as u64);
        report.bytes(&format!("{label} database"), database_bytes(&dirs));
        report.count(&format!("{label} compactions so far"), compactions as u64);
        for path in [Path::Http, Path::Messages] {
            let measured = measure(&dirs, &clock, &servers, path, &cwd).await;
            turns += SAMPLES + 1;
            let name = path.name();
            report.time(&format!("{label} {name} first after start"), &measured.first_after_start);
            let stats =
                report.time(&format!("{label} {name} send->first text"), &measured.first_text);
            report.time(&format!("{label} {name} send->request"), &measured.to_request);
            report.time(&format!("{label} {name} send->ack"), &measured.to_ack);
            report.bytes(&format!("{label} {name} request body"), servers.last_body(path));
            report.count(&format!("{label} {name} compactions"), measured.compactions as u64);
            points.entry(name).or_default().push((turns as u64, stats.p50));
        }
        if stopped {
            report.count("stopped_at_n (turns)", turns as u64);
            break;
        }
    }
    report.time("history build", &[build_time]);
    for (name, points) in &points {
        report.slope(&format!("{name} first text over turns"), points);
    }
    report.finish();
}
