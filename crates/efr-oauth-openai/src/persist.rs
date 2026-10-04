//! The credential store, called from async code.
//!
//! `SecretStore` methods block on file or keyring IO, so every call runs on tokio's
//! blocking pool, as the trait asks.

use std::sync::Arc;

use efr_credentials::{CredentialId, CredentialRecord, OAuthTokens, SecretStore};

use crate::OAuthError;

/// The OAuth tokens stored under `id`.
pub(crate) async fn load_tokens(
    store: &Arc<dyn SecretStore>,
    id: &CredentialId,
) -> Result<OAuthTokens, OAuthError> {
    let task_store = Arc::clone(store);
    let task_id = id.clone();
    let loaded = tokio::task::spawn_blocking(move || task_store.load(&task_id))
        .await
        .map_err(|source| OAuthError::StoreTask { id: id.clone(), source })?
        .map_err(|source| OAuthError::Store { id: id.clone(), source })?;
    match loaded {
        Some(CredentialRecord::OAuth(tokens)) => Ok(tokens),
        Some(_) => Err(OAuthError::NotOAuth { id: id.clone() }),
        None => Err(OAuthError::NotLoggedIn { id: id.clone() }),
    }
}

/// Saves `tokens` under `id`, replacing the record there.
pub(crate) async fn save_tokens(
    store: &Arc<dyn SecretStore>,
    id: &CredentialId,
    tokens: OAuthTokens,
) -> Result<(), OAuthError> {
    let task_store = Arc::clone(store);
    let task_id = id.clone();
    tokio::task::spawn_blocking(move || task_store.save(&task_id, &CredentialRecord::OAuth(tokens)))
        .await
        .map_err(|source| OAuthError::StoreTask { id: id.clone(), source })?
        .map_err(|source| OAuthError::Store { id: id.clone(), source })
}

#[cfg(test)]
mod tests;
