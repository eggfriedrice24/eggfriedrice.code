use efr_test_support::TestClock;
use pretty_assertions::assert_eq;
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::json;
use wiremock::matchers::{body_json, body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{Encoding, Grant, TokenResponse, request, scrub};
use crate::testing::{ACCOUNT, at, config, fixture, http, jwt, tokens};
use crate::{GrantKind, OAuthConfig, OAuthError};

fn json_body(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(fixture(name), "application/json")
}

fn error_body(status: u16, name: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_raw(fixture(name), "application/json")
}

async fn exchange(server: &MockServer, encoding: Encoding) -> Result<TokenResponse, OAuthError> {
    let code = SecretString::from("code-from-callback");
    let verifier = SecretString::from("verifier-1234");
    let grant = Grant::AuthorizationCode {
        code: &code,
        redirect_uri: "http://127.0.0.1:1455/auth/callback",
        verifier: &verifier,
    };
    request(&http(&TestClock::new()), &config(server), grant, encoding).await
}

async fn refresh(server: &MockServer, refresh_token: &str) -> Result<TokenResponse, OAuthError> {
    let refresh_token = SecretString::from(refresh_token);
    let grant = Grant::RefreshToken { refresh_token: &refresh_token };
    request(&http(&TestClock::new()), &config(server), grant, Encoding::Form).await
}

#[tokio::test]
async fn the_code_exchange_is_form_encoded() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(header("accept", "application/json"))
        .and(body_string(
            "grant_type=authorization_code&client_id=app_EMoamEEZ73f0CkXaXp7hrann\
             &code=code-from-callback\
             &redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback\
             &code_verifier=verifier-1234",
        ))
        .respond_with(json_body("authorization_code.json"))
        .expect(1)
        .mount(&server)
        .await;

    let response = exchange(&server, Encoding::Form).await.unwrap();
    assert_eq!(response.refresh_token.unwrap().expose_secret(), "rt_fixture_issued_at_login");
    assert!(response.id_token.is_some());
    assert_eq!(response.expires_in, Some(3600));
}

#[tokio::test]
async fn the_refresh_grant_is_form_encoded() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string(
            "grant_type=refresh_token&client_id=app_EMoamEEZ73f0CkXaXp7hrann\
             &refresh_token=rt%2Bold%2Fvalue",
        ))
        .respond_with(json_body("refresh_token.json"))
        .expect(1)
        .mount(&server)
        .await;

    let response = refresh(&server, "rt+old/value").await.unwrap();
    assert_eq!(response.refresh_token.unwrap().expose_secret(), "rt_fixture_rotated");
    assert_eq!(response.expires_in, None);
}

#[tokio::test]
async fn the_json_switch_sends_the_same_parameters_as_an_object() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({
            "grant_type": "authorization_code",
            "client_id": "app_EMoamEEZ73f0CkXaXp7hrann",
            "code": "code-from-callback",
            "redirect_uri": "http://127.0.0.1:1455/auth/callback",
            "code_verifier": "verifier-1234",
        })))
        .respond_with(json_body("authorization_code.json"))
        .expect(1)
        .mount(&server)
        .await;

    exchange(&server, Encoding::Json).await.unwrap();
}

#[tokio::test]
async fn an_oauth_error_body_names_the_code_and_description() {
    let server = MockServer::start().await;
    Mock::given(path("/oauth/token"))
        .respond_with(error_body(400, "error_invalid_grant.json"))
        .mount(&server)
        .await;

    let error = refresh(&server, "rt-old").await.unwrap_err();
    let OAuthError::TokenRejected { grant, status, error, description } = error else {
        panic!("unexpected error: {error:?}");
    };
    assert_eq!(grant, GrantKind::RefreshToken);
    assert_eq!(status, 400);
    assert_eq!(error.as_deref(), Some("invalid_grant"));
    assert_eq!(description.as_deref(), Some("Invalid refresh token"));
}

#[tokio::test]
async fn an_openai_error_body_names_the_nested_code_and_message() {
    let server = MockServer::start().await;
    Mock::given(path("/oauth/token"))
        .respond_with(error_body(401, "error_refresh_token_expired.json"))
        .mount(&server)
        .await;

    let error = refresh(&server, "rt-old").await.unwrap_err();
    let OAuthError::TokenRejected { status, error, description, .. } = error else {
        panic!("unexpected error: {error:?}");
    };
    assert_eq!(status, 401);
    assert_eq!(error.as_deref(), Some("refresh_token_expired"));
    assert_eq!(
        description.as_deref(),
        Some("Your refresh token has expired. Please log out and sign in again.")
    );
}

#[tokio::test]
async fn a_body_that_is_not_json_still_reports_the_status() {
    let server = MockServer::start().await;
    Mock::given(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"))
        .mount(&server)
        .await;

    let error = exchange(&server, Encoding::Form).await.unwrap_err();
    assert!(
        matches!(
            error,
            OAuthError::TokenRejected {
                grant: GrantKind::AuthorizationCode,
                status: 502,
                error: None,
                description: None,
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn an_echoed_secret_is_scrubbed_from_the_error() {
    let server = MockServer::start().await;
    Mock::given(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "refresh token rt-secret-value is not valid",
        })))
        .mount(&server)
        .await;

    let error = refresh(&server, "rt-secret-value").await.unwrap_err();
    let debug = format!("{error:?}");
    assert!(!debug.contains("rt-secret-value"), "{debug}");
    assert!(debug.contains("refresh token [REDACTED] is not valid"), "{debug}");
}

#[tokio::test]
async fn a_success_body_without_an_access_token_is_a_decode_error() {
    let server = MockServer::start().await;
    Mock::given(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "token_type": "Bearer" })))
        .mount(&server)
        .await;

    let error = refresh(&server, "rt-old").await.unwrap_err();
    assert!(
        matches!(error, OAuthError::TokenDecode { grant: GrantKind::RefreshToken, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_is_a_request_error() {
    // A port that was free a moment ago; wiremock keeps its servers in a pool, so a
    // dropped mock server would still answer.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config =
        OAuthConfig { issuer: format!("http://127.0.0.1:{port}"), ..OAuthConfig::default() };
    let refresh_token = SecretString::from("rt-old");
    let grant = Grant::RefreshToken { refresh_token: &refresh_token };
    let clock = TestClock::new();
    let http = http(&clock);
    let sending = request(&http, &config, grant, Encoding::Form);
    let mut sending = std::pin::pin!(sending);
    // A refused connection is retried with backoff on the test clock; move the clock
    // until the policy gives up.
    let error = loop {
        tokio::select! {
            result = &mut sending => break result.unwrap_err(),
            () = clock.wait_for_sleeps(1) => {
                clock.advance_to_next_deadline();
            }
        }
    };
    assert!(
        matches!(error, OAuthError::TokenRequest { grant: GrantKind::RefreshToken, .. }),
        "{error:?}"
    );
}

#[test]
fn expires_in_sets_the_expiry_from_the_time_of_the_answer() {
    let response = TokenResponse {
        access_token: SecretString::from(jwt(&json!({ "exp": at(9999).as_second() }))),
        refresh_token: None,
        id_token: None,
        expires_in: Some(3600),
    };
    let tokens = response.into_tokens(at(100), None);
    assert_eq!(tokens.expires_at, Some(at(3700)));
}

#[test]
fn without_expires_in_the_access_tokens_exp_claim_is_the_expiry() {
    let response = TokenResponse {
        access_token: SecretString::from(jwt(&json!({ "exp": at(7200).as_second() }))),
        refresh_token: None,
        id_token: None,
        expires_in: None,
    };
    assert_eq!(response.into_tokens(at(0), None).expires_at, Some(at(7200)));
}

#[test]
fn an_opaque_token_without_expires_in_has_no_known_expiry() {
    let response = TokenResponse {
        access_token: SecretString::from("opaque"),
        refresh_token: None,
        id_token: None,
        expires_in: None,
    };
    assert_eq!(response.into_tokens(at(0), None).expires_at, None);
}

#[test]
fn a_refresh_without_new_tokens_keeps_the_old_refresh_token_id_token_and_account() {
    let mut previous = tokens("old-access", Some("rt-old"), Some(at(60)));
    previous.id_token = Some(SecretString::from("old-id-token"));
    let response = TokenResponse {
        access_token: SecretString::from("new-access"),
        refresh_token: None,
        id_token: None,
        expires_in: Some(3600),
    };
    let renewed = response.into_tokens(at(0), Some(&previous));
    assert_eq!(renewed.access_token.expose_secret(), "new-access");
    assert_eq!(renewed.refresh_token.unwrap().expose_secret(), "rt-old");
    assert_eq!(renewed.id_token.unwrap().expose_secret(), "old-id-token");
    assert_eq!(renewed.account_id.as_deref(), Some(ACCOUNT));
}

#[test]
fn new_tokens_replace_the_old_ones_and_name_their_own_account() {
    let previous = tokens("old-access", Some("rt-old"), Some(at(60)));
    let response = TokenResponse {
        access_token: SecretString::from("new-access"),
        refresh_token: Some(SecretString::from("rt-new")),
        id_token: Some(SecretString::from(jwt(&json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "other-account" },
        })))),
        expires_in: None,
    };
    let renewed = response.into_tokens(at(0), Some(&previous));
    assert_eq!(renewed.refresh_token.unwrap().expose_secret(), "rt-new");
    assert_eq!(renewed.account_id.as_deref(), Some("other-account"));
}

#[test]
fn scrubbing_replaces_secrets_before_it_shortens() {
    let long = format!("{}rt-secret", "x".repeat(295));
    let clean = scrub(&long, &["rt-secret"]);
    assert!(!clean.contains("rt-"), "{clean}");
    assert!(clean.ends_with("..."), "{clean}");
    assert_eq!(clean.chars().count(), 303);
    assert_eq!(scrub("fine", &[""]), "fine");
}
