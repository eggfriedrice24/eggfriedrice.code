//! Answering a command that waits for input, over a `TestDaemon` with a real zsh and
//! the local Responses server as the model: a password typed through `input.respond`
//! reaches the program and nothing else, a password prompt that no client can answer is
//! stopped at once, also when the last client that could answer leaves while it waits,
//! and one of two such clients leaving stops nothing. A call approved as one that waits
//! for input runs past its timeout while a client that can answer follows, and a manual
//! answer reaches a silent command that reported no wait. Every test drives a real zsh
//! and skips with a message unless `EFR_TEST_ZSH=1`.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, CallId, ConversationId,
    ConversationSubscribe, ConversationSubscribeItem, Event, EventEnvelope, InputRespond,
    InputRespondResult, InputWait, Method, PromptSendResult, SecretText, Seq,
};
use efr_stdx::time::Clock as _;
use efr_test_daemon::{
    Client, ItemStream, ResponsesAnswer, ResponsesServer, TTY, TestDaemon, command_id, events_until,
};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;

use crate::support::zsh_enabled;

/// A program that reads a password as getpass does: echo off, one line, echo on. It
/// prints how long the line was, never the line.
const GETPASS: &str =
    r#"sh -c 'stty -echo; printf "pw: "; IFS= read -r p; stty echo; printf "\nlen=%s\n" "${#p}"'"#;

/// A program that prints a line and then reads one in line mode with echo on, without a
/// prompt, so no wait is reported; it prints what it read.
const SILENT_READ: &str = r#"sh -c 'printf "ready\n"; IFS= read -r a; printf "got=%s\n" "$a"'"#;

/// The password the user types: nothing may hold it but the program that reads it.
const SECRET: &str = "hunter2-efr-secret";

/// Everything every span and event of this process logs, at every level.
fn logs() -> Arc<Mutex<Vec<u8>>> {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    Arc::clone(LOGS.get_or_init(|| {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&logs);
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || LogWriter(Arc::clone(&sink)))
            .finish();
        // NOTE: nextest runs each test in a process of its own; under cargo test the
        // second test finds the subscriber already set, which is the same one.
        let _ = tracing::subscriber::set_global_default(subscriber);
        logs
    }))
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn logged() -> String {
    String::from_utf8_lossy(&logs().lock().unwrap()).into_owned()
}

/// A daemon over a real zsh whose model asks for one `shell` call of [`GETPASS`] and
/// then says `done`.
async fn daemon(server: &ResponsesServer) -> TestDaemon {
    let arguments = serde_json::json!({ "command": GETPASS, "timeout_seconds": 600 });
    daemon_calling(server, &arguments).await
}

/// A daemon over a real zsh whose model asks for one `shell` call with `arguments` and
/// then says `done`.
async fn daemon_calling(server: &ResponsesServer, arguments: &serde_json::Value) -> TestDaemon {
    server.push(ResponsesAnswer::tool_call("call_1", "shell", arguments));
    server.push(ResponsesAnswer::text("done"));
    TestDaemon::builder().local_pty().responses(server).persistent().start().await.unwrap()
}

/// Subscribes `client` to the conversation from the start, saying whether a person at
/// it can answer a waiting command.
async fn subscribe(
    client: &Client,
    conversation_id: ConversationId,
    answers_input: bool,
) -> ItemStream<ConversationSubscribeItem> {
    subscribe_after(client, conversation_id, Seq::ZERO, answers_input).await
}

/// Subscribes `client` to the events of the conversation after `after`.
async fn subscribe_after(
    client: &Client,
    conversation_id: ConversationId,
    after: Seq,
    answers_input: bool,
) -> ItemStream<ConversationSubscribeItem> {
    let params = ConversationSubscribe {
        conversation_id,
        after_seq: Some(after),
        answers_input,
        drafts: false,
    };
    client.stream(Method::ConversationSubscribe(params)).await.unwrap()
}

/// The events of one subscription item.
fn envelopes(item: ConversationSubscribeItem) -> Vec<EventEnvelope> {
    match item {
        ConversationSubscribeItem::Event(envelope) => vec![envelope],
        ConversationSubscribeItem::Snapshot(snapshot) => snapshot.events,
        other => panic!("{other:?}"),
    }
}

/// The events of `stream` up to the first that `stop` accepts. Whenever none comes for
/// a while and a sleep of the daemon ends within one second, the daemon's clock moves
/// one second, at most `max_seconds` times. This lets the run look for input.
///
/// The clock moves only while the daemon waits for it. After the run stops a command,
/// the daemon does work in real time before the call's end reaches the stream: it
/// interrupts the program, takes the snapshot after the call and writes the events.
/// No sleep of the daemon ends soon while it does that work, so the clock stays where
/// the run stopped the command, also on a machine under load.
///
/// The wait has a bound in real time too, [`REAL_TIME_LIMIT`]: a daemon that hangs with
/// no deadline of its own never moves the clock, and the test fails with the events it
/// saw, long before nextest ends it.
async fn events_while_time_passes(
    daemon: &TestDaemon,
    stream: &mut ItemStream<ConversationSubscribeItem>,
    max_seconds: u32,
    mut stop: impl FnMut(&Event) -> bool,
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
                    let done = stop(&envelope.event);
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
                    assert!(moved < max_seconds, "nothing came in {max_seconds}s: {seen:#?}");
                    clock.advance(step);
                    moved += 1;
                }
            }
        }
    }
}

/// How long [`events_while_time_passes`] may take in real time. A whole test of this
/// module takes less than 0.1 s on an idle machine, and each wait only a part of it:
/// the daemon interrupts a program, takes a snapshot and writes some events. The limit
/// is more than a hundred times that, for a machine under heavy load, and a quarter of
/// nextest's limit for the whole test (60 s), so the test fails with what it saw.
const REAL_TIME_LIMIT: Duration = Duration::from_secs(15);

/// The events of `stream` while the daemon's clock moves `seconds` seconds, one second
/// each time nothing comes for a while.
async fn events_for_seconds(
    daemon: &TestDaemon,
    stream: &mut ItemStream<ConversationSubscribeItem>,
    seconds: u32,
) -> Vec<EventEnvelope> {
    let mut seen = Vec::new();
    let mut moved = 0;
    loop {
        tokio::select! {
            biased;
            item = stream.next() => seen.extend(envelopes(item.unwrap().unwrap())),
            () = idle() => {
                if moved == seconds {
                    return seen;
                }
                daemon.clock().advance(Duration::from_secs(1));
                moved += 1;
            }
        }
    }
}

/// The input waits that `events` report, in order.
fn input_waits(events: &[EventEnvelope]) -> Vec<InputWait> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallInputChanged { input, .. } => Some(*input),
            _ => None,
        })
        .collect()
}

/// Lets time pass until the run reports that the program waits for hidden input;
/// returns the call and the sequence number of that event.
async fn until_hidden(
    daemon: &TestDaemon,
    stream: &mut ItemStream<ConversationSubscribeItem>,
) -> (CallId, Seq) {
    let seen = events_while_time_passes(daemon, stream, 10, |event| {
        matches!(event, Event::ToolCallInputChanged { .. } | Event::ToolCallCompleted { .. })
    })
    .await;
    match seen.last() {
        Some(EventEnvelope {
            seq,
            event: Event::ToolCallInputChanged { call_id, input: InputWait::Hidden, .. },
            ..
        }) => (*call_id, *seq),
        other => panic!("{other:?}"),
    }
}

/// Lets every other task run for a while, without real time.
async fn idle() {
    for _ in 0..2000 {
        tokio::task::yield_now().await;
    }
}

/// Sends the prompt, approves the shell call and waits, without moving the clock, until
/// the program shows its prompt; returns the conversation and the stream.
///
/// The clock stands still while the hidden zsh starts and the program runs up to its
/// prompt, which happen in real time: the shell's startup timer must not pass.
async fn prompt_until_the_password_prompt(
    daemon: &TestDaemon,
    client: &Client,
    answers_input: bool,
) -> (ConversationId, ItemStream<ConversationSubscribeItem>) {
    prompt_until_shown(daemon, client, answers_input, "pw: ").await
}

/// Sends the prompt, approves the shell call and waits, without moving the clock, until
/// the program's output shows `shown`; returns the conversation and the stream.
async fn prompt_until_shown(
    daemon: &TestDaemon,
    client: &Client,
    answers_input: bool,
    shown: &str,
) -> (ConversationId, ItemStream<ConversationSubscribeItem>) {
    let sent: PromptSendResult = client.call(daemon.prompt(1, "update", TTY)).await.unwrap();
    let mut stream = subscribe(client, sent.conversation_id, answers_input).await;
    let seen = events_until(&mut stream, |event| {
        matches!(event, Event::ApprovalRequested { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();
    let Some(Event::ApprovalRequested { call_id, .. }) = seen.last().map(|e| e.event.clone())
    else {
        panic!("{seen:#?}");
    };
    let approve = Method::ApprovalRespond(ApprovalRespond {
        command_id: command_id(2),
        conversation_id: sent.conversation_id,
        call_id,
        decision: ApprovalDecision::Allow,
    });
    let _: ApprovalRespondResult = client.call(approve).await.unwrap();
    let shown = events_until(&mut stream, |event| {
        matches!(event, Event::ToolCallOutputUpdated { tail, .. } if tail.contains(shown))
            || matches!(event, Event::ToolCallCompleted { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();
    assert!(
        matches!(shown.last().map(|e| &e.event), Some(Event::ToolCallOutputUpdated { .. })),
        "{shown:#?}"
    );
    (sent.conversation_id, stream)
}

/// Every file under `root` that holds `needle`.
fn files_holding(root: &Path, needle: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        // A directory or file that went away while the daemon stopped holds nothing.
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_symlink() {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
            } else if let Ok(bytes) = std::fs::read(&path)
                && bytes.windows(needle.len()).any(|window| window == needle)
            {
                found.push(path.display().to_string());
            }
        }
    }
    found
}

#[tokio::test]
async fn shell_a_password_typed_through_input_respond_reaches_only_the_program() {
    if !zsh_enabled("shell_a_password_typed_through_input_respond_reaches_only_the_program") {
        return;
    }
    let logs = logs();
    let server = ResponsesServer::start().await;
    let daemon = daemon(&server).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream) =
        prompt_until_the_password_prompt(&daemon, &client, true).await;

    let seen = events_while_time_passes(&daemon, &mut stream, 10, |event| {
        matches!(event, Event::ToolCallInputChanged { .. } | Event::ToolCallCompleted { .. })
    })
    .await;
    let (call_id, input): (CallId, InputWait) = match seen.last().map(|e| e.event.clone()) {
        Some(Event::ToolCallInputChanged { call_id, input, .. }) => (call_id, input),
        other => panic!("{other:?}"),
    };
    assert_eq!(input, InputWait::Hidden);

    let answer = Method::InputRespond(InputRespond {
        conversation_id,
        call_id,
        text: SecretText::new(SECRET),
        hidden: true,
        manual: false,
    });
    let _: InputRespondResult = client.call(answer).await.unwrap();
    let rest = events_until(&mut stream, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();
    assert!(
        matches!(rest.last().map(|e| &e.event), Some(Event::TurnCompleted { .. })),
        "{rest:#?}"
    );
    let completed = rest.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, exit_code, .. } => Some((output.clone(), *exit_code)),
        _ => None,
    });
    let (output, exit_code) = completed.unwrap();
    assert!(output.contains(&format!("len={}", SECRET.len())), "{output}");
    assert_eq!(exit_code, Some(0));
    assert!(rest.iter().any(|envelope| matches!(
        &envelope.event,
        Event::ToolCallInputChanged { input: InputWait::None, .. }
    )));

    // The model read the length, never the password.
    let requests = server.received();
    assert_eq!(requests.len(), 2);
    let second = requests[1].body.to_string();
    assert!(second.contains(&format!("len={}", SECRET.len())), "{second}");
    assert!(!second.contains(SECRET));
    let events = daemon.events(&client, conversation_id).await.unwrap();
    assert!(!serde_json::to_string(&events).unwrap().contains(SECRET));

    drop((stream, client));
    // Kept, so the tree outlives the daemon.
    let dirs = Arc::clone(daemon.dirs());
    daemon.stop().await.unwrap();
    // The database and the recording hold what the program printed...
    let length = format!("len={}", SECRET.len());
    assert!(!files_holding(dirs.root(), length.as_bytes()).is_empty(), "the scan sees the tree");
    // ...but no file of the daemon's tree holds the password.
    assert_eq!(files_holding(dirs.root(), SECRET.as_bytes()), Vec::<String>::new());
    let logged = logged();
    assert!(logged.contains("an answer was typed for a waiting command"), "the logs were captured");
    assert!(!logged.contains(SECRET), "a log holds the password");
    drop(logs);
}

#[tokio::test]
async fn shell_a_password_prompt_that_no_client_can_answer_is_stopped_at_once() {
    if !zsh_enabled("shell_a_password_prompt_that_no_client_can_answer_is_stopped_at_once") {
        return;
    }
    let server = ResponsesServer::start().await;
    let daemon = daemon(&server).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (_, mut stream) = prompt_until_the_password_prompt(&daemon, &client, false).await;
    let shown = daemon.clock().now();

    let mut seen = events_while_time_passes(&daemon, &mut stream, 10, |event| {
        matches!(event, Event::ToolCallCompleted { .. } | Event::TurnFailed { .. })
    })
    .await;
    // The call's timeout is ten minutes; the stop comes within seconds of the prompt.
    let waited = daemon.clock().now().duration_since(shown);
    assert!(waited.as_secs() <= 10, "{waited:?}");
    seen.extend(
        events_until(&mut stream, |event| {
            matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
        })
        .await
        .unwrap(),
    );
    let completed = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, is_error, exit_code, .. } => {
            Some((output.clone(), *is_error, *exit_code))
        }
        _ => None,
    });
    let (output, is_error, exit_code) = completed.unwrap();
    assert!(is_error);
    assert_eq!(exit_code, None);
    assert!(output.contains("asked for hidden input, such as a password"), "{output}");
    assert!(output.contains("no user could answer it at a terminal"), "{output}");
    let waits: Vec<InputWait> = seen
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallInputChanged { input, .. } => Some(*input),
            _ => None,
        })
        .collect();
    assert_eq!(waits, [InputWait::Hidden, InputWait::None]);

    // The model got the stopped text as the call's result.
    let requests = server.received();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].body.to_string().contains("efr interrupted it"));
    drop((stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_a_hidden_wait_stops_promptly_when_the_only_client_that_can_answer_leaves() {
    if !zsh_enabled(
        "shell_a_hidden_wait_stops_promptly_when_the_only_client_that_can_answer_leaves",
    ) {
        return;
    }
    let server = ResponsesServer::start().await;
    let daemon = daemon(&server).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream) =
        prompt_until_the_password_prompt(&daemon, &client, true).await;
    let (_, hidden_at) = until_hidden(&daemon, &mut stream).await;

    // The one subscription that could answer ends; the client watches on without it.
    drop(stream);
    let left = daemon.clock().now();
    let mut watching = subscribe_after(&client, conversation_id, hidden_at, false).await;
    let mut seen = events_while_time_passes(&daemon, &mut watching, 10, |event| {
        matches!(event, Event::ToolCallCompleted { .. } | Event::TurnFailed { .. })
    })
    .await;
    // The call's timeout is ten minutes; the stop comes at one of the next looks.
    let waited = daemon.clock().now().duration_since(left);
    assert!(waited.as_secs() <= 3, "{waited:?}");
    seen.extend(
        events_until(&mut watching, |event| {
            matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
        })
        .await
        .unwrap(),
    );
    let completed = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, is_error, exit_code, .. } => {
            Some((output.clone(), *is_error, *exit_code))
        }
        _ => None,
    });
    let (output, is_error, exit_code) = completed.unwrap();
    assert!(is_error);
    assert_eq!(exit_code, None);
    assert!(output.contains("no user could answer it at a terminal"), "{output}");
    assert_eq!(input_waits(&seen), [InputWait::None]);
    drop((watching, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_a_hidden_wait_goes_on_when_one_of_two_clients_that_can_answer_leaves() {
    if !zsh_enabled("shell_a_hidden_wait_goes_on_when_one_of_two_clients_that_can_answer_leaves") {
        return;
    }
    let server = ResponsesServer::start().await;
    let daemon = daemon(&server).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream) =
        prompt_until_the_password_prompt(&daemon, &client, true).await;
    let (call_id, hidden_at) = until_hidden(&daemon, &mut stream).await;

    // A second client that can answer, on a connection of its own, comes and goes. Its
    // first item, the wait replayed, shows that the daemon counts it.
    let other = daemon.client_for_tty(TTY).await.unwrap();
    let before = Seq::new(hidden_at.get() - 1);
    let mut second = subscribe_after(&other, conversation_id, before, true).await;
    let replayed = envelopes(second.next().await.unwrap().unwrap());
    assert!(replayed.iter().any(|envelope| envelope.seq == hidden_at), "{replayed:#?}");
    drop((second, other));
    let seen = events_for_seconds(&daemon, &mut stream, 5).await;
    assert!(
        !seen.iter().any(|envelope| matches!(envelope.event, Event::ToolCallCompleted { .. })),
        "{seen:#?}"
    );
    assert_eq!(input_waits(&seen), Vec::<InputWait>::new(), "the program still waits");

    // The client that stayed answers, and the program reads the password.
    let answer = Method::InputRespond(InputRespond {
        conversation_id,
        call_id,
        text: SecretText::new(SECRET),
        hidden: true,
        manual: false,
    });
    let _: InputRespondResult = client.call(answer).await.unwrap();
    let rest = events_until(&mut stream, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();
    let completed = rest.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, is_error, exit_code, .. } => {
            Some((output.clone(), *is_error, *exit_code))
        }
        _ => None,
    });
    let (output, is_error, exit_code) = completed.unwrap();
    assert!(!is_error, "{output}");
    assert_eq!(exit_code, Some(0));
    assert!(output.contains(&format!("len={}", SECRET.len())), "{output}");
    drop((stream, client));
    daemon.stop().await.unwrap();
}

/// The output and the exit code of the completed tool call among `events`.
fn completed_call(events: &[EventEnvelope]) -> (String, Option<i32>) {
    let completed = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, exit_code, .. } => Some((output.clone(), *exit_code)),
        _ => None,
    });
    completed.unwrap_or_else(|| panic!("{events:#?}"))
}

#[tokio::test]
async fn shell_an_approved_interactive_call_runs_past_its_timeout_while_a_client_can_answer() {
    if !zsh_enabled(
        "shell_an_approved_interactive_call_runs_past_its_timeout_while_a_client_can_answer",
    ) {
        return;
    }
    let server = ResponsesServer::start().await;
    // The line names `less`, which may wait for input at the terminal, so the user
    // approves the call as one that does; `less` itself never runs.
    let command = format!("true || less; {GETPASS}");
    let arguments = serde_json::json!({ "command": command, "timeout_seconds": 5 });
    let daemon = daemon_calling(&server, &arguments).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream) =
        prompt_until_the_password_prompt(&daemon, &client, true).await;
    let (call_id, _) = until_hidden(&daemon, &mut stream).await;

    // Four times the call's timeout passes; the call keeps running.
    let seen = events_for_seconds(&daemon, &mut stream, 20).await;
    assert!(
        !seen.iter().any(|envelope| matches!(envelope.event, Event::ToolCallCompleted { .. })),
        "{seen:#?}"
    );
    let answer = Method::InputRespond(InputRespond {
        conversation_id,
        call_id,
        text: SecretText::new(SECRET),
        hidden: true,
        manual: false,
    });
    let _: InputRespondResult = client.call(answer).await.unwrap();
    let rest = events_until(&mut stream, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();
    let (output, exit_code) = completed_call(&rest);
    assert!(output.contains(&format!("len={}", SECRET.len())), "{output}");
    assert_eq!(exit_code, Some(0));
    drop((stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_a_manual_answer_reaches_a_silent_command_and_a_plain_one_does_not() {
    if !zsh_enabled("shell_a_manual_answer_reaches_a_silent_command_and_a_plain_one_does_not") {
        return;
    }
    let server = ResponsesServer::start().await;
    let arguments = serde_json::json!({ "command": SILENT_READ, "timeout_seconds": 600 });
    let daemon = daemon_calling(&server, &arguments).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, mut stream) = prompt_until_shown(&daemon, &client, true, "ready").await;
    let started =
        daemon.events(&client, conversation_id).await.unwrap().iter().find_map(|envelope| {
            match &envelope.event {
                Event::ToolCallStarted { call_id, manual_input, .. } => {
                    Some((*call_id, *manual_input))
                }
                _ => None,
            }
        });
    let (call_id, manual_input) = started.unwrap();
    assert!(manual_input, "the client may offer a manual answer for the call");
    let seen = events_for_seconds(&daemon, &mut stream, 5).await;
    assert_eq!(input_waits(&seen), Vec::<InputWait>::new(), "no prompt, so no wait");

    let answer = |manual: bool| {
        Method::InputRespond(InputRespond {
            conversation_id,
            call_id,
            text: SecretText::new("hello"),
            hidden: false,
            manual,
        })
    };
    let plain = client.call::<InputRespondResult>(answer(false)).await;
    assert!(
        matches!(&plain, Err(efr_test_daemon::ClientError::Server { body }) if body.code == efr_protocol::ErrorCode::Conflict),
        "{plain:?}"
    );
    let _: InputRespondResult = client.call(answer(true)).await.unwrap();
    let rest = events_until(&mut stream, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();
    let (output, exit_code) = completed_call(&rest);
    assert!(output.contains("got=hello"), "{output}");
    assert_eq!(exit_code, Some(0));

    // The call ended: an answer for it finds nothing.
    let late = client.call::<InputRespondResult>(answer(true)).await;
    assert!(
        matches!(&late, Err(efr_test_daemon::ClientError::Server { body }) if body.code == efr_protocol::ErrorCode::NotFound),
        "{late:?}"
    );
    drop((stream, client));
    daemon.stop().await.unwrap();
}
