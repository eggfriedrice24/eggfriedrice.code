//! Provider credentials over a `TestDaemon`: the real OpenAI provider against a local
//! Responses server refreshes its token once on a 401, and `admin.login_openai` hands
//! out the authorize URL, runs one login at a time and ends with what the browser said.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

use efr_protocol::{
    AdminLoginOpenAi, AdminLoginOpenAiItem, ErrorCode, Event, Method, PromptSendResult,
};
use efr_test_daemon::{
    API_KEY, ClientError, ItemStream, Replay, ResponsesAnswer, ResponsesServer, TTY, TestDaemon,
    events_until,
};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const UNAUTHORIZED: &str = r#"{"error":{"message":"Your authentication token has expired.","type":"invalid_request_error","code":"token_expired"}}"#;

#[tokio::test]
async fn provider_401_refresh_once() {
    let replay = Replay::run("provider_401_refresh_once").await.unwrap();

    let received = replay.server().unwrap().received();
    assert_eq!(received.len(), 2, "the refused request and its one retry");
    let bearer = format!("Bearer {API_KEY}");
    for request in &received {
        assert_eq!(request.authorization.as_deref(), Some(bearer.as_str()));
    }
    assert_eq!(received[0].body, received[1].body, "the retry is the same request");
    assert_eq!(replay.events().await.unwrap().last().unwrap().event.kind(), "turn_completed");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn a_second_401_fails_the_turn_as_unauthorized() {
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::new(401, UNAUTHORIZED));
    server.push(ResponsesAnswer::new(401, UNAUTHORIZED));
    server.push(ResponsesAnswer::text("never sent"));
    let daemon = TestDaemon::builder().responses(&server).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let sent: PromptSendResult = client.call(daemon.prompt(1, "hello", TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let events = events_until(&mut follow, |event| {
        matches!(event, Event::TurnFailed { .. } | Event::TurnCompleted { .. })
    })
    .await
    .unwrap();

    let Event::TurnFailed { error, .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert_eq!(server.received().len(), 2, "one refresh, never a third try");
    assert_eq!(server.remaining(), 1);
    drop((follow, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_text_answer_from_the_responses_server_completes_the_turn() {
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::text("Hello from the server."));
    let daemon = TestDaemon::builder().responses(&server).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let sent: PromptSendResult = client.call(daemon.prompt(1, "hello", TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let events = events_until(&mut follow, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();

    let text = events.iter().find_map(|envelope| match &envelope.event {
        Event::AssistantMessageCompleted { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(text.as_deref(), Some("Hello from the server."), "{events:#?}");
    let request = &server.received()[0].body;
    assert_eq!(request["stream"], true);
    assert_eq!(request["instructions"], efr_test_daemon::SYSTEM_PROMPT);
    drop((follow, client));
    daemon.stop().await.unwrap();
}

/// The first item of a login stream: the authorize URL, or `None` when the port of the
/// callback (1455, fixed by the client id) is taken on this machine.
#[expect(clippy::print_stderr, reason = "a skipped test says why")]
async fn authorize_url(stream: &mut ItemStream<AdminLoginOpenAiItem>) -> Option<String> {
    match stream.next().await.unwrap() {
        Ok(AdminLoginOpenAiItem::AuthorizeUrl { url }) => Some(url),
        Err(ClientError::Server { body }) if body.code == ErrorCode::Busy => {
            eprintln!("skipping the login test: the callback port is taken ({})", body.message);
            None
        }
        other => panic!("expected the authorize URL, got {other:?}"),
    }
}

#[tokio::test]
async fn the_login_hands_out_the_url_runs_alone_and_ends_with_the_browsers_answer() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let mut login = client
        .stream::<AdminLoginOpenAiItem>(Method::AdminLoginOpenAi(AdminLoginOpenAi::default()))
        .await
        .unwrap();
    let Some(url) = authorize_url(&mut login).await else {
        return;
    };

    assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"), "{url}");
    for part in [
        "client_id=app_EMoamEEZ73f0CkXaXp7hrann",
        "code_challenge_method=S256",
        "redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback",
        "response_type=code",
    ] {
        assert!(url.contains(part), "{part} missing from {url}");
    }

    // A second login while this one waits is refused.
    let other = daemon.client().await.unwrap();
    let mut second = other
        .stream::<AdminLoginOpenAiItem>(Method::AdminLoginOpenAi(AdminLoginOpenAi::default()))
        .await
        .unwrap();
    match second.next().await.unwrap() {
        Err(ClientError::Server { body }) => assert_eq!(body.code, ErrorCode::Busy),
        other => panic!("a second login must be refused, got {other:?}"),
    }

    // The browser comes back with a refusal: the login ends with it and stores nothing.
    let state =
        url.split(['?', '&']).find_map(|pair| pair.strip_prefix("state=")).unwrap().to_owned();
    let mut browser = tokio::net::TcpStream::connect("127.0.0.1:1455").await.unwrap();
    let request = format!(
        "GET /auth/callback?error=access_denied&state={state} HTTP/1.1\r\nHost: 127.0.0.1:1455\r\nConnection: close\r\n\r\n"
    );
    browser.write_all(request.as_bytes()).await.unwrap();
    let mut page = String::new();
    browser.read_to_string(&mut page).await.unwrap();
    assert!(page.starts_with("HTTP/1.1 400"), "{page}");
    match login.next().await.unwrap() {
        Err(ClientError::Server { body }) => assert_eq!(body.code, ErrorCode::Unauthorized),
        other => panic!("a refused login must end with an error, got {other:?}"),
    }
    let data = daemon.dirs().dirs().data();
    assert!(!data.join("secrets/openai-subscription.json").exists());
    drop((login, second, other, client));
    daemon.stop().await.unwrap();
}
