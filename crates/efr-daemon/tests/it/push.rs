//! Pushes to the paired phones over a `TestDaemon`, against a fake Expo push service
//! (`TestDaemonBuilder::expo_push`), never the real one: the capability of `hello`,
//! `device.push_set`, the rule of when to push, the repeats, the retries, the dead
//! tokens and `admin.push_test`.
//!
//! The daemon runs on a manual clock. A question pushes when its follower or its turn
//! finds you away; a turn that a phone started pushes at its end, here after 0 seconds
//! (`notify.after_secs = 0`). The look of a waiting turn, the waits between the tries
//! of a request, the receipts and the waits of the push test end only when the clock
//! moves, so a test waits until the daemon starts each sleep and then moves the clock
//! to its end ([`pass`]).

use std::time::Duration;

use efr_conversation::PRESENCE_CHECK;
use efr_http::ExpoPush;
use efr_protocol::{
    AdminPushTest, AdminPushTestResult, ApprovalDecision, ApprovalRespond, ApprovalRespondResult,
    CallId, ConversationId, ConversationSubscribe, ConversationSubscribeItem, DeviceId, DeviceList,
    DeviceListResult, DevicePushSet, DevicePushSetResult, DeviceRevoke, DeviceRevokeResult,
    ErrorCode, Event, LeaseReport, LeaseReportResult, Method, Presence, PresenceSet,
    PresenceSetResult, PromptSendResult, PushTestResult, PushTestRow, SecretText,
};
use efr_stdx::text::notice_place;
use efr_test_daemon::{
    Client, ClientError, FakePhone, ItemStream, PairedPhone, TTY, TestDaemon, TestDaemonError,
    command_id, events_until,
};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::sandbox::{REVIEWER_ALLOWS, ScriptedModel};
use crate::support;

/// The Expo push token of phone `n`.
fn token(n: usize) -> String {
    format!("ExponentPushToken[phone{n}]")
}

/// The ticket id that the fake gives the push to phone `n`.
fn ticket_id(n: usize) -> String {
    format!("id-phone{n}")
}

/// The phone of `token`.
fn phone_of(token: &str) -> usize {
    let inside = token.strip_prefix("ExponentPushToken[phone").and_then(|t| t.strip_suffix(']'));
    inside.unwrap().parse().unwrap()
}

/// The fake's answer to a send: an `ok` ticket for each message, or the error code of
/// `refused` for a message to its phone. The message of an error holds the token, as
/// the example of the Expo docs does.
fn tickets(refused: &'static [(usize, &'static str)]) -> impl Fn(&Request) -> ResponseTemplate {
    move |request: &Request| {
        let messages: Vec<Value> = serde_json::from_slice(&request.body).unwrap();
        let tickets: Vec<Value> = messages
            .iter()
            .map(|message| {
                let to = message["to"].as_str().unwrap();
                match refused.iter().find(|(phone, _)| *phone == phone_of(to)) {
                    Some((_, code)) => json!({
                        "status": "error",
                        "message": format!("\"{to}\" is not a registered push notification recipient"),
                        "details": {"error": code}
                    }),
                    None => json!({"status": "ok", "id": ticket_id(phone_of(to))}),
                }
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "data": tickets }))
    }
}

/// The fake's answer to a read of receipts: `receipts` for its phones, where `null` is
/// no receipt yet, and `ok` for each other id.
fn receipts(receipts: Vec<(usize, Value)>) -> impl Fn(&Request) -> ResponseTemplate {
    move |request: &Request| {
        let asked: Value = serde_json::from_slice(&request.body).unwrap();
        let mut data = serde_json::Map::new();
        for id in asked["ids"].as_array().unwrap() {
            let id = id.as_str().unwrap();
            let receipt = match receipts.iter().find(|(n, _)| ticket_id(*n) == id) {
                Some((_, receipt)) => receipt.clone(),
                None => json!({"status": "ok"}),
            };
            if !receipt.is_null() {
                data.insert(id.to_owned(), receipt);
            }
        }
        ResponseTemplate::new(200).set_body_json(json!({ "data": data }))
    }
}

/// A fake Expo push service with [`tickets`] of `refused` and [`receipts`] of `given`.
async fn expo(refused: &'static [(usize, &'static str)], given: Vec<(usize, Value)>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path(ExpoPush::SEND_PATH)).respond_with(tickets(refused)).mount(&server).await;
    Mock::given(path(ExpoPush::RECEIPTS_PATH)).respond_with(receipts(given)).mount(&server).await;
    server
}

/// The bodies of the sends that `server` got, in order.
async fn sends(server: &MockServer) -> Vec<Value> {
    let requests = server.received_requests().await.unwrap();
    requests
        .iter()
        .filter(|request| request.url.path() == ExpoPush::SEND_PATH)
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect()
}

/// Waits until `server` got `count` sends, and returns them.
async fn sends_until(server: &MockServer, count: usize) -> Vec<Value> {
    Wait::new("the sends of the fake Expo push service")
        .until_some_async(async || {
            let sent = sends(server).await;
            (sent.len() >= count).then_some(sent)
        })
        .await
        .unwrap()
}

/// Waits until the daemon starts a sleep that `of` takes, after the first `after`
/// sleeps of its clock, and moves the clock to its end; returns how many sleeps the
/// clock had then. A move before the sleep starts would not end it, and a move by more
/// could end the timeout of a request that is on its way.
async fn pass(daemon: &TestDaemon, after: usize, of: impl Fn(Duration) -> bool) -> usize {
    let clock = daemon.clock();
    let (index, wait) = Wait::new("a sleep of the daemon")
        .until_some(|| {
            let sleeps = clock.requested_sleeps();
            sleeps.iter().copied().enumerate().skip(after).find(|(_, wait)| of(*wait))
        })
        .await
        .unwrap();
    clock.advance(wait);
    index + 1
}

/// The answer of `admin.push_test` from `terminal`, while the clock of `daemon` passes
/// one sleep of each of `waits`, in order.
async fn push_test(
    daemon: &TestDaemon,
    terminal: &Client,
    waits: &[&dyn Fn(Duration) -> bool],
) -> AdminPushTestResult {
    let before = sleeps(daemon);
    let call = terminal.call::<AdminPushTestResult>(Method::AdminPushTest(AdminPushTest {}));
    let clock = async {
        let mut after = before;
        for wait in waits {
            after = pass(daemon, after, wait).await;
        }
    };
    let (tested, ()) = tokio::join!(call, clock);
    tested.unwrap()
}

/// How many sleeps the clock of `daemon` started so far.
fn sleeps(daemon: &TestDaemon) -> usize {
    daemon.clock().requested_sleeps().len()
}

/// A wait between two tries of a request: at most `most`.
fn try_wait(most: u64) -> impl Fn(Duration) -> bool {
    move |wait| !wait.is_zero() && wait <= Duration::from_secs(most)
}

/// The tokens of the messages of `send`.
fn tokens_of(send: &Value) -> Vec<String> {
    let messages = send.as_array().unwrap();
    messages.iter().map(|message| message["to"].as_str().unwrap().to_owned()).collect()
}

/// A daemon with the remote listener on, its pushes to `expo`, and a model that calls
/// `write_file` `writes` times, each one a question for the user, and then answers.
async fn start(expo: &MockServer, writes: usize) -> TestDaemon {
    let calls = (0..writes)
        .map(|n| ("write_file", json!({"path": format!("note{n}.md"), "content": "x"})))
        .collect();
    TestDaemon::builder()
        .remote()
        .expo_push(expo.uri())
        .custom_provider(ScriptedModel::reviewed(calls, REVIEWER_ALLOWS))
        .config(|config| config.notify.after_secs = 0)
        .start()
        .await
        .unwrap()
}

/// Pairs `count` phones, each with its push token on.
async fn phones(daemon: &TestDaemon, count: usize) -> Vec<PairedPhone> {
    let mut phones = Vec::new();
    for _ in 0..count {
        phones.push(PairedPhone::pair(daemon).await.unwrap());
    }
    // NOTE: the clock stands, so the phones paired at one time, and efrd lists them by
    // their id then; phone n is the nth of that list.
    phones.sort_by_key(|phone| phone.device_id);
    for (n, phone) in (1..).zip(&mut phones) {
        set_token(phone, Some(&token(n))).await.unwrap();
    }
    phones
}

async fn set_token(phone: &mut PairedPhone, token: Option<&str>) -> Result<(), TestDaemonError> {
    let params = DevicePushSet { token: token.map(SecretText::new) };
    let _: DevicePushSetResult = phone.phone.call(Method::DevicePushSet(params)).await?;
    Ok(())
}

/// Whether each device has push on, by its id.
async fn push_on(terminal: &Client) -> Vec<(DeviceId, bool)> {
    let listed: DeviceListResult =
        terminal.call(Method::DeviceList(DeviceList::default())).await.unwrap();
    listed.devices.iter().map(|device| (device.device_id, device.push)).collect()
}

/// The terminal sends `text` as a new prompt; returns its conversation and a
/// subscription to it.
async fn prompt(
    daemon: &TestDaemon,
    terminal: &Client,
    text: &str,
) -> (ConversationId, ItemStream<ConversationSubscribeItem>) {
    let sent: PromptSendResult = terminal.call(daemon.prompt(1, text, TTY)).await.unwrap();
    let subscribe = ConversationSubscribe {
        conversation_id: sent.conversation_id,
        after_seq: None,
        answers_input: true,
        answers_questions: true,
        drafts: false,
    };
    let events = terminal.stream(Method::ConversationSubscribe(subscribe)).await.unwrap();
    (sent.conversation_id, events)
}

/// Reads `events` until the next event that `stop` takes, and returns it.
async fn next(
    events: &mut ItemStream<ConversationSubscribeItem>,
    stop: fn(&Event) -> bool,
) -> Event {
    let seen = events_until(events, stop).await.unwrap();
    seen.last().map(|envelope| envelope.event.clone()).unwrap()
}

/// Reads `events` until the next approval, and returns its call.
async fn next_approval(events: &mut ItemStream<ConversationSubscribeItem>) -> CallId {
    match next(events, |event| matches!(event, Event::ApprovalRequested { .. })).await {
        Event::ApprovalRequested { call_id, .. } => call_id,
        other => panic!("no approval: {other:?}"),
    }
}

fn allow(n: u128, conversation_id: ConversationId, call_id: CallId) -> Method {
    let decision = ApprovalDecision::Allow;
    let command_id = command_id(n);
    Method::ApprovalRespond(ApprovalRespond { command_id, conversation_id, call_id, decision })
}

/// The terminal says that you are away from `conversation_id`.
async fn away(terminal: &Client, n: u128, conversation_id: ConversationId) {
    let set = PresenceSet {
        command_id: command_id(n),
        conversation_id: Some(conversation_id),
        presence: Presence::Away,
    };
    let _: PresenceSetResult = terminal.call(Method::PresenceSet(set)).await.unwrap();
}

fn refusal<T: std::fmt::Debug>(result: Result<T, ClientError>) -> ErrorCode {
    match result {
        Err(ClientError::Server { body }) => body.code,
        other => panic!("expected a refusal from the daemon, got {other:?}"),
    }
}

#[tokio::test]
async fn a_phone_sets_its_token_and_only_the_tailnet_listener_takes_one() {
    let server = expo(&[], Vec::new()).await;
    let daemon = start(&server, 0).await;
    let terminal = daemon.client_for_tty(TTY).await.unwrap();
    assert_eq!(terminal.hello().capabilities.push, None, "the Unix socket takes no token");
    let set = terminal
        .call::<DevicePushSetResult>(Method::DevicePushSet(DevicePushSet { token: None }))
        .await;
    assert_eq!(refusal(set), ErrorCode::Forbidden);

    let mut phone = PairedPhone::pair(&daemon).await.unwrap();
    // NOTE: the last one is a raw APNs token, which is no Expo token.
    for bad in ["ExponentPushToken[]", "ExponentPushToken[a b]", &"0f".repeat(32)] {
        let Err(TestDaemonError::PhoneServer { body }) = set_token(&mut phone, Some(bad)).await
        else {
            panic!("the token {bad} is not refused");
        };
        assert_eq!(body.code, ErrorCode::Invalid, "{bad}");
    }
    assert_eq!(push_on(&terminal).await, [(phone.device_id, false)]);
    set_token(&mut phone, Some(&token(1))).await.unwrap();
    assert_eq!(push_on(&terminal).await, [(phone.device_id, true)]);
    set_token(&mut phone, None).await.unwrap();
    assert_eq!(push_on(&terminal).await, [(phone.device_id, false)]);

    let tested: AdminPushTestResult =
        terminal.call(Method::AdminPushTest(AdminPushTest {})).await.unwrap();
    assert_eq!(tested.devices, [], "no paired phone has push on");
    assert_eq!(sends(&server).await, Vec::<Value>::new());
    drop((phone, terminal));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_phone_hello_names_push_only_while_presence_push_is_on() {
    let server = expo(&[], Vec::new()).await;
    for on in [true, false] {
        let daemon = TestDaemon::builder()
            .remote()
            .expo_push(server.uri())
            .config(|config| config.presence.push = on)
            .start()
            .await
            .unwrap();
        let mut phone = FakePhone::connect(daemon.remote_addr().await.unwrap()).await.unwrap();
        let hello = phone.hello(None).await.unwrap();
        assert_eq!(hello.capabilities.push, on.then_some(true));
        drop(phone);
        daemon.stop().await.unwrap();
    }
}

/// A question at the terminal waits there; when you turn away, its turn hands it to the
/// push task, which sends one request for both phones, once.
#[tokio::test]
async fn an_approval_pushes_once_to_both_phones_when_you_turn_away() {
    let logs = support::logs();
    let server = expo(&[], Vec::new()).await;
    let daemon = start(&server, 1).await;
    let terminal = daemon.client_for_tty(TTY).await.unwrap();
    let phones = phones(&daemon, 2).await;

    let before = sleeps(&daemon);
    let (conversation_id, mut events) = prompt(&daemon, &terminal, "write the note").await;
    let call_id = next_approval(&mut events).await;
    away(&terminal, 2, conversation_id).await;
    let looked = pass(&daemon, before, |wait| wait == PRESENCE_CHECK).await;

    let sent = sends_until(&server, 1).await;
    let place = notice_place(None, Some(daemon.cwd()));
    let (conversation, daemon_id) = (conversation_id.to_string(), daemon.daemon_id().to_string());
    let message = |n: usize| {
        json!({
            "to": token(n),
            "title": "efr",
            "body": format!("{place}: a question waits"),
            "sound": "default",
            "priority": "high",
            "ttl": 300,
            "collapseId": conversation,
            "threadId": conversation,
            "data": {"d": daemon_id, "c": conversation}
        })
    };
    assert_eq!(sent, [json!([message(1), message(2)])]);
    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(request.method.as_str(), "POST");
    assert_eq!(request.headers.get("accept").unwrap(), "application/json");
    assert_eq!(request.headers.get("content-type").unwrap(), "application/json");
    assert!(request.headers.get("accept-encoding").is_none());

    // NOTE: the next look of the turn finds you away again; the question pushed once.
    pass(&daemon, looked, |wait| wait == PRESENCE_CHECK).await;
    let _: ApprovalRespondResult = terminal.call(allow(3, conversation_id, call_id)).await.unwrap();
    next(&mut events, |event| matches!(event, Event::TurnCompleted { .. })).await;
    assert_eq!(sends(&server).await.len(), 1);
    let logged = String::from_utf8_lossy(&logs.lock().unwrap()).into_owned();
    assert!(!logged.contains("ExponentPushToken"), "no log line holds a token");
    drop((events, phones, terminal));
    daemon.stop().await.unwrap();
}

/// A question of the model that no client can answer finds nobody there at once, but
/// you are still at the terminal, so no push goes. Its turn hands it over at each look,
/// so the push goes when you turn away later.
#[tokio::test]
async fn a_question_refused_at_its_first_look_pushes_when_you_turn_away_later() {
    let server = expo(&[], Vec::new()).await;
    let ask = json!({
        "question": "Which database?",
        "options": [{ "label": "Postgres" }, { "label": "SQLite" }],
    });
    let daemon = TestDaemon::builder()
        .remote()
        .expo_push(server.uri())
        .custom_provider(ScriptedModel::reviewed(vec![("ask_user", ask)], REVIEWER_ALLOWS))
        .start()
        .await
        .unwrap();
    let terminal = daemon.client_for_tty(TTY).await.unwrap();
    let phones = phones(&daemon, 1).await;

    let before = sleeps(&daemon);
    let sent: PromptSendResult =
        terminal.call(daemon.prompt(1, "pick a store", TTY)).await.unwrap();
    let looked = pass(&daemon, before, |wait| wait == PRESENCE_CHECK).await;
    let looked = pass(&daemon, looked, |wait| wait == PRESENCE_CHECK).await;
    assert!(sends(&server).await.is_empty(), "you are at the terminal");

    away(&terminal, 2, sent.conversation_id).await;
    pass(&daemon, looked, |wait| wait == PRESENCE_CHECK).await;
    let sent = sends_until(&server, 1).await;
    assert_eq!(tokens_of(&sent[0]), [token(1)]);
    drop((phones, terminal));
    daemon.stop().await.unwrap();
}

/// A phone in front gets no push; a revoked phone gets none; an answer from the phone
/// leaves you away from the terminal, so the next question pushes at once.
#[tokio::test]
async fn a_visible_lease_holds_the_push_and_a_revoked_phone_gets_none() {
    let server = expo(&[], Vec::new()).await;
    let daemon = start(&server, 2).await;
    let terminal = daemon.client_for_tty(TTY).await.unwrap();
    let mut phones = phones(&daemon, 2).await;

    let before = sleeps(&daemon);
    let (conversation_id, mut events) = prompt(&daemon, &terminal, "write two notes").await;
    let lease = |visible| {
        let conversations = vec![conversation_id];
        Method::LeaseReport(LeaseReport { conversations, ptys: Vec::new(), visible })
    };
    let _: LeaseReportResult = phones[0].phone.call(lease(true)).await.unwrap();
    let first = next_approval(&mut events).await;
    away(&terminal, 2, conversation_id).await;
    pass(&daemon, before, |wait| wait == PRESENCE_CHECK).await;
    let sent = sends_until(&server, 1).await;
    assert_eq!(tokens_of(&sent[0]), [token(2)], "phone 1 is in front");

    let revoke = DeviceRevoke { device_id: phones[1].device_id };
    let _: DeviceRevokeResult = terminal.call(Method::DeviceRevoke(revoke)).await.unwrap();
    let _: LeaseReportResult = phones[0].phone.call(lease(false)).await.unwrap();
    let _: ApprovalRespondResult =
        phones[0].phone.call(allow(3, conversation_id, first)).await.unwrap();
    next_approval(&mut events).await;

    let sent = sends_until(&server, 2).await;
    assert_eq!(tokens_of(&sent[1]), [token(1)], "the revoked phone gets none");
    drop((events, phones, terminal));
    daemon.stop().await.unwrap();
}

/// The end of a turn that a phone started pushes; a dead token in a ticket turns push
/// off for its phone at once, and one in a receipt 15 minutes later. A newer token of
/// a phone stays.
#[tokio::test]
async fn a_dead_token_in_a_ticket_or_a_receipt_turns_push_off() {
    let dead = json!({"status": "error", "message": token(2), "details": {"error": "DeviceNotRegistered"}});
    let server = expo(&[(1, "DeviceNotRegistered")], vec![(2, dead)]).await;
    let daemon = start(&server, 0).await;
    let terminal = daemon.client_for_tty(TTY).await.unwrap();
    let mut phones = phones(&daemon, 2).await;
    let (conversation_id, mut events) = prompt(&daemon, &terminal, "hello").await;
    next(&mut events, |event| matches!(event, Event::TurnCompleted { .. })).await;

    let Method::PromptSend(mut params) = daemon.prompt(2, "and from the phone", TTY) else {
        unreachable!("a prompt is a prompt.send")
    };
    params.conversation_id = Some(conversation_id);
    params.context = None;
    let before = sleeps(&daemon);
    let _: PromptSendResult = phones[0].phone.call(Method::PromptSend(params)).await.unwrap();

    let sent = sends_until(&server, 1).await;
    let place = notice_place(None, Some(daemon.cwd()));
    assert_eq!(sent[0][0]["body"], format!("{place}: done in 0s"));
    assert_eq!(sent[0][0]["ttl"], 3600);
    let (one, two) = (phones[0].device_id, phones[1].device_id);
    Wait::new("the dead token of the ticket to go")
        .until_some_async(async || {
            (push_on(&terminal).await == [(one, false), (two, true)]).then_some(())
        })
        .await
        .unwrap();
    set_token(&mut phones[0], Some(&token(1))).await.unwrap();

    pass(&daemon, before, |wait| wait == Duration::from_secs(15 * 60)).await;
    Wait::new("the dead token of the receipt to go")
        .until_some_async(async || {
            (push_on(&terminal).await == [(one, true), (two, false)]).then_some(())
        })
        .await
        .unwrap();
    drop((events, phones, terminal));
    daemon.stop().await.unwrap();
}

/// A 429 and a 503 are sent again after the waits of the policy; a 400 is not. A send
/// with tokens of two Expo projects goes again as one request for each message.
#[tokio::test]
async fn a_429_and_a_503_are_sent_again_and_two_projects_go_one_by_one() {
    let server = MockServer::start().await;
    for status in [429, 503] {
        let busy = ResponseTemplate::new(status);
        Mock::given(path(ExpoPush::SEND_PATH))
            .respond_with(busy)
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    let mixed = json!({"errors": [{"code": "PUSH_TOO_MANY_EXPERIENCE_IDS", "message": token(1)}]});
    Mock::given(path(ExpoPush::SEND_PATH))
        .respond_with(ResponseTemplate::new(400).set_body_json(mixed))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path(ExpoPush::SEND_PATH)).respond_with(tickets(&[])).mount(&server).await;
    let daemon = start(&server, 1).await;
    let terminal = daemon.client_for_tty(TTY).await.unwrap();
    let phones = phones(&daemon, 2).await;

    let before = sleeps(&daemon);
    let (conversation_id, mut events) = prompt(&daemon, &terminal, "write the note").await;
    next_approval(&mut events).await;
    away(&terminal, 2, conversation_id).await;
    let looked = pass(&daemon, before, |wait| wait == PRESENCE_CHECK).await;
    sends_until(&server, 1).await;
    let waited = pass(&daemon, looked, try_wait(2)).await;
    sends_until(&server, 2).await;
    pass(&daemon, waited, try_wait(4)).await;

    let sent = sends_until(&server, 5).await;
    let tokens: Vec<Vec<String>> = sent.iter().map(tokens_of).collect();
    let both = vec![token(1), token(2)];
    // NOTE: the 400 went once: a second copy would come before the two single sends.
    assert_eq!(tokens, [both.clone(), both.clone(), both, vec![token(1)], vec![token(2)]]);
    drop((events, phones, terminal));
    daemon.stop().await.unwrap();
}

/// `admin.push_test` sends `test` to each phone and tells what became of it after the
/// receipts: sent, no receipt yet, or not sent with the code. A dead token goes.
#[tokio::test]
async fn the_push_test_tells_what_became_of_each_phone() {
    const REFUSED: &[(usize, &str)] = &[(3, "DeviceNotRegistered"), (4, "InvalidCredentials")];
    let server = expo(REFUSED, vec![(2, Value::Null)]).await;
    let daemon = start(&server, 0).await;
    let terminal = daemon.client_for_tty(TTY).await.unwrap();
    let phones = phones(&daemon, 4).await;

    let exactly = |secs| move |wait| wait == Duration::from_secs(secs);
    let tested = push_test(&daemon, &terminal, &[&exactly(2), &exactly(4), &exactly(8)]).await;

    let row = |result, code: Option<&str>| PushTestRow {
        name: "paired phone".to_owned(),
        result,
        code: code.map(str::to_owned),
    };
    let expected = [
        row(PushTestResult::Sent, None),
        row(PushTestResult::NoReceipt, None),
        row(PushTestResult::NotSent, Some("DeviceNotRegistered")),
        row(PushTestResult::NotSent, Some("InvalidCredentials")),
    ];
    assert_eq!(tested.devices, expected);
    let sent = sends(&server).await;
    assert_eq!(sent[0][0]["body"], "test");
    assert_eq!(sent[0][0].get("collapseId"), None, "a test opens no conversation");
    let on: Vec<bool> = push_on(&terminal).await.into_iter().map(|(_, on)| on).collect();
    assert_eq!(on, [true, true, false, true], "the dead token went");
    drop((phones, terminal));
    daemon.stop().await.unwrap();
}

/// A whole send that Expo refuses, or one that gets no answer, is `not_sent` for each
/// phone, with the code or with none.
#[tokio::test]
async fn the_push_test_names_a_refused_send_and_no_answer() {
    let server = MockServer::start().await;
    let unauthorized = json!({"errors": [{"code": "UNAUTHORIZED", "message": "no access token"}]});
    Mock::given(path(ExpoPush::SEND_PATH))
        .respond_with(ResponseTemplate::new(401).set_body_json(unauthorized))
        .mount(&server)
        .await;
    let daemon = start(&server, 0).await;
    let terminal = daemon.client_for_tty(TTY).await.unwrap();
    let phones = phones(&daemon, 1).await;

    let tested: AdminPushTestResult =
        terminal.call(Method::AdminPushTest(AdminPushTest {})).await.unwrap();
    let [row] = tested.devices.as_slice() else { panic!("one phone: {tested:?}") };
    assert_eq!((row.result, row.code.as_deref()), (PushTestResult::NotSent, Some("UNAUTHORIZED")));

    server.reset().await;
    Mock::given(path(ExpoPush::SEND_PATH))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let tested = push_test(&daemon, &terminal, &[&try_wait(2), &try_wait(4), &try_wait(8)]).await;
    let [row] = tested.devices.as_slice() else { panic!("one phone: {tested:?}") };
    assert_eq!((row.result, row.code.as_deref()), (PushTestResult::NotSent, None));
    assert_eq!(sends(&server).await.len(), 4, "four tries");
    drop((phones, terminal));
    daemon.stop().await.unwrap();
}
