use std::sync::Arc;
use std::time::Duration;

use efr_credentials::{CredentialRecord, FileStore, SecretStore};
use efr_http::{HttpClient, HttpRequest};
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;
use secrecy::ExposeSecret as _;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{LoginCompleted, OpenAiLogin};
use crate::testing::{ACCOUNT, EMAIL, MemoryStore, at, config, credential, fixture, http};
use crate::{OAuthConfig, OAuthError, pkce};

fn login(config: OAuthConfig, store: Arc<dyn SecretStore>, clock: &TestClock) -> OpenAiLogin {
    let rng = Arc::new(TestRng::new(7));
    OpenAiLogin::new(config, http(clock), store, credential(), clock.shared(), rng)
}

async fn token_endpoint(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(response)
        .mount(server)
        .await;
}

fn issued_tokens() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(fixture("authorization_code.json"), "application/json")
}

fn query(url: &Url, name: &str) -> String {
    url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned()).unwrap()
}

/// What the browser does after the user signs in: follow the redirect of the
/// authorize URL `url` with `extra` parameters and the login's own `state`.
async fn browser_returns(client: &HttpClient, url: &Url, extra: &str) -> u16 {
    let callback = format!("{}?{extra}&state={}", query(url, "redirect_uri"), query(url, "state"));
    client.send(&HttpRequest::get(&callback).unwrap()).await.unwrap().status().as_u16()
}

#[tokio::test]
async fn a_login_exchanges_the_code_with_its_verifier_and_saves_the_tokens() {
    let server = MockServer::start().await;
    token_endpoint(&server, issued_tokens()).await;
    let clock = TestClock::new();
    let store = Arc::new(MemoryStore::default());
    let login = login(config(&server), store.clone(), &clock);

    let pending = login.start().await.unwrap();
    let url = pending.authorize_url().clone();
    let redirect = Url::parse(&query(&url, "redirect_uri")).unwrap();
    assert_eq!(redirect.host_str(), Some("127.0.0.1"));
    assert_eq!(redirect.path(), "/auth/callback");
    assert_ne!(redirect.port(), Some(0));

    let completing = tokio::spawn(pending.complete());
    assert_eq!(browser_returns(&http(&clock), &url, "code=the-code").await, 200);
    let completed = completing.await.unwrap().unwrap();

    assert_eq!(
        completed,
        LoginCompleted {
            account_id: Some(ACCOUNT.to_owned()),
            email: Some(EMAIL.to_owned()),
            expires_at: Some(at(3600)),
        }
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let form: Vec<(String, String)> =
        url::form_urlencoded::parse(&requests[0].body).into_owned().collect();
    let field = |name: &str| {
        form.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone()).unwrap()
    };
    assert_eq!(field("grant_type"), "authorization_code");
    assert_eq!(field("code"), "the-code");
    assert_eq!(field("redirect_uri"), redirect.as_str());
    assert_eq!(pkce::challenge(&field("code_verifier")), query(&url, "code_challenge"));

    let saved = store.tokens().unwrap();
    assert_eq!(saved.refresh_token.unwrap().expose_secret(), "rt_fixture_issued_at_login");
    assert_eq!(saved.account_id.as_deref(), Some(ACCOUNT));
    assert_eq!(saved.expires_at, Some(at(3600)));
    assert!(saved.id_token.is_some());
}

#[tokio::test]
async fn a_second_login_is_refused_while_the_first_is_pending() {
    let server = MockServer::start().await;
    let clock = TestClock::new();
    let login = login(config(&server), Arc::new(MemoryStore::default()), &clock);

    let first = login.start().await.unwrap();
    assert!(matches!(login.start().await, Err(OAuthError::LoginInProgress)));
    drop(first);
    login.start().await.unwrap();
}

#[tokio::test]
async fn a_held_port_fails_the_start_and_frees_the_slot() {
    let server = MockServer::start().await;
    let clock = TestClock::new();
    let holder = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = holder.local_addr().unwrap().port();
    let login = login(
        OAuthConfig { callback_port: port, ..config(&server) },
        Arc::new(MemoryStore::default()),
        &clock,
    );

    assert!(matches!(login.start().await, Err(OAuthError::Bind { .. })));
    drop(holder);
    login.start().await.unwrap();
}

#[tokio::test]
async fn a_login_times_out_on_the_injected_clock_and_frees_the_port() {
    let server = MockServer::start().await;
    let clock = TestClock::new();
    let timeout = Duration::from_secs(600);
    let login = login(
        OAuthConfig { login_timeout: timeout, ..config(&server) },
        Arc::new(MemoryStore::default()),
        &clock,
    );

    let pending = login.start().await.unwrap();
    let port = Url::parse(&query(pending.authorize_url(), "redirect_uri")).unwrap().port().unwrap();
    let completing = tokio::spawn(pending.complete());
    clock.wait_for_sleeps(1).await;
    clock.advance(timeout - Duration::from_secs(1));
    assert!(!completing.is_finished());
    clock.advance(Duration::from_secs(1));

    let error = completing.await.unwrap().unwrap_err();
    assert!(matches!(error, OAuthError::TimedOut { after } if after == timeout), "{error:?}");
    tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    login.start().await.unwrap();
}

#[tokio::test]
async fn a_declined_login_ends_without_a_token_request() {
    let server = MockServer::start().await;
    Mock::given(path("/oauth/token")).respond_with(issued_tokens()).expect(0).mount(&server).await;
    let clock = TestClock::new();
    let store = Arc::new(MemoryStore::default());
    let login = login(config(&server), store.clone(), &clock);

    let pending = login.start().await.unwrap();
    let url = pending.authorize_url().clone();
    let completing = tokio::spawn(pending.complete());
    assert_eq!(browser_returns(&http(&clock), &url, "error=access_denied").await, 400);

    let error = completing.await.unwrap().unwrap_err();
    assert!(matches!(error, OAuthError::Authorization { .. }), "{error:?}");
    assert_eq!(store.saves(), 0);
}

#[tokio::test]
async fn a_rejected_code_saves_nothing() {
    let server = MockServer::start().await;
    token_endpoint(
        &server,
        ResponseTemplate::new(400)
            .set_body_raw(fixture("error_invalid_grant.json"), "application/json"),
    )
    .await;
    let clock = TestClock::new();
    let store = Arc::new(MemoryStore::default());
    let login = login(config(&server), store.clone(), &clock);

    let pending = login.start().await.unwrap();
    let url = pending.authorize_url().clone();
    let completing = tokio::spawn(pending.complete());
    browser_returns(&http(&clock), &url, "code=used-code").await;

    let error = completing.await.unwrap().unwrap_err();
    assert!(matches!(error, OAuthError::TokenRejected { status: 400, .. }), "{error:?}");
    assert_eq!(store.saves(), 0);
    login.start().await.unwrap();
}

#[tokio::test]
async fn the_tokens_land_in_a_private_file_store() {
    let server = MockServer::start().await;
    token_endpoint(&server, issued_tokens()).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FileStore::new(dir.path().join("secrets")));
    let clock = TestClock::new();
    let login = login(config(&server), store.clone(), &clock);

    let pending = login.start().await.unwrap();
    let url = pending.authorize_url().clone();
    let completing = tokio::spawn(pending.complete());
    browser_returns(&http(&clock), &url, "code=the-code").await;
    completing.await.unwrap().unwrap();

    let Some(CredentialRecord::OAuth(tokens)) = store.load(&credential()).unwrap() else {
        panic!("no OAuth record was saved");
    };
    assert_eq!(tokens.account_id.as_deref(), Some(ACCOUNT));
    let mode = std::fs::metadata(store.path_of(&credential())).unwrap().permissions();
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777, 0o600);
}

#[tokio::test]
async fn debug_shows_neither_the_state_nor_the_challenge() {
    let server = MockServer::start().await;
    let clock = TestClock::new();
    let login = login(config(&server), Arc::new(MemoryStore::default()), &clock);
    let pending = login.start().await.unwrap();
    let url = pending.authorize_url().clone();

    for debug in [format!("{pending:?}"), format!("{login:?}")] {
        assert!(!debug.contains(&query(&url, "state")), "{debug}");
        assert!(!debug.contains(&query(&url, "code_challenge")), "{debug}");
    }
}
