//! A command that waits for input in the hidden shell (v0.0.4 shell items, DS3, DS4,
//! DS6 and DS50) end to end: the model's call, efrd's engine and the hidden shell's
//! looks, the real `efr-sbx` of `just test-sandbox` (`EFR_TEST_SBX_BIN`) and a real
//! hidden zsh (`EFR_TEST_ZSH=1`). The tests of `input_respond` run the same rules with
//! the fake launcher; here the waits come from a real exit child and a real sandbox.
//!
//! Each command prints `efr-ready` before it asks, and the clock stands still until
//! that marker shows: the hidden zsh and the launcher start in real time. Then the
//! daemon's clock moves one second at a time, and only while a sleep of the daemon
//! waits for it. A command in the sandbox that must outlive its call reads a FIFO that
//! the test writes. No test waits on a fixed time, and no test types a password into a
//! real program but a small `sh` script of the test.
//!
//! The tests say `skipped:` without the launcher or zsh, or when the probe says that
//! the sandbox is unavailable; `just test-sandbox` fails on such a line on a ready
//! machine. Their dirs stay below the target dir, not below /tmp, which the sandbox
//! replaces with its own.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    ApprovalRespond, ApprovalRespondResult, CallId, ConversationId, ConversationSubscribe,
    ConversationSubscribeItem, ErrorCode, Event, EventEnvelope, InputRespond, InputRespondResult,
    InputWait, Launch, Method, Mode, PromptSendResult, SecretText, Seq, ShellCommandEnd,
    ShellStatus, ShellStatusResult, TurnSettings,
};
use efr_stdx::time::Clock as _;
use efr_test_daemon::{Client, ClientError, ItemStream, TTY, TestDaemon, events_until};
use efr_test_support::TestDirs;
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::netview::target_dirs;
use super::{real_launcher, skip, with_zsh};
use crate::jobs_wake::{Answer, REVIEWER_ASKS, Recorder};
use crate::support::{files_holding, zsh_enabled};

/// The marker that each command prints before it asks.
const READY: &str = "efr-ready";

/// The timeout of each call, in seconds: short, so the clock passes it in a few steps.
const TIMEOUT: u64 = 2;

/// A program that asks for a password as `getpass` does: echo off, then the prompt,
/// then one line in line mode. It prints the length of what it read, never the text.
const GETPASS: &str =
    r#"sh -c 'stty -echo; printf "pw: "; IFS= read -r p; stty echo; printf "\nlen=%s\n" "${#p}"'"#;

/// A program that turns line mode off with echo on and asks for a password, as a relay
/// such as `ssh` or `sudo`'s own pty does. It prints what it read.
const RAW_PASSWORD: &str =
    r#"sh -c 'stty -icanon; printf "Password: "; IFS= read -r p; printf "\ngot=%s\n" "$p"'"#;

/// The password the user types: nothing may hold it but the program that reads it.
const SECRET: &str = "hunter2-efr-busy-secret";

/// The tree of a test, below the target dir: the dir of the fake `scp` first on the
/// hidden shell's `PATH`, and a FIFO below the home dir, which a contained command can
/// read.
struct Place {
    dirs: Arc<TestDirs>,
    bin: PathBuf,
    fifo: PathBuf,
}

impl Place {
    fn new() -> Place {
        let dirs = target_dirs();
        let bin = dirs.create_dir("bin").unwrap();
        let gate = dirs.create_dir("home/gate").unwrap();
        let fifo = gate.join("go");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        Place { dirs, bin, fifo }
    }

    /// A line that prints [`READY`], waits until the test writes the FIFO, then runs
    /// `then`.
    fn gated(then: &str) -> String {
        format!("echo {READY}; read answer < ~/gate/go; {then}")
    }

    /// A fake `scp` on the hidden shell's `PATH` that prints [`READY`], runs `then` and
    /// copies nothing. An upload runs in the exit child, outside the sandbox, after an
    /// answer; the line is only `scp notes.txt h:notes.txt`, which may ask for a
    /// password (DS2).
    fn fake_scp(&self, then: &str) -> &'static str {
        let scp = self.bin.join("scp");
        std::fs::write(&scp, format!("#!/bin/sh\necho {READY}\n{then}\n")).unwrap();
        std::fs::set_permissions(&scp, std::fs::Permissions::from_mode(0o755)).unwrap();
        "scp notes.txt h:notes.txt"
    }

    /// The files below the tree that hold [`SECRET`], once the daemon stopped.
    fn holding_the_secret(&self) -> Vec<String> {
        // NOTE: a read of the FIFO would wait for a writer forever.
        std::fs::remove_file(&self.fifo).unwrap();
        let seen = files_holding(self.dirs.root(), READY.as_bytes());
        assert!(!seen.is_empty(), "the scan sees the record of the call");
        files_holding(self.dirs.root(), SECRET.as_bytes())
    }

    /// Lets the command go on past the FIFO; it waits for the reader.
    async fn release(&self) {
        let fifo = self.fifo.clone();
        tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
    }

    /// A daemon in `auto` with the real launcher, the probe's own result, a real hidden
    /// zsh and a model that calls `shell` with `input` once. The reviewer hands each exit
    /// to the user. `None` after a `skipped:` line.
    async fn daemon(&self, test: &str, input: serde_json::Value) -> Option<TestDaemon> {
        if !zsh_enabled(test) {
            return None;
        }
        let launcher = real_launcher(test)?;
        let user_runtime = self.dirs.create_dir("xrt").unwrap();
        std::fs::create_dir_all(self.dirs.home().join("project")).unwrap();
        let env = BTreeMap::from([
            ("HOME".to_owned(), self.dirs.home().display().to_string()),
            ("PATH".to_owned(), format!("{}:/usr/bin:/bin", self.bin.display())),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("XDG_RUNTIME_DIR".to_owned(), user_runtime.display().to_string()),
        ]);
        crate::full::write_config(
            &self.dirs,
            "[permissions]\nmode = \"auto\"\n\n[jobs]\nmax_wakes_per_hour = 0\n",
        );
        let answers: Vec<Answer> = vec![Some(("shell", input)), None];
        // NOTE: the clock starts after the files of the test were written, so no file of
        // the test counts as a program that changed in the turn (which asks the user).
        let start = jiff::Timestamp::now().checked_add(jiff::SignedDuration::from_mins(1)).unwrap();
        let daemon = with_zsh(TestDaemon::builder())
            .clock(efr_test_support::TestClock::starting_at(start))
            .dirs(Arc::clone(&self.dirs))
            .persistent()
            .shell_env(env)
            .sandbox_launcher(&launcher)
            .config(|config| {
                config.permissions.mode = Mode::Auto;
                config.jobs.max_wakes_per_hour = 0;
            })
            .custom_provider(Recorder::reviewed(answers, REVIEWER_ASKS))
            .start()
            .await
            .unwrap();
        let client = daemon.client().await.unwrap();
        let status: AdminStatusResult =
            client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
        let sandbox = status.sandbox.unwrap();
        if !sandbox.available {
            // NOTE: just test-sandbox fails when this says skipped on a ready machine.
            skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
            drop(client);
            daemon.stop().await.unwrap();
            return None;
        }
        let add = Method::AdminProjectAdd(AdminProjectAdd {
            path: daemon.cwd().to_path_buf(),
            name: None,
            git_root: false,
        });
        let _: AdminProjectAddResult = client.call(add).await.unwrap();
        std::fs::write(daemon.cwd().join("notes.txt"), "notes\n").unwrap();
        drop(client);
        Some(daemon)
    }
}

/// The `shell` input of `line` with the timeout `seconds`.
fn call(line: &str, seconds: u64) -> serde_json::Value {
    json!({ "command": line, "timeout_seconds": seconds })
}

/// Sends the prompt from the test's terminal in `auto`, follows it on the same client,
/// which can answer a waiting command, and allows each question, until the call's output
/// shows [`READY`]. Returns the conversation, the stream and the questions' events. The
/// clock stands still meanwhile.
async fn start(
    daemon: &TestDaemon,
    client: &Client,
) -> (ConversationId, ItemStream<ConversationSubscribeItem>, Vec<EventEnvelope>) {
    let Method::PromptSend(mut params) = daemon.prompt(1, "copy my notes", TTY) else {
        unreachable!()
    };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let conversation_id = sent.conversation_id;
    let subscribe = ConversationSubscribe {
        conversation_id,
        after_seq: Some(Seq::ZERO),
        answers_input: true,
        drafts: false,
    };
    let mut stream = client.stream(Method::ConversationSubscribe(subscribe)).await.unwrap();
    let mut questions = Vec::new();
    let mut answers = 100;
    loop {
        let seen = events_until(&mut stream, |event| {
            matches!(event, Event::ToolCallOutputUpdated { tail, .. } if tail.contains(READY))
                || matches!(
                    event,
                    Event::ApprovalRequested { .. }
                        | Event::ToolCallCompleted { .. }
                        | Event::TurnFailed { .. }
                )
        })
        .await
        .unwrap();
        match seen.last().map(|envelope| envelope.event.clone()) {
            Some(Event::ApprovalRequested { call_id, .. }) => {
                questions.push(seen.last().unwrap().clone());
                answers += 1;
                let allow = Method::ApprovalRespond(ApprovalRespond {
                    command_id: efr_test_daemon::command_id(answers),
                    conversation_id,
                    call_id,
                    decision: ApprovalDecision::Allow,
                });
                let _: ApprovalRespondResult = client.call(allow).await.unwrap();
            }
            Some(Event::ToolCallOutputUpdated { .. }) => {
                return (conversation_id, stream, questions);
            }
            other => panic!("the call did not start: {other:?} {seen:#?}"),
        }
    }
}

/// The events of one subscription item.
fn envelopes(item: ConversationSubscribeItem) -> Vec<EventEnvelope> {
    match item {
        ConversationSubscribeItem::Event(envelope) => vec![envelope],
        ConversationSubscribeItem::Snapshot(snapshot) => snapshot.events,
        other => panic!("{other:?}"),
    }
}

/// Lets other tasks run for a while.
async fn idle() {
    for _ in 0..2000 {
        tokio::task::yield_now().await;
    }
}

/// How long [`events_while_time_passes`] may take in real time: the launcher starts and
/// stops a sandbox in real time, which a loaded machine makes slow, and nextest ends a
/// test after 60 s.
const REAL_TIME_LIMIT: Duration = Duration::from_secs(30);

/// The events of `stream` up to the first that `stop` accepts. Whenever none comes for
/// a while and a sleep of the daemon ends within one second, the daemon's clock moves
/// one second, at most `max_seconds` times. With no `stop`, it returns after
/// `max_seconds` moves. The clock moves only while the daemon waits for it.
async fn events_while_time_passes(
    daemon: &TestDaemon,
    stream: &mut ItemStream<ConversationSubscribeItem>,
    max_seconds: u32,
    mut stop: Option<&mut dyn FnMut(&Event) -> bool>,
) -> Vec<EventEnvelope> {
    let step = Duration::from_secs(1);
    let started = Instant::now();
    let mut seen = Vec::new();
    let mut moved = 0;
    loop {
        tokio::select! {
            biased;
            item = stream.next() => {
                for envelope in envelopes(item.unwrap().unwrap()) {
                    let done = stop.as_mut().is_some_and(|stop| stop(&envelope.event));
                    seen.push(envelope);
                    if done {
                        return seen;
                    }
                }
            }
            () = idle() => {
                let waited = started.elapsed();
                assert!(
                    waited < REAL_TIME_LIMIT,
                    "the end did not come in {waited:?} of real time ({moved}s on the \
                     daemon's clock): {seen:#?}"
                );
                let clock = daemon.clock();
                let soon = clock.now().checked_add(step).unwrap();
                if clock.next_deadline().is_some_and(|deadline| deadline <= soon) {
                    if moved == max_seconds {
                        assert!(stop.is_none(), "nothing came in {max_seconds}s: {seen:#?}");
                        return seen;
                    }
                    clock.advance(step);
                    moved += 1;
                }
            }
        }
    }
}

/// Lets time pass up to the first event that `stop` accepts, at most 10 s on the clock.
async fn until(
    daemon: &TestDaemon,
    stream: &mut ItemStream<ConversationSubscribeItem>,
    mut stop: impl FnMut(&Event) -> bool,
) -> Vec<EventEnvelope> {
    events_while_time_passes(daemon, stream, 10, Some(&mut stop)).await
}

/// The last event of `events`.
fn last(events: &[EventEnvelope]) -> Event {
    events.last().unwrap().event.clone()
}

/// The hidden answer [`SECRET`] for the command of `call`.
fn password(conversation_id: ConversationId, call_id: CallId) -> Method {
    Method::InputRespond(InputRespond {
        conversation_id,
        call_id,
        text: SecretText::new(SECRET),
        hidden: true,
        manual: false,
    })
}

/// The code of a refused request.
fn code<T: std::fmt::Debug>(answer: Result<T, ClientError>) -> ErrorCode {
    match answer {
        Err(ClientError::Server { body }) => body.code,
        other => panic!("{other:?}"),
    }
}

/// The launch of the one tool call of the conversation, from its record.
async fn launch(daemon: &TestDaemon, client: &Client, conversation: ConversationId) -> Launch {
    let events = daemon.events(client, conversation).await.unwrap();
    let launches: Vec<Option<Launch>> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallStarted { launch, .. } => Some(launch.clone()),
            _ => None,
        })
        .collect();
    match launches.as_slice() {
        [Some(launch)] => launch.clone(),
        other => panic!("one call with a launch: {other:?}"),
    }
}

#[tokio::test]
async fn shell_exit_child_password_wait_is_kept_and_answered() {
    let test = "shell_exit_child_password_wait_is_kept_and_answered";
    let place = Place::new();
    let line = place.fake_scp(GETPASS);
    let Some(daemon) = place.daemon(test, call(line, TIMEOUT)).await else { return };
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream, questions) = start(&daemon, &client).await;

    // The user approved an upload that may ask for a password, outside the sandbox: the
    // question said so (DS1).
    let [question] = questions.try_into().unwrap_or_else(|q: Vec<_>| panic!("{q:#?}"));
    let Event::ApprovalRequested { interactive, .. } = question.event else { unreachable!() };
    assert!(interactive, "the question says that you may need to type your password");

    let seen = until(&daemon, &mut stream, |event| {
        matches!(event, Event::ToolCallInputChanged { input: InputWait::Hidden, .. })
            || matches!(event, Event::ToolCallCompleted { .. })
    })
    .await;
    let Event::ToolCallInputChanged { call_id, .. } = last(&seen) else { panic!("{seen:#?}") };
    let launched = launch(&daemon, &client, conversation_id).await;
    assert_eq!(launched, Launch::Unsandboxed, "the exit child runs it");

    // The call's timeout passes three times; a person who can answer follows, so the
    // wait goes on (DS3, DS4).
    let waited = events_while_time_passes(&daemon, &mut stream, 3 * TIMEOUT as u32, None).await;
    assert!(
        !waited.iter().any(|envelope| matches!(envelope.event, Event::ToolCallCompleted { .. })),
        "{waited:#?}"
    );
    let busy: ShellStatusResult =
        client.call(Method::ShellStatus(ShellStatus { conversation_id })).await.unwrap();
    assert_eq!(busy.busy, None, "the call still runs; nothing is left running");

    let _: InputRespondResult = client.call(password(conversation_id, call_id)).await.unwrap();
    let seen = until(&daemon, &mut stream, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await;

    let completed = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, exit_code, left_running, .. } => {
            Some((output.clone(), *exit_code, *left_running))
        }
        _ => None,
    });
    let (output, exit_code, left_running) = completed.unwrap_or_else(|| panic!("{seen:#?}"));
    assert_eq!((exit_code, left_running), (Some(0), false), "{output}");
    assert!(output.contains(&format!("len={}", SECRET.len())), "{output}");
    assert!(
        !seen.iter().any(|envelope| matches!(envelope.event, Event::ShellCommandDetached { .. }))
    );
    drop((stream, client));
    daemon.stop().await.unwrap();
    assert_eq!(place.holding_the_secret(), Vec::<String>::new());
}

#[tokio::test]
async fn shell_contained_orphan_hidden_wait_is_stopped() {
    let test = "shell_contained_orphan_hidden_wait_is_stopped";
    let place = Place::new();
    let line = Place::gated(GETPASS);
    let Some(daemon) = place.daemon(test, call(&line, TIMEOUT)).await else { return };
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream, questions) = start(&daemon, &client).await;
    assert_eq!(questions.len(), 0, "a contained call asks nothing");

    // The call answers at its timeout, and its command goes on in the sandbox.
    let seen = until(&daemon, &mut stream, |event| {
        matches!(event, Event::ShellCommandDetached { .. } | Event::TurnFailed { .. })
    })
    .await;
    let Event::ShellCommandDetached { call_id, contained, .. } = last(&seen) else {
        panic!("{seen:#?}")
    };
    assert!(contained);
    let launched = launch(&daemon, &client, conversation_id).await;
    assert!(matches!(launched, Launch::Contained { .. }), "{launched:?}");
    place.release().await;

    // A client that can answer follows, and still efr types no password into the
    // sandbox: the answer is refused, and the wait stops at the next look (DS50).
    let refused = client.call::<InputRespondResult>(password(conversation_id, call_id)).await;
    assert_eq!(code(refused), ErrorCode::Conflict);
    let seen =
        until(&daemon, &mut stream, |event| matches!(event, Event::ShellCommandEnded { .. })).await;
    let Event::ShellCommandEnded { call_id: ended, how, .. } = last(&seen) else { unreachable!() };
    assert_eq!((ended, how), (call_id, ShellCommandEnd::Limit));
    let busy: ShellStatusResult =
        client.call(Method::ShellStatus(ShellStatus { conversation_id })).await.unwrap();
    assert_eq!(busy.busy, None, "the shell is free");

    drop((stream, client));
    daemon.stop().await.unwrap();
    assert_eq!(place.holding_the_secret(), Vec::<String>::new());
}

#[tokio::test]
async fn shell_contained_raw_mode_password_prompt_gets_no_answer() {
    let test = "shell_contained_raw_mode_password_prompt_gets_no_answer";
    let place = Place::new();
    let line = format!("echo {READY}; {RAW_PASSWORD}");
    let Some(daemon) = place.daemon(test, call(&line, 600)).await else { return };
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream, questions) = start(&daemon, &client).await;
    assert_eq!(questions.len(), 0, "a contained call asks nothing");

    // Sandboxed code put the terminal in raw mode and printed a password prompt: the
    // wait looks secret, and efr stops the command at the look that sees it (DS50).
    let seen = until(&daemon, &mut stream, |event| {
        matches!(event, Event::ToolCallInputChanged { input: InputWait::Visible, .. })
            || matches!(event, Event::ToolCallCompleted { .. })
    })
    .await;
    let Event::ToolCallInputChanged { call_id, looks_secret, .. } = last(&seen) else {
        panic!("{seen:#?}")
    };
    assert!(looks_secret, "{seen:#?}");
    // A visible answer from a client that can answer never reaches it: refused while it
    // waits, or not found once efr stopped it.
    let typed = Method::InputRespond(InputRespond {
        conversation_id,
        call_id,
        text: SecretText::new(SECRET),
        hidden: false,
        manual: false,
    });
    let refused = code(client.call::<InputRespondResult>(typed).await);
    assert!(matches!(refused, ErrorCode::Conflict | ErrorCode::NotFound), "{refused:?}");
    let seen = until(&daemon, &mut stream, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await;

    let output = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, left_running, .. } => {
            Some((output.clone(), *left_running))
        }
        _ => None,
    });
    let (output, left_running) = output.unwrap_or_else(|| panic!("{seen:#?}"));
    assert!(output.contains("asked for a secret inside the sandbox"), "{output}");
    assert!(!output.contains("got="), "the program read no answer: {output}");
    assert!(!left_running);
    drop((stream, client));
    daemon.stop().await.unwrap();
    assert_eq!(place.holding_the_secret(), Vec::<String>::new());
}
