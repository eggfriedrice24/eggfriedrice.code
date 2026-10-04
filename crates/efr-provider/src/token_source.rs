//! Where a provider gets its access token.

use std::fmt;

use async_trait::async_trait;
use secrecy::SecretString;

use crate::ProviderError;

/// An access token and the account it belongs to.
///
/// The subscription backend routes each request by a `chatgpt-account-id` header whose
/// value is a claim of the token. The source that holds the token reads the claim
/// (`efr-oauth-openai` parses the JWT), so a provider never parses a token, and the
/// token and its account always come from the same login, even across a refresh or a
/// new login between two requests. `Debug` does not show the token.
#[derive(Debug, Clone)]
pub struct AccessToken {
    secret: SecretString,
    account_id: Option<String>,
}

impl AccessToken {
    /// A token that names no account, such as an API key.
    pub fn new(secret: SecretString) -> Self {
        AccessToken { secret, account_id: None }
    }

    /// The same token, belonging to the account `account_id`.
    pub fn with_account_id(mut self, account_id: impl Into<String>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }

    /// The bearer token.
    pub fn secret(&self) -> &SecretString {
        &self.secret
    }

    /// The account the token belongs to, when its source knows one. It identifies the
    /// account but grants nothing, so it is not a secret.
    pub fn account_id(&self) -> Option<&str> {
        self.account_id.as_deref()
    }
}

/// Hands a provider the access token for each request.
///
/// This is the edge that keeps a provider client away from login state: a provider
/// holds an `Arc<dyn TokenSource>` and never sees a refresh token or learns how its
/// token was obtained. `efr-oauth-openai` implements it for the subscription login,
/// refreshing ahead of expiry; [`StaticToken`] implements it for an API key. The daemon
/// composes the two halves from the stored credentials.
#[async_trait]
pub trait TokenSource: Send + Sync + fmt::Debug {
    /// The token to send with the next request, with its account when the source knows
    /// one. It may refresh first, so it may take a network round trip. Fails with
    /// [`ProviderError::NotLoggedIn`] when there are no credentials, and with
    /// [`ProviderError::Token`] when they cannot be turned into a token.
    async fn access_token(&self) -> Result<AccessToken, ProviderError>;

    /// Marks the current token as rejected, because the provider answered 401 with it.
    /// The next [`access_token`](TokenSource::access_token) refreshes instead of
    /// returning it again. A source that cannot refresh does nothing.
    async fn invalidate(&self);
}

/// A token that never changes: an API key from the config or the credential store.
///
/// [`invalidate`](TokenSource::invalidate) does nothing, so a rejected key stays
/// rejected and the provider reports [`ProviderError::Unauthorized`] after its one
/// retry. The key names no account. `Debug` does not show the key.
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
    async fn access_token(&self) -> Result<AccessToken, ProviderError> {
        Ok(AccessToken::new(self.token.clone()))
    }

    async fn invalidate(&self) {}
}

#[cfg(test)]
mod tests;
