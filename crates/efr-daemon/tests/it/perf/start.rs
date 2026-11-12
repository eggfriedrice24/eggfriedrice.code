//! The `bench_perf_start_*` benches: where the time goes from the start of `efr send`
//! to the first text of the answer on the client. `bench_perf_start_phases` took the
//! place of `bench_perf_ttft`, whose fake servers answered at once and so had no
//! network phase.
//!
//! Each measured prompt runs as `efr send` runs it: a new connection and its hello,
//! `prompt.send`, its ack, `conversation.subscribe` with drafts, then the frames to the
//! end of the turn. The daemon is the real one in this process, with the real
//! providers against [`ProviderServer`]. Three clocks give the instants of one
//! prompt: the client (this test), the marks inside efrd and its crates
//! (`efr_stdx::mark`), and the provider server. A span is the time between two named
//! instants, and each line of the report is a span of a state on a path:
//! `<state>|<path>|<span>`.
//!
//! The paths: `ws` is the `openai-subscription` provider over the Responses WebSocket,
//! `http` is the `openai-api` provider with `[openai] websocket = "off"`, `messages` is
//! the `anthropic-api` provider.

use std::collections::BTreeMap;
use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::{Duration, Instant};

use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, ConversationSubscribe,
    ConversationSubscribeItem, DraftPart, Event, Method, Mode, PromptSendResult, ProviderWarm,
    ProviderWarmResult,
};
use efr_stdx::time::{Clock as _, SystemClock};
use efr_test_daemon::{
    FakeLauncher, ModelsRule, ProviderServer, Saw, TestDaemon, TestDaemonBuilder, command_id,
};
use efr_test_support::bench::Report;
use efr_test_support::{TestClock, TestDirs};
use futures::StreamExt as _;
use serde_json::json;

use super::{
    Follower, PerfModel, Plan, builder, git_fixture, next_command, prompt, register, run_turn, tree,
};

/// The terminal of the bench's prompts.
const TTY: &str = "/dev/pts/perf-start";

/// The model of the Messages path.
const CLAUDE: &str = "claude-opus-5-5";

/// The key of the account `anthropic-api.work` in the accounts bench.
const WORK_KEY: &str = "sk-ant-efr-work-key";

/// The size of the file that each turn of the history reads.
const OUTPUT_BYTES: usize = 3 * 1024;

/// The most real time that the build of the history may take in all.
const BUILD_BUDGET: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    Ws,
    Http,
    Messages,
}

impl Path {
    const ALL: [Path; 3] = [Path::Ws, Path::Http, Path::Messages];

    fn name(self) -> &'static str {
        match self {
            Path::Ws => "ws",
            Path::Http => "http",
            Path::Messages => "messages",
        }
    }

    /// A builder of a daemon on this path against `server`.
    fn builder(
        self,
        dirs: &Arc<TestDirs>,
        clock: &TestClock,
        server: &ProviderServer,
    ) -> TestDaemonBuilder {
        let base = builder(dirs, clock);
        match self {
            Path::Ws => base.subscription_at(&server.base_url("sub"), &server.issuer()),
            Path::Http => base.responses_at(&server.base_url("openai")).config(|settings| {
                settings.openai.websocket = efr_config::WebSocketChoice::Off;
            }),
            Path::Messages => base.messages_at(&server.base_url("anthropic")),
        }
    }
}

/// The named instants of one prompt, and how often each mark came.
#[derive(Debug, Default)]
struct Timeline {
    at: BTreeMap<String, Instant>,
    counts: BTreeMap<String, u64>,
    /// The kind of the turn's last event, or the error of a refused prompt.
    end: String,
    compactions: usize,
}

impl Timeline {
    fn set(&mut self, name: &str) {
        self.at.entry(name.to_owned()).or_insert_with(Instant::now);
    }

    fn put(&mut self, name: &str, at: Instant) {
        self.at.entry(name.to_owned()).or_insert(at);
        *self.counts.entry(name.to_owned()).or_insert(0) += 1;
    }

    fn span(&self, from: &str, to: &str) -> Option<Duration> {
        Some(self.at.get(to)?.saturating_duration_since(*self.at.get(from)?))
    }

    fn count(&self, name: &str) -> u64 {
        self.counts.get(name).copied().unwrap_or(0)
    }

    /// Takes the marks of efrd and the log of `server` into the timeline.
    fn collect(&mut self, server: &ProviderServer) {
        for mark in efr_stdx::mark::drain() {
            if !mark.detail.is_empty() {
                self.put(&format!("{}:{}", mark.name, mark.detail), mark.at);
            }
            self.put(mark.name, mark.at);
        }
        for seen in server.drain() {
            let name = match seen.what {
                Saw::Accepted => "s.accepted",
                Saw::Post => "s.post",
                Saw::Headers => "s.headers",
                Saw::FirstEvent => "s.first_event",
                Saw::Upgrade => "s.upgrade",
                Saw::Upgraded => "s.upgraded",
                Saw::Create => "s.create",
                Saw::Ping => "s.ping",
                Saw::Pong => "s.pong",
                Saw::Models => "s.models",
                Saw::Token => "s.token",
            };
            self.put(name, seen.at);
            if matches!(seen.what, Saw::Post | Saw::Create) {
                self.put("s.request", seen.at);
                self.counts.insert("s.request_bytes".to_owned(), seen.bytes as u64);
            }
        }
    }
}

/// The spans of the report: a label and the two instants.
const SPANS: &[(&str, &str, &str)] = &[
    ("T0 start -> first text (hello + all)", "c.start", "c.first_text"),
    ("T1 send -> first text", "c.send", "c.first_text"),
    ("T2 send -> request at the provider", "c.send", "s.request"),
    ("T3 first text -> end of turn", "c.first_text", "c.end"),
    ("A0 connect + hello round trip", "c.start", "c.hello"),
    ("A1 hello: /proc walk 1", "hello.begin", "hello.peer"),
    ("B0 send -> ack", "c.send", "c.acked"),
    ("B1 client -> dispatch", "c.send", "dispatch.begin:prompt.send"),
    ("B2 /proc walk 2", "dispatch.begin:prompt.send", "dispatch.authorized:prompt.send"),
    ("B3 receipt read", "dispatch.authorized:prompt.send", "ps.receipt"),
    ("B4 auto reprobe", "ps.receipt", "ps.reprobe"),
    ("B5 provider_of_prompt", "ps.reprobe", "ps.provider"),
    ("B6 list checks", "ps.provider", "ps.checked"),
    ("B7 routing", "ps.checked", "ps.routed"),
    ("B8 actor open + mailbox", "ps.routed", "actor.prompt.begin"),
    ("B9 admit", "actor.prompt.begin", "actor.prompt.admitted"),
    ("B10 commit 1 (prompt_queued)", "actor.prompt.admitted", "actor.prompt.committed"),
    ("B11 reply -> client", "actor.prompt.committed", "c.acked"),
    ("C0 ack -> first frame", "c.acked", "c.first_frame"),
    ("C1 ack -> subscribe dispatched", "c.acked", "dispatch.begin:conversation.subscribe"),
    ("C2 subscribe: /proc walk 3", "sub.begin", "sub.peer"),
    ("C3 subscribe: start read", "sub.peer", "sub.read"),
    ("C4 subscribe: start sent", "sub.read", "sub.started"),
    ("D0 commit 1 -> provider call", "actor.prompt.committed", "prov.begin"),
    ("D1 spawn turn task", "actor.prompt.committed", "turn.begin"),
    ("D2 snapshot read", "turn.begin", "turn.snapshot"),
    ("D3 scope + git", "turn.snapshot", "turn.scope"),
    ("D4 settings + router", "turn.scope", "turn.settings"),
    ("D5 commit 2 (turn_started)", "turn.settings", "turn.started"),
    ("D6 scratch", "turn.started", "turn.scratch"),
    ("D7 shell state", "turn.scratch", "turn.shell"),
    ("D8 preamble + window", "turn.shell", "turn.window"),
    ("D9 tools + base", "turn.window", "turn.base"),
    ("D10 guard", "turn.base", "turn.guard"),
    ("D11 with_window", "turn.guard", "turn.request"),
    ("D12 respond -> provider", "turn.request", "prov.begin"),
    ("E0 provider call -> request at the provider", "prov.begin", "s.request"),
    ("E1 body build", "prov.begin", "prov.body"),
    ("E2 http: encode", "prov.body", "prov.encoded"),
    ("E3 http: key or token read", "prov.post.begin", "prov.token"),
    ("E4 http: connect + write", "prov.token", "s.post"),
    ("E5 ws: take or open the socket", "ws.begin", "ws.taken"),
    ("E5a ws: token", "ws.connect.begin", "ws.connect.token"),
    ("E5b ws: handshake", "ws.connect.token", "ws.connected"),
    ("E6 ws: plan + encode", "ws.taken", "ws.encoded"),
    ("E7 ws: ping -> pong or first message", "ws.probe.begin", "ws.probe.end"),
    ("E7a ws: ping -> pong at the provider", "s.ping", "s.pong"),
    ("E8 ws: encoded -> frame at the provider", "ws.encoded", "s.create"),
    ("F0 provider: request -> first event out", "s.request", "s.first_event"),
    ("G0 first event out -> first text", "s.first_event", "c.first_text"),
    ("G1 first event out -> stream opened", "s.first_event", "stream.opened"),
    ("G1h headers out -> stream opened", "s.headers", "stream.opened"),
    ("G2 stream opened -> first event", "stream.opened", "stream.first_event"),
    ("G3 first event -> first text delta", "stream.first_event", "stream.text"),
    ("G4 text delta -> client", "stream.text", "c.first_text"),
    ("H0 first text commit", "stream.record.begin", "stream.record.end"),
    ("R0 failed attempt -> retry draft on the client", "prov.headers", "c.retry"),
    ("R1 failed attempt -> retry draft sent", "prov.headers", "draft.retrying"),
];

/// The counts of the report: a label and the name.
const COUNTS: &[(&str, &str)] = &[
    ("n new TCP connections", "s.accepted"),
    ("n posts", "s.post"),
    ("n ws creates", "s.create"),
    ("n ws upgrades", "s.upgrade"),
    ("n pings", "s.ping"),
    ("n model list fetches", "s.models"),
    ("n key file reads", "key.read"),
    ("n http attempts", "prov.attempt"),
    ("n waits for a list", "catalog.wait.begin"),
    ("n actor spawns", "actor.spawned"),
    ("n actor trims", "actor.trimmed"),
    ("n request bytes", "s.request_bytes"),
];

/// The samples of one state on one path.
#[derive(Debug, Default)]
struct Samples(Vec<Timeline>);

impl Samples {
    fn report(&self, report: &mut Report, state: &str, path: &str) {
        let prefix = format!("{state}|{path}|");
        for (label, from, to) in SPANS {
            let times: Vec<Duration> =
                self.0.iter().filter_map(|timeline| timeline.span(from, to)).collect();
            if !times.is_empty() {
                report.time(&format!("{prefix}{label}"), &times);
            }
        }
        for (label, name) in COUNTS {
            let mut counts: Vec<u64> = self.0.iter().map(|timeline| timeline.count(name)).collect();
            counts.sort_unstable();
            if counts.last().is_some_and(|max| *max > 0) {
                report.count(&format!("{prefix}{label} (median)"), counts[counts.len() / 2]);
                report.count(&format!("{prefix}{label} (max)"), counts[counts.len() - 1]);
            }
        }
        let compactions: usize = self.0.iter().map(|timeline| timeline.compactions).sum();
        report.count(&format!("{prefix}n compactions"), compactions as u64);
        let bad = self.0.iter().filter(|timeline| timeline.end != "turn_completed").count();
        report.count(&format!("{prefix}n prompts that did not complete"), bad as u64);
    }
}

/// One prompt of `text` from [`TTY`] in `cwd` on `daemon`, as `efr send` runs it, with
/// its timeline. `settings` changes the prompt before it goes.
async fn one(
    daemon: &TestDaemon,
    server: &ProviderServer,
    cwd: &FsPath,
    mode: Option<Mode>,
    settings: impl FnOnce(&mut efr_protocol::PromptSend),
) -> Timeline {
    // NOTE: what the daemon did since the last prompt, such as a fetch of a model
    // list, is not this prompt's.
    efr_stdx::mark::drain();
    server.drain();
    let mut timeline = Timeline::default();
    let mut params = prompt(TTY, cwd, "how is the build?", mode);
    settings(&mut params);
    timeline.set("c.start");
    let client = daemon.client_for_tty(TTY).await.unwrap();
    timeline.set("c.hello");
    timeline.set("c.send");
    let sent: Result<PromptSendResult, _> = client.call(Method::PromptSend(params)).await;
    timeline.set("c.acked");
    let sent = match sent {
        Ok(sent) => sent,
        Err(error) => {
            timeline.end = format!("refused: {error}");
            timeline.collect(server);
            return timeline;
        }
    };
    let subscribe = ConversationSubscribe {
        conversation_id: sent.conversation_id,
        after_seq: Some(sent.seq),
        answers_input: true,
        drafts: true,
    };
    let mut stream = client.stream(Method::ConversationSubscribe(subscribe)).await.unwrap();
    timeline.set("c.subscribed");
    let turn = sent.turn_id;
    'follow: while let Some(item) = stream.next().await {
        timeline.set("c.first_frame");
        let envelopes = match item.unwrap() {
            ConversationSubscribeItem::Event(envelope) => vec![envelope],
            ConversationSubscribeItem::Snapshot(snapshot) => snapshot.events,
            ConversationSubscribeItem::Draft(draft) => {
                if matches!(draft.draft, DraftPart::Text { .. }) && draft.turn_id == turn {
                    timeline.set("c.first_text");
                }
                if matches!(draft.draft, DraftPart::Retrying { .. }) && draft.turn_id == turn {
                    timeline.set("c.retry");
                }
                Vec::new()
            }
            _ => Vec::new(),
        };
        for envelope in envelopes {
            match &envelope.event {
                Event::AssistantMessageUpdated { turn_id, .. }
                | Event::AssistantMessageCompleted { turn_id, .. }
                    if *turn_id == turn =>
                {
                    timeline.set("c.first_text");
                }
                Event::ConversationCompacted { .. } => timeline.compactions += 1,
                Event::ApprovalRequested { turn_id, call_id, .. } if *turn_id == turn => {
                    let answer = Method::ApprovalRespond(ApprovalRespond {
                        command_id: command_id(next_command()),
                        conversation_id: sent.conversation_id,
                        call_id: *call_id,
                        decision: ApprovalDecision::Allow,
                    });
                    let _: ApprovalRespondResult = client.call(answer).await.unwrap();
                }
                Event::TurnCompleted { turn_id, .. }
                | Event::TurnFailed { turn_id, .. }
                | Event::TurnInterrupted { turn_id, .. }
                    if *turn_id == turn =>
                {
                    timeline.set("c.end");
                    timeline.end = envelope.event.kind().to_owned();
                    break 'follow;
                }
                _ => {}
            }
        }
    }
    drop(stream);
    drop(client);
    timeline.collect(server);
    timeline
}

/// A prompt with no change.
fn plain(_: &mut efr_protocol::PromptSend) {}

/// Lets the daemon's tasks run for a moment of real time, such as after a move of the
/// clock.
async fn settle() {
    SystemClock.sleep(Duration::from_millis(60)).await;
}

/// The text of the file that each turn of the history reads.
fn output_text() -> String {
    let line = "step: build the crate, run the tests, read the report, fix the warnings\n";
    line.repeat(OUTPUT_BYTES / line.len() + 1)[..OUTPUT_BYTES].to_owned()
}

/// Grows the conversation of [`TTY`] in `cwd` to `target` turns with [`PerfModel`];
/// gives the turns that it reached within the budget.
async fn grow(
    dirs: &Arc<TestDirs>,
    clock: &TestClock,
    cwd: &FsPath,
    turns: &mut usize,
    target: usize,
    spent: &mut Duration,
) {
    let answer = "I read the notes. They list the steps of the build and the tests.";
    let model = PerfModel::new(Plan::call("read_file", json!({ "path": "notes.txt" }), answer));
    let daemon =
        Box::pin(builder(dirs, clock).custom_provider(model as Arc<_>).start()).await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let begin = Instant::now();
    while *turns < target && *spent + begin.elapsed() < BUILD_BUDGET {
        let run =
            run_turn(&client, prompt(TTY, cwd, &format!("read the notes {turns}"), None)).await;
        assert_eq!(run.end_kind(), "turn_completed", "{:#?}", run.events);
        *turns += 1;
    }
    *spent += begin.elapsed();
    drop(client);
    daemon.stop().await.unwrap();
}

fn claude(server: &ProviderServer) {
    server.set(|knobs| knobs.claude = vec![CLAUDE.to_owned()]);
}

/// The states of a prompt on each path, with a history of 20, 200 and 2,000 turns: the
/// first prompt after a start, the second one at once, a warm one; and with 20 turns
/// also a prompt after 31 s of quiet (on `ws` also on a socket that died without a
/// close), after the provider closed the connection, the same with a warm-up of the
/// prompt line (`provider.warm`) before it, after
/// 10 min of idle time, and against a provider that answers slowly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_start_phases() {
    efr_stdx::mark::enable();
    let dirs = tree();
    let cwd = dirs.create_dir("home/project").unwrap();
    std::fs::write(cwd.join("notes.txt"), output_text()).unwrap();
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let server = ProviderServer::start().await.unwrap();
    claude(&server);
    let mut report = Report::new("bench_perf_start_phases");
    let mut quiet_budget = None;
    let mut warm_budgets = Vec::new();
    let mut turns = 0;
    let mut spent = Duration::ZERO;
    for (rung, target) in [("h20", 20), ("h200", 200), ("h2000", 2_000)] {
        grow(&dirs, &clock, &cwd, &mut turns, target, &mut spent).await;
        report.count(&format!("{rung} turns before the samples"), turns as u64);
        for path in Path::ALL {
            let mut daemon = Box::pin(path.builder(&dirs, &clock, &server).start()).await.unwrap();
            // NOTE: not measured: the first start on a tree fetches the first model list
            // of the Messages path; every later start reads it from the cache file.
            one(&daemon, &server, &cwd, None, plain).await;
            let mut first = Samples::default();
            let mut second = Samples::default();
            for _ in 0..3 {
                Box::pin(daemon.restart()).await.unwrap();
                settle().await;
                first.0.push(one(&daemon, &server, &cwd, None, plain).await);
                second.0.push(one(&daemon, &server, &cwd, None, plain).await);
            }
            turns += 7;
            let mut warm = Samples::default();
            for _ in 0..7 {
                warm.0.push(one(&daemon, &server, &cwd, None, plain).await);
            }
            turns += 7;
            first.report(&mut report, &format!("{rung} first"), path.name());
            second.report(&mut report, &format!("{rung} second"), path.name());
            warm.report(&mut report, &format!("{rung} warm"), path.name());
            if rung == "h20" {
                let mut quiet = Samples::default();
                for _ in 0..5 {
                    clock.advance(Duration::from_secs(31));
                    settle().await;
                    quiet.0.push(one(&daemon, &server, &cwd, None, plain).await);
                }
                quiet.report(&mut report, "h20 quiet31s", path.name());

                let mut quiet_slow = Samples::default();
                server.set(|knobs| knobs.pong = Duration::from_millis(100));
                for _ in 0..3 {
                    clock.advance(Duration::from_secs(31));
                    settle().await;
                    quiet_slow.0.push(one(&daemon, &server, &cwd, None, plain).await);
                }
                server.set(|knobs| knobs.pong = Duration::ZERO);
                quiet_slow.report(&mut report, "h20 quiet31s+pong100ms", path.name());
                if path == Path::Ws {
                    let mut sends: Vec<Duration> = quiet_slow
                        .0
                        .iter()
                        .filter_map(|timeline| timeline.span("c.send", "c.first_text"))
                        .collect();
                    sends.sort_unstable();
                    quiet_budget = sends.get(sends.len() / 2).copied();
                }

                // NOTE: the WebSocket died without a close, as after a sleep of the
                // laptop: the call must still reach HTTP within the ping time of 5 s.
                // The fall to HTTP drops the connection, so a prompt before each
                // sample opens a live one.
                if path == Path::Ws {
                    let mut dead = Samples::default();
                    for _ in 0..3 {
                        one(&daemon, &server, &cwd, None, plain).await;
                        clock.advance(Duration::from_secs(31));
                        settle().await;
                        server.set(|knobs| knobs.dead = true);
                        dead.0.push(one(&daemon, &server, &cwd, None, plain).await);
                        server.set(|knobs| knobs.dead = false);
                    }
                    dead.report(&mut report, "h20 quiet31s+dead", path.name());
                    turns += 6;
                }

                // NOTE: the answer of each prompt closes its connection, so each
                // measured prompt needs a new one.
                let mut cold = Samples::default();
                server.set(|knobs| knobs.close = true);
                one(&daemon, &server, &cwd, None, plain).await;
                settle().await;
                for _ in 0..5 {
                    cold.0.push(one(&daemon, &server, &cwd, None, plain).await);
                    settle().await;
                }
                cold.report(&mut report, "h20 coldconn", path.name());

                let mut cold_slow = Samples::default();
                server.set(|knobs| knobs.upgrade = Duration::from_millis(100));
                for _ in 0..3 {
                    cold_slow.0.push(one(&daemon, &server, &cwd, None, plain).await);
                    settle().await;
                }
                cold_slow.report(&mut report, "h20 coldconn+upgrade100ms", path.name());

                // NOTE: the user types the prompt line a minute after the last prompt, so
                // `provider.warm` opens the connection that the server closed; on the HTTP
                // paths a warm-up fetches the model list only a minute after a prompt.
                let mut warmed = Samples::default();
                for _ in 0..3 {
                    clock.advance(Duration::from_secs(61));
                    settle().await;
                    warm_up(&daemon, &cwd).await;
                    warmed.0.push(one(&daemon, &server, &cwd, None, plain).await);
                    settle().await;
                }
                server.set(|knobs| knobs.upgrade = Duration::ZERO);
                warmed.report(&mut report, "h20 coldconn+upgrade100ms+warm", path.name());
                warm_budgets.push((path, warmed));

                let mut idle = Samples::default();
                for _ in 0..3 {
                    clock.advance(Duration::from_secs(601));
                    settle().await;
                    idle.0.push(one(&daemon, &server, &cwd, None, plain).await);
                    settle().await;
                }
                server.set(|knobs| knobs.close = false);
                idle.report(&mut report, "h20 idle10m+coldconn", path.name());

                let mut slow = Samples::default();
                one(&daemon, &server, &cwd, None, plain).await;
                server.set(|knobs| {
                    knobs.headers = Duration::from_millis(200);
                    knobs.first_event = Duration::from_millis(300);
                });
                for _ in 0..3 {
                    slow.0.push(one(&daemon, &server, &cwd, None, plain).await);
                }
                server.set(|knobs| {
                    knobs.headers = Duration::ZERO;
                    knobs.first_event = Duration::ZERO;
                });
                slow.report(&mut report, "h20 warm+provider500ms", path.name());
                turns += 25;
            }
            daemon.stop().await.unwrap();
        }
        if spent >= BUILD_BUDGET {
            report.count("stopped_at_n (turns)", turns as u64);
            break;
        }
    }
    report.time("history build", &[spent]);
    report.finish();
    // The budget: a call on a quiet connection does not wait for the pong of its ping,
    // which takes 100 ms here (the level of a quiet connection with a fast pong is 4 to
    // 5 ms).
    let quiet = quiet_budget.expect("the ws path ran the quiet state");
    assert!(
        quiet < QUIET_PROMPT_BUDGET,
        "h20 quiet31s+pong100ms|ws: send -> first text, p50 {quiet:?}"
    );
    // The budgets of the warm-up: the prompt after it opens no connection on any path,
    // and on `ws` it does not wait for the upgrade of 100 ms (the warm level is 3.5 to
    // 4 ms).
    assert_eq!(warm_budgets.len(), Path::ALL.len(), "every path ran the warm state");
    for (path, warmed) in &warm_budgets {
        for timeline in &warmed.0 {
            let opened = timeline.count("s.accepted") + timeline.count("s.upgrade");
            assert_eq!(
                opened,
                0,
                "h20 coldconn+upgrade100ms+warm|{}: a new connection",
                path.name()
            );
        }
        if *path == Path::Ws {
            let mut sends: Vec<Duration> = warmed
                .0
                .iter()
                .filter_map(|timeline| timeline.span("c.send", "c.first_text"))
                .collect();
            sends.sort_unstable();
            let p50 = sends[sends.len() / 2];
            assert!(
                p50 < WARM_PROMPT_BUDGET,
                "h20 coldconn+upgrade100ms+warm|ws: send -> first text, p50 {p50:?}"
            );
        }
    }
}

/// The budget of T1 (send -> first text) of a prompt whose WebSocket the warm-up of
/// its prompt line opened, when the upgrade takes 100 ms.
const WARM_PROMPT_BUDGET: Duration = Duration::from_millis(50);

/// Sends `provider.warm` for [`TTY`] in `cwd`, as the zsh plugin does while the user
/// types a prompt line, and waits until the warm-up ends.
async fn warm_up(daemon: &TestDaemon, cwd: &FsPath) {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let params = ProviderWarm {
        tty: Some(TTY.to_owned()),
        pwd: Some(cwd.to_owned()),
        model: None,
        provider: None,
        new_conversation: false,
    };
    efr_stdx::mark::drain();
    let _: ProviderWarmResult = client.call(Method::ProviderWarm(params)).await.unwrap();
    let begin = Instant::now();
    while !efr_stdx::mark::drain().iter().any(|mark| mark.name == "warm.done") {
        assert!(begin.elapsed() < Duration::from_secs(10), "the warm-up did not end");
        SystemClock.sleep(Duration::from_millis(2)).await;
    }
}

/// The budget of T1 (send -> first text) of a call on a WebSocket after 31 quiet
/// seconds, when the pong takes 100 ms: the request goes out with the ping.
const QUIET_PROMPT_BUDGET: Duration = Duration::from_millis(50);

/// A prompt on the `http` path in a plain directory, in a git work tree, in a
/// registered project, in `auto`, and with a live hidden shell (zsh) that stays or
/// that the user's `cd` moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_start_places() {
    efr_stdx::mark::enable();
    let mut report = Report::new("bench_perf_start_places");
    let zsh = crate::support::zsh_enabled("bench_perf_start_places");
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let server = ProviderServer::start().await.unwrap();
    claude(&server);
    for path in [Path::Http, Path::Messages] {
        let dirs = tree();
        let plain_dir = dirs.create_dir("home/plain").unwrap();
        let repo = dirs.create_dir("home/repo").unwrap();
        git_fixture(&repo, 50).await;
        let project = dirs.create_dir("home/project").unwrap();
        git_fixture(&project, 50).await;
        let sub = project.join("d000");
        let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
        let mut base = path
            .builder(&dirs, &clock, &server)
            .sandbox_launcher(fake.path())
            .probe_override(crate::sandbox::ready());
        if zsh {
            base = base.local_pty();
        }
        let daemon = Box::pin(base.start()).await.unwrap();
        register(&daemon, &project).await;
        let cases: [(&str, &FsPath, Option<Mode>); 5] = [
            ("plain dir, cautious", &plain_dir, None),
            ("git repo, cautious", &repo, None),
            ("project, cautious", &project, None),
            ("project, auto", &project, Some(Mode::Auto)),
            ("project, manual", &project, Some(Mode::Manual)),
        ];
        for (label, cwd, mode) in cases {
            one(&daemon, &server, cwd, mode, plain).await;
            let mut samples = Samples::default();
            for _ in 0..7 {
                samples.0.push(one(&daemon, &server, cwd, mode, plain).await);
            }
            samples.report(&mut report, label, path.name());
        }
        if zsh {
            // NOTE: one turn runs a command, so the conversation of the project has a
            // hidden shell from here on.
            server.set(|knobs| {
                knobs.tool_once = Some(("shell".to_owned(), json!({ "command": "true" })));
            });
            let started = one(&daemon, &server, &project, Some(Mode::Auto), plain).await;
            report.count(
                &format!("shell turn|{}|completed (1: yes)", path.name()),
                u64::from(started.end == "turn_completed"),
            );
            let mut stays = Samples::default();
            for _ in 0..7 {
                stays.0.push(one(&daemon, &server, &project, Some(Mode::Auto), plain).await);
            }
            stays.report(&mut report, "project, auto, live shell", path.name());
            let mut moves = Samples::default();
            for n in 0..6 {
                let cwd = if n % 2 == 0 { &sub } else { &project };
                moves.0.push(one(&daemon, &server, cwd, Some(Mode::Auto), plain).await);
            }
            moves.report(&mut report, "project, auto, live shell, user cd", path.name());
        } else {
            report.count("skipped: the live shell cases need EFR_TEST_ZSH=1", 1);
        }
        daemon.stop().await.unwrap();
    }
    report.finish();
}

/// Stores `key` as the credential of the provider `id` in the tree `dirs`.
fn store_key(dirs: &TestDirs, id: &str, key: &str) {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
    let dir = dirs.dirs().data().join("secrets");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).unwrap();
    let record = json!({ "version": 1, "kind": "api_key", "key": key });
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(dir.join(format!("{id}.json")))
        .unwrap();
    std::io::Write::write_all(&mut file, format!("{record}\n").as_bytes()).unwrap();
}

/// The budget of `provider_of_prompt` (B5) for each prompt after the first one of a
/// start while `[model] provider` is the account: such a prompt asks for no list fetch
/// and waits for none. The healthy level is 0.02 to 0.04 ms; one fetch was 0.13 to
/// 2,727 ms before slice 4 of the streaming plan.
const ACCOUNT_PROMPT_BUDGET: Duration = Duration::from_millis(5);

/// Suspect S2: what a prompt waits when an Anthropic account has a login and no model
/// list, next to a healthy provider. The account `anthropic-api.work` has a key whose
/// list fetch fails with a 401, fails with a 500, or takes 2 s. Only the first fetch
/// after a start holds a prompt ([`ACCOUNT_PROMPT_BUDGET`]).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_start_accounts() {
    efr_stdx::mark::enable();
    let mut report = Report::new("bench_perf_start_accounts");
    let mut over = Vec::new();
    let rules: [(&str, Option<ModelsRule>); 4] = [
        ("work healthy", None),
        ("work 401", Some(ModelsRule { status: 401, delay: Duration::ZERO })),
        (
            "work 401 after 300ms",
            Some(ModelsRule { status: 401, delay: Duration::from_millis(300) }),
        ),
        ("work 500", Some(ModelsRule { status: 500, delay: Duration::ZERO })),
    ];
    for (rule_name, rule) in rules {
        // The provider of new conversations: the healthy OpenAI key, the healthy
        // Anthropic key, or the account.
        for configured in ["openai-api", "anthropic-api", "anthropic-api.work"] {
            let dirs = tree();
            let cwd = dirs.create_dir("home/project").unwrap();
            let clock = TestClock::new();
            let _follower = Follower::start(&clock);
            let server = ProviderServer::start().await.unwrap();
            claude(&server);
            if let Some(rule) = &rule {
                let rule = rule.clone();
                server.set(|knobs| {
                    knobs.models.insert(format!("Bearer {WORK_KEY}"), rule);
                });
            }
            store_key(&dirs, "anthropic-api.work", WORK_KEY);
            let name = configured.to_owned();
            let daemon = Box::pin(
                builder(&dirs, &clock)
                    .responses_at(&server.base_url("openai"))
                    .also_messages_at(&server.base_url("anthropic"))
                    .config(move |settings| {
                        settings.openai.websocket = efr_config::WebSocketChoice::Off;
                    })
                    .start(),
            )
            .await
            .unwrap();
            // NOTE: `[model] provider` goes through the config file, as the user sets
            // it, because the builder pins the provider of its first choice there.
            let file =
                efr_daemon::ConfigFile::open(&dirs.dirs().config().join(efr_daemon::CONFIG_FILE))
                    .unwrap();
            let mut edit = file.edit().unwrap();
            edit.set_text("model.provider", &name).unwrap();
            file.write(&edit).unwrap();
            let client = daemon.client().await.unwrap();
            let reload = Method::AdminConfigReload(efr_protocol::AdminConfigReload {});
            let changed: efr_protocol::AdminConfigReloadResult = client.call(reload).await.unwrap();
            report.count(
                &format!("{rule_name}|{configured}|config reload applied (1: yes)"),
                u64::from(changed.applied),
            );
            drop(client);
            // The first fetches of the start end.
            SystemClock.sleep(Duration::from_millis(600)).await;
            let asks: [(&str, Option<&str>, Option<&str>); 4] = [
                ("default model", None, None),
                ("claude model that a list names", Some(CLAUDE), None),
                ("claude model that no list names", Some("claude-future-9"), None),
                ("work/claude (names the account)", Some(CLAUDE), Some("work")),
            ];
            let mut nth = 0;
            for (ask, model, provider) in asks {
                let mut samples = Samples::default();
                for _ in 0..5 {
                    let timeline = one(&daemon, &server, &cwd, None, |params| {
                        params.settings.model = model.map(str::to_owned);
                        params.settings.provider = provider.map(str::to_owned);
                    })
                    .await;
                    nth += 1;
                    if configured == "anthropic-api.work" && nth > 1 {
                        let fetches = timeline.count("s.models");
                        let held = timeline.span("ps.reprobe", "ps.provider");
                        if fetches > 0 || held.is_none_or(|held| held > ACCOUNT_PROMPT_BUDGET) {
                            over.push(format!(
                                "{rule_name}, {ask}, prompt {nth}: {fetches} list fetches, {held:?}"
                            ));
                        }
                    }
                    samples.0.push(timeline);
                }
                let state = format!("{rule_name}, configured {configured}, {ask}");
                samples.report(&mut report, &state, "mixed");
            }
            daemon.stop().await.unwrap();
        }
    }
    report.finish();
    assert!(
        over.is_empty(),
        "a prompt after the first one of a start waits for the list of the account \
         (budget: no list fetch, B5 at or below {ACCOUNT_PROMPT_BUDGET:?}): {over:#?}"
    );
}

/// The longest time from the failed attempt to the retry draft on the client, at the
/// median of the samples of a path (slice 6 of the streaming plan): the terminal says
/// why it waits in the frame after the failure.
const RETRY_SIGN_BUDGET: Duration = Duration::from_millis(50);

/// A model call whose first attempt gets a 503: how long the retry holds the first
/// text back, on the two HTTP paths, and how soon the client hears of the retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_start_retry() {
    efr_stdx::mark::enable();
    let mut report = Report::new("bench_perf_start_retry");
    let mut over = Vec::new();
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let server = ProviderServer::start().await.unwrap();
    claude(&server);
    for path in [Path::Http, Path::Messages] {
        let dirs = tree();
        let cwd = dirs.create_dir("home/project").unwrap();
        let daemon = Box::pin(path.builder(&dirs, &clock, &server).start()).await.unwrap();
        one(&daemon, &server, &cwd, None, plain).await;
        one(&daemon, &server, &cwd, None, plain).await;
        let mut samples = Samples::default();
        for _ in 0..5 {
            server.set(|knobs| knobs.fail_once = Some(503));
            samples.0.push(one(&daemon, &server, &cwd, None, plain).await);
        }
        samples.report(&mut report, "warm, one 503 first", path.name());
        let mut signs: Vec<Duration> = samples
            .0
            .iter()
            .filter_map(|timeline| timeline.span("prov.headers", "c.retry"))
            .collect();
        report.count(
            &format!(
                "warm, one 503 first|{}|n prompts whose client heard of the retry",
                path.name()
            ),
            signs.len() as u64,
        );
        signs.sort_unstable();
        let median = signs.get(signs.len() / 2).copied();
        if signs.len() < samples.0.len() || median.is_none_or(|median| median > RETRY_SIGN_BUDGET) {
            over.push((path.name(), samples.0.len(), signs));
        }
        daemon.stop().await.unwrap();
    }
    report.finish();
    assert!(
        over.is_empty(),
        "the client must hear of each retry, at the median within {RETRY_SIGN_BUDGET:?} of \
         the failed attempt (path, prompts, times): {over:?}"
    );
}

/// A prompt after 11 min of idle time on the clock, with a history of 200 turns: the
/// actor dropped its turns after 10 min, the heap trim ran 30 s later, the WebSocket
/// closed, and the provider closed the HTTP connection. A warm prompt of the same
/// daemon is the base.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_start_idle() {
    efr_stdx::mark::enable();
    let mut report = Report::new("bench_perf_start_idle");
    let dirs = tree();
    let cwd = dirs.create_dir("home/project").unwrap();
    std::fs::write(cwd.join("notes.txt"), output_text()).unwrap();
    let clock = TestClock::new();
    let _follower = Follower::start(&clock);
    let server = ProviderServer::start().await.unwrap();
    claude(&server);
    let mut turns = 0;
    let mut spent = Duration::ZERO;
    grow(&dirs, &clock, &cwd, &mut turns, 200, &mut spent).await;
    report.count("turns before the samples", turns as u64);
    for path in Path::ALL {
        let daemon = Box::pin(path.builder(&dirs, &clock, &server).start()).await.unwrap();
        one(&daemon, &server, &cwd, None, plain).await;
        one(&daemon, &server, &cwd, None, plain).await;
        let mut warm = Samples::default();
        let mut idle = Samples::default();
        for _ in 0..4 {
            warm.0.push(one(&daemon, &server, &cwd, None, plain).await);
            server.set(|knobs| knobs.close = true);
            one(&daemon, &server, &cwd, None, plain).await;
            server.set(|knobs| knobs.close = false);
            clock.advance(Duration::from_secs(601));
            SystemClock.sleep(Duration::from_millis(150)).await;
            clock.advance(Duration::from_secs(31));
            SystemClock.sleep(Duration::from_millis(300)).await;
            idle.0.push(one(&daemon, &server, &cwd, None, plain).await);
            one(&daemon, &server, &cwd, None, plain).await;
        }
        warm.report(&mut report, "h200 warm", path.name());
        idle.report(&mut report, "h200 idle11m", path.name());
        daemon.stop().await.unwrap();
    }
    report.finish();
}
