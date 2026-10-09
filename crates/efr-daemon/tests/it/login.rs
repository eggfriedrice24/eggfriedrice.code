//! Provider credentials over a `TestDaemon`: the real subscription provider against a
//! local Responses server refreshes its login once on a 401 and saves the new tokens,
//! the API key provider fails at the first 401 with the server's message, and `admin.login_openai` hands
//! out the authorize URL, runs one login at a time and ends with what the browser said.
//!
//! `admin.login_api_key` checks a key against the local server's `/v1/models` with the
//! organization and project headers, or an Anthropic key against the local Messages
//! API with its version and workspace headers, stores it and shows only its hint; a
//! refused key,
//! a check without an answer and a key with whitespace store nothing; `check: false`
//! stores without a request; a login to the running provider fetches the new key's
//! model list; `admin.logout` deletes the key, and `admin.status` lists every
//! provider. No key reaches a log line, an error, the event log or any file but its
//! credential.

use std::sync::Arc;

use efr_protocol::{
    AdminLoginApiKey, AdminLoginApiKeyResult, AdminLoginOpenAi, AdminLoginOpenAiItem, AdminLogout,
    AdminLogoutResult, AdminStatus, AdminStatusResult, ErrorCode, Event, LoginKind, Method,
    ModelsList, ModelsListResult, PromptSendResult, SecretText,
};
use efr_test_daemon::{
    API_KEY, Client, ClientError, ItemStream, MessagesAnswer, MessagesServer, ModelsAnswer,
    REFRESHED_ACCESS_TOKEN, REFRESHED_REFRESH_TOKEN, Replay, ResponsesAnswer, ResponsesServer,
    SUBSCRIPTION_ACCESS_TOKEN, SUBSCRIPTION_ACCOUNT, SUBSCRIPTION_CREDENTIAL,
    SUBSCRIPTION_REFRESH_TOKEN, TTY, TestDaemon, events_until,
};
use efr_test_support::Wait;
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::support::{files_holding, logged, logs};

const UNAUTHORIZED: &str = r#"{"error":{"message":"Your authentication token has expired.","type":"invalid_request_error","code":"token_expired"}}"#;

#[tokio::test]
async fn provider_401_refresh_once() {
    let replay = Replay::run("provider_401_refresh_once").await.unwrap();
    let server = replay.server().unwrap();

    let received = server.received();
    assert_eq!(received.len(), 2, "the refused request and its one retry");
    assert_eq!(
        received[0].authorization.as_deref(),
        Some(format!("Bearer {SUBSCRIPTION_ACCESS_TOKEN}").as_str())
    );
    assert_eq!(
        received[1].authorization.as_deref(),
        Some(format!("Bearer {REFRESHED_ACCESS_TOKEN}").as_str()),
        "the retry carries the refreshed token"
    );
    for request in &received {
        assert_eq!(request.account_id.as_deref(), Some(SUBSCRIPTION_ACCOUNT));
    }
    assert_eq!(received[0].body, received[1].body, "the retry is the same request");
    let [refresh] = server.token_requests().try_into().unwrap();
    assert_eq!(refresh.form.get("grant_type").map(String::as_str), Some("refresh_token"));
    assert_eq!(
        refresh.form.get("refresh_token").map(String::as_str),
        Some(SUBSCRIPTION_REFRESH_TOKEN)
    );
    let saved = replay.daemon().dirs().dirs().data().join(SUBSCRIPTION_CREDENTIAL);
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(saved).unwrap()).unwrap();
    assert_eq!(saved["access_token"], REFRESHED_ACCESS_TOKEN, "the refreshed login is saved");
    assert_eq!(saved["refresh_token"], REFRESHED_REFRESH_TOKEN);
    assert_eq!(saved["account_id"], SUBSCRIPTION_ACCOUNT);
    assert_eq!(replay.events().await.unwrap().last().unwrap().event.kind(), "turn_completed");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn the_api_key_provider_fails_the_turn_at_the_first_401_with_the_servers_message() {
    let server = ResponsesServer::start().await;
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
    assert_eq!(
        error.message,
        "the provider rejected the credentials: Your authentication token has expired."
    );
    let bearer = format!("Bearer {API_KEY}");
    let received = server.received();
    assert_eq!(received.len(), 1, "a key cannot refresh, so the same key is not sent again");
    assert_eq!(received[0].authorization.as_deref(), Some(bearer.as_str()));
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

/// An OpenAI key, as a test spells it. Only the local server ever sees it.
const OPENAI_KEY: &str = "sk-proj-efr-login-test-key-9f3c";

/// An Anthropic key, as a test spells it. Only the local server ever sees it.
const ANTHROPIC_KEY: &str = "sk-ant-api03-efr-login-test-key-a1b2";

/// The credential file of the `openai-api` provider, below the data root.
const API_CREDENTIAL: &str = "secrets/openai-api.json";

/// The credential file of the `anthropic-api` provider, below the data root.
const ANTHROPIC_CREDENTIAL: &str = "secrets/anthropic-api.json";

fn login(provider: &str, key: &str, check: bool) -> Method {
    Method::AdminLoginApiKey(AdminLoginApiKey {
        provider: provider.to_owned(),
        key: SecretText::new(key),
        check,
    })
}

fn logout(provider: &str) -> Method {
    Method::AdminLogout(AdminLogout { provider: provider.to_owned() })
}

async fn status(client: &Client) -> AdminStatusResult {
    client.call(Method::AdminStatus(AdminStatus {})).await.unwrap()
}

/// The answer of the API key backend's `/v1/models`: ids only.
fn api_list(ids: &[&str]) -> ModelsAnswer {
    let data: Vec<Value> = ids.iter().map(|id| json!({ "id": id, "object": "model" })).collect();
    ModelsAnswer::json(200, &json!({ "object": "list", "data": data }))
}

/// The error body of a refused key, with the masked key that OpenAI's text holds.
fn refused() -> ModelsAnswer {
    ModelsAnswer::json(
        401,
        &json!({ "error": {
            "message": "Incorrect API key provided: sk-proj-****9f3c.",
            "type": "invalid_request_error",
            "code": "invalid_api_key",
        }}),
    )
}

/// A daemon whose key checks go to `server`, with an organization and a project.
async fn checked_by(server: &ResponsesServer) -> TestDaemon {
    let base_url = server.base_url();
    TestDaemon::builder()
        .persistent()
        .config(|config| {
            config.openai.api_base_url = Some(base_url);
            config.openai.organization = Some("org-efr".to_owned());
            config.openai.project = Some("proj_efr".to_owned());
        })
        .start()
        .await
        .unwrap()
}

/// The server's error of a call that failed.
fn refusal(error: ClientError) -> efr_protocol::ErrorBody {
    match error {
        ClientError::Server { body } => body,
        other => panic!("expected the daemon's refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn an_openai_key_is_checked_stored_and_shown_only_by_its_hint() {
    let logs = logs();
    let server = ResponsesServer::start().await;
    server.set_models(api_list(&["gpt-5.5"]));
    let daemon = checked_by(&server).await;
    let client = daemon.client().await.unwrap();

    let result: AdminLoginApiKeyResult =
        client.call(login("openai-api", OPENAI_KEY, true)).await.unwrap();

    assert_eq!(
        result,
        AdminLoginApiKeyResult {
            provider: "openai-api".to_owned(),
            key_hint: "sk-proj-...9f3c".to_owned(),
            checked: true,
            active: false,
        }
    );
    let [check] = server.models_requests().try_into().unwrap();
    assert_eq!(check.authorization, Some(format!("Bearer {OPENAI_KEY}")));
    assert_eq!(check.organization.as_deref(), Some("org-efr"));
    assert_eq!(check.project.as_deref(), Some("proj_efr"));
    let providers = status(&client).await.providers;
    let api = providers.iter().find(|provider| provider.provider == "openai-api").unwrap();
    assert!(api.logged_in);
    assert_eq!(api.login, Some(LoginKind::ApiKey));
    assert_eq!(api.key_hint.as_deref(), Some("sk-proj-...9f3c"));
    assert!(!format!("{providers:?}").contains(OPENAI_KEY));

    drop(client);
    let dirs = Arc::clone(daemon.dirs());
    daemon.stop().await.unwrap();
    let credential = dirs.dirs().data().join(API_CREDENTIAL).display().to_string();
    assert_eq!(files_holding(dirs.root(), OPENAI_KEY.as_bytes()), [credential]);
    let logged = logged();
    assert!(logged.contains("login completed"), "the logs were captured");
    assert!(!logged.contains(OPENAI_KEY), "a log line holds the key");
    drop(logs);
}

#[tokio::test]
async fn a_refused_key_is_not_stored_and_the_error_says_what_the_server_said() {
    let logs = logs();
    let server = ResponsesServer::start().await;
    server.set_models(refused());
    let daemon = checked_by(&server).await;
    let client = daemon.client().await.unwrap();

    let error = client.call::<AdminLoginApiKeyResult>(login("openai-api", OPENAI_KEY, true)).await;

    let body = refusal(error.unwrap_err());
    assert_eq!(body.code, ErrorCode::Unauthorized);
    assert_eq!(
        body.message,
        "the check of the key for openai-api failed: the provider rejected the credentials: Incorrect API key provided: sk-proj-****9f3c."
    );
    let dirs = Arc::clone(daemon.dirs());
    assert!(!dirs.dirs().data().join(API_CREDENTIAL).exists());
    assert!(!status(&client).await.providers.iter().any(|provider| provider.logged_in));
    drop(client);
    daemon.stop().await.unwrap();
    assert_eq!(files_holding(dirs.root(), OPENAI_KEY.as_bytes()), Vec::<String>::new());
    assert!(!logged().contains(OPENAI_KEY), "a log line holds the key");
    drop(logs);
}

#[tokio::test]
async fn a_check_without_an_answer_stores_nothing_and_no_check_stores_the_key() {
    let daemon = TestDaemon::builder()
        .config(|config| {
            // NOTE: nothing listens on the discard port of the loopback.
            config.openai.api_base_url = Some("http://127.0.0.1:9/v1".to_owned());
        })
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let credential = daemon.dirs().dirs().data().join(API_CREDENTIAL);

    let error = client.call::<AdminLoginApiKeyResult>(login("openai-api", OPENAI_KEY, true)).await;

    let body = refusal(error.unwrap_err());
    assert_eq!(body.code, ErrorCode::Internal);
    assert!(
        body.message.starts_with("the check of the key for openai-api failed: "),
        "{}",
        body.message
    );
    assert!(!body.message.contains(OPENAI_KEY));
    assert!(!credential.exists());

    let stored: AdminLoginApiKeyResult =
        client.call(login("openai-api", OPENAI_KEY, false)).await.unwrap();

    assert!(!stored.checked);
    assert_eq!(stored.key_hint, "sk-proj-...9f3c");
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&credential).unwrap()).unwrap();
    assert_eq!(saved["kind"], "api_key");
    assert_eq!(saved["key"], OPENAI_KEY);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_key_with_whitespace_or_for_the_subscription_is_refused_before_a_check() {
    let server = ResponsesServer::start().await;
    server.set_models(api_list(&["gpt-5.5"]));
    let daemon = checked_by(&server).await;
    let client = daemon.client().await.unwrap();

    let cases = [
        (login("openai-api", "sk-proj-two words", true), "the key holds whitespace"),
        (login("openai-api", "sk-admin-efr-test", true), "an admin key cannot call models"),
        (login("openai-subscription", OPENAI_KEY, true), "takes no API key"),
        (login("gemini", OPENAI_KEY, true), "efr has no provider \"gemini\""),
    ];
    for (method, said) in cases {
        let body = refusal(client.call::<AdminLoginApiKeyResult>(method).await.unwrap_err());
        assert_eq!(body.code, ErrorCode::Invalid);
        assert!(body.message.contains(said), "{}", body.message);
        assert!(!body.message.contains("two words"), "{}", body.message);
    }
    assert!(server.models_requests().is_empty(), "no key was sent anywhere");
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn an_anthropic_key_that_its_check_does_not_accept_is_not_stored() {
    let server = ResponsesServer::start().await;
    let base_url = server.base_url();
    let daemon = TestDaemon::builder()
        .config(|config| config.anthropic.base_url = Some(base_url))
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();

    let error =
        client.call::<AdminLoginApiKeyResult>(login("anthropic-api", ANTHROPIC_KEY, true)).await;

    let body = refusal(error.unwrap_err());
    assert!(
        body.message.starts_with("the check of the key for anthropic-api failed: "),
        "{}",
        body.message
    );
    assert!(!body.message.contains(ANTHROPIC_KEY));
    assert!(!daemon.dirs().dirs().data().join(ANTHROPIC_CREDENTIAL).exists());
    drop(client);
    daemon.stop().await.unwrap();
}

/// A daemon whose Anthropic key checks go to `server`, with a workspace id.
async fn checked_by_anthropic(server: &MessagesServer) -> TestDaemon {
    let base_url = server.base_url();
    TestDaemon::builder()
        .persistent()
        .config(|config| {
            config.anthropic.base_url = Some(base_url);
            config.anthropic.workspace_id = Some("wrkspc_efr".to_owned());
        })
        .start()
        .await
        .unwrap()
}

#[tokio::test]
async fn an_anthropic_key_is_checked_with_its_headers_and_stored() {
    let logs = logs();
    let server = MessagesServer::start().await;
    server.set_models(vec![MessagesServer::model("claude-opus-5-5")]);
    let daemon = checked_by_anthropic(&server).await;
    let client = daemon.client().await.unwrap();

    let result: AdminLoginApiKeyResult =
        client.call(login("anthropic-api", ANTHROPIC_KEY, true)).await.unwrap();

    assert_eq!(
        result,
        AdminLoginApiKeyResult {
            provider: "anthropic-api".to_owned(),
            key_hint: "sk-ant-...a1b2".to_owned(),
            checked: true,
            active: false,
        }
    );
    let [check] = server.models_requests().try_into().unwrap();
    assert_eq!(check.limit.as_deref(), Some("1"));
    assert_eq!(check.authorization, Some(format!("Bearer {ANTHROPIC_KEY}")));
    assert_eq!(check.version.as_deref(), Some("2023-06-01"));
    assert_eq!(check.workspace_id.as_deref(), Some("wrkspc_efr"));
    assert!(server.received().is_empty(), "the check runs no model");

    drop(client);
    let dirs = Arc::clone(daemon.dirs());
    daemon.stop().await.unwrap();
    let credential = dirs.dirs().data().join(ANTHROPIC_CREDENTIAL).display().to_string();
    assert_eq!(files_holding(dirs.root(), ANTHROPIC_KEY.as_bytes()), [credential]);
    assert!(!logged().contains(ANTHROPIC_KEY), "a log line holds the key");
    drop(logs);
}

#[tokio::test]
async fn a_refused_anthropic_key_is_unauthorized_with_the_servers_message() {
    let cases = [
        (
            MessagesAnswer::error(401, "authentication_error", "invalid x-api-key"),
            "the provider rejected the credentials: invalid x-api-key",
        ),
        (
            MessagesAnswer::error(
                400,
                "invalid_request_error",
                "anthropic-workspace-id is required when authenticating with an identity-linked API key",
            ),
            "anthropic-workspace-id is required when authenticating with an identity-linked API key",
        ),
    ];
    for (answer, said) in cases {
        let server = MessagesServer::start().await;
        server.refuse_models(answer);
        let daemon = checked_by_anthropic(&server).await;
        let client = daemon.client().await.unwrap();

        let error = client
            .call::<AdminLoginApiKeyResult>(login("anthropic-api", ANTHROPIC_KEY, true))
            .await;

        let body = refusal(error.unwrap_err());
        assert_eq!(body.code, ErrorCode::Unauthorized, "{}", body.message);
        assert!(
            body.message.starts_with("the check of the key for anthropic-api failed: "),
            "{}",
            body.message
        );
        assert!(body.message.contains(said), "{}", body.message);
        assert!(!body.message.contains(ANTHROPIC_KEY));
        assert_eq!(server.models_requests().len(), 1, "a person waits, so the check goes once");
        let dirs = Arc::clone(daemon.dirs());
        assert!(!dirs.dirs().data().join(ANTHROPIC_CREDENTIAL).exists());
        drop(client);
        daemon.stop().await.unwrap();
        assert_eq!(files_holding(dirs.root(), ANTHROPIC_KEY.as_bytes()), Vec::<String>::new());
    }
}

#[tokio::test]
async fn a_logout_deletes_the_key_and_the_status_lists_every_provider() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let credential = daemon.dirs().dirs().data().join(ANTHROPIC_CREDENTIAL);

    let stored: AdminLoginApiKeyResult =
        client.call(login("anthropic-api", ANTHROPIC_KEY, false)).await.unwrap();

    assert_eq!(stored.key_hint, "sk-ant-...a1b2");
    assert!(!stored.active, "new conversations keep the provider of the config");
    assert!(credential.exists());
    let providers = status(&client).await.providers;
    let shown: Vec<(&str, bool, bool, Option<&str>)> = providers
        .iter()
        .map(|provider| {
            let hint = provider.key_hint.as_deref();
            (provider.provider.as_str(), provider.logged_in, provider.active, hint)
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("openai-subscription", false, true, None),
            ("openai-api", false, false, None),
            ("anthropic-api", true, false, Some("sk-ant-...a1b2")),
        ]
    );

    let out: AdminLogoutResult = client.call(logout("anthropic-api")).await.unwrap();
    let again: AdminLogoutResult = client.call(logout("anthropic-api")).await.unwrap();
    let unknown = client.call::<AdminLogoutResult>(logout("anthropic")).await.unwrap_err();

    assert!(out.logged_out);
    assert!(!again.logged_out, "nothing was left to delete");
    assert_eq!(refusal(unknown).code, ErrorCode::Invalid, "the daemon takes provider ids only");
    assert!(!credential.exists());
    assert!(!status(&client).await.providers.iter().any(|provider| provider.logged_in));
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_new_key_for_the_running_provider_brings_the_model_list_of_that_key() {
    let server = ResponsesServer::start().await;
    let daemon = TestDaemon::builder().responses(&server).start().await.unwrap();
    let client = daemon.client().await.unwrap();
    // The fetch at start finds no list (404), so the built-in table stays.
    Wait::new("the fetch at start").until(|| !server.models_requests().is_empty()).await.unwrap();
    server.set_models(api_list(&["gpt-5.5", "gpt-6-luna"]));

    let result: AdminLoginApiKeyResult =
        client.call(login("openai-api", OPENAI_KEY, true)).await.unwrap();

    assert!(result.active);
    let list = Wait::new("the list of the new key")
        .until_some_async(async || {
            let list: ModelsListResult =
                client.call(Method::ModelsList(ModelsList::default())).await.unwrap();
            (list.models.len() == 2).then_some(list)
        })
        .await
        .unwrap();
    let ids: Vec<&str> = list.models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, ["gpt-6-luna", "gpt-5.5"]);
    assert!(list.models.iter().all(|model| !model.efforts.iter().any(|effort| effort == "ultra")));
    let requests = server.models_requests();
    assert_eq!(requests[0].authorization, Some(format!("Bearer {API_KEY}")));
    assert!(
        requests[1..]
            .iter()
            .all(|request| request.authorization == Some(format!("Bearer {OPENAI_KEY}"))),
        "the check and the fetch after it send the new key"
    );
    drop(client);
    daemon.stop().await.unwrap();
}
