use std::sync::Arc;

use pretty_assertions::assert_eq;
use secrecy::{ExposeSecret as _, SecretString};

use super::{StaticToken, TokenSource};

fn source() -> Arc<dyn TokenSource> {
    Arc::new(StaticToken::new(SecretString::from("sk-test-key")))
}

#[tokio::test]
async fn a_static_token_returns_its_key() {
    let source = source();
    let token = source.access_token().await.unwrap();
    assert_eq!(token.expose_secret(), "sk-test-key");
}

#[tokio::test]
async fn invalidating_a_static_token_changes_nothing() {
    let source = source();
    source.invalidate().await;
    let token = source.access_token().await.unwrap();
    assert_eq!(token.expose_secret(), "sk-test-key");
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
