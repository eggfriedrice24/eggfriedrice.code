//! The login flow the daemon drives for `admin.login_openai`.
//!
//! [`OpenAiLogin::start`] draws the PKCE verifier and the `state`, binds the callback
//! listener and returns a [`PendingLogin`] whose URL the daemon streams to the CLI.
//! [`PendingLogin::complete`] waits for the browser, exchanges the code and saves the
//! tokens. Dropping the pending login at any point cancels it and frees the port.

use std::fmt;
use std::sync::Arc;

use efr_credentials::{CredentialId, SecretStore};
use efr_http::HttpClient;
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use jiff::Timestamp;
use secrecy::{ExposeSecret as _, SecretString};
use url::Url;

use crate::callback::{CallbackListener, LoginSlot, SlotGuard};
use crate::pkce::{self, Pkce};
use crate::token::{self, Grant};
use crate::{OAuthConfig, OAuthError, authorize, claims, persist};

/// Starts subscription logins, one at a time.
///
/// The daemon builds one and keeps it; every login it starts saves its tokens under the
/// same credential id, which is where [`OpenAiTokenSource`](crate::OpenAiTokenSource)
/// reads them.
pub struct OpenAiLogin {
    config: OAuthConfig,
    http: HttpClient,
    store: Arc<dyn SecretStore>,
    credential: CredentialId,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
    slot: LoginSlot,
}

impl OpenAiLogin {
    /// A login that talks to `config`'s issuer through `http`, saves the tokens in
    /// `store` under `credential`, times out on `clock` and draws the PKCE verifier and
    /// the `state` from `rng`.
    pub fn new(
        config: OAuthConfig,
        http: HttpClient,
        store: Arc<dyn SecretStore>,
        credential: CredentialId,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
    ) -> Self {
        OpenAiLogin { config, http, store, credential, clock, rng, slot: LoginSlot::default() }
    }

    /// Binds the callback listener and returns the login to complete.
    ///
    /// Fails with [`OAuthError::LoginInProgress`] while another login of this value is
    /// pending, and with [`OAuthError::Bind`] when another program holds the port.
    pub async fn start(&self) -> Result<PendingLogin, OAuthError> {
        let guard = self.slot.claim()?;
        let listener = CallbackListener::bind(self.config.callback_port).await?;
        let pkce = Pkce::generate(&*self.rng);
        let state = pkce::state(&*self.rng);
        let redirect_uri = authorize::redirect_uri(listener.port());
        let url = authorize::authorize_url(
            &self.config,
            &redirect_uri,
            pkce.challenge(),
            state.expose_secret(),
        )?;
        tracing::info!(port = listener.port(), "subscription login started");
        Ok(PendingLogin {
            url,
            listener,
            pkce,
            state,
            redirect_uri,
            config: self.config.clone(),
            http: self.http.clone(),
            store: Arc::clone(&self.store),
            credential: self.credential.clone(),
            clock: Arc::clone(&self.clock),
            _guard: guard,
        })
    }
}

impl fmt::Debug for OpenAiLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiLogin")
            .field("config", &self.config)
            .field("credential", &self.credential)
            .field("slot", &self.slot)
            .finish_non_exhaustive()
    }
}

/// A login whose listener is bound and whose URL the user has yet to open.
///
/// It holds the login slot and the port until [`complete`](PendingLogin::complete)
/// returns or it is dropped.
pub struct PendingLogin {
    url: Url,
    listener: CallbackListener,
    pkce: Pkce,
    state: SecretString,
    redirect_uri: String,
    config: OAuthConfig,
    http: HttpClient,
    store: Arc<dyn SecretStore>,
    credential: CredentialId,
    clock: Arc<dyn Clock>,
    _guard: SlotGuard,
}

impl PendingLogin {
    /// The URL the user opens in a browser. It holds the `state` and the PKCE
    /// challenge, so it is shown to the user and not logged.
    pub fn authorize_url(&self) -> &Url {
        &self.url
    }

    /// Waits for the browser to come back, exchanges the code and saves the tokens.
    ///
    /// The listener closes as soon as the callback is answered, before the exchange.
    /// Fails with [`OAuthError::TimedOut`] when no callback with the right `state`
    /// arrives within the configured timeout, measured on the injected clock.
    pub async fn complete(self) -> Result<LoginCompleted, OAuthError> {
        let PendingLogin {
            listener,
            pkce,
            state,
            redirect_uri,
            config,
            http,
            store,
            credential,
            clock,
            _guard,
            ..
        } = self;
        let after = config.login_timeout;
        let code = clock
            .timeout(after, listener.wait(state))
            .await
            .map_err(|_| OAuthError::TimedOut { after })??;
        let grant = Grant::AuthorizationCode {
            code: &code,
            redirect_uri: &redirect_uri,
            verifier: pkce.verifier(),
        };
        let response = token::request(&http, &config, grant, token::ENCODING).await?;
        let tokens = response.into_tokens(clock.now(), None);
        let claims = claims::of_tokens(tokens.id_token.as_ref(), &tokens.access_token);
        let completed = LoginCompleted {
            account_id: tokens.account_id.clone(),
            email: claims.email,
            expires_at: tokens.expires_at,
        };
        persist::save_tokens(&store, &credential, tokens).await?;
        tracing::info!(
            credential = %credential,
            has_account = completed.account_id.is_some(),
            "subscription login completed"
        );
        Ok(completed)
    }
}

impl fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingLogin")
            .field("port", &self.listener.port())
            .field("credential", &self.credential)
            .finish_non_exhaustive()
    }
}

/// What a finished login tells the user. The tokens themselves are in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LoginCompleted {
    /// The ChatGPT account the tokens act for, when the claims named one.
    pub account_id: Option<String>,
    /// The email address of the user, when the claims named one.
    pub email: Option<String>,
    /// When the access token expires, when known. The token source refreshes it before
    /// then.
    pub expires_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests;
