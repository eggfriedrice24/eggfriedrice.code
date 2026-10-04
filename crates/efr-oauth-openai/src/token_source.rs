//! `OpenAiTokenSource`: the provider's tokens from the saved login, refreshed ahead of
//! expiry.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use efr_credentials::{CredentialId, OAuthTokens, SecretStore};
use efr_http::HttpClient;
use efr_provider::{AccessToken, ProviderError, TokenSource};
use efr_stdx::time::Clock;
use jiff::{SignedDuration, Timestamp};
use secrecy::ExposeSecret as _;

use crate::token::{self, Grant};
use crate::{OAuthConfig, OAuthError, persist};

/// How long before its expiry an access token is refreshed: five minutes, the window
/// codex uses (`codex-rs/login/src/auth/manager.rs:204`). A request that starts with a
/// token this close to its end could see it expire before the stream finishes.
const REFRESH_AHEAD: SignedDuration = SignedDuration::from_mins(5);

/// The [`TokenSource`] of the subscription login.
///
/// It reads the tokens that [`OpenAiLogin`](crate::OpenAiLogin) saved and keeps them in
/// memory. When less than five minutes remain on the injected clock, the next
/// [`access_token`](TokenSource::access_token) refreshes them and saves the new record;
/// any number of concurrent callers share that one refresh.
/// [`invalidate`](TokenSource::invalidate), called by the provider after a 401, makes
/// the next call refresh once even when the clock says the token is still good.
///
/// A refresh that fails while the current token has not yet expired is logged and the
/// current token is used, so a brief outage of the token endpoint does not fail a
/// request that could still succeed.
pub struct OpenAiTokenSource {
    config: OAuthConfig,
    http: HttpClient,
    store: Arc<dyn SecretStore>,
    credential: CredentialId,
    clock: Arc<dyn Clock>,
    cache: Mutex<Cache>,
    // NOTE: the one tokio mutex of the crate. Its guard is held across the store read
    // and the refresh request, so that a second caller waits for the first refresh
    // instead of sending its own.
    refreshing: tokio::sync::Mutex<()>,
}

/// The tokens in memory, and whether the provider rejected them.
#[derive(Debug, Default)]
struct Cache {
    tokens: Option<OAuthTokens>,
    rejected: bool,
}

impl OpenAiTokenSource {
    /// A source for the tokens saved in `store` under `credential`, refreshed at
    /// `config`'s token endpoint through `http`, with expiry judged on `clock`.
    pub fn new(
        config: OAuthConfig,
        http: HttpClient,
        store: Arc<dyn SecretStore>,
        credential: CredentialId,
        clock: Arc<dyn Clock>,
    ) -> Self {
        OpenAiTokenSource {
            config,
            http,
            store,
            credential,
            clock,
            cache: Mutex::new(Cache::default()),
            refreshing: tokio::sync::Mutex::new(()),
        }
    }

    /// Forgets the tokens in memory, so the next call reads the store again. The daemon
    /// calls it after a new login, which may be for another account.
    pub fn clear_cache(&self) {
        *self.lock_cache() = Cache::default();
    }

    /// The token, refreshed first when it is due.
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "the guard serialises refreshes: concurrent callers wait for one refresh"
    )]
    async fn token(&self) -> Result<AccessToken, OAuthError> {
        if let Some(token) = self.cached() {
            return Ok(token);
        }
        let _refreshing = self.refreshing.lock().await;
        // Another caller may have refreshed while this one waited for the guard.
        if let Some(token) = self.cached() {
            return Ok(token);
        }
        self.renew().await
    }

    /// The cached token, when it is neither rejected nor due for a refresh.
    fn cached(&self) -> Option<AccessToken> {
        let deadline = refresh_deadline(self.clock.now());
        let cache = self.lock_cache();
        let tokens = cache.tokens.as_ref()?;
        (!cache.rejected && !tokens.expires_before(deadline)).then(|| access_token(tokens))
    }

    /// Reads the store and refreshes when the stored token is due or rejected. Runs
    /// under the refresh guard.
    async fn renew(&self) -> Result<AccessToken, OAuthError> {
        let rejected = {
            let cache = self.lock_cache();
            cache
                .tokens
                .as_ref()
                .filter(|_| cache.rejected)
                .map(|tokens| tokens.access_token.clone())
        };
        // The store, not the cache, is the truth here: a login since the cache was filled
        // has replaced the record, and its token was never rejected.
        let stored = persist::load_tokens(&self.store, &self.credential).await?;
        let now = self.clock.now();
        let is_rejected = rejected
            .is_some_and(|token| token.expose_secret() == stored.access_token.expose_secret());
        let still_valid = !is_rejected && !stored.expires_before(now);
        if !is_rejected && !stored.expires_before(refresh_deadline(now)) {
            return Ok(self.remember(stored));
        }
        let Some(refresh_token) = stored.refresh_token.clone() else {
            return if still_valid {
                Ok(self.remember(stored))
            } else {
                Err(OAuthError::NoRefreshToken { id: self.credential.clone() })
            };
        };
        tracing::debug!(
            credential = %self.credential,
            rejected = is_rejected,
            "refreshing the access token"
        );
        let grant = Grant::RefreshToken { refresh_token: &refresh_token };
        match token::request(&self.http, &self.config, grant, token::ENCODING).await {
            Ok(response) => {
                let renewed = response.into_tokens(self.clock.now(), Some(&stored));
                let token = self.remember(renewed.clone());
                if let Err(error) =
                    persist::save_tokens(&self.store, &self.credential, renewed).await
                {
                    // NOTE: the refresh token may have rotated, so the stored one may no
                    // longer work after a restart; the request in hand still can.
                    tracing::warn!(%error, "the refreshed tokens could not be saved");
                }
                Ok(token)
            }
            Err(error) if still_valid => {
                tracing::warn!(
                    %error,
                    "the token refresh failed; the current token stays in use until it expires"
                );
                Ok(self.remember(stored))
            }
            Err(error) => Err(error),
        }
    }

    /// Puts `tokens` in the cache as not rejected and returns their access token.
    fn remember(&self, tokens: OAuthTokens) -> AccessToken {
        let token = access_token(&tokens);
        *self.lock_cache() = Cache { tokens: Some(tokens), rejected: false };
        token
    }

    fn lock_cache(&self) -> MutexGuard<'_, Cache> {
        // Every write replaces the whole cache or one flag, so a panic elsewhere while
        // holding the lock cannot leave it half written.
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl TokenSource for OpenAiTokenSource {
    async fn access_token(&self) -> Result<AccessToken, ProviderError> {
        self.token().await.map_err(|error| match error {
            OAuthError::NotLoggedIn { .. } => ProviderError::NotLoggedIn,
            error => ProviderError::Token { source: Box::new(error) },
        })
    }

    async fn invalidate(&self) {
        let mut cache = self.lock_cache();
        if cache.tokens.is_some() {
            cache.rejected = true;
        }
    }
}

impl fmt::Debug for OpenAiTokenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiTokenSource")
            .field("config", &self.config)
            .field("credential", &self.credential)
            .finish_non_exhaustive()
    }
}

/// A token due before this instant is refreshed.
fn refresh_deadline(now: Timestamp) -> Timestamp {
    now.checked_add(REFRESH_AHEAD).unwrap_or(Timestamp::MAX)
}

fn access_token(tokens: &OAuthTokens) -> AccessToken {
    let token = AccessToken::new(tokens.access_token.clone());
    match &tokens.account_id {
        Some(account_id) => token.with_account_id(account_id.clone()),
        None => token,
    }
}

#[cfg(test)]
mod tests;
