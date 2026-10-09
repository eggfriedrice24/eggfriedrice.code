use std::sync::Arc;

use pretty_assertions::assert_eq;
use secrecy::{ExposeSecret as _, SecretString};

use super::{AccessToken, StaticToken, TokenSource};

fn source() -> Arc<dyn TokenSource> {
    Arc::new(StaticToken::new(SecretString::from("sk-test-key")))
}

#[tokio::test]
async fn a_static_token_returns_its_key_without_an_account() {
    let source = source();
    let token = source.access_token().await.unwrap();
    assert_eq!(token.secret().expose_secret(), "sk-test-key");
    assert_eq!(token.account_id(), None);
}

#[test]
fn a_static_token_cannot_refresh() {
    assert!(!source().refreshable());
}

#[tokio::test]
async fn invalidating_a_static_token_changes_nothing() {
    let source = source();
    source.invalidate().await;
    let token = source.access_token().await.unwrap();
    assert_eq!(token.secret().expose_secret(), "sk-test-key");
}

#[test]
fn an_access_token_carries_its_account() {
    let token =
        AccessToken::new(SecretString::from("eyJ.access.sig")).with_account_id("0b6c2f4e-account");
    assert_eq!(token.secret().expose_secret(), "eyJ.access.sig");
    assert_eq!(token.account_id(), Some("0b6c2f4e-account"));
}

#[test]
fn the_debug_of_an_access_token_shows_the_account_but_not_the_token() {
    let token =
        AccessToken::new(SecretString::from("eyJ.access.sig")).with_account_id("0b6c2f4e-account");
    let debug = format!("{token:?}");
    assert!(!debug.contains("eyJ.access.sig"), "{debug}");
    assert!(debug.contains("REDACTED"), "{debug}");
    assert!(debug.contains("0b6c2f4e-account"), "{debug}");
}

#[test]
fn debug_does_not_show_the_key() {
    let source = StaticToken::new(SecretString::from("sk-test-key"));
    let debug = format!("{source:?}");
    assert!(!debug.contains("sk-test-key"), "{debug}");
    assert!(debug.contains("REDACTED"), "{debug}");
}

#[test]
fn the_trait_object_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn TokenSource>();
}
