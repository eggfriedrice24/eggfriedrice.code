//! The model learns the outcome of each port of its jobs (v0.0.4 L2): a note of efr in
//! its next request, also in the middle of the turn that started the job, once for each
//! outcome, and the outcome of each port in the `jobs` tool. The model of these tests
//! waits at a gate of the test before it answers one request, so the test makes the
//! forward happen between two requests of one turn.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use efr_conversation::NOT_USER_NOTE;
use efr_protocol::{
    ConversationId, Event, EventEnvelope, ForwardOutcome, Method, Mode, PromptSendResult,
    TurnSettings,
};
use efr_provider::{
    ContentBlock, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request,
    StopReason,
};
use efr_test_daemon::{TTY, TestDaemon, echo_server};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::{Notify, Semaphore};

use super::{
    Forwarded, LOCALHOST, Setup, forwarded, outputs, release, reload_with, through, until, wait_on,
};
use crate::sandbox::REVIEWER_ALLOWS;

/// The note of a port that a program of the user's machine holds, as the model reads it.
const IN_USE_NOTE: &str = "efr: job j1 listens on 127.0.0.1:8767; not forwarded: \
    127.0.0.1:8767 is in use on this machine. The user cannot open it in a browser, and your \
    calls do not reach it; choose another port.";

/// The note of a forwarded port, as the model reads it.
const FORWARDED_NOTE: &str = "efr: job j1 listens on 127.0.0.1:8765; forwarded: the user can \
    open http://localhost:8765, and your contained calls reach it at 127.0.0.1:8765.";

/// The note of a port that a reload stopped, as the model reads it.
const STOPPED_NOTE: &str = "efr: job j1: port 8765 is no longer forwarded \
    (jobs.forward_ports is off). The user cannot open it now, and your contained calls do not \
    reach it.";

/// A model that makes each call of `calls` in turn, then says `done`, and keeps every
/// request of a turn. At the request `gate_at` it tells [`reached`](Self::reached) and
/// waits until the test opens the gate; the request is built already then.
#[derive(Debug)]
struct GatedModel {
    id: ProviderId,
    calls: Vec<(String, Value)>,
    gate_at: Option<usize>,
    reached: Notify,
    gate: Semaphore,
    asked: AtomicUsize,
    requests: Mutex<Vec<Request>>,
}

impl GatedModel {
    fn new(calls: Vec<(&str, Value)>, gate_at: Option<usize>) -> Arc<GatedModel> {
        Arc::new(GatedModel {
            id: ProviderId::new("test").unwrap(),
            calls: calls.into_iter().map(|(name, input)| (name.to_owned(), input)).collect(),
            gate_at,
            reached: Notify::new(),
            gate: Semaphore::new(0),
            asked: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        })
    }

    /// Every request of a turn so far, in order.
    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Lets the request at the gate go on.
    fn open(&self) {
        self.gate.add_permits(1);
    }
}

#[async_trait]
impl Provider for GatedModel {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        if request.one_shot {
            let events = vec![
                ProviderEvent::TextDelta { text: REVIEWER_ALLOWS.to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ];
            return Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))));
        }
        let n = self.asked.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        if self.gate_at == Some(n) {
            self.reached.notify_one();
            self.gate.acquire().await.unwrap().forget();
        }
        let events = match self.calls.get(n) {
            Some((name, input)) => {
                let call_id = format!("call_{n}");
                vec![
                    ProviderEvent::ToolCallStart {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        freeform: false,
                    },
                    ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
                    ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
                ]
            }
            None => vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ],
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// How often `text` is in the text blocks of `request`.
fn count_in(request: &Request, text: &str) -> usize {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .map(|block| block.matches(text).count())
        .sum()
}

/// Sends the prompt `text` in `auto` from the test's terminal, with the command id `n`;
/// it does not wait for the turn.
async fn send(daemon: &TestDaemon, n: u128, text: &str) -> ConversationId {
    let client = daemon.client().await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(n, text, TTY) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    sent.conversation_id
}

/// The events of `conversation` up to the end of its `nth` turn.
async fn until_end(
    daemon: &TestDaemon,
    conversation: ConversationId,
    nth: usize,
) -> Vec<EventEnvelope> {
    let mut ends = 0;
    until(daemon, conversation, |event| {
        if matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. }) {
            ends += 1;
        }
        ends == nth
    })
    .await
}

/// Sends the prompt `text` with the command id `n`, the conversation's `n`th turn, and
/// waits for its end.
async fn turn(daemon: &TestDaemon, n: u128, text: &str) -> ConversationId {
    let conversation = send(daemon, n, text).await;
    until_end(daemon, conversation, usize::try_from(n).unwrap()).await;
    conversation
}

#[tokio::test]
async fn port_in_use_note_reaches_the_same_turn() {
    // NOTE: the live test of 2026-10-22: a server of the user holds 8767, and the
    // model's job listens there. The forward's outcome comes after the job started and
    // before the model's next step.
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let job = json!({ "command": wait_on(&fifo), "background": true });
    let model = GatedModel::new(vec![("shell", job), ("jobs", json!({}))], Some(1));
    let daemon = setup.builder_with(Arc::clone(&model) as Arc<dyn Provider>).start().await.unwrap();
    daemon.host_ports().set_in_use(8767);
    let conversation = send(&daemon, 1, "start a server on 8767").await;
    model.reached.notified().await;
    let mut forwarder = setup.forwarder().await;
    forwarder.listen(LOCALHOST, 8767, None).await.unwrap();
    let seen = forwarded(&daemon, conversation, 1).await;
    assert_eq!(seen, [Forwarded::new("j1", "127.0.0.1", 8767, ForwardOutcome::InUse)]);
    model.open();
    let events = until_end(&daemon, conversation, 1).await;
    assert!(matches!(events.last().unwrap().event, Event::TurnCompleted { .. }), "{events:?}");

    let requests = model.requests();
    assert_eq!(requests.len(), 3, "the job, the jobs tool, the answer");
    assert_eq!(count_in(&requests[1], IN_USE_NOTE), 0, "built before the outcome");
    assert_eq!(count_in(&requests[2], IN_USE_NOTE), 1, "the next request of the same turn");
    let last = requests[2].messages.last().unwrap();
    assert_eq!(
        last.content,
        [ContentBlock::Text { text: format!("{NOT_USER_NOTE}\n{IN_USE_NOTE}") }],
        "a message of efr, not of the user"
    );
    let listed = &outputs(&events)[1];
    assert!(
        listed.contains("ports 8767 not forwarded: 127.0.0.1:8767 is in use on this machine"),
        "the jobs tool shows the port: {listed}"
    );

    // NOTE: the next turn has the note in its history only: the model reads it once.
    turn(&daemon, 2, "and now?").await;
    let requests = model.requests();
    assert_eq!(requests.len(), 4);
    let next = requests.last().unwrap();
    assert_eq!(count_in(next, IN_USE_NOTE), 1, "no second note of the same outcome");
    assert_eq!(count_in(next, NOT_USER_NOTE), 1);
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn forwarded_note_reaches_the_next_turn() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let job = json!({ "command": wait_on(&fifo), "background": true });
    let model = GatedModel::new(vec![("shell", job)], None);
    let daemon = setup.builder_with(Arc::clone(&model) as Arc<dyn Provider>).start().await.unwrap();
    let conversation = turn(&daemon, 1, "start the server").await;
    let mut forwarder = setup.forwarder().await;
    let echo = echo_server().await.unwrap();
    forwarder.listen(LOCALHOST, 8765, Some(echo)).await.unwrap();
    let seen = forwarded(&daemon, conversation, 1).await;
    assert_eq!(seen, [Forwarded::new("j1", "127.0.0.1", 8765, ForwardOutcome::Forwarded)]);
    assert!(model.requests().iter().all(|request| count_in(request, FORWARDED_NOTE) == 0));

    turn(&daemon, 2, "is it up?").await;
    let requests = model.requests();
    assert_eq!(requests.len(), 3, "the job, the answer, the next turn's answer");
    assert_eq!(count_in(&requests[2], FORWARDED_NOTE), 1, "the first request after it");
    // NOTE: the same port again after a gone is the same forward: efrd records no event
    // for it, so the model gets no new note.
    let real = daemon.host_ports().real_port(8765).unwrap();
    forwarder.gone(LOCALHOST, 8765).await.unwrap();
    forwarder.listen(LOCALHOST, 8765, Some(echo)).await.unwrap();
    Wait::new("the port is back")
        .until_some_async(async || through(real, b"back").await.ok().filter(|b| b == b"back"))
        .await
        .unwrap();
    turn(&daemon, 3, "again?").await;
    let requests = model.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(count_in(requests.last().unwrap(), FORWARDED_NOTE), 1, "only in the history");
    assert_eq!(count_in(requests.last().unwrap(), NOT_USER_NOTE), 1);
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn reload_off_note_reaches_the_model() {
    // NOTE: the review of 2026-10-22: the forwards closed a second time after the
    // reload and forgot the `stopped` notes before the model read them.
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let job = json!({ "command": wait_on(&fifo), "background": true });
    let model = GatedModel::new(vec![("shell", job), ("jobs", json!({}))], Some(1));
    let daemon = setup.builder_with(Arc::clone(&model) as Arc<dyn Provider>).start().await.unwrap();
    let conversation = send(&daemon, 1, "start the server").await;
    model.reached.notified().await;
    let mut forwarder = setup.forwarder().await;
    forwarder.listen(LOCALHOST, 8765, None).await.unwrap();
    forwarded(&daemon, conversation, 1).await;
    reload_with(&setup, &daemon, "[jobs]\nforward_ports = false\n").await;
    let seen = forwarded(&daemon, conversation, 2).await;
    assert_eq!(seen[1].outcome, ForwardOutcome::Stopped, "{seen:?}");
    forwarder.closed().await;
    model.open();
    let events = until_end(&daemon, conversation, 1).await;
    assert!(matches!(events.last().unwrap().event, Event::TurnCompleted { .. }), "{events:?}");

    let requests = model.requests();
    assert_eq!(requests.len(), 3, "the job, the jobs tool, the answer");
    assert_eq!(count_in(&requests[1], STOPPED_NOTE), 0, "built before the reload");
    assert_eq!(count_in(&requests[2], STOPPED_NOTE), 1, "once, in the next request");
    assert_eq!(count_in(&requests[2], FORWARDED_NOTE), 1, "the outcome before it");
    let listed = &outputs(&events)[1];
    assert!(
        listed.contains("ports 8765 no longer forwarded (jobs.forward_ports is off)"),
        "the jobs tool shows the port: {listed}"
    );
    release(&fifo).await;
    daemon.stop().await.unwrap();
}
