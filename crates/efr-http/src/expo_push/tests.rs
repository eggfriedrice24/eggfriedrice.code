use std::future::{pending, ready};
use std::sync::Mutex;

use efr_stdx::time::Sleep;
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;
use crate::HttpConfig;
use crate::testing::{FixedRng, InstantClock, start};

const TOKEN: &str = "ExponentPushToken[xxxxxxxxxxxxxxxxxxxxxx]";

/// A clock whose sleeps shorter than [`REQUEST_TIMEOUT`] finish at once and move `now`
/// forward, and whose longer sleeps never finish: the waits between tries pass, and no
/// try times out.
#[derive(Debug)]
struct WaitClock {
    now: Mutex<Timestamp>,
    waits: Mutex<Vec<Duration>>,
}

impl WaitClock {
    fn new() -> Self {
        WaitClock { now: Mutex::new(start()), waits: Mutex::new(Vec::new()) }
    }

    fn waits(&self) -> Vec<Duration> {
        self.waits.lock().unwrap().clone()
    }
}

impl Clock for WaitClock {
    fn now(&self) -> Timestamp {
        *self.now.lock().unwrap()
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        if duration >= REQUEST_TIMEOUT {
            return Box::pin(pending());
        }
        self.waits.lock().unwrap().push(duration);
        let mut now = self.now.lock().unwrap();
        *now = now.checked_add(SignedDuration::try_from(duration).unwrap()).unwrap();
        Box::pin(ready(()))
    }
}

fn push(server: &MockServer, clock: Arc<dyn Clock>) -> ExpoPush {
    let rng: Arc<dyn Rng> = Arc::new(FixedRng(0));
    let client = HttpClient::new(&HttpConfig::default(), Arc::clone(&clock), Arc::clone(&rng));
    ExpoPush::new(&format!("{}/", server.uri()), client.unwrap(), clock, rng).unwrap()
}

fn message(conversation: Option<&str>, ttl: u64) -> PushMessage {
    PushMessage::new(
        TOKEN,
        "app: a question waits".to_owned(),
        Duration::from_secs(ttl),
        conversation.map(str::to_owned),
        "d-1".to_owned(),
    )
}

fn ok(body: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

async fn requests(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap()
}

#[tokio::test]
async fn a_send_posts_the_messages_as_one_json_array_with_no_gzip() {
    let server = MockServer::start().await;
    let expected = json!([
        {
            "to": TOKEN,
            "title": "efr",
            "body": "app: a question waits",
            "sound": "default",
            "priority": "high",
            "ttl": 300,
            "collapseId": "c-1",
            "threadId": "c-1",
            "data": {"d": "d-1", "c": "c-1"}
        },
        {
            "to": TOKEN,
            "title": "efr",
            "body": "app: a question waits",
            "sound": "default",
            "priority": "high",
            "ttl": 3600,
            "data": {"d": "d-1"}
        }
    ]);
    Mock::given(method("POST"))
        .and(path(SEND_PATH))
        .and(header("accept", "application/json"))
        .and(header("content-type", "application/json"))
        .and(body_json(&expected))
        .respond_with(ok(json!({"data": [
            {"status": "ok", "id": "XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX"},
            {"status": "ok", "id": "YYYYYYYY-YYYY-YYYY-YYYY-YYYYYYYYYYYY"}
        ]})))
        .expect(1)
        .mount(&server)
        .await;
    let push = push(&server, Arc::new(WaitClock::new()));

    let answer = push.send(&[message(Some("c-1"), 300), message(None, 3600)]).await.unwrap();

    let ids = ["XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX", "YYYYYYYY-YYYY-YYYY-YYYY-YYYYYYYYYYYY"];
    let tickets = ids.map(|id| Ticket::Ok(TicketId::new(id).unwrap())).to_vec();
    assert_eq!(answer, ExpoAnswer::Data(tickets));
    let seen = requests(&server).await;
    assert!(seen[0].headers.get("accept-encoding").is_none(), "the answer comes as plain JSON");
    assert!(seen[0].headers.get("authorization").is_none(), "efr sends no access token");
}

#[tokio::test]
async fn a_ticket_keeps_its_code_and_no_value_holds_the_text_of_its_message() {
    let server = MockServer::start().await;
    let refused = format!("\"{TOKEN}\" is not a registered push notification recipient");
    Mock::given(path(SEND_PATH))
        .respond_with(ok(json!({"data": [
            {"status": "error", "message": refused, "details": {"error": "DeviceNotRegistered"}},
            {"status": "error", "message": refused, "details": {"error": TOKEN}},
            {"status": "error", "message": refused},
            {"status": "ok", "id": TOKEN}
        ]})))
        .mount(&server)
        .await;
    let push = push(&server, Arc::new(WaitClock::new()));

    let answer = push.send(&[message(None, 60)]).await.unwrap();

    let not_registered = PushCode::new(DEVICE_NOT_REGISTERED);
    let expected = vec![
        Ticket::Failed(not_registered),
        Ticket::Failed(None),
        Ticket::Failed(None),
        Ticket::Failed(None),
    ];
    assert_eq!(answer, ExpoAnswer::Data(expected));
    assert!(!format!("{answer:?}").contains("xxxx"));
}

#[tokio::test]
async fn the_receipts_of_due_tickets_come_by_their_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RECEIPTS_PATH))
        .and(header("accept", "application/json"))
        .and(body_json(json!({"ids": ["a-1", "b-2", "c-3"]})))
        .respond_with(ok(json!({"data": {
            "a-1": {"status": "ok"},
            "b-2": {"status": "error", "message": TOKEN, "details": {"error": "MessageTooBig"}},
            "<bad id>": {"status": "ok"}
        }})))
        .mount(&server)
        .await;
    let push = push(&server, Arc::new(WaitClock::new()));
    let ids = ["a-1", "b-2", "c-3"].map(|id| TicketId::new(id).unwrap());

    let ExpoAnswer::Data(mut receipts) = push.receipts(&ids).await.unwrap() else {
        panic!("the receipts");
    };

    receipts.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    let expected = vec![
        (ids[0].clone(), Receipt::Ok),
        (ids[1].clone(), Receipt::Failed(PushCode::new("MessageTooBig"))),
    ];
    assert_eq!(receipts, expected, "c-3 has no receipt yet");
}

#[tokio::test]
async fn an_errors_answer_refuses_the_whole_send_once_with_its_code() {
    let server = MockServer::start().await;
    let body = json!({"errors": [
        {"code": "PUSH_TOO_MANY_EXPERIENCE_IDS", "message": TOKEN, "details": {}}
    ]});
    Mock::given(path(SEND_PATH))
        .respond_with(ResponseTemplate::new(400).set_body_json(body))
        .mount(&server)
        .await;
    let clock = Arc::new(WaitClock::new());
    let push = push(&server, clock.clone());

    let answer = push.send(&[message(None, 60), message(None, 60)]).await.unwrap();

    let code = PushCode::new(TOO_MANY_EXPERIENCE_IDS);
    assert_eq!(answer, ExpoAnswer::Refused { status: StatusCode::BAD_REQUEST, code });
    assert_eq!(requests(&server).await.len(), 1, "a 400 goes once");
    assert_eq!(clock.waits(), []);
}

#[tokio::test]
async fn an_errors_array_with_a_success_status_refuses_too() {
    let server = MockServer::start().await;
    Mock::given(path(SEND_PATH))
        .respond_with(ok(json!({"errors": [{"code": "UNAUTHORIZED", "message": "no token"}]})))
        .mount(&server)
        .await;
    let push = push(&server, Arc::new(WaitClock::new()));

    let answer = push.send(&[message(None, 60)]).await.unwrap();

    let code = PushCode::new("UNAUTHORIZED");
    assert_eq!(answer, ExpoAnswer::Refused { status: StatusCode::OK, code });
}

#[tokio::test]
async fn a_429_and_a_503_go_again_after_the_waits_of_the_policy() {
    for status in [429, 503] {
        let server = MockServer::start().await;
        Mock::given(path(SEND_PATH))
            .respond_with(ResponseTemplate::new(status).set_body_string("<html>busy</html>"))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(path(SEND_PATH))
            .respond_with(ok(json!({"data": [{"status": "ok", "id": "a-1"}]})))
            .mount(&server)
            .await;
        let clock = Arc::new(WaitClock::new());
        let push = push(&server, clock.clone());

        let answer = push.send(&[message(None, 60)]).await.unwrap();

        let tickets = vec![Ticket::Ok(TicketId::new("a-1").unwrap())];
        assert_eq!(answer, ExpoAnswer::Data(tickets), "{status}");
        assert_eq!(requests(&server).await.len(), 3);
        // NOTE: the fixed generator draws no jitter, so each wait is half of its base.
        assert_eq!(clock.waits(), [Duration::from_secs(1), Duration::from_secs(2)]);
    }
}

#[tokio::test]
async fn a_retry_after_is_followed_and_the_last_refusal_comes_back() {
    let server = MockServer::start().await;
    let busy = ResponseTemplate::new(429)
        .insert_header("retry-after", "7")
        .set_body_json(json!({"errors": [{"code": "TOO_MANY_REQUESTS"}]}));
    Mock::given(path(SEND_PATH)).respond_with(busy).mount(&server).await;
    let clock = Arc::new(WaitClock::new());
    let push = push(&server, clock.clone());

    let answer = push.send(&[message(None, 3600)]).await.unwrap();

    let code = PushCode::new("TOO_MANY_REQUESTS");
    assert_eq!(answer, ExpoAnswer::Refused { status: StatusCode::TOO_MANY_REQUESTS, code });
    assert_eq!(requests(&server).await.len(), 4, "four tries");
    assert_eq!(clock.waits(), [Duration::from_secs(7); 3]);
}

#[tokio::test]
async fn no_try_starts_after_the_ttl_of_a_message() {
    let server = MockServer::start().await;
    Mock::given(path(SEND_PATH)).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    let push = push(&server, Arc::new(WaitClock::new()));

    let sent = push.send(&[message(None, 3600), message(None, 2)]).await;

    assert!(matches!(sent, Err(HttpError::Timeout { .. })), "{sent:?}");
    // The tries start at 0 s and 1 s; the third would start at 3 s.
    assert_eq!(requests(&server).await.len(), 2);
}

#[tokio::test]
async fn a_try_ends_after_the_timeout_on_the_clock() {
    let server = MockServer::start().await;
    Mock::given(path(SEND_PATH))
        .respond_with(ok(json!({"data": []})).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    let clock = Arc::new(InstantClock::new());
    let push = push(&server, clock.clone());

    let sent = push.send(&[message(None, 3600)]).await;

    assert!(matches!(sent, Err(HttpError::Timeout { .. })), "{sent:?}");
    let timeouts = clock.sleeps().iter().filter(|wait| **wait == REQUEST_TIMEOUT).count();
    assert_eq!(timeouts, 4, "each of the four tries ends at the timeout");
}

#[tokio::test]
async fn a_success_whose_body_is_not_the_json_of_a_send_fails() {
    let server = MockServer::start().await;
    Mock::given(path(SEND_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    let push = push(&server, Arc::new(WaitClock::new()));

    let sent = push.send(&[message(None, 60)]).await;

    assert!(matches!(sent, Err(HttpError::DecodeJson { .. })), "{sent:?}");
}

#[test]
fn a_code_is_1_to_64_letters_or_underscores() {
    for code in ["DeviceNotRegistered", "PUSH_TOO_MANY_EXPERIENCE_IDS", "a", &"A".repeat(64)] {
        assert_eq!(PushCode::new(code).map(|code| code.as_str().to_owned()).as_deref(), Some(code));
    }
    for text in ["", "Device Not", "ExponentPushToken[x]", "code1", &"A".repeat(65), "Ünicode"] {
        assert_eq!(PushCode::new(text), None, "{text}");
    }
}

#[test]
fn a_ticket_id_is_1_to_64_letters_digits_or_dashes() {
    assert!(TicketId::new("XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX").is_some());
    for text in ["", "a b", "ExponentPushToken[x]", &"a".repeat(65)] {
        assert_eq!(TicketId::new(text), None, "{text}");
    }
}

#[test]
fn a_base_that_is_not_an_http_url_is_refused() {
    let clock: Arc<dyn Clock> = Arc::new(WaitClock::new());
    let rng: Arc<dyn Rng> = Arc::new(FixedRng(0));
    for base in ["exp.host", "ftp://exp.host", ""] {
        let client = HttpClient::new(&HttpConfig::default(), Arc::clone(&clock), Arc::clone(&rng));
        let made = ExpoPush::new(base, client.unwrap(), Arc::clone(&clock), Arc::clone(&rng));
        assert!(made.is_err(), "{base}");
    }
}

#[test]
fn the_debug_form_of_a_message_holds_no_token() {
    assert!(!format!("{:?}", message(None, 60)).contains("xxxx"));
}
