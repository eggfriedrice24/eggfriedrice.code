//! Where a provider gets its access token.

use std::fmt;

use async_trait::async_trait;
use secrecy::SecretString;

use crate::ProviderError;

/// Hands a provider the access token for each request.
///
/// This is the edge that keeps a provider client away from login state: a provider
/// holds an `Arc<dyn TokenSource>` and never sees a refresh token or learns how its
/// token was obtained. `efr-oauth-openai` implements it for the subscription login,
/// refreshing ahead of expiry; [`StaticToken`] implements it for an API key. The daemon
/// composes the two halves from the stored credentials.
#[async_trait]
pub trait TokenSource: Send + Sync + fmt::Debug {
    /// The token to send with the next request. It may refresh first, so it may take a
    /// network round trip. Fails with [`ProviderError::NotLoggedIn`] when there are no
    /// credentials, and with [`ProviderError::Token`] when they cannot be turned into a
    /// token.
    async fn access_token(&self) -> Result<SecretString, ProviderError>;

    /// Marks the current token as rejected, because the provider answered 401 with it.
    /// The next [`access_token`](TokenSource::access_token) refreshes instead of
    /// returning it again. A source that cannot refresh does nothing.
    async fn invalidate(&self);
}

/// A token that never changes: an API key from the config or the credential store.
///
/// [`invalidate`](TokenSource::invalidate) does nothing, so a rejected key stays
/// rejected and the provider reports [`ProviderError::Unauthorized`] after its one
/// retry. `Debug` does not show the key.
#[derive(Debug, Clone)]
pub struct StaticToken {
    token: SecretString,
}

impl StaticToken {
    /// A source that always returns `token`.
    pub fn new(token: SecretString) -> Self {
        StaticToken { token }
    }
}

#[async_trait]
impl TokenSource for StaticToken {
    async fn access_token(&self) -> Result<SecretString, ProviderError> {
        Ok(self.token.clone())
    }

    async fn invalidate(&self) {}
}

#[cfg(test)]
mod tests;
