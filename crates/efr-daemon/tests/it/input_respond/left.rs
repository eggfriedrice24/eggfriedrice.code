//! A command that its call left running in the hidden shell at the timeout, over a
//! `TestDaemon` with a real zsh: the call and the conversation record it, a client that
//! can answer gets its waits with the row that asks and answers them, a wait for a
//! password that nobody may answer stops, the user stops it or closes the shell, the
//! next call fails at once while it waits, and the next turn's preamble names it.
//!
//! Each command first prints `efr-ready` and then reads a FIFO that the test writes
//! after the call answered, so it waits for input only once it is left running, and no
//! test waits on a fixed time. The `auto` tests use the fake launcher of
//! `efr-test-daemon`, which runs the line with `/bin/sh` in the hidden zsh's terminal.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, ApprovedBy, CallId, ConversationId,
    ConversationSubscribe, ConversationSubscribeItem, ErrorCode, Event, EventEnvelope,
    InputRespond, InputRespondResult, InputWait, Method, Mode, Origin, PromptSendResult,
    SecretText, Seq, ShellCommandEnd, ShellInterrupt, ShellInterruptResult, ShellStatus,
    ShellStatusResult, TurnSettings,
};
use efr_stdx::time::Clock as _;
use efr_test_daemon::{
    Client, ClientError, FakeLauncher, ItemStream, ResponsesAnswer, ResponsesServer, TTY,
    TestDaemon, TestDaemonBuilder, command_id, events_until,
};
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{GETPASS, SECRET, events_while_time_passes};
use crate::sandbox::{REVIEWER_ALLOWS, ScriptedModel, ready};
use crate::support::{files_holding, zsh_enabled};

/// The marker that each command prints before it waits for the test.
const READY: &str = "efr-ready";

/// The timeout of each call, in seconds: short, so the clock passes it in a few steps.
const TIMEOUT: u64 = 2;

/// A daemon's tree, with a FIFO below it that a command reads and the test writes.
struct Place {
    dirs: Arc<TestDirs>,
    fifo: PathBuf,
}

impl Place {
    fn new() -> Place {
        let dirs = Arc::new(TestDirs::new().unwrap());
        let fifo = dirs.root().join("go");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        Place { dirs, fifo }
    }

    /// A line that prints [`READY`], waits until the test writes the FIFO, then runs
    /// `then`.
    fn line(&self, then: &str) -> String {
        format!("echo {READY}; cat '{}' > /dev/null; {then}", self.fifo.display())
    }

    /// The `shell` input of `line` with the short timeout.
    fn call(line: &str) -> serde_json::Value {
        json!({ "command": line, "timeout_seconds": TIMEOUT })
    }

    /// A daemon over a real zsh in this tree.
    fn builder(&self) -> TestDaemonBuilder {
        TestDaemon::builder().local_pty().dirs(Arc::clone(&self.dirs)).persistent()
    }

    /// A daemon over a real zsh whose model gives `answers` in order, in the mode of
    /// the config.
    async fn daemon(&self, server: &ResponsesServer, answers: Vec<ResponsesAnswer>) -> TestDaemon {
        for answer in answers {
            server.push(answer);
        }
        self.builder().responses(server).start().await.unwrap()
    }

    /// A daemon in `auto` over a real zsh and the fake launcher, whose model calls
    /// `shell` with `input` once; the reviewer allows each exit.
    async fn auto_daemon(&self, input: serde_json::Value) -> TestDaemon {
        let fake = FakeLauncher::write(&self.dirs.root().join("fake")).unwrap();
        let model = ScriptedModel::reviewed(vec![("shell", input)], REVIEWER_ALLOWS);
        let bin = self.dirs.root().join("bin");
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let env = BTreeMap::from([
            ("HOME".to_owned(), self.dirs.home().to_string_lossy().into_owned()),
            ("PATH".to_owned(), path),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ]);
        let daemon = self
            .builder()
            .shell_env(env)
            .sandbox_launcher(fake.path())
            .probe_override(ready())
            .custom_provider(model)
            .start()
            .await
            .unwrap();
        std::fs::write(daemon.cwd().join("notes.txt"), "notes\n").unwrap();
        daemon
    }

    /// A fake `scp` first on the hidden shell's `PATH` of [`auto_daemon`]: it runs
    /// [`line`](Self::line) of `then` and copies nothing. An upload that the reviewer
    /// allows runs alone, so the line is only `scp notes.txt h:notes.txt`.
    fn fake_scp(&self, then: &str) -> &'static str {
        let bin = self.dirs.create_dir("bin").unwrap();
        let script = format!("#!/bin/sh\n{}\n", self.line(then));
        let scp = bin.join("scp");
        std::fs::write(&scp, script).unwrap();
        std::fs::set_permissions(&scp, std::fs::Permissions::from_mode(0o755)).unwrap();
        "scp notes.txt h:notes.txt"
    }

    /// Lets the command go on past the FIFO; it waits for the reader.
    async fn release(&self) {
        let fifo = self.fifo.clone();
        tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
    }

    /// The notice file of the test's terminal, as text.
    fn notices(&self) -> String {
        let path = self.dirs.dirs().runtime().join("notices").join("pts-efr-test");
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// Waits until a notice line of the test's terminal holds `text`.
    async fn notice(&self, text: &str) -> String {
        // NOTE: efrd writes each line with one write, after the commit of its event.
        Wait::new("the notice")
            .until_some(|| {
                self.notices().lines().find(|line| line.contains(text)).map(str::to_owned)
            })
            .await
            .unwrap_or_else(|_| panic!("no notice holds {text:?}: {:?}", self.notices()))
    }
}

/// Subscribes `client` to the conversation from the start.
async fn follow(
    client: &Client,
    conversation_id: ConversationId,
    answers_input: bool,
) -> ItemStream<ConversationSubscribeItem> {
    let params = ConversationSubscribe {
        conversation_id,
        after_seq: Some(Seq::ZERO),
        answers_input,
        drafts: false,
    };
    client.stream(Method::ConversationSubscribe(params)).await.unwrap()
}

/// Sends the prompt with the command id `n` from `sender` in `mode` (the config's when
/// `None`), follows it on `watcher` and answers each question with yes, until the
/// call's output shows [`READY`]. The clock stands still meanwhile: the hidden zsh
/// starts in real time.
async fn start(
    daemon: &TestDaemon,
    sender: &Client,
    watcher: &Client,
    mode: Option<Mode>,
    answers_input: bool,
) -> (ConversationId, ItemStream<ConversationSubscribeItem>) {
    // NOTE: the prompt names the host and the verb of the upload in the `auto` tests,
    // so the reviewer, not the user, judges it.
    let Method::PromptSend(mut params) = daemon.prompt(1, "upload notes.txt to h", TTY) else {
        unreachable!()
    };
    params.settings = TurnSettings { mode, ..TurnSettings::default() };
    let sent: PromptSendResult = sender.call(Method::PromptSend(params)).await.unwrap();
    let mut stream = follow(watcher, sent.conversation_id, answers_input).await;
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
                answers += 1;
                let approve = Method::ApprovalRespond(ApprovalRespond {
                    command_id: command_id(answers),
                    conversation_id: sent.conversation_id,
                    call_id,
                    decision: ApprovalDecision::Allow,
                });
                let _: ApprovalRespondResult = watcher.call(approve).await.unwrap();
            }
            Some(Event::ToolCallOutputUpdated { .. }) => return (sent.conversation_id, stream),
            other => panic!("the call did not start: {other:?} {seen:#?}"),
        }
    }
}

/// Lets time pass until the call answers, past its timeout; returns its call, the
/// `tool_call_completed` and the events up to the turn's end.
async fn until_left(
    daemon: &TestDaemon,
    stream: &mut ItemStream<ConversationSubscribeItem>,
) -> (CallId, Event, Vec<EventEnvelope>) {
    let mut seen = events_while_time_passes(daemon, stream, 10, |event| {
        matches!(event, Event::ToolCallCompleted { .. } | Event::TurnFailed { .. })
    })
    .await;
    let completed = seen.last().unwrap().event.clone();
    let Event::ToolCallCompleted { call_id, .. } = completed else { panic!("{seen:#?}") };
    seen.extend(
        events_until(stream, |event| {
            matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
        })
        .await
        .unwrap(),
    );
    (call_id, completed, seen)
}

/// Lets time pass until the command left running reports a wait; returns the event.
async fn until_waits(
    daemon: &TestDaemon,
    stream: &mut ItemStream<ConversationSubscribeItem>,
) -> Event {
    let seen = events_while_time_passes(daemon, stream, 10, |event| {
        matches!(event, Event::ShellCommandInputChanged { input, .. } if *input != InputWait::None)
            || matches!(event, Event::ShellCommandEnded { .. })
    })
    .await;
    seen.last().unwrap().event.clone()
}

/// Lets time pass until the command left running ends; returns how, with its exit code.
async fn until_ended(
    daemon: &TestDaemon,
    stream: &mut ItemStream<ConversationSubscribeItem>,
) -> (ShellCommandEnd, Option<i32>) {
    let seen = events_while_time_passes(daemon, stream, 10, |event| {
        matches!(event, Event::ShellCommandEnded { .. })
    })
    .await;
    match seen.last().map(|envelope| &envelope.event) {
        Some(Event::ShellCommandEnded { how, exit_code, .. }) => (*how, *exit_code),
        other => panic!("{other:?}"),
    }
}

/// What `shell.status` says of the conversation.
async fn status(client: &Client, conversation_id: ConversationId) -> ShellStatusResult {
    client.call(Method::ShellStatus(ShellStatus { conversation_id })).await.unwrap()
}

/// `shell.interrupt` of `call` with the command id `n`.
fn interrupt(n: u128, conversation_id: ConversationId, call_id: CallId, close: bool) -> Method {
    Method::ShellInterrupt(ShellInterrupt {
        conversation_id,
        call_id,
        command_id: command_id(n),
        close,
    })
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

#[tokio::test]
async fn shell_timed_out_call_records_detached_and_left_running() {
    if !zsh_enabled("shell_timed_out_call_records_detached_and_left_running") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let line = place.line("echo efr-after");
    let answers = vec![
        ResponsesAnswer::tool_call("call_1", "shell", &Place::call(&line)),
        ResponsesAnswer::text("done"),
    ];
    let daemon = place.daemon(&server, answers).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream) = start(&daemon, &client, &client, None, false).await;

    let (call_id, completed, seen) = until_left(&daemon, &mut stream).await;

    let Event::ToolCallCompleted { left_running, is_error, .. } = completed else { unreachable!() };
    assert!(left_running, "the call answered while its command goes on");
    assert!(!is_error);
    let detached = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ShellCommandDetached {
            call_id: id, command, contained, input, approved_by, ..
        } => Some((*id, command.clone(), *contained, *input, *approved_by)),
        _ => None,
    });
    // The line needed no question in this mode, so nobody approved it.
    let asked = daemon.events(&client, conversation_id).await.unwrap().iter().any(|envelope| {
        matches!(&envelope.event, Event::ApprovalRequested { call_id: id, .. } if *id == call_id)
    });
    let approved_by = asked.then_some(ApprovedBy::You);
    assert_eq!(detached, Some((call_id, line.clone(), false, InputWait::None, approved_by)));
    let busy = status(&client, conversation_id).await.busy.unwrap();
    assert_eq!(
        (busy.call_id, busy.command.as_str(), busy.input),
        (call_id, line.as_str(), InputWait::None)
    );
    // The model read that the command still runs.
    let requests = server.received();
    assert!(requests[1].body.to_string().contains("still running"), "{}", requests[1].body);

    place.release().await;
    assert_eq!(until_ended(&daemon, &mut stream).await, (ShellCommandEnd::Ended, Some(0)));
    assert_eq!(status(&client, conversation_id).await.busy, None, "the shell is free");
    drop((stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_orphan_wait_reaches_a_follower_with_its_prompt_and_its_answer_types() {
    if !zsh_enabled("shell_orphan_wait_reaches_a_follower_with_its_prompt_and_its_answer_types") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let answers = vec![
        ResponsesAnswer::tool_call("call_1", "shell", &Place::call(&place.line(GETPASS))),
        ResponsesAnswer::text("done"),
    ];
    let daemon = place.daemon(&server, answers).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream) = start(&daemon, &client, &client, None, true).await;
    let (call_id, _, _) = until_left(&daemon, &mut stream).await;

    place.release().await;
    let wait = until_waits(&daemon, &mut stream).await;

    let Event::ShellCommandInputChanged { call_id: waiting, input, contained, prompt, .. } = wait
    else {
        panic!("{wait:?}")
    };
    assert_eq!((waiting, input, contained), (call_id, InputWait::Hidden, false));
    assert!(prompt.starts_with("pw:"), "{prompt:?}");
    let busy = status(&client, conversation_id).await.busy.unwrap();
    assert_eq!(busy.input, InputWait::Hidden);
    assert!(busy.prompt.starts_with("pw:"), "{busy:?}");
    let _: InputRespondResult = client.call(password(conversation_id, call_id)).await.unwrap();
    assert_eq!(until_ended(&daemon, &mut stream).await, (ShellCommandEnd::Ended, Some(0)));

    drop((stream, client));
    let dirs = Arc::clone(daemon.dirs());
    daemon.stop().await.unwrap();
    // NOTE: a read of the FIFO would wait for a writer forever.
    std::fs::remove_file(&place.fifo).unwrap();
    // The program printed the length into the recording, and no file holds the
    // password.
    let length = format!("len={}", SECRET.len());
    assert!(!files_holding(dirs.root(), length.as_bytes()).is_empty(), "the program read it");
    assert_eq!(files_holding(dirs.root(), SECRET.as_bytes()), Vec::<String>::new());
}

/// Starts a turn from the test's terminal whose prompt client then leaves, so the
/// terminal's notices are written, and follows it on a client of no terminal that
/// cannot answer. The call is `line` with the hard flag (`less`, which never runs).
async fn unfollowed(
    place: &Place,
    server: &ResponsesServer,
    line: &str,
) -> (TestDaemon, Client, ConversationId, ItemStream<ConversationSubscribeItem>) {
    let flagged = format!("true || less; {line}");
    let answers = vec![
        ResponsesAnswer::tool_call("call_1", "shell", &Place::call(&flagged)),
        ResponsesAnswer::text("done"),
    ];
    let daemon = place.daemon(server, answers).await;
    let sender = daemon.client_for_tty(TTY).await.unwrap();
    let watcher = daemon.client().await.unwrap();
    let (conversation_id, stream) = start(&daemon, &sender, &watcher, None, false).await;
    drop(sender);
    (daemon, watcher, conversation_id, stream)
}

#[tokio::test]
async fn shell_orphan_wait_makes_a_notice_without_follower() {
    if !zsh_enabled("shell_orphan_wait_makes_a_notice_without_follower") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let (daemon, watcher, conversation_id, mut stream) =
        unfollowed(&place, &server, &place.line(GETPASS)).await;
    let (call_id, _, _) = until_left(&daemon, &mut stream).await;
    place.notice("the hidden shell still runs true || less; echo efr-ready").await;

    place.release().await;
    let wait = until_waits(&daemon, &mut stream).await;

    // The user approved a line that may ask at the terminal, so the wait stays while
    // nobody follows, and the next prompt says where to answer it.
    assert!(
        matches!(wait, Event::ShellCommandInputChanged { input: InputWait::Hidden, .. }),
        "{wait:?}"
    );
    let notice = place
        .notice("and waits for a password (asked by \"pw:\"; you approved it). Type the password only after ,shell; never at this prompt.")
        .await;
    assert!(notice.starts_with("efr: "), "{notice}");
    let _: ShellInterruptResult =
        watcher.call(interrupt(50, conversation_id, call_id, false)).await.unwrap();
    assert_eq!(until_ended(&daemon, &mut stream).await.0, ShellCommandEnd::Stopped);
    drop((stream, watcher));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_limit_records_ended_limit() {
    if !zsh_enabled("shell_limit_records_ended_limit") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let line = format!("true || less; {}", place.line(GETPASS));
    let answers = vec![
        ResponsesAnswer::tool_call("call_1", "shell", &Place::call(&line)),
        ResponsesAnswer::text("done"),
    ];
    for answer in answers {
        server.push(answer);
    }
    let daemon = place
        .builder()
        .responses(&server)
        .config(|config| config.shell.interactive_timeout_minutes = 1)
        .start()
        .await
        .unwrap();
    let sender = daemon.client_for_tty(TTY).await.unwrap();
    let watcher = daemon.client().await.unwrap();
    let (_, mut stream) = start(&daemon, &sender, &watcher, None, false).await;
    drop(sender);
    let _ = until_left(&daemon, &mut stream).await;
    place.release().await;
    let started = daemon.clock().now();

    // The kept wait ends a minute after the command's start.
    let seen = events_while_time_passes(&daemon, &mut stream, 90, |event| {
        matches!(event, Event::ShellCommandEnded { .. })
    })
    .await;

    let Some(Event::ShellCommandEnded { how, .. }) = seen.last().map(|envelope| &envelope.event)
    else {
        panic!("{seen:#?}")
    };
    assert_eq!(*how, ShellCommandEnd::Limit);
    let waited = daemon.clock().now().duration_since(started);
    assert!(waited.as_secs() >= 50, "it ended at the limit, not at once: {waited:?}");
    place.notice("in the hidden shell: it waited for input for 1 minute").await;
    drop((stream, watcher));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_reviewer_approved_orphan_wait_is_stopped_with_a_notice() {
    if !zsh_enabled("shell_reviewer_approved_orphan_wait_is_stopped_with_a_notice") {
        return;
    }
    let place = Place::new();
    // An upload that may ask for a password, of a fake scp; the reviewer allows it.
    let line = place.fake_scp(GETPASS);
    let daemon = place.auto_daemon(Place::call(line)).await;
    let sender = daemon.client_for_tty(TTY).await.unwrap();
    let watcher = daemon.client().await.unwrap();
    let (conversation_id, mut stream) =
        start(&daemon, &sender, &watcher, Some(Mode::Auto), false).await;
    drop(sender);
    let (_, _, seen) = until_left(&daemon, &mut stream).await;
    let approved = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ShellCommandDetached { approved_by, contained, .. } => {
            Some((*approved_by, *contained))
        }
        _ => None,
    });
    let all = daemon.events(&watcher, conversation_id).await.unwrap();
    let kinds: Vec<&str> = all.iter().map(|envelope| envelope.event.kind()).collect();
    assert_eq!(approved, Some((Some(ApprovedBy::Reviewer), false)), "{kinds:?}");

    place.release().await;
    let (how, _) = until_ended(&daemon, &mut stream).await;

    // Nobody follows, and no person approved the line: the password wait stops at once.
    assert_eq!(how, ShellCommandEnd::Limit);
    place.notice("in the hidden shell: it asked for a password while nobody followed").await;
    assert!(!place.notices().contains("Type the password"), "{}", place.notices());
    drop((stream, watcher));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_contained_orphan_secret_answer_is_refused() {
    if !zsh_enabled("shell_contained_orphan_secret_answer_is_refused") {
        return;
    }
    let place = Place::new();
    let daemon = place.auto_daemon(Place::call(&place.line(GETPASS))).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream) =
        start(&daemon, &client, &client, Some(Mode::Auto), true).await;
    let (call_id, _, seen) = until_left(&daemon, &mut stream).await;
    let contained = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ShellCommandDetached { contained, .. } => Some(*contained),
        _ => None,
    });
    assert_eq!(contained, Some(true));
    place.release().await;

    // A client that can answer follows, and still efr types no password into the
    // sandbox: the answer is refused, and the wait stops at the next look.
    let refused = client.call::<InputRespondResult>(password(conversation_id, call_id)).await;
    assert_eq!(code(refused), ErrorCode::Conflict);
    assert_eq!(until_ended(&daemon, &mut stream).await.0, ShellCommandEnd::Limit);

    drop((stream, client));
    let dirs = Arc::clone(daemon.dirs());
    daemon.stop().await.unwrap();
    // NOTE: a read of the FIFO would wait for a writer forever.
    std::fs::remove_file(&place.fifo).unwrap();
    assert_eq!(files_holding(dirs.root(), SECRET.as_bytes()), Vec::<String>::new());
}

/// A daemon whose model leaves one command running that waits on the FIFO, followed by
/// the test's terminal; returns the daemon, the client, the conversation, the call and
/// the stream.
async fn left_running(
    place: &Place,
    server: &ResponsesServer,
    then: &str,
    later: Vec<ResponsesAnswer>,
) -> (TestDaemon, Client, ConversationId, CallId, ItemStream<ConversationSubscribeItem>) {
    let mut answers = vec![
        ResponsesAnswer::tool_call("call_1", "shell", &Place::call(&place.line(then))),
        ResponsesAnswer::text("done"),
    ];
    answers.extend(later);
    let daemon = place.daemon(server, answers).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream) = start(&daemon, &client, &client, None, true).await;
    let (call_id, _, _) = until_left(&daemon, &mut stream).await;
    (daemon, client, conversation_id, call_id, stream)
}

#[tokio::test]
async fn shell_interrupt_stops_the_orphan() {
    if !zsh_enabled("shell_interrupt_stops_the_orphan") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let (daemon, client, conversation_id, call_id, mut stream) =
        left_running(&place, &server, "echo efr-after", Vec::new()).await;

    let _: ShellInterruptResult =
        client.call(interrupt(50, conversation_id, call_id, false)).await.unwrap();

    assert_eq!(until_ended(&daemon, &mut stream).await.0, ShellCommandEnd::Stopped);
    assert_eq!(status(&client, conversation_id).await.busy, None);
    // The command no longer runs: a new stop finds nothing, and a retry of the first
    // gets the same answer.
    let again =
        client.call::<ShellInterruptResult>(interrupt(51, conversation_id, call_id, false)).await;
    assert_eq!(code(again), ErrorCode::NotFound);
    let retried =
        client.call::<ShellInterruptResult>(interrupt(51, conversation_id, call_id, false)).await;
    assert_eq!(code(retried), ErrorCode::NotFound);
    drop((stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_interrupt_close_ends_the_shell() {
    if !zsh_enabled("shell_interrupt_close_ends_the_shell") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let (daemon, client, conversation_id, call_id, mut stream) =
        left_running(&place, &server, "echo efr-after", Vec::new()).await;

    let _: ShellInterruptResult =
        client.call(interrupt(50, conversation_id, call_id, true)).await.unwrap();

    let seen = events_while_time_passes(&daemon, &mut stream, 10, |event| {
        matches!(event, Event::ShellExited { .. })
    })
    .await;
    let ended = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ShellCommandEnded { how, .. } => Some(*how),
        _ => None,
    });
    assert_eq!(ended, Some(ShellCommandEnd::Closed), "{seen:#?}");
    assert_eq!(status(&client, conversation_id).await.busy, None);
    drop((stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_next_call_fails_at_once_with_held_by_text() {
    if !zsh_enabled("shell_next_call_fails_at_once_with_held_by_text") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let later = vec![
        ResponsesAnswer::tool_call("call_2", "shell", &json!({ "command": "echo hi" })),
        ResponsesAnswer::text("done"),
    ];
    let (daemon, client, conversation_id, call_id, mut stream) =
        left_running(&place, &server, GETPASS, later).await;
    place.release().await;
    let wait = until_waits(&daemon, &mut stream).await;
    assert!(matches!(wait, Event::ShellCommandInputChanged { .. }), "{wait:?}");
    let before = daemon.clock().now();

    let Method::PromptSend(mut params) = daemon.prompt(3, "and now", TTY) else { unreachable!() };
    params.conversation_id = Some(conversation_id);
    let _: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let seen = events_until(&mut stream, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();

    let output = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, is_error, .. } => Some((output.clone(), *is_error)),
        _ => None,
    });
    let (output, is_error) = output.unwrap_or_else(|| panic!("{seen:#?}"));
    assert!(is_error);
    assert!(output.starts_with("The hidden shell is busy: `echo efr-ready"), "{output}");
    assert!(output.contains("waits for a password"), "{output}");
    assert!(output.contains(",shell"), "{output}");
    assert!(output.contains("The command was not run"), "{output}");
    assert_eq!(daemon.clock().now(), before, "the call failed at once, with no wait");
    let _: ShellInterruptResult =
        client.call(interrupt(50, conversation_id, call_id, true)).await.unwrap();
    drop((stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_preamble_names_the_busy_shell() {
    if !zsh_enabled("shell_preamble_names_the_busy_shell") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let later = vec![ResponsesAnswer::text("still waiting")];
    let (daemon, client, conversation_id, call_id, mut stream) =
        left_running(&place, &server, "echo efr-after", later).await;

    let Method::PromptSend(mut params) = daemon.prompt(3, "is it done?", TTY) else {
        unreachable!()
    };
    params.conversation_id = Some(conversation_id);
    let _: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    events_until(&mut stream, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();

    let requests = server.received();
    let third = requests[2].body.to_string();
    assert!(third.contains("The hidden shell still runs: `echo efr-ready"), "{third}");
    let _: ShellInterruptResult =
        client.call(interrupt(50, conversation_id, call_id, true)).await.unwrap();
    drop((stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_reviewed_exit_with_may_prompt_gets_the_limit() {
    if !zsh_enabled("shell_reviewed_exit_with_may_prompt_gets_the_limit") {
        return;
    }
    let place = Place::new();
    // The reviewer allows an upload that may ask for a password (a fake scp): while a
    // client that can answer follows, the call runs past its timeout, also with no
    // wait.
    let line = place.fake_scp("echo efr-after");
    let daemon = place.auto_daemon(Place::call(line)).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream) =
        start(&daemon, &client, &client, Some(Mode::Auto), true).await;
    let all = daemon.events(&client, conversation_id).await.unwrap();
    let kinds: Vec<&str> = all.iter().map(|envelope| envelope.event.kind()).collect();
    assert!(kinds.contains(&"exit_review_started"), "{kinds:?}");
    assert!(!kinds.contains(&"approval_requested"), "the reviewer allowed it: {kinds:?}");

    let seen = super::events_for_seconds(&daemon, &mut stream, 3 * TIMEOUT as u32).await;
    assert!(
        !seen.iter().any(|envelope| matches!(envelope.event, Event::ToolCallCompleted { .. })),
        "{seen:#?}"
    );
    place.release().await;
    let seen = events_until(&mut stream, |event| matches!(event, Event::ToolCallCompleted { .. }))
        .await
        .unwrap();

    let Some(Event::ToolCallCompleted { exit_code, left_running, output, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!(exit_code, Some(0), "{output}");
    assert!(!left_running);
    assert!(output.contains("efr-after"), "{output}");
    drop((stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_interrupt_from_phone_is_forbidden() {
    // The scope check refuses it before any shell is asked, so no zsh runs here.
    let daemon = TestDaemon::start().await.unwrap();
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();
    let conversation_id = ConversationId::from_uuid(uuid::Uuid::from_u128(1));
    let call_id = CallId::from_uuid(uuid::Uuid::from_u128(2));

    let stop =
        phone.call::<ShellInterruptResult>(interrupt(1, conversation_id, call_id, false)).await;
    let close =
        phone.call::<ShellInterruptResult>(interrupt(2, conversation_id, call_id, true)).await;

    assert_eq!(code(stop), ErrorCode::Forbidden);
    assert_eq!(code(close), ErrorCode::Forbidden);
    // The phone may read the status; this conversation does not exist.
    let read =
        phone.call::<ShellStatusResult>(Method::ShellStatus(ShellStatus { conversation_id })).await;
    assert_eq!(code(read), ErrorCode::NotFound);
    drop(phone);
    daemon.stop().await.unwrap();
}
