use std::sync::Arc;
use std::time::Duration;

use efr_credentials::CredentialRecord;
use efr_provider::{ExposeSecret as _, ProviderError, SecretString, TokenSource};
use efr_test_support::TestClock;
use futures::future::join_all;
use pretty_assertions::assert_eq;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::OpenAiTokenSource;
use crate::OAuthError;
use crate::testing::{ACCOUNT, MemoryStore, at, config, credential, fixture, http, tokens};

/// True for the access token of `fixtures/tokens/refresh_token.json`, told apart by its
/// claims.
fn is_refreshed(token: &str) -> bool {
    let claims = crate::claims::parse(token).unwrap();
    claims.expires_at == Some(at(7200)) && claims.account_id.as_deref() == Some(ACCOUNT)
}

fn source(server: &MockServer, store: Arc<MemoryStore>, clock: &TestClock) -> OpenAiTokenSource {
    OpenAiTokenSource::new(config(server), http(clock), store, credential(), clock.shared())
}

/// Answers every refresh grant with the fixture `name` and expects `times` of them.
async fn refreshes(server: &MockServer, status: u16, name: &str, times: u64) {
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(status).set_body_raw(fixture(name), "application/json"))
        .expect(times)
        .mount(server)
        .await;
}

async fn refresh_count(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

async fn secret_of(source: &OpenAiTokenSource) -> String {
    source.access_token().await.unwrap().secret().expose_secret().to_owned()
}

fn oauth_error(error: &ProviderError) -> &OAuthError {
    let ProviderError::Token { source } = error else {
        panic!("unexpected error: {error:?}");
    };
    source.downcast_ref::<OAuthError>().unwrap()
}

#[tokio::test]
async fn a_fresh_token_comes_back_with_its_account_without_a_refresh() {
    let server = MockServer::start().await;
    refreshes(&server, 200, "refresh_token.json", 0).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(3600))));
    let source = source(&server, store, &clock);

    let token = source.access_token().await.unwrap();
    assert_eq!(token.secret().expose_secret(), "access-1");
    assert_eq!(token.account_id(), Some(ACCOUNT));
}

#[tokio::test]
async fn the_refresh_starts_when_five_minutes_remain_on_the_clock() {
    let server = MockServer::start().await;
    refreshes(&server, 200, "refresh_token.json", 1).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(600))));
    let source = source(&server, store, &clock);

    assert_eq!(secret_of(&source).await, "access-1");
    clock.advance(Duration::from_secs(4 * 60));
    assert_eq!(secret_of(&source).await, "access-1");
    clock.advance(Duration::from_secs(59));
    assert_eq!(secret_of(&source).await, "access-1", "5 min 1 s remain");
    assert_eq!(refresh_count(&server).await, 0);

    clock.advance(Duration::from_secs(1));
    assert!(is_refreshed(&secret_of(&source).await), "5 min remain");
    assert_eq!(refresh_count(&server).await, 1);
    clock.advance(Duration::from_secs(60));
    assert!(is_refreshed(&secret_of(&source).await));
}

#[tokio::test]
async fn the_refreshed_tokens_are_saved_with_the_rotated_refresh_token() {
    let server = MockServer::start().await;
    refreshes(&server, 200, "refresh_token.json", 1).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(60))));
    let source = source(&server, store.clone(), &clock);

    let token = source.access_token().await.unwrap();
    assert_eq!(token.account_id(), Some(ACCOUNT));
    let request = &server.received_requests().await.unwrap()[0];
    let body = String::from_utf8(request.body.clone()).unwrap();
    assert!(body.contains("refresh_token=rt-1"), "{body}");

    assert_eq!(store.saves(), 1);
    let saved = store.tokens().unwrap();
    assert!(is_refreshed(saved.access_token.expose_secret()));
    assert_eq!(saved.refresh_token.unwrap().expose_secret(), "rt_fixture_rotated");
    assert_eq!(saved.expires_at, Some(at(7200)));
    assert_eq!(saved.account_id.as_deref(), Some(ACCOUNT));
}

#[tokio::test]
async fn a_refresh_that_returns_only_an_access_token_keeps_the_rest() {
    let server = MockServer::start().await;
    refreshes(&server, 200, "refresh_access_only.json", 1).await;
    let clock = TestClock::new();
    let mut old = tokens("access-1", Some("rt-1"), Some(at(60)));
    old.id_token = Some(SecretString::from("id-token-1"));
    let store = MemoryStore::with_tokens(old);
    let source = source(&server, store.clone(), &clock);

    let token = source.access_token().await.unwrap();
    assert_eq!(token.account_id(), Some(ACCOUNT));
    let saved = store.tokens().unwrap();
    assert_eq!(saved.refresh_token.unwrap().expose_secret(), "rt-1");
    assert_eq!(saved.id_token.unwrap().expose_secret(), "id-token-1");
    assert_eq!(saved.expires_at, Some(at(3600)), "expires_in counts from the clock");
}

#[tokio::test]
async fn concurrent_callers_share_one_refresh() {
    let server = MockServer::start().await;
    refreshes(&server, 200, "refresh_token.json", 1).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(60))));
    let source = source(&server, store.clone(), &clock);

    let results = join_all((0..16).map(|_| source.access_token())).await;
    for result in results {
        assert!(is_refreshed(result.unwrap().secret().expose_secret()));
    }
    assert_eq!(refresh_count(&server).await, 1);
    assert_eq!(store.saves(), 1);
}

#[tokio::test]
async fn invalidate_forces_one_refresh_of_a_token_the_clock_calls_fresh() {
    let server = MockServer::start().await;
    refreshes(&server, 200, "refresh_token.json", 1).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(3600))));
    let source = source(&server, store, &clock);

    assert_eq!(secret_of(&source).await, "access-1");
    source.invalidate().await;
    let results = join_all((0..8).map(|_| source.access_token())).await;
    for result in results {
        assert!(is_refreshed(result.unwrap().secret().expose_secret()));
    }
    assert!(is_refreshed(&secret_of(&source).await));
    assert_eq!(refresh_count(&server).await, 1);
}

#[tokio::test]
async fn invalidate_before_any_token_was_handed_out_does_nothing() {
    let server = MockServer::start().await;
    refreshes(&server, 200, "refresh_token.json", 0).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(3600))));
    let source = source(&server, store, &clock);

    source.invalidate().await;
    assert_eq!(secret_of(&source).await, "access-1");
}

#[tokio::test]
async fn a_rejected_token_gives_way_to_a_new_login_without_a_refresh() {
    let server = MockServer::start().await;
    refreshes(&server, 200, "refresh_token.json", 0).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(3600))));
    let source = source(&server, store.clone(), &clock);

    assert_eq!(secret_of(&source).await, "access-1");
    store.put(CredentialRecord::OAuth(tokens("access-2", Some("rt-2"), Some(at(3600)))));
    source.invalidate().await;
    assert_eq!(secret_of(&source).await, "access-2");
}

#[tokio::test]
async fn clear_cache_reads_a_new_login_from_the_store() {
    let server = MockServer::start().await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(3600))));
    let source = source(&server, store.clone(), &clock);

    assert_eq!(secret_of(&source).await, "access-1");
    let mut other = tokens("access-2", Some("rt-2"), Some(at(3600)));
    other.account_id = Some("other-account".to_owned());
    store.put(CredentialRecord::OAuth(other));
    assert_eq!(secret_of(&source).await, "access-1", "the cache still holds the old login");

    source.clear_cache();
    let token = source.access_token().await.unwrap();
    assert_eq!(token.secret().expose_secret(), "access-2");
    assert_eq!(token.account_id(), Some("other-account"));
}

#[tokio::test]
async fn no_stored_credential_means_not_logged_in() {
    let server = MockServer::start().await;
    let clock = TestClock::new();
    let source = source(&server, Arc::new(MemoryStore::default()), &clock);

    let error = source.access_token().await.unwrap_err();
    assert!(matches!(error, ProviderError::NotLoggedIn), "{error:?}");
}

#[tokio::test]
async fn an_api_key_record_is_not_a_login() {
    let server = MockServer::start().await;
    let clock = TestClock::new();
    let store = Arc::new(MemoryStore::default());
    store.put(CredentialRecord::ApiKey { key: SecretString::from("sk-test") });
    let source = source(&server, store, &clock);

    let error = source.access_token().await.unwrap_err();
    let oauth = oauth_error(&error);
    assert!(matches!(oauth, OAuthError::NotOAuth { .. }), "{oauth:?}");
    assert!(oauth.needs_login());
    assert!(!format!("{error:?}").contains("sk-test"));
}

#[tokio::test]
async fn without_a_refresh_token_a_token_is_used_until_it_expires() {
    let server = MockServer::start().await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", None, Some(at(120))));
    let source = source(&server, store, &clock);

    assert_eq!(secret_of(&source).await, "access-1", "two minutes remain");
    clock.advance(Duration::from_secs(120));
    let error = source.access_token().await.unwrap_err();
    assert!(matches!(oauth_error(&error), OAuthError::NoRefreshToken { .. }), "{error:?}");
    assert_eq!(refresh_count(&server).await, 0);
}

#[tokio::test]
async fn a_rejected_refresh_of_an_expired_token_needs_a_new_login() {
    let server = MockServer::start().await;
    refreshes(&server, 400, "error_invalid_grant.json", 1).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(-1))));
    let source = source(&server, store.clone(), &clock);

    let error = source.access_token().await.unwrap_err();
    let oauth = oauth_error(&error);
    assert!(matches!(oauth, OAuthError::TokenRejected { status: 400, .. }), "{oauth:?}");
    assert!(oauth.needs_login());
    assert_eq!(store.saves(), 0);
}

#[tokio::test]
async fn a_failed_refresh_keeps_a_token_that_has_not_expired() {
    let server = MockServer::start().await;
    refreshes(&server, 400, "error_refresh_token_expired.json", 3).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(120))));
    let source = source(&server, store, &clock);

    assert_eq!(secret_of(&source).await, "access-1");
    // The token is still due, so the next call tries again.
    assert_eq!(secret_of(&source).await, "access-1");
    clock.advance(Duration::from_secs(120));
    assert!(source.access_token().await.is_err());
}

#[tokio::test]
async fn a_failed_refresh_never_falls_back_to_a_rejected_token() {
    let server = MockServer::start().await;
    refreshes(&server, 400, "error_invalid_grant.json", 1).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), Some(at(3600))));
    let source = source(&server, store, &clock);

    assert_eq!(secret_of(&source).await, "access-1");
    source.invalidate().await;
    let error = source.access_token().await.unwrap_err();
    assert!(matches!(oauth_error(&error), OAuthError::TokenRejected { .. }), "{error:?}");
}

#[tokio::test]
async fn a_refreshed_token_is_used_even_when_saving_it_fails() {
    let server = MockServer::start().await;
    refreshes(&server, 200, "refresh_token.json", 1).await;
    let clock = TestClock::new();
    let store = MemoryStore::failing_saves(tokens("access-1", Some("rt-1"), Some(at(60))));
    let source = source(&server, store.clone(), &clock);

    assert!(is_refreshed(&secret_of(&source).await));
    assert!(is_refreshed(&secret_of(&source).await), "the cache holds the new token");
    assert_eq!(store.saves(), 1);
    assert_eq!(store.access_token().as_deref(), Some("access-1"));
}

#[tokio::test]
async fn a_token_without_an_expiry_is_never_refreshed_ahead() {
    let server = MockServer::start().await;
    refreshes(&server, 200, "refresh_token.json", 0).await;
    let clock = TestClock::new();
    let store = MemoryStore::with_tokens(tokens("access-1", Some("rt-1"), None));
    let source = source(&server, store, &clock);

    clock.advance(Duration::from_secs(365 * 24 * 3600));
    assert_eq!(secret_of(&source).await, "access-1");
}

#[tokio::test]
async fn debug_shows_no_token() {
    let server = MockServer::start().await;
    let clock = TestClock::new();
    let store =
        MemoryStore::with_tokens(tokens("access-secret", Some("rt-secret"), Some(at(3600))));
    let source = source(&server, store, &clock);
    source.access_token().await.unwrap();

    let debug = format!("{source:?}");
    assert!(!debug.contains("access-secret"), "{debug}");
    assert!(!debug.contains("rt-secret"), "{debug}");
    assert!(debug.contains("openai-subscription"), "{debug}");
}

#[test]
fn the_source_is_a_token_source_object() {
    fn assert_token_source(_: Arc<dyn TokenSource>) {}
    let clock = TestClock::new();
    let source = OpenAiTokenSource::new(
        crate::OAuthConfig::default(),
        http(&clock),
        Arc::new(MemoryStore::default()),
        credential(),
        clock.shared(),
    );
    assert_token_source(Arc::new(source));
}
