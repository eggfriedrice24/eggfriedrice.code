//! The remote listener end to end, as the phone app meets it: a test daemon with its
//! remote listener on, a fake tailscaled, and a phone on a real TCP stream.
//!
//! The listener binds a free port of `127.0.0.1` in place of the tailnet address, and
//! the fake whois says which node and user each peer is, as tailscaled's does. So a
//! phone that connects from loopback is the phone's node of the owner unless the test
//! says otherwise. The real listener never binds loopback.
//!
//! The terminal is a client on the Unix socket. It opens the pairing and confirms it,
//! sends the prompt, answers the question that only the user's own machine may answer,
//! and revokes the device. The phone pairs with a software P-256 key, proves it,
//! subscribes with drafts, follows the reply as it streams, and answers a question.

use std::collections::BTreeMap;
use std::net::SocketAddrV4;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, CallId, ConversationId,
    ConversationStatus, ConversationSubscribe, ConversationSubscribeItem, ConversationsList,
    ConversationsListResult, ConversationsWatch, ConversationsWatchItem, DeviceEnroll,
    DeviceEnrollResult, DeviceId, DeviceList, DeviceListResult, DevicePairConfirm,
    DevicePairConfirmResult, DevicePairStart, DevicePairStartItem, DeviceRevoke,
    DeviceRevokeResult, DraftPart, ErrorBody, ErrorCode, Event, HelloResult, Method,
    PROTOCOL_VERSION, PairEnd, PromptSendResult, RequestId, ScopeName, TurnId,
};
use efr_provider::StopReason;
use efr_provider::{Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request};
use efr_test_daemon::{
    Client, FakePhone, ItemStream, PhoneKey, TTY, TestDaemon, TestDaemonError, command_id,
};
use efr_test_support::tailscaled;
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::mpsc;

/// The scopes of a paired phone.
const PHONE_SCOPES: [ScopeName; 3] = [ScopeName::Read, ScopeName::Operate, ScopeName::Approve];

/// The name that the phone enrolls with.
const PHONE_NAME: &str = "test phone";

/// What the refusals of `device.prove` say, the same for every cause.
const NOT_PROVED: &str = "the device could not prove its key";

/// A model whose provider events the test sends one at a time. Each model call
/// streams the events up to its `done`.
#[derive(Debug)]
struct Lockstep {
    id: ProviderId,
    /// The test's events. The stream of a model call takes the receiver and puts it
    /// back at its `done`, so the next call goes on with it.
    events: Arc<Mutex<Option<mpsc::UnboundedReceiver<ProviderEvent>>>>,
}

#[async_trait]
impl Provider for Lockstep {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let receiver = self.events.lock().unwrap().take().expect("one model call at a time");
        let holder = Arc::clone(&self.events);
        let stream = futures::stream::unfold(Some(receiver), move |receiver| {
            let holder = Arc::clone(&holder);
            async move {
                let mut receiver = receiver?;
                let event = receiver.recv().await?;
                if matches!(event, ProviderEvent::Done { .. }) {
                    *holder.lock().unwrap() = Some(receiver);
                    return Some((Ok(event), None));
                }
                Some((Ok(event), Some(receiver)))
            }
        });
        Ok(Box::pin(stream))
    }
}

/// A test daemon with the remote listener on and a model that the test drives.
async fn start() -> (TestDaemon, mpsc::UnboundedSender<ProviderEvent>) {
    let (model, events) = mpsc::unbounded_channel();
    let provider = Lockstep {
        id: ProviderId::new("test").unwrap(),
        events: Arc::new(Mutex::new(Some(events))),
    };
    let daemon = TestDaemon::builder()
        .remote()
        .custom_provider(Arc::new(provider) as Arc<dyn Provider>)
        .config(|config| config.conversation.draft_interval_ms = 0)
        .start()
        .await
        .unwrap();
    (daemon, model)
}

/// The error body of a request that the daemon refused.
fn refused<T: std::fmt::Debug>(result: Result<T, TestDaemonError>) -> ErrorBody {
    match result {
        Err(TestDaemonError::PhoneServer { body }) => body,
        other => panic!("the daemon did not refuse the phone's request: {other:?}"),
    }
}

/// A pairing that the terminal opened, up to the phone's `device.enroll`.
struct Pairing {
    terminal: ItemStream<DevicePairStartItem>,
    pairing_id: efr_protocol::PairingId,
    phone: FakePhone,
    hello: HelloResult,
    enroll: RequestId,
}

/// The terminal opens a pairing; the phone reads the QR code, connects to the address
/// in it, checks the daemon before it spends the token, and enrolls `key`. The
/// terminal then sees the phone's facts.
async fn enroll(daemon: &TestDaemon, terminal: &Client, key: &PhoneKey) -> Pairing {
    let listening = daemon.remote_addr().await.unwrap();
    let mut items = terminal
        .stream::<DevicePairStartItem>(Method::DevicePairStart(DevicePairStart::default()))
        .await
        .unwrap();
    let Some(Ok(DevicePairStartItem::Started { pairing_id, qr, .. })) = items.next().await else {
        panic!("the pairing starts with the QR code");
    };
    assert_eq!(qr.daemon_id, daemon.daemon_id());
    assert_eq!(SocketAddrV4::new(qr.host, qr.port), listening, "the QR names the listener");
    assert_eq!(qr.fqdn.as_deref(), Some("desk.example.ts.net"));
    let url = qr.url();
    assert!(url.expose_secret().starts_with("efr://pair?v=1&d="), "the link the camera opens");

    let mut phone = FakePhone::connect(SocketAddrV4::new(qr.host, qr.port)).await.unwrap();
    let hello = phone.hello(None).await.unwrap();
    assert_eq!((hello.daemon_id, hello.protocol), (qr.daemon_id, PROTOCOL_VERSION));
    assert_eq!(hello.capabilities.device_prove, Some(true));
    let enroll = DeviceEnroll {
        token: qr.token.clone(),
        name: PHONE_NAME.to_owned(),
        public_key: key.public_key(),
    };
    let enroll = phone.send(Method::DeviceEnroll(enroll)).await.unwrap();

    let Some(Ok(DevicePairStartItem::Pending {
        pairing_id: pending,
        device_name,
        node_name,
        user_login,
        key_fingerprint,
        ..
    })) = items.next().await
    else {
        panic!("the terminal sees the phone's facts");
    };
    assert_eq!(pending, pairing_id);
    assert_eq!(
        (device_name.as_str(), node_name.as_str(), user_login.as_str()),
        (PHONE_NAME, "phone.example.ts.net", tailscaled::LOGIN)
    );
    assert_eq!(key_fingerprint, key.fingerprint(), "the terminal shows the key it pins");
    Pairing { terminal: items, pairing_id, phone, hello, enroll }
}

/// Pairs `key` through the terminal's yes; returns the phone, still on its first
/// connection and not yet proved, its hello and the new device.
async fn pair(
    daemon: &TestDaemon,
    terminal: &Client,
    key: &PhoneKey,
) -> (FakePhone, HelloResult, DeviceId) {
    let Pairing { terminal: mut items, pairing_id, mut phone, hello, enroll } =
        enroll(daemon, terminal, key).await;
    let confirmed: DevicePairConfirmResult = terminal
        .call(Method::DevicePairConfirm(DevicePairConfirm { pairing_id, accept: true }))
        .await
        .unwrap();
    let device_id = confirmed.device_id.expect("a yes writes the device");
    let enrolled: DeviceEnrollResult = phone.result(enroll).await.unwrap();
    assert_eq!(enrolled.device_id, device_id);
    assert_eq!(enrolled.scopes, PHONE_SCOPES);
    let Some(Ok(DevicePairStartItem::Paired { device_id: paired, .. })) = items.next().await else {
        panic!("the terminal's pairing ends with the device");
    };
    assert_eq!(paired, device_id);
    assert!(items.next().await.is_none());
    (phone, hello, device_id)
}

/// A phone's subscription and what it built so far, as the app's reducer does.
struct Follow {
    id: RequestId,
    /// The text of each message, from the drafts and the updates.
    texts: BTreeMap<(TurnId, u32), String>,
    /// The completed text of each message, in order.
    completed: Vec<String>,
    drafts: usize,
    /// The question that waits, and whether only the terminal may answer it.
    asked: Option<(CallId, bool)>,
}

impl Follow {
    async fn start(phone: &mut FakePhone, conversation_id: ConversationId) -> Follow {
        let params = ConversationSubscribe {
            conversation_id,
            after_seq: None,
            answers_input: false,
            drafts: true,
        };
        let id = phone.send(Method::ConversationSubscribe(params)).await.unwrap();
        let first: ConversationSubscribeItem = phone.next_item(id).await.unwrap().unwrap();
        assert!(matches!(first, ConversationSubscribeItem::Snapshot(_)), "{first:?}");
        Follow { id, texts: BTreeMap::new(), completed: Vec::new(), drafts: 0, asked: None }
    }

    /// The text of every message so far, joined.
    fn text(&self) -> String {
        self.texts.values().map(String::as_str).collect()
    }

    /// Reads the phone's items until `done` accepts the state after an item, with the
    /// event of the item (`None` for a draft).
    async fn until(
        &mut self,
        phone: &mut FakePhone,
        done: impl Fn(&Follow, Option<&Event>) -> bool,
    ) {
        loop {
            let item: ConversationSubscribeItem =
                phone.next_item(self.id).await.unwrap().expect("the subscription ended");
            match item {
                ConversationSubscribeItem::Draft(draft) => {
                    self.drafts += 1;
                    if let DraftPart::Text { index, offset, delta } = draft.draft {
                        let text = self.texts.entry((draft.turn_id, index)).or_default();
                        join_at(text, offset, &delta);
                    }
                    if done(self, None) {
                        return;
                    }
                }
                ConversationSubscribeItem::Event(envelope) => {
                    match &envelope.event {
                        Event::AssistantMessageUpdated { turn_id, index, offset, delta } => {
                            let text = self.texts.entry((*turn_id, *index)).or_default();
                            join_at(text, *offset, delta);
                        }
                        Event::AssistantMessageCompleted { turn_id, index, text } => {
                            let built = self.texts.entry((*turn_id, *index)).or_default();
                            assert_eq!(built, text, "the drafts build the completed text");
                            self.completed.push(text.clone());
                        }
                        Event::ApprovalRequested { call_id, local_only, .. } => {
                            self.asked = Some((*call_id, *local_only));
                        }
                        _ => {}
                    }
                    if done(self, Some(&envelope.event)) {
                        return;
                    }
                }
                other => panic!("an item of no known kind: {other:?}"),
            }
        }
    }

    /// Reads until the phone's text holds `text`.
    async fn until_text(&mut self, phone: &mut FakePhone, text: &str) {
        self.until(phone, |follow, _| follow.text() == text).await;
    }

    /// Reads until an event of `kind`.
    async fn until_event(&mut self, phone: &mut FakePhone, kind: &str) {
        self.until(phone, |_, event| event.is_some_and(|event| event.kind() == kind)).await;
    }
}

/// Appends `delta` at byte `offset` of `text`; the part that `text` already has is cut
/// off, and a gap fails the test.
fn join_at(text: &mut String, offset: u64, delta: &str) {
    let have = text.len() as u64;
    assert!(offset <= have, "a gap at {offset}, the text has {have} bytes");
    let skip = usize::try_from(have - offset).unwrap();
    if let Some(rest) = delta.get(skip..) {
        text.push_str(rest);
    }
}

/// The model writes `text` in deltas of a few words; the test waits until the phone
/// built each one from its drafts, so every delta reaches the phone.
async fn stream_text(
    model: &mpsc::UnboundedSender<ProviderEvent>,
    phone: &mut FakePhone,
    follow: &mut Follow,
    sent: &mut String,
    text: &str,
) {
    for piece in text.split_inclusive(' ') {
        model.send(ProviderEvent::TextDelta { text: piece.to_owned() }).unwrap();
        sent.push_str(piece);
        follow.until_text(phone, sent).await;
    }
}

/// The model calls `tool` with `input` and ends its answer for the call.
fn call_tool(model: &mpsc::UnboundedSender<ProviderEvent>, tool: &str, input: &serde_json::Value) {
    let call_id = format!("call_{tool}");
    let start = ProviderEvent::ToolCallStart {
        call_id: call_id.clone(),
        name: tool.to_owned(),
        freeform: false,
    };
    model.send(start).unwrap();
    model.send(ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() }).unwrap();
    model
        .send(ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None })
        .unwrap();
}

fn answer(
    n: u128,
    conversation_id: ConversationId,
    call_id: CallId,
    decision: ApprovalDecision,
) -> Method {
    Method::ApprovalRespond(ApprovalRespond {
        command_id: command_id(n),
        conversation_id,
        call_id,
        decision,
    })
}

const NOTES: &str = "# Notes\n\n- buy milk\n";
const NEW_NOTES: &str = "# Notes\n\n- buy milk and bread\n";

#[tokio::test]
async fn a_phone_pairs_proves_follows_a_streamed_reply_answers_a_question_and_is_revoked() {
    let (daemon, model) = start().await;
    let terminal = daemon.client_for_tty(TTY).await.unwrap();
    let key = PhoneKey::generate().unwrap();
    let (mut phone, hello, device_id) = pair(&daemon, &terminal, &key).await;

    // The app proves on the connection that enrolled, as on each later one.
    let proved = phone.prove(&key, &hello, device_id).await.unwrap();
    assert_eq!(proved.scopes, PHONE_SCOPES);
    let listed: DeviceListResult =
        terminal.call(Method::DeviceList(DeviceList::default())).await.unwrap();
    let [device] = listed.devices.as_slice() else { panic!("one device: {listed:?}") };
    assert_eq!((device.device_id, device.name.as_str()), (device_id, PHONE_NAME));
    assert_eq!(device.node, tailscaled::PHONE_NODE);
    assert_eq!(device.key_fingerprint, key.fingerprint());
    assert!(device.last_seen_at.is_some(), "the proof marks the device as seen");
    let admin = phone.call::<DeviceListResult>(Method::DeviceList(DeviceList::default())).await;
    assert_eq!(refused(admin).code, ErrorCode::Forbidden, "a phone is never admin");
    let routed = phone.call::<PromptSendResult>(daemon.prompt(9, "hi", TTY)).await;
    assert_eq!(refused(routed).code, ErrorCode::Invalid, "a phone sits at no terminal");

    // The terminal starts a turn; the phone finds it and follows it with drafts.
    std::fs::write(daemon.cwd().join("notes.md"), NOTES).unwrap();
    let sent: PromptSendResult = terminal
        .call(daemon.prompt(1, "update my notes and restart the server", TTY))
        .await
        .unwrap();
    let conversation_id = sent.conversation_id;
    let list: ConversationsListResult =
        phone.call(Method::ConversationsList(ConversationsList::default())).await.unwrap();
    assert!(list.conversations.iter().any(|summary| summary.id == conversation_id));
    let mut follow = Follow::start(&mut phone, conversation_id).await;
    let mut text = String::new();
    stream_text(&model, &mut phone, &mut follow, &mut text, "I change the notes now. ").await;
    assert!(follow.drafts > 0, "the reply streams to the phone as drafts");

    // A question that a phone may answer.
    let notes = json!({ "path": "notes.md", "content": NEW_NOTES });
    call_tool(&model, "write_file", &notes);
    follow.until_event(&mut phone, "approval_requested").await;
    let (call_id, local_only) = follow.asked.take().unwrap();
    assert!(!local_only);
    let allowed: ApprovalRespondResult =
        phone.call(answer(2, conversation_id, call_id, ApprovalDecision::Allow)).await.unwrap();
    assert!(allowed.seq.get() > 0);
    follow.until_event(&mut phone, "tool_call_completed").await;
    assert_eq!(std::fs::read_to_string(daemon.cwd().join("notes.md")).unwrap(), NEW_NOTES);

    // A question that only the user's own machine may answer.
    stream_text(&model, &mut phone, &mut follow, &mut text, "Now I restart the server. ").await;
    call_tool(&model, "shell", &json!({ "command": "sudo systemctl restart nginx" }));
    follow.until_event(&mut phone, "approval_requested").await;
    let (call_id, local_only) = follow.asked.take().unwrap();
    assert!(local_only, "a privileged command asks the terminal");
    let from_phone = phone
        .call::<ApprovalRespondResult>(answer(3, conversation_id, call_id, ApprovalDecision::Allow))
        .await;
    assert_eq!(refused(from_phone).code, ErrorCode::Forbidden);
    let denied: ApprovalRespondResult =
        terminal.call(answer(4, conversation_id, call_id, ApprovalDecision::Deny)).await.unwrap();
    assert!(denied.seq.get() > allowed.seq.get());
    follow.until_event(&mut phone, "tool_call_completed").await;

    stream_text(&model, &mut phone, &mut follow, &mut text, "The server still runs.").await;
    model
        .send(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None })
        .unwrap();
    follow.until_event(&mut phone, "turn_completed").await;
    assert_eq!(follow.completed.concat(), text, "every message completed with its text");
    assert_eq!(follow.text(), text, "the drafts and the updates hold every message");

    // A revoke closes the live socket at once, and the next proof fails.
    let revoked: DeviceRevokeResult =
        terminal.call(Method::DeviceRevoke(DeviceRevoke { device_id })).await.unwrap();
    phone.closed().await.unwrap();
    let mut next = FakePhone::connect(daemon.remote_addr().await.unwrap()).await.unwrap();
    let hello = next.hello(Some(device_id)).await.unwrap();
    let again = next.prove(&key, &hello, device_id).await;
    let body = refused(again);
    assert_eq!((body.code, body.message.as_str()), (ErrorCode::Unauthorized, NOT_PROVED));
    let listed: DeviceListResult =
        terminal.call(Method::DeviceList(DeviceList::default())).await.unwrap();
    assert_eq!(listed.devices[0].revoked_at, Some(revoked.revoked_at));

    drop((next, terminal));
    daemon.stop().await.unwrap();
}

/// Reads the rows of the phone's watch `id` until one of `conversation_id` has
/// `status`.
async fn watched_until(
    phone: &mut FakePhone,
    id: RequestId,
    conversation_id: ConversationId,
    status: ConversationStatus,
) {
    loop {
        let item: ConversationsWatchItem =
            phone.next_item(id).await.unwrap().expect("the watch ended");
        let ConversationsWatchItem::Changed { conversation } = item else {
            panic!("a row, not {item:?}");
        };
        assert_eq!(conversation.summary.id, conversation_id);
        if conversation.summary.status == status {
            return;
        }
    }
}

#[tokio::test]
async fn a_proved_phone_watches_a_turn_that_asks_it_a_question() {
    let (daemon, model) = start().await;
    let terminal = daemon.client_for_tty(TTY).await.unwrap();
    let key = PhoneKey::generate().unwrap();
    let (mut phone, hello, device_id) = pair(&daemon, &terminal, &key).await;
    phone.prove(&key, &hello, device_id).await.unwrap();
    assert_eq!(hello.capabilities.conversations_watch, Some(true));

    let watch = phone.send(Method::ConversationsWatch(ConversationsWatch::default())).await;
    let watch = watch.unwrap();
    let first: ConversationsWatchItem = phone.next_item(watch).await.unwrap().unwrap();
    assert_eq!(first, ConversationsWatchItem::Snapshot { conversations: Vec::new() });

    std::fs::write(daemon.cwd().join("notes.md"), NOTES).unwrap();
    let sent: PromptSendResult =
        terminal.call(daemon.prompt(1, "update my notes", TTY)).await.unwrap();
    let conversation_id = sent.conversation_id;
    watched_until(&mut phone, watch, conversation_id, ConversationStatus::Running).await;

    // The question shows in the list of the phone, and its answer takes it away.
    let notes = json!({ "path": "notes.md", "content": NEW_NOTES });
    call_tool(&model, "write_file", &notes);
    watched_until(&mut phone, watch, conversation_id, ConversationStatus::AwaitingApproval).await;
    let events = daemon.events(&terminal, conversation_id).await.unwrap();
    let call_id = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::ApprovalRequested { call_id, .. } => Some(*call_id),
            _ => None,
        })
        .unwrap();
    let _: ApprovalRespondResult =
        phone.call(answer(2, conversation_id, call_id, ApprovalDecision::Allow)).await.unwrap();
    watched_until(&mut phone, watch, conversation_id, ConversationStatus::Running).await;

    model.send(ProviderEvent::TextDelta { text: "Done.".to_owned() }).unwrap();
    model
        .send(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None })
        .unwrap();
    watched_until(&mut phone, watch, conversation_id, ConversationStatus::Idle).await;

    // A new watch starts with the conversation.
    let again = phone.send(Method::ConversationsWatch(ConversationsWatch::default())).await;
    let again = again.unwrap();
    let Some(ConversationsWatchItem::Snapshot { conversations }) =
        phone.next_item(again).await.unwrap()
    else {
        panic!("a watch starts with a snapshot");
    };
    let [row] = conversations.as_slice() else { panic!("one row: {conversations:?}") };
    assert_eq!((row.summary.id, row.summary.status), (conversation_id, ConversationStatus::Idle));
    assert_eq!(std::fs::read_to_string(daemon.cwd().join("notes.md")).unwrap(), NEW_NOTES);
    drop((phone, terminal));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_pairing_that_the_terminal_refuses_writes_no_device() {
    let (daemon, _model) = start().await;
    let terminal = daemon.client().await.unwrap();
    let key = PhoneKey::generate().unwrap();
    let Pairing { terminal: mut items, pairing_id, mut phone, enroll, .. } =
        enroll(&daemon, &terminal, &key).await;
    let confirmed: DevicePairConfirmResult = terminal
        .call(Method::DevicePairConfirm(DevicePairConfirm { pairing_id, accept: false }))
        .await
        .unwrap();
    assert_eq!(confirmed.device_id, None);
    let body = refused(phone.result::<DeviceEnrollResult>(enroll).await);
    assert_eq!(body.code, ErrorCode::Forbidden, "{body:?}");
    let Some(Ok(DevicePairStartItem::Ended { reason, .. })) = items.next().await else {
        panic!("the terminal's pairing ends");
    };
    assert_eq!(reason, PairEnd::Refused);
    let listed: DeviceListResult =
        terminal.call(Method::DeviceList(DeviceList::default())).await.unwrap();
    assert!(listed.devices.is_empty());
    drop((phone, terminal));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn the_listener_refuses_other_nodes_and_users_with_no_byte_and_other_origins() {
    let (daemon, _model) = start().await;
    let addr = daemon.remote_addr().await.unwrap();
    let fake = daemon.tailscaled().unwrap();
    let peers = [
        ("the self node", Some(tailscaled::whois(tailscaled::SELF_NODE, tailscaled::USER, &[]))),
        ("a tagged node", Some(tailscaled::whois("nTAGGEDCNTRL", tailscaled::USER, &["tag:ci"]))),
        ("a sharee node", Some(tailscaled::sharee_whois("nSHAREDCNTRL", tailscaled::USER))),
        (
            "another user",
            Some(tailscaled::whois(tailscaled::PHONE_NODE, tailscaled::USER + 1, &[])),
        ),
        ("an unknown peer", None),
    ];
    for (peer, whois) in peers {
        fake.set_whois(whois);
        let status = FakePhone::handshake(addr, Some(&format!("http://{addr}"))).await.unwrap();
        assert_eq!(status, "", "{peer} gets no byte");
    }

    fake.set_whois(Some(tailscaled::whois(tailscaled::PHONE_NODE, tailscaled::USER, &[])));
    let port = addr.port();
    for origin in [
        "http://evil.example".to_owned(),
        "null".to_owned(),
        format!("http://localhost:{port}"),
        format!("https://{addr}"),
        format!("http://{addr}.evil.example"),
    ] {
        let refused = FakePhone::connect_with_origin(addr, Some(&origin)).await;
        let Err(TestDaemonError::PhoneRefused { status }) = refused else {
            panic!("{origin} opens no socket");
        };
        assert!(status.starts_with("HTTP/1.1 403"), "{origin}: {status}");
    }
    let mut native = FakePhone::connect_with_origin(addr, None).await.unwrap();
    assert!(native.hello(None).await.unwrap().prove.is_some(), "the app sends no Origin");
    let mut web = FakePhone::connect(addr).await.unwrap();
    assert!(web.hello(None).await.unwrap().prove.is_some(), "or the listener's own");
    let whois = fake.seen().iter().filter(|target| target.contains("/whois?")).count();
    assert!(whois >= 12, "every connection asks the whois first: {whois}");
    drop((native, web));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_wrong_signature_or_a_replayed_challenge_proves_nothing() {
    let (daemon, _model) = start().await;
    let terminal = daemon.client().await.unwrap();
    let key = PhoneKey::generate().unwrap();
    let (mut first, first_hello, device_id) = pair(&daemon, &terminal, &key).await;
    let signature = key.sign(&first_hello, device_id).unwrap();

    let mut second = FakePhone::connect(daemon.remote_addr().await.unwrap()).await.unwrap();
    let second_hello = second.hello(Some(device_id)).await.unwrap();
    assert_ne!(second_hello.challenge, first_hello.challenge, "each connection has its own");
    let wrong_key = PhoneKey::generate().unwrap();
    let wrong = second.prove(&wrong_key, &second_hello, device_id).await;
    let body = refused(wrong);
    assert_eq!((body.code, body.message.as_str()), (ErrorCode::Unauthorized, NOT_PROVED));
    let replayed = second
        .call::<serde_json::Value>(Method::DeviceProve(efr_protocol::DeviceProve {
            device_id,
            signature: signature.clone(),
        }))
        .await;
    let body = refused(replayed);
    assert_eq!((body.code, body.message.as_str()), (ErrorCode::Unauthorized, NOT_PROVED));

    // On its own connection the signature proves the key, once.
    let proved = first.prove(&key, &first_hello, device_id).await.unwrap();
    assert_eq!(proved.scopes, PHONE_SCOPES);
    let twice = first
        .call::<serde_json::Value>(Method::DeviceProve(efr_protocol::DeviceProve {
            device_id,
            signature,
        }))
        .await;
    assert_eq!(refused(twice).code, ErrorCode::Conflict, "one proof per connection");

    // The third refusal closes the socket.
    let signature = wrong_key.sign(&second_hello, device_id).unwrap();
    second
        .send(Method::DeviceProve(efr_protocol::DeviceProve { device_id, signature }))
        .await
        .unwrap();
    second.closed().await.unwrap();
    drop((first, terminal));
    daemon.stop().await.unwrap();
}
