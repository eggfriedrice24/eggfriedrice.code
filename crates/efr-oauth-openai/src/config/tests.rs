use std::time::Duration;

use pretty_assertions::assert_eq;

use super::OAuthConfig;

#[test]
fn the_default_holds_openais_values() {
    let config = OAuthConfig::default();
    assert_eq!(config.issuer, "https://auth.openai.com");
    assert_eq!(config.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
    assert_eq!(config.originator, "efr");
    assert_eq!(config.callback_port, 1455);
    assert_eq!(config.login_timeout, Duration::from_secs(600));
}

#[test]
fn endpoints_join_the_issuer_with_or_without_a_trailing_slash() {
    let mut config = OAuthConfig::default();
    assert_eq!(config.endpoint("/oauth/token"), "https://auth.openai.com/oauth/token");
    config.issuer = "http://127.0.0.1:4000/".to_owned();
    assert_eq!(config.endpoint("/oauth/token"), "http://127.0.0.1:4000/oauth/token");
}
