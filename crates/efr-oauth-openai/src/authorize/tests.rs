use pretty_assertions::assert_eq;

use super::{authorize_url, redirect_uri};
use crate::{OAuthConfig, OAuthError};

#[test]
fn the_redirect_names_the_ipv4_loopback_and_the_callback_path() {
    assert_eq!(redirect_uri(1455), "http://127.0.0.1:1455/auth/callback");
    assert_eq!(redirect_uri(40123), "http://127.0.0.1:40123/auth/callback");
}

#[test]
fn the_authorize_url_carries_every_parameter_in_order() {
    let url = authorize_url(
        &OAuthConfig::default(),
        &redirect_uri(1455),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        "state-123",
    )
    .unwrap();
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("auth.openai.com"));
    assert_eq!(url.path(), "/oauth/authorize");
    let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let expected = [
        ("response_type", "code"),
        ("client_id", "app_EMoamEEZ73f0CkXaXp7hrann"),
        ("redirect_uri", "http://127.0.0.1:1455/auth/callback"),
        ("scope", "openid profile email offline_access"),
        ("code_challenge", "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", "state-123"),
        ("originator", "efr"),
    ];
    let expected: Vec<(String, String)> =
        expected.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect();
    assert_eq!(pairs, expected);
}

#[test]
fn the_authorize_url_encodes_the_redirect_and_the_scopes() {
    let url = authorize_url(&OAuthConfig::default(), &redirect_uri(1455), "c", "s").unwrap();
    let query = url.query().unwrap();
    assert!(
        query.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback"),
        "{query}"
    );
    assert!(query.contains("scope=openid+profile+email+offline_access"), "{query}");
}

#[test]
fn the_config_sets_the_issuer_client_and_originator() {
    let config = OAuthConfig {
        issuer: "http://127.0.0.1:4000/".to_owned(),
        client_id: "app_test".to_owned(),
        originator: "efr-test".to_owned(),
        ..OAuthConfig::default()
    };
    let url = authorize_url(&config, &redirect_uri(0), "c", "s").unwrap();
    assert!(url.as_str().starts_with("http://127.0.0.1:4000/oauth/authorize?"), "{url}");
    let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    assert!(pairs.contains(&("client_id".to_owned(), "app_test".to_owned())), "{pairs:?}");
    assert!(pairs.contains(&("originator".to_owned(), "efr-test".to_owned())), "{pairs:?}");
}

#[test]
fn an_issuer_that_is_not_a_url_is_an_error() {
    let config = OAuthConfig { issuer: "not a url".to_owned(), ..OAuthConfig::default() };
    let error = authorize_url(&config, &redirect_uri(1455), "c", "s").unwrap_err();
    assert!(matches!(error, OAuthError::InvalidIssuer { ref issuer, .. } if issuer == "not a url"));
}
