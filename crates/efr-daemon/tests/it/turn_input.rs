//! The input row of a turn over a `TestDaemon`: a late steer that becomes a prompt,
//! `prompt.withdraw`, and the interrupt of Esc that withdraws prompts and sends unread
//! steers again. A retried command answers exactly as the first time, also the
//! sequence numbers inside a result. A model that waits for the test holds each turn,
//! so nothing depends on timing.

use std::sync::Arc;

use async_trait::async_trait;
use efr_protocol::{
    ConversationId, ErrorCode, Event, EventEnvelope, LateSteer, Method, PromptSendResult,
    PromptWithdraw, PromptWithdrawResult, ShellContext, TurnId, TurnInterrupt, TurnInterruptResult,
    TurnSettings, TurnSteer, TurnSteerResult, WithdrawTarget,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{Client, ClientError, TTY, TestDaemon, command_id, events_until};
use pretty_assertions::assert_eq;
use serde_json::Value;
use tokio::sync::Semaphore;

/// A model that answers `ok` to each call once the test lets one more call through.
#[derive(Debug)]
struct Gated {
    id: ProviderId,
    calls: Arc<Semaphore>,
}

impl Gated {
    fn new() -> Arc<Self> {
        Arc::new(Gated { id: ProviderId::new("test").unwrap(), calls: Arc::new(Semaphore::new(0)) })
    }

    /// Lets `count` more model calls answer.
    fn allow(&self, count: usize) {
        self.calls.add_permits(count);
    }
}

#[async_trait]
impl Provider for Gated {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let calls = Arc::clone(&self.calls);
        let events = futures::stream::once(async move {
            // NOTE: the permit is used up, so each call takes one.
            calls.acquire().await.map(|permit| permit.forget()).ok();
            futures::stream::iter([
                Ok(ProviderEvent::TextDelta { text: "ok".to_owned() }),
                Ok(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None }),
            ])
        });
        Ok(Box::pin(futures::StreamExt::flatten(events)))
    }
}

async fn start(model: &Arc<Gated>) -> (TestDaemon, Client) {
    let daemon = TestDaemon::builder()
        .custom_provider(Arc::clone(model) as Arc<dyn Provider>)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    (daemon, client)
}

/// Sends a prompt and waits until its turn started, so it holds in the model.
async fn running(daemon: &TestDaemon, client: &Client, n: u128) -> PromptSendResult {
    let sent: PromptSendResult = client.call(daemon.prompt(n, "first", TTY)).await.unwrap();
    let mut follow = daemon.follow(client, sent.conversation_id).await.unwrap();
    let turn_id = sent.turn_id;
    events_until(
        &mut follow,
        |e| matches!(e, Event::TurnStarted { turn_id: t, .. } if *t == turn_id),
    )
    .await
    .unwrap();
    sent
}

fn steer(n: u128, conversation_id: ConversationId, turn_id: TurnId, queue: bool) -> Method {
    let if_late = queue.then(|| LateSteer::Queue {
        context: None,
        last_command: None,
        settings: TurnSettings::default(),
    });
    Method::TurnSteer(TurnSteer {
        command_id: command_id(n),
        conversation_id,
        turn_id: Some(turn_id),
        text: format!("steer {n}"),
        if_late,
    })
}

fn withdraw(n: u128, conversation_id: ConversationId, target: WithdrawTarget) -> Method {
    Method::PromptWithdraw(PromptWithdraw { command_id: command_id(n), conversation_id, target })
}

/// Waits until `turn_id` ended, and returns the events of the conversation so far.
async fn until_end(
    daemon: &TestDaemon,
    client: &Client,
    conversation_id: ConversationId,
    turn_id: TurnId,
) -> Vec<EventEnvelope> {
    let mut follow = daemon.follow(client, conversation_id).await.unwrap();
    events_until(&mut follow, |e| {
        e.turn_id() == Some(turn_id)
            && matches!(
                e,
                Event::TurnCompleted { .. }
                    | Event::TurnFailed { .. }
                    | Event::TurnInterrupted { .. }
                    | Event::PromptWithdrawn { .. }
            )
    })
    .await
    .unwrap()
}

fn code(refused: Result<Value, ClientError>) -> ErrorCode {
    match refused {
        Err(ClientError::Server { body }) => body.code,
        other => panic!("a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_late_steer_becomes_a_prompt_and_a_retry_answers_from_its_receipt() {
    let model = Gated::new();
    let (daemon, client) = start(&model).await;
    let sent = running(&daemon, &client, 1).await;
    let conversation_id = sent.conversation_id;
    let stop = Method::TurnInterrupt(TurnInterrupt {
        command_id: command_id(2),
        conversation_id,
        turn_id: Some(sent.turn_id),
        resend_steers: Vec::new(),
        resend_as: None,
        withdraw_steers: Vec::new(),
        withdraw: Vec::new(),
    });
    let _: TurnInterruptResult = client.call(stop).await.unwrap();

    let refused = client.call::<Value>(steer(3, conversation_id, sent.turn_id, false)).await;
    let first: Value = client.call(steer(4, conversation_id, sent.turn_id, true)).await.unwrap();
    let retried: Value = client.call(steer(4, conversation_id, sent.turn_id, true)).await.unwrap();

    assert_eq!(code(refused), ErrorCode::Conflict, "a late steer without if_late");
    assert_eq!(retried, first, "the receipt answers the retry");
    let queued: TurnSteerResult = serde_json::from_value(first).unwrap();
    assert!(queued.queued);
    model.allow(1);
    let events = until_end(&daemon, &client, conversation_id, queued.turn_id).await;
    assert!(!events.iter().any(|e| matches!(e.event, Event::TurnSteered { .. })));
    let prompt = events.iter().find(|e| e.seq == queued.seq).unwrap();
    assert!(
        matches!(&prompt.event, Event::PromptQueued { text, .. } if text == "steer 4"),
        "{prompt:?}"
    );
    assert!(matches!(events.last().unwrap().event, Event::TurnCompleted { .. }));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_late_steer_to_a_conversation_without_an_actor_starts_a_prompt() {
    let model = Gated::new();
    let mut daemon = TestDaemon::builder()
        .persistent()
        .custom_provider(Arc::clone(&model) as Arc<dyn Provider>)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    model.allow(1);
    let sent: PromptSendResult = client.call(daemon.prompt(1, "first", TTY)).await.unwrap();
    until_end(&daemon, &client, sent.conversation_id, sent.turn_id).await;
    drop(client);
    daemon.restart().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let refused = client.call::<Value>(steer(2, sent.conversation_id, sent.turn_id, false)).await;
    let queued: TurnSteerResult =
        client.call(steer(3, sent.conversation_id, sent.turn_id, true)).await.unwrap();

    assert_eq!(code(refused), ErrorCode::Conflict);
    assert!(queued.queued);
    model.allow(1);
    let events = until_end(&daemon, &client, sent.conversation_id, queued.turn_id).await;
    assert!(matches!(events.last().unwrap().event, Event::TurnCompleted { .. }));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_queued_prompt_is_withdrawn_once_and_a_retry_answers_from_its_receipt() {
    let model = Gated::new();
    let (daemon, client) = start(&model).await;
    let sent = running(&daemon, &client, 1).await;
    let conversation_id = sent.conversation_id;
    let second: PromptSendResult = client.call(daemon.prompt(2, "second", TTY)).await.unwrap();
    let third: PromptSendResult = client.call(daemon.prompt(3, "third", TTY)).await.unwrap();

    let by_tty = WithdrawTarget::NewestFromTty { tty: TTY.to_owned() };
    let newest: Value = client.call(withdraw(4, conversation_id, by_tty.clone())).await.unwrap();
    let retried: Value = client.call(withdraw(4, conversation_id, by_tty)).await.unwrap();
    let by_turn = WithdrawTarget::Turn { turn_id: second.turn_id };
    let taken: PromptWithdrawResult =
        client.call(withdraw(5, conversation_id, by_turn.clone())).await.unwrap();
    let again = client.call::<Value>(withdraw(6, conversation_id, by_turn)).await;
    let unknown = WithdrawTarget::Turn { turn_id: TurnId::from_uuid(uuid::Uuid::from_u128(9)) };
    let unknown = client.call::<Value>(withdraw(7, conversation_id, unknown)).await;
    let none_left = WithdrawTarget::NewestFromTty { tty: TTY.to_owned() };
    let none_left = client.call::<Value>(withdraw(8, conversation_id, none_left)).await;

    assert_eq!(retried, newest, "the receipt answers the retry, with the event's seq");
    let newest: PromptWithdrawResult = serde_json::from_value(newest).unwrap();
    assert_eq!(newest.withdrawn.turn_id, third.turn_id);
    assert_eq!(
        (taken.withdrawn.turn_id, taken.withdrawn.text.as_str()),
        (second.turn_id, "second")
    );
    assert_eq!(code(again), ErrorCode::Conflict);
    assert_eq!(code(unknown), ErrorCode::NotFound);
    assert_eq!(code(none_left), ErrorCode::NotFound);
    let events = daemon.events(&client, conversation_id).await.unwrap();
    let event = events.iter().find(|e| e.seq == newest.withdrawn.seq).unwrap();
    assert!(
        matches!(event.event, Event::PromptWithdrawn { turn_id, .. } if turn_id == third.turn_id)
    );
    model.allow(1);
    until_end(&daemon, &client, conversation_id, sent.turn_id).await;
    let events = daemon.events(&client, conversation_id).await.unwrap();
    let started = events.iter().filter(|e| matches!(e.event, Event::TurnStarted { .. })).count();
    assert_eq!(started, 1, "no withdrawn prompt started");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn esc_withdraws_and_resends_in_one_step_and_a_retry_gets_the_same_numbers() {
    let model = Gated::new();
    let (daemon, client) = start(&model).await;
    let sent = running(&daemon, &client, 1).await;
    let conversation_id = sent.conversation_id;
    let steered: TurnSteerResult =
        client.call(steer(2, conversation_id, sent.turn_id, true)).await.unwrap();
    let queued: PromptSendResult = client.call(daemon.prompt(3, "queued", TTY)).await.unwrap();
    let esc = Method::TurnInterrupt(TurnInterrupt {
        command_id: command_id(4),
        conversation_id,
        turn_id: Some(sent.turn_id),
        resend_steers: vec![steered.seq],
        resend_as: None,
        withdraw_steers: Vec::new(),
        withdraw: vec![queued.turn_id],
    });

    let first: Value = client.call(esc.clone()).await.unwrap();
    let retried: Value = client.call(esc).await.unwrap();

    assert_eq!(retried, first, "the nested sequence numbers come back too");
    let result: TurnInterruptResult = serde_json::from_value(first).unwrap();
    let resent = result.resent.unwrap();
    assert_eq!(resent.steers, vec![steered.seq]);
    assert_eq!(result.withdrawn.len(), 1);
    assert_eq!(result.withdrawn[0].text, "queued");
    model.allow(1);
    let events = until_end(&daemon, &client, conversation_id, resent.turn_id).await;
    let by_seq = |seq| events.iter().find(|e| e.seq == seq).map(|e| e.event.clone());
    assert!(matches!(by_seq(result.seq), Some(Event::TurnInterruptRequested { .. })));
    assert!(matches!(by_seq(result.withdrawn[0].seq), Some(Event::PromptWithdrawn { .. })));
    assert_eq!(
        by_seq(resent.seq),
        Some(Event::PromptQueued {
            turn_id: resent.turn_id,
            command_id: command_id(4),
            text: "steer 2".to_owned(),
            origin: efr_protocol::Origin::Shell,
            context: daemon_context(&daemon),
            settings: TurnSettings::default(),
            steers: vec![steered.seq],
        }),
        "the prompt has the context of the interrupted turn's prompt"
    );
    assert!(matches!(events.last().unwrap().event, Event::TurnCompleted { .. }));
    daemon.stop().await.unwrap();
}

/// The context of [`TestDaemon::prompt`] from [`TTY`].
fn daemon_context(daemon: &TestDaemon) -> Option<ShellContext> {
    let mut context = ShellContext::new(daemon.cwd());
    context.tty = Some(TTY.to_owned());
    Some(context)
}
