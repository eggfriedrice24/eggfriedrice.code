//! A running `shell` call that the user moves into a background job with Ctrl+B
//! (`call.background`), over a `TestDaemon` with a real zsh and the local Responses
//! server as the model: the preview and the confirm of a call outside the sandbox, the
//! move at once of a contained call, the job that adopts the call's old shell, its end,
//! its stop, its time limit and efrd's stop, the new hidden shell of the next call, and
//! the refusals.
//!
//! Each command prints `efr-ready` and then reads a FIFO that the test writes after the
//! move, so no test waits on a fixed time. The tests set `jobs.max_wakes_per_hour = 0`
//! unless they test the wake, so no wake turn races the test. Every test but the scope
//! test drives a real zsh and skips with a message unless `EFR_TEST_ZSH=1`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, Base64Bytes, CallBackground,
    CallBackgroundResult, CallId, ConversationId, ConversationSubscribe, ConversationSubscribeItem,
    ErrorCode, Event, EventEnvelope, JobId, JobState, JobStopBy, JobsStop, JobsStopResult, Launch,
    Method, Mode, Origin, PromptSendResult, PtyWrite, PtyWriteResult, Seq, TurnSettings,
};
use efr_stdx::time::Clock as _;
use efr_test_daemon::{
    Client, ClientError, FakeLauncher, ItemStream, ResponsesAnswer, ResponsesServer, TTY,
    TestDaemon, TestDaemonBuilder, command_id, events_until,
};
use efr_test_support::TestDirs;
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::sandbox::{REVIEWER_ALLOWS, ScriptedModel, ready};
use crate::support::zsh_enabled;

/// The marker that each command prints before it waits for the test.
const READY: &str = "efr-ready";

/// How long a test lets pass in real time while it moves the daemon's clock.
const REAL_TIME_LIMIT: Duration = Duration::from_secs(30);

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

    /// A daemon over a real zsh in this tree, with no wake turns.
    fn builder(&self) -> TestDaemonBuilder {
        TestDaemon::builder()
            .local_pty()
            .dirs(Arc::clone(&self.dirs))
            .persistent()
            .config(|settings| settings.jobs.max_wakes_per_hour = 0)
    }

    /// A daemon whose model gives `answers` in order.
    async fn daemon(&self, server: &ResponsesServer, answers: Vec<ResponsesAnswer>) -> TestDaemon {
        for answer in answers {
            server.push(answer);
        }
        self.builder().responses(server).start().await.unwrap()
    }

    /// Lets the command go on past the FIFO; it waits for the reader.
    async fn release(&self) {
        let fifo = self.fifo.clone();
        tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
    }
}

/// The `shell` input of `line`, with a timeout that no test reaches.
fn call(line: &str) -> serde_json::Value {
    json!({ "command": line, "timeout_seconds": 600 })
}

/// A model that calls `shell` with `line`, then says `done`.
fn calls(line: &str) -> Vec<ResponsesAnswer> {
    vec![ResponsesAnswer::tool_call("call_1", "shell", &call(line)), ResponsesAnswer::text("done")]
}

/// Sends the prompt with the command id `n` in `mode` (the config's when `None`),
/// follows it and answers each question with yes, until the call's output shows
/// [`READY`]. Returns the conversation, the call and the stream.
async fn start(
    daemon: &TestDaemon,
    client: &Client,
    n: u128,
    mode: Option<Mode>,
) -> (ConversationId, CallId, ItemStream<ConversationSubscribeItem>) {
    let Method::PromptSend(mut params) = daemon.prompt(n, "run the build", TTY) else {
        unreachable!()
    };
    params.settings = TurnSettings { mode, ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let conversation_id = sent.conversation_id;
    let subscribe = ConversationSubscribe {
        conversation_id,
        after_seq: Some(Seq::ZERO),
        // NOTE: a person who can answer follows, as in `efr`: the run looks for waits.
        answers_input: true,
        drafts: false,
    };
    let mut stream = client.stream(Method::ConversationSubscribe(subscribe)).await.unwrap();
    let mut answers = 1000 + n * 100;
    let mut call = None;
    loop {
        let seen = events_until(&mut stream, |event| {
            matches!(event, Event::ToolCallOutputUpdated { tail, .. } if tail.contains(READY))
                || matches!(
                    event,
                    Event::ApprovalRequested { .. }
                        | Event::ToolCallCompleted { .. }
                        | Event::TurnCompleted { .. }
                        | Event::TurnFailed { .. }
                )
        })
        .await
        .unwrap();
        for envelope in &seen {
            if let Event::ToolCallStarted { call_id, .. } = &envelope.event {
                call = Some(*call_id);
            }
        }
        match seen.last().map(|envelope| envelope.event.clone()) {
            Some(Event::ApprovalRequested { call_id, .. }) => {
                answers += 1;
                let approve = Method::ApprovalRespond(ApprovalRespond {
                    command_id: command_id(answers),
                    conversation_id,
                    call_id,
                    decision: ApprovalDecision::Allow,
                });
                let _: ApprovalRespondResult = client.call(approve).await.unwrap();
            }
            Some(Event::ToolCallOutputUpdated { .. }) => {
                return (conversation_id, call.unwrap(), stream);
            }
            // NOTE: an earlier call of the turn ended; the one that prints comes next.
            Some(Event::ToolCallCompleted { .. }) => {}
            other => panic!("the call did not start: {other:?} {seen:#?}"),
        }
    }
}

/// `call.background` of `call` with the command id `n`.
fn background(
    n: u128,
    conversation_id: ConversationId,
    call_id: CallId,
    confirm: Option<u64>,
) -> Method {
    Method::CallBackground(CallBackground {
        conversation_id,
        call_id,
        // NOTE: apart from the ids of the prompts and the answers.
        command_id: command_id(20_000 + n),
        confirm,
    })
}

/// Moves a call outside the sandbox: the preview, then the confirm with its token.
async fn moved_after_yes(
    client: &Client,
    conversation_id: ConversationId,
    call_id: CallId,
) -> JobId {
    let preview: CallBackgroundResult =
        client.call(background(1, conversation_id, call_id, None)).await.unwrap();
    let CallBackgroundResult::Preview { token, .. } = preview else { panic!("{preview:?}") };
    let moved: CallBackgroundResult =
        client.call(background(2, conversation_id, call_id, Some(token))).await.unwrap();
    let CallBackgroundResult::Moved { job_id } = moved else { panic!("{moved:?}") };
    job_id
}

/// The message of a refused request, which must be `invalid`.
fn refusal<T: std::fmt::Debug>(answer: Result<T, ClientError>) -> String {
    match answer {
        Err(ClientError::Server { body }) => {
            assert_eq!(body.code, ErrorCode::Invalid, "{body:?}");
            body.message
        }
        other => panic!("{other:?}"),
    }
}

/// The events of `stream` up to the first `job_ended`, which comes last.
async fn until_job_ended(stream: &mut ItemStream<ConversationSubscribeItem>) -> Vec<EventEnvelope> {
    events_until(stream, |event| matches!(event, Event::JobEnded { .. })).await.unwrap()
}

/// The events of one subscription item.
fn envelopes(item: ConversationSubscribeItem) -> Vec<EventEnvelope> {
    match item {
        ConversationSubscribeItem::Event(envelope) => vec![envelope],
        ConversationSubscribeItem::Snapshot(snapshot) => snapshot.events,
        _ => Vec::new(),
    }
}

/// The events of `stream` up to the first that `stop` accepts, while the daemon's
/// clock moves one second whenever a sleep of the daemon ends within it and no event
/// came for a while, at most `max_seconds` times.
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
            () = efr_stdx::time::SystemClock.sleep(Duration::from_millis(50)) => {
                assert!(started.elapsed() < REAL_TIME_LIMIT, "no end in real time: {seen:#?}");
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

/// The text of the job's output file `path`, read after its `job_ended`.
fn output_of(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[tokio::test]
async fn ctrl_b_moves_a_direct_call_and_the_turn_goes_on() {
    if !zsh_enabled("ctrl_b_moves_a_direct_call_and_the_turn_goes_on") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let line = place.line("echo efr-after; sh -c 'exit 3'");
    let daemon = place.daemon(&server, calls(&line)).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;

    let preview: CallBackgroundResult =
        client.call(background(1, conversation_id, call_id, None)).await.unwrap();
    let CallBackgroundResult::Preview { token, command, max_minutes, exits } = preview else {
        panic!("{preview:?}")
    };
    assert_eq!((command.as_str(), max_minutes), (line.as_str(), 60));
    assert!(!exits.is_empty(), "the job asks in cautious, and the question says so");
    let stale = client
        .call::<CallBackgroundResult>(background(9, conversation_id, call_id, Some(token + 1)))
        .await;
    assert_eq!(refusal(stale), "the question expired; press ctrl+b again");
    let job_id = moved_after_yes(&client, conversation_id, call_id).await;
    assert_eq!(job_id, JobId::nth(1));

    let seen = events_until(&mut stream, |event| matches!(event, Event::TurnCompleted { .. }))
        .await
        .unwrap();
    let started = seen
        .iter()
        .position(|envelope| matches!(envelope.event, Event::JobStarted { .. }))
        .expect("job_started");
    let completed = seen
        .iter()
        .position(|envelope| matches!(envelope.event, Event::ToolCallCompleted { .. }))
        .expect("tool_call_completed");
    assert!(started < completed, "the job is recorded before the call answers");
    let Event::JobStarted { job, .. } = &seen[started].event else { unreachable!() };
    let moved_from = job.moved_from.clone().unwrap();
    assert_eq!((moved_from.call_id, job.launch.clone()), (call_id, Launch::Unsandboxed));
    assert!(!moved_from.exits.is_empty(), "the record keeps the exits that the yes covered");
    let Event::ToolCallCompleted { output, exit_code, is_error, left_running, .. } =
        &seen[completed].event
    else {
        unreachable!()
    };
    assert!(output.contains(READY), "the output so far comes first: {output}");
    assert!(output.contains("the user moved this command to the background"), "{output}");
    assert!(output.contains("job j1"), "{output}");
    assert_eq!((*exit_code, *is_error, *left_running), (None, false, false));
    let old_pty = seen.iter().find_map(|envelope| match envelope.event {
        Event::ShellStarted { pty_id, .. } => Some(pty_id),
        _ => None,
    });

    // NOTE: the old PTY id never reaches a shell of the conversation again (DS56).
    if let Some(pty_id) = old_pty {
        let write =
            Method::PtyWrite(PtyWrite { pty_id, data: Base64Bytes::new(b"echo hi\n".to_vec()) });
        let written = client.call::<PtyWriteResult>(write).await;
        assert!(
            matches!(&written, Err(ClientError::Server { body }) if body.code == ErrorCode::NotFound),
            "{written:?}"
        );
    }

    place.release().await;
    let seen = until_job_ended(&mut stream).await;
    let Some(Event::JobEnded { state, exit_code, launcher_lost, stopped_by, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!((state, exit_code, stopped_by), (JobState::Exited, Some(3), None));
    assert!(!launcher_lost, "a line typed into the shell has no launcher to lose");
    assert!(
        !seen.iter().any(|envelope| matches!(envelope.event, Event::ShellExited { .. })),
        "no client reads that the conversation's shell exited: {seen:#?}"
    );
    let output = output_of(&job.output);
    assert!(output.contains(READY) && output.contains("efr-after"), "{output:?}");
    assert!(!output.contains('\u{1b}'), "no escape bytes: {output:?}");
}

#[tokio::test]
async fn moved_job_ends_with_exit_code_and_notice() {
    if !zsh_enabled("moved_job_ends_with_exit_code_and_notice") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let line = place.line("sh -c 'exit 7'");
    let mut answers = calls(&line);
    answers.push(ResponsesAnswer::text("noted"));
    for answer in answers {
        server.push(answer);
    }
    let daemon = place
        .builder()
        .config(|settings| settings.jobs.max_wakes_per_hour = 6)
        .responses(&server)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;
    moved_after_yes(&client, conversation_id, call_id).await;
    events_until(&mut stream, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();

    place.release().await;

    let seen = until_job_ended(&mut stream).await;
    let Some(Event::JobEnded { state, exit_code, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!((state, exit_code), (JobState::Exited, Some(7)));
    let seen =
        events_until(&mut stream, |event| matches!(event, Event::JobNotice { .. })).await.unwrap();
    let Some(Event::JobNotice { job_id, woke, text, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!(job_id, JobId::nth(1));
    assert!(woke, "the end of a moved job wakes the model");
    assert!(text.contains("j1"), "{text}");
}

#[tokio::test]
async fn next_call_gets_a_new_shell_in_old_cwd() {
    if !zsh_enabled("next_call_gets_a_new_shell_in_old_cwd") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let work = place.dirs.create_dir("work").unwrap();
    let line = format!("cd '{}' && {}", work.display(), place.line("true"));
    let first = format!("cd '{}'", work.display());
    let answers = vec![
        ResponsesAnswer::tool_call("call_0", "shell", &call(&first)),
        ResponsesAnswer::tool_call("call_1", "shell", &call(&line)),
        ResponsesAnswer::text("done"),
        ResponsesAnswer::tool_call("call_2", "shell", &call("pwd; echo pid=$$")),
        ResponsesAnswer::text("done"),
    ];
    let daemon = place.daemon(&server, answers).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;
    moved_after_yes(&client, conversation_id, call_id).await;
    let first_turn =
        events_until(&mut stream, |event| matches!(event, Event::TurnCompleted { .. }))
            .await
            .unwrap();
    let old_pid = first_turn.iter().find_map(|envelope| match envelope.event {
        Event::ShellStarted { pid, .. } => pid,
        _ => None,
    });

    let Method::PromptSend(mut params) = daemon.prompt(2, "where am i", TTY) else {
        unreachable!()
    };
    params.conversation_id = Some(conversation_id);
    params.settings = TurnSettings { mode: Some(Mode::Cautious), ..TurnSettings::default() };
    let _: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let mut answers = 5000;
    let output = loop {
        let seen = events_until(&mut stream, |event| {
            matches!(event, Event::ApprovalRequested { .. })
                || matches!(event, Event::ToolCallCompleted { output, .. } if output.contains("pid="))
        })
        .await
        .unwrap();
        match seen.last().map(|envelope| envelope.event.clone()) {
            Some(Event::ApprovalRequested { call_id, .. }) => {
                answers += 1;
                let approve = Method::ApprovalRespond(ApprovalRespond {
                    command_id: command_id(answers),
                    conversation_id,
                    call_id,
                    decision: ApprovalDecision::Allow,
                });
                let _: ApprovalRespondResult = client.call(approve).await.unwrap();
            }
            Some(Event::ToolCallCompleted { output, .. }) => break output,
            other => panic!("{other:?}"),
        }
    };
    assert!(output.contains(&work.display().to_string()), "the new shell starts there: {output}");
    if let Some(old_pid) = old_pid {
        assert!(!output.contains(&format!("pid={old_pid}\n")), "a new shell: {output}");
    }
    place.release().await;
}

#[tokio::test]
async fn move_refused_when_jobs_off() {
    if !zsh_enabled("move_refused_when_jobs_off") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    for answer in calls(&place.line("true")) {
        server.push(answer);
    }
    let daemon = place
        .builder()
        .config(|settings| settings.jobs.enabled = false)
        .responses(&server)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, _stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;

    let answer =
        client.call::<CallBackgroundResult>(background(1, conversation_id, call_id, None)).await;

    assert_eq!(refusal(answer), "jobs are off (jobs.enabled = false)");
    place.release().await;
}

#[tokio::test]
async fn move_refused_while_waiting() {
    if !zsh_enabled("move_refused_while_waiting") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    // NOTE: a program of its own reads, so the shell itself does not hold the terminal.
    let line = format!("echo {READY}; sh -c 'printf \"name? \"; read -r x; echo got=$x'");
    let daemon = place.daemon(&server, calls(&line)).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;
    events_while_time_passes(&daemon, &mut stream, 10, |event| {
        matches!(event, Event::ToolCallInputChanged { input, .. } if *input != efr_protocol::InputWait::None)
    })
    .await;

    let answer =
        client.call::<CallBackgroundResult>(background(1, conversation_id, call_id, None)).await;
    let CallBackgroundResult::Preview { token, .. } = answer.unwrap() else { panic!() };
    let answer = client
        .call::<CallBackgroundResult>(background(2, conversation_id, call_id, Some(token)))
        .await;

    assert_eq!(refusal(answer), "the command waits for input; answer it first");
    client
        .call::<PtyWriteResult>(Method::PtyWrite(PtyWrite {
            pty_id: pty_of(&daemon, conversation_id).await,
            data: Base64Bytes::new(b"me\n".to_vec()),
        }))
        .await
        .unwrap();
    let seen = events_until(&mut stream, |event| matches!(event, Event::ToolCallCompleted { .. }))
        .await
        .unwrap();
    assert!(
        !seen.iter().any(|envelope| matches!(envelope.event, Event::JobStarted { .. })),
        "nothing moved"
    );
}

/// The PTY of the conversation's hidden shell, from its `shell_started`.
async fn pty_of(daemon: &TestDaemon, conversation_id: ConversationId) -> efr_protocol::PtyId {
    let client = daemon.client().await.unwrap();
    let events = daemon.events(&client, conversation_id).await.unwrap();
    events
        .iter()
        .rev()
        .find_map(|envelope| match envelope.event {
            Event::ShellStarted { pty_id, .. } => Some(pty_id),
            _ => None,
        })
        .unwrap()
}

#[tokio::test]
async fn move_from_phone_is_forbidden() {
    let daemon = TestDaemon::builder().start().await.unwrap();
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();
    let conversation_id = ConversationId::from_uuid(uuid::Uuid::from_u128(1));
    let call_id = CallId::from_uuid(uuid::Uuid::from_u128(2));

    let answer =
        phone.call::<CallBackgroundResult>(background(1, conversation_id, call_id, None)).await;

    assert!(
        matches!(&answer, Err(ClientError::Server { body }) if body.code == ErrorCode::Forbidden),
        "{answer:?}"
    );
}

#[tokio::test]
async fn move_after_the_run_ended_answers_ended() {
    if !zsh_enabled("move_after_the_run_ended_answers_ended") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let daemon = place.daemon(&server, calls(&place.line("true"))).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;
    place.release().await;
    events_until(&mut stream, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();

    let answer =
        client.call::<CallBackgroundResult>(background(1, conversation_id, call_id, None)).await;

    assert_eq!(refusal(answer), "the call does not run");
}

#[tokio::test]
async fn stop_of_moved_job_sigint_then_close() {
    if !zsh_enabled("stop_of_moved_job_sigint_then_close") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    // NOTE: the command ignores SIGINT, so the stop closes its shell after the grace.
    let line = format!("sh -c 'trap \"\" INT; echo {READY}; exec sleep 600'");
    let daemon = place.daemon(&server, calls(&line)).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;
    let job_id = moved_after_yes(&client, conversation_id, call_id).await;
    events_until(&mut stream, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();

    let stop = Method::JobsStop(JobsStop {
        command_id: command_id(30_000),
        conversation_id: Some(conversation_id),
        job_id: job_id.clone(),
    });
    let stopper = {
        let client = daemon.client_for_tty(TTY).await.unwrap();
        tokio::spawn(async move { client.call::<JobsStopResult>(stop).await })
    };
    let seen = events_while_time_passes(&daemon, &mut stream, 20, |event| {
        matches!(event, Event::JobEnded { .. })
    })
    .await;

    let Some(Event::JobEnded { state, stopped_by, launcher_lost, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!((state, stopped_by), (JobState::Stopped, Some(JobStopBy::User)));
    assert!(!launcher_lost);
    let stopped = stopper.await.unwrap().unwrap();
    assert_eq!(stopped.job.id, job_id);
    assert!(
        daemon.clock().now() >= stopped.job.started_at.checked_add(Duration::from_secs(6)).unwrap(),
        "the close came after the grace"
    );
}

#[tokio::test]
async fn moved_direct_job_stop_kills_a_setsid_child() {
    if !zsh_enabled("moved_direct_job_stop_kills_a_setsid_child") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    // NOTE: `sh` runs no job control, so `setsid` is no group leader and execs the
    // sleep at once: `$!` is its pid. It outlives `sh` below the hidden shell. The pid
    // goes to a file: the run's live output shows one change only, its first line.
    let pid_file = place.dirs.root().join("child");
    let line = format!(
        "sh -c 'setsid sleep 600 > /dev/null 2>&1 & echo $! > {}'; {}",
        pid_file.display(),
        place.line("true")
    );
    let daemon = place.daemon(&server, calls(&line)).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;
    let job_id = moved_after_yes(&client, conversation_id, call_id).await;
    events_until(&mut stream, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();
    let child: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    assert!(Path::new(&format!("/proc/{child}")).exists());

    let stop = Method::JobsStop(JobsStop {
        command_id: command_id(30_000),
        conversation_id: Some(conversation_id),
        job_id,
    });
    let stopper = {
        let client = daemon.client_for_tty(TTY).await.unwrap();
        tokio::spawn(async move { client.call::<JobsStopResult>(stop).await })
    };
    let seen = events_while_time_passes(&daemon, &mut stream, 20, |event| {
        matches!(event, Event::JobEnded { .. })
    })
    .await;

    let Some(Event::JobEnded { stopped_by, survivors, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!(stopped_by, Some(JobStopBy::User));
    assert_eq!(survivors, Vec::<String>::new());
    stopper.await.unwrap().unwrap();
    let gone = efr_test_support::Wait::new("the setsid child's end")
        .until(|| {
            std::fs::read_to_string(format!("/proc/{child}/stat"))
                .map_or(true, |stat| stat.contains(") Z "))
        })
        .await;
    assert!(gone.is_ok(), "the child that left the shell's session was killed (DS55)");
}

#[tokio::test]
async fn moved_job_time_limit_counts_from_move() {
    if !zsh_enabled("moved_job_time_limit_counts_from_move") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    for answer in calls(&place.line("true")) {
        server.push(answer);
    }
    let daemon = place
        .builder()
        .config(|settings| settings.jobs.default_minutes = 1)
        .responses(&server)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;
    // NOTE: the run time before the move does not count (DS24).
    daemon.clock().advance(Duration::from_secs(50));
    moved_after_yes(&client, conversation_id, call_id).await;
    let seen =
        events_until(&mut stream, |event| matches!(event, Event::JobStarted { .. })).await.unwrap();
    let Some(Event::JobStarted { job, .. }) = seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!(job.ends_by, job.started_at.checked_add(Duration::from_secs(60)).unwrap());
    assert_eq!(job.moved_from.unwrap().after_secs, 50);

    daemon.clock().advance(Duration::from_secs(60));

    let seen = events_while_time_passes(&daemon, &mut stream, 20, |event| {
        matches!(event, Event::JobEnded { .. })
    })
    .await;
    let Some(Event::JobEnded { state, stopped_by, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!((state, stopped_by), (JobState::TimedOut, Some(JobStopBy::TimeLimit)));
}

#[tokio::test]
async fn efrd_stop_stops_moved_jobs() {
    if !zsh_enabled("efrd_stop_stops_moved_jobs") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let mut daemon = place.daemon(&server, calls(&place.line("true"))).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;
    moved_after_yes(&client, conversation_id, call_id).await;
    events_until(&mut stream, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();
    drop(stream);
    drop(client);

    daemon.restart().await.unwrap();

    let client = daemon.client().await.unwrap();
    let events = daemon.events(&client, conversation_id).await.unwrap();
    let ended = events.iter().find_map(|envelope| match &envelope.event {
        Event::JobEnded { stopped_by, .. } => Some(*stopped_by),
        _ => None,
    });
    assert_eq!(ended, Some(Some(JobStopBy::DaemonStop)), "{events:#?}");
}

#[tokio::test]
async fn moved_job_hidden_wait_is_stopped() {
    if !zsh_enabled("moved_job_hidden_wait_is_stopped") {
        return;
    }
    let place = Place::new();
    let server = ResponsesServer::start().await;
    let getpass = r#"sh -c 'stty -echo; printf "password: "; IFS= read -r p; stty echo'"#;
    let daemon = place.daemon(&server, calls(&place.line(getpass))).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, 1, Some(Mode::Cautious)).await;
    moved_after_yes(&client, conversation_id, call_id).await;
    events_until(&mut stream, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();

    place.release().await;

    // NOTE: nobody follows a job, so the wait for a password stops at once (DS27).
    let seen = events_while_time_passes(&daemon, &mut stream, 30, |event| {
        matches!(event, Event::JobEnded { .. })
    })
    .await;
    let Some(Event::JobEnded { state, stopped_by, exit_code, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!((state, stopped_by), (JobState::Exited, None));
    assert!(exit_code.is_some_and(|code| code != 0), "SIGINT ended the read: {exit_code:?}");
}

#[tokio::test]
async fn ctrl_b_moves_a_contained_call_at_once() {
    if !zsh_enabled("ctrl_b_moves_a_contained_call_at_once") {
        return;
    }
    let place = Place::new();
    let fake = FakeLauncher::write(&place.dirs.root().join("fake")).unwrap();
    let line = place.line("exit 5");
    let model = ScriptedModel::reviewed(vec![("shell", call(&line))], REVIEWER_ALLOWS);
    let env = BTreeMap::from([
        ("HOME".to_owned(), place.dirs.home().to_string_lossy().into_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
    ]);
    let daemon = place
        .builder()
        .shell_env(env)
        .sandbox_launcher(fake.path())
        .probe_override(ready())
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) = start(&daemon, &client, 1, Some(Mode::Auto)).await;

    // NOTE: a contained call keeps its sandbox, so the key press is the answer.
    let moved: CallBackgroundResult =
        client.call(background(1, conversation_id, call_id, None)).await.unwrap();
    let CallBackgroundResult::Moved { job_id } = moved else { panic!("{moved:?}") };
    assert_eq!(job_id, JobId::nth(1));
    let seen = events_until(&mut stream, |event| matches!(event, Event::TurnCompleted { .. }))
        .await
        .unwrap();
    let job = seen
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::JobStarted { job, .. } => Some(job.clone()),
            _ => None,
        })
        .unwrap();
    assert!(matches!(job.launch, Launch::Contained { .. }), "{job:?}");
    assert_eq!(job.moved_from.unwrap().exits, []);
    let call_dir = place
        .dirs
        .dirs()
        .runtime()
        .join("sbx")
        .join(conversation_id.to_string())
        .join(call_id.to_string());
    assert!(call_dir.join(efr_sandbox::MOVED_FILE).exists(), "the launcher writes no state");

    place.release().await;

    let seen = until_job_ended(&mut stream).await;
    let Some(Event::JobEnded { state, exit_code, launcher_lost, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!((state, exit_code), (JobState::Exited, Some(5)));
    assert!(!launcher_lost, "the launcher's result in the call's dir was read");
    // NOTE: the job's end removes its dirs right after it recorded `job_ended`.
    efr_test_support::Wait::new("the call's dir to go")
        .until(|| !call_dir.exists())
        .await
        .expect("the job removed the call's dir");
    let shell_dir = call_dir.parent().unwrap();
    assert!(shell_dir.is_dir(), "the shell's dir, with the sandbox state, stays");
}
