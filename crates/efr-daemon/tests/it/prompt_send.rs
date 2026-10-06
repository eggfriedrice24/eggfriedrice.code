//! `prompt.send` over a `TestDaemon`: a whole turn, a prompt queued behind a running
//! turn, the scope following the user between turns, and the refusals.

use efr_protocol::{
    ConversationStatus, ConversationsList, ConversationsListResult, EffectiveSettings, ErrorCode,
    Event, Method, Mode, OverriddenSettings, PromptSend, PromptSendResult, Scope, TurnSettings,
};
use efr_test_daemon::{ClientError, Replay, TTY, TestDaemon};
use pretty_assertions::assert_eq;
use serde_json::Value;

fn sent(replay: &Replay, frame: u64) -> PromptSendResult {
    let result = replay.result(frame).unwrap().clone().unwrap();
    serde_json::from_value(result).unwrap()
}

fn kinds(events: &[efr_protocol::EventEnvelope]) -> Vec<&str> {
    events.iter().map(|envelope| envelope.event.kind()).collect()
}

#[tokio::test]
async fn single_turn_text() {
    let replay = Replay::run("single_turn_text").await.unwrap();

    let result = sent(&replay, 1);
    assert!(!result.queued);
    assert_eq!(Some(result.conversation_id), replay.conversation());
    let list: ConversationsListResult = replay
        .client()
        .call(Method::ConversationsList(ConversationsList::default()))
        .await
        .unwrap();
    let [summary] = list.conversations.as_slice() else { panic!("{list:?}") };
    assert_eq!(summary.id, result.conversation_id);
    assert_eq!(summary.title.as_deref(), Some("say hello"));
    assert_eq!(summary.tty.as_deref(), Some(TTY));
    assert_eq!(summary.status, ConversationStatus::Idle);
    let events = replay.events().await.unwrap();
    assert_eq!(
        kinds(&events),
        [
            "conversation_created",
            "prompt_queued",
            "turn_started",
            "assistant_message_updated",
            "assistant_message_completed",
            "turn_completed",
        ]
    );
    assert_eq!(events[1].seq, result.seq, "the result names the prompt's own event");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn queue_second_prompt() {
    let replay = Replay::run("queue_second_prompt").await.unwrap();

    let (first, second) = (sent(&replay, 1), sent(&replay, 2));
    assert!(!first.queued);
    assert!(second.queued, "the second prompt arrived while the first turn ran");
    assert_eq!(second.conversation_id, first.conversation_id, "the terminal's conversation");
    assert_ne!(second.turn_id, first.turn_id);
    let events = replay.events().await.unwrap();
    let started: Vec<_> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .collect();
    assert_eq!(started, [first.turn_id, second.turn_id], "turns run one after the other");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn cwd_move_between_turns() {
    let replay = Replay::run("cwd_move_between_turns").await.unwrap();

    let cwd = replay.daemon().cwd().to_path_buf();
    let events = replay.events().await.unwrap();
    let started: Vec<_> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::TurnStarted { cwd, scope, .. } => Some((cwd.clone(), scope.clone())),
            _ => None,
        })
        .collect();
    let project = replay.scenario().spec().project.unwrap().0.parse().unwrap();
    assert_eq!(
        started,
        [(cwd.join("alpha"), Scope::Machine), (cwd.join("beta"), Scope::Project(project))]
    );
    let changed = events.iter().filter(|envelope| envelope.event.kind() == "scope_changed");
    assert_eq!(changed.count(), 1, "only the move into the project changes the scope");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn an_empty_prompt_is_invalid_and_stays_refused_on_a_retry() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let empty = daemon.prompt(1, "   ", TTY);
    let first = client.call::<Value>(empty.clone()).await;
    let again = client.call::<Value>(empty).await;

    let (Err(ClientError::Server { body: first }), Err(ClientError::Server { body: again })) =
        (first, again)
    else {
        panic!("an empty prompt must be refused");
    };
    assert_eq!(first.code, ErrorCode::Invalid);
    assert_eq!(again, first);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_prompt_for_a_conversation_that_does_not_exist_is_not_found() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();

    let Method::PromptSend(params) = daemon.prompt(1, "hello", TTY) else { unreachable!() };
    let unknown = "0192f0c1-7a00-7000-8000-00000000dead".parse().unwrap();
    let method = Method::PromptSend(PromptSend { conversation_id: Some(unknown), ..params });
    let refused = client.call::<Value>(method).await;

    let Err(ClientError::Server { body }) = refused else { panic!("{refused:?}") };
    assert_eq!(body.code, ErrorCode::NotFound);
    drop(client);
    daemon.stop().await.unwrap();
}

/// No process has this pid: the kernel's limit is far below it.
const GONE: u32 = 4_000_000_000;

/// Sends a prompt from `tty` by the shell `shell_pid` and waits until its turn ended,
/// so the conversation's last activity is known.
async fn send_from(
    daemon: &TestDaemon,
    client: &efr_test_daemon::Client,
    n: u128,
    tty: &str,
    shell_pid: u32,
) -> PromptSendResult {
    let Method::PromptSend(mut params) = daemon.prompt(n, "hello", tty) else {
        unreachable!("TestDaemon::prompt makes a prompt.send")
    };
    if let Some(context) = &mut params.context {
        context.shell_pid = Some(shell_pid);
    }
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let mut follow = daemon.follow(client, sent.conversation_id).await.unwrap();
    efr_test_daemon::events_until(&mut follow, |event| {
        matches!(event, Event::TurnFailed { turn_id, .. } | Event::TurnCompleted { turn_id, .. } if *turn_id == sent.turn_id)
    })
    .await
    .unwrap();
    sent
}

#[tokio::test]
async fn a_reused_terminal_starts_over_only_once_its_old_shell_is_gone() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let alive = std::process::id();

    let first = send_from(&daemon, &client, 1, "/dev/pts/a", alive).await;
    let nested = send_from(&daemon, &client, 2, "/dev/pts/a", GONE).await;
    assert_eq!(nested.conversation_id, first.conversation_id, "the first shell still runs");

    let closed = send_from(&daemon, &client, 3, "/dev/pts/b", GONE).await;
    let reused = send_from(&daemon, &client, 4, "/dev/pts/b", alive).await;
    assert_ne!(reused.conversation_id, closed.conversation_id, "a new tab on a closed tab's pts");
    let again = send_from(&daemon, &client, 5, "/dev/pts/b", alive).await;
    assert_eq!(again.conversation_id, reused.conversation_id, "the new tab keeps its own");
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn the_shell_of_a_terminal_is_remembered_across_a_restart() {
    let mut daemon = TestDaemon::builder().persistent().start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let closed = send_from(&daemon, &client, 1, TTY, GONE).await;
    drop(client);

    daemon.restart().await.unwrap();
    let client = daemon.client().await.unwrap();
    let reused = send_from(&daemon, &client, 2, TTY, std::process::id()).await;

    assert_ne!(reused.conversation_id, closed.conversation_id);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn an_idle_terminal_starts_over_after_the_configured_hours() {
    let daemon = TestDaemon::builder()
        .config(|config| config.conversation.tty_idle_hours = 1)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let pid = std::process::id();

    let first = send_from(&daemon, &client, 1, TTY, pid).await;
    daemon.clock().advance(std::time::Duration::from_secs(50 * 60));
    let second = send_from(&daemon, &client, 2, TTY, pid).await;
    daemon.clock().advance(std::time::Duration::from_secs(61 * 60));
    let third = send_from(&daemon, &client, 3, TTY, pid).await;

    assert_eq!(second.conversation_id, first.conversation_id, "50 minutes is not idle");
    assert_ne!(third.conversation_id, second.conversation_id, "61 minutes is");
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_prompt_with_a_model_outside_the_list_is_invalid_with_the_choices() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(1, "hello", TTY) else { unreachable!() };
    params.settings.model = Some("gpt-4o".to_owned());

    let refused = client.call::<Value>(Method::PromptSend(params)).await;

    let Err(ClientError::Server { body }) = refused else { panic!("{refused:?}") };
    assert_eq!(body.code, ErrorCode::Invalid);
    assert!(body.message.starts_with("the model gpt-4o is not in the model list"), "{body:?}");
    let data = body.data.unwrap();
    assert_eq!(data["setting"], "model");
    let choices = data["choices"].as_array().unwrap();
    assert!(choices.iter().any(|choice| choice == "gpt-5.5"), "{choices:?}");
    let list: ConversationsListResult =
        client.call(Method::ConversationsList(ConversationsList::default())).await.unwrap();
    assert!(list.conversations.is_empty(), "nothing was recorded");
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_prompts_settings_are_answered_and_recorded_on_its_turn() {
    let daemon = TestDaemon::builder()
        .config(|config| config.model.effort = Some("low".to_owned()))
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(1, "hello", TTY) else { unreachable!() };
    params.settings =
        TurnSettings { mode: Some(Mode::Auto), model: Some("gpt-6-sol".to_owned()), effort: None };

    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let seen = efr_test_daemon::events_until(
        &mut follow,
        |event| matches!(event, Event::TurnStarted { turn_id, .. } if *turn_id == sent.turn_id),
    )
    .await
    .unwrap();

    let expected = EffectiveSettings {
        mode: Mode::Auto,
        model: "gpt-6-sol".to_owned(),
        effort: Some("low".to_owned()),
        overridden: OverriddenSettings { mode: true, model: true, effort: false },
        fallback: None,
    };
    assert_eq!(sent.settings, Some(expected.clone()));
    let started = seen.iter().find_map(|envelope| match &envelope.event {
        Event::TurnStarted { settings, .. } => Some(settings.clone()),
        _ => None,
    });
    assert_eq!(started, Some(Some(expected)));
    drop(client);
    daemon.stop().await.unwrap();
}
