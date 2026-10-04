use std::sync::Arc;

use efr_credentials::{CredentialRecord, SecretStore};
use pretty_assertions::assert_eq;
use secrecy::{ExposeSecret as _, SecretString};

use super::{load_tokens, save_tokens};
use crate::OAuthError;
use crate::testing::{MemoryStore, at, credential, tokens};

#[tokio::test]
async fn saved_tokens_load_back() {
    let memory = Arc::new(MemoryStore::default());
    let store: Arc<dyn SecretStore> = memory.clone();
    save_tokens(&store, &credential(), tokens("access-1", Some("rt-1"), Some(at(60))))
        .await
        .unwrap();

    let loaded = load_tokens(&store, &credential()).await.unwrap();
    assert_eq!(loaded.access_token.expose_secret(), "access-1");
    assert_eq!(loaded.expires_at, Some(at(60)));
    assert_eq!(memory.saves(), 1);
}

#[tokio::test]
async fn a_missing_record_is_not_logged_in() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let error = load_tokens(&store, &credential()).await.unwrap_err();
    assert!(matches!(error, OAuthError::NotLoggedIn { ref id } if *id == credential()));
}

#[tokio::test]
async fn an_api_key_record_is_not_oauth() {
    let memory = Arc::new(MemoryStore::default());
    memory.put(CredentialRecord::ApiKey { key: SecretString::from("sk-test") });
    let store: Arc<dyn SecretStore> = memory;
    let error = load_tokens(&store, &credential()).await.unwrap_err();
    assert!(matches!(error, OAuthError::NotOAuth { .. }), "{error:?}");
}

#[tokio::test]
async fn a_failing_save_names_the_credential() {
    let store: Arc<dyn SecretStore> = MemoryStore::failing_saves(tokens("a", None, None));
    let error = save_tokens(&store, &credential(), tokens("b", None, None)).await.unwrap_err();
    assert!(matches!(error, OAuthError::Store { ref id, .. } if *id == credential()));
}
