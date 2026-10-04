//! The one public error type of the crate.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use efr_credentials::{CredentialId, CredentialsError};
use efr_http::HttpError;

/// The grant a token request made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GrantKind {
    /// The exchange of the authorization code from the login callback.
    AuthorizationCode,
    /// The exchange of a refresh token for a new access token.
    RefreshToken,
}

impl fmt::Display for GrantKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GrantKind::AuthorizationCode => "authorization code",
            GrantKind::RefreshToken => "refresh token",
        })
    }
}

/// Every way the login or a token refresh can fail.
///
/// No variant carries a token, an authorization code, a PKCE verifier or the OAuth
/// `state`. Text that the authorization server sends back (an error code, a
/// description) is kept, with any of the request's secrets replaced first.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OAuthError {
    /// A login is already waiting for its callback or exchanging its code.
    #[error("another login is already in progress")]
    LoginInProgress,

    /// The callback listener could not be bound, usually because another program holds
    /// the port.
    #[error("could not listen for the login callback on {addr}")]
    Bind {
        /// The address the listener asked for.
        addr: SocketAddr,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// The callback listener could not accept a connection.
    #[error("the login callback listener stopped accepting connections")]
    Accept {
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// No usable callback arrived before the login timeout, measured on the injected
    /// clock.
    #[error("no login callback arrived within {}s", .after.as_secs())]
    TimedOut {
        /// The login timeout.
        after: Duration,
    },

    /// The authorization server sent the browser back with an error, for example
    /// because the user declined.
    #[error("the authorization server refused the login with {error:?}")]
    Authorization {
        /// The OAuth error code, such as `access_denied`.
        error: String,
        /// The server's description, when it sent one.
        description: Option<String>,
    },

    /// The callback carried the right `state` but no authorization code.
    #[error("the login callback carried no authorization code")]
    MissingCode,

    /// The configured issuer does not make a valid URL.
    #[error("the issuer {issuer:?} does not make a valid URL")]
    InvalidIssuer {
        /// The configured issuer.
        issuer: String,
        /// The error from the URL parser.
        #[source]
        source: url::ParseError,
    },

    /// A token request could not be sent, or its response could not be read.
    #[error("the {grant} grant could not be sent to the token endpoint")]
    TokenRequest {
        /// The grant.
        grant: GrantKind,
        /// The HTTP layer's error.
        #[source]
        source: HttpError,
    },

    /// The token endpoint answered a grant with an error status.
    #[error(
        "the token endpoint rejected the {grant} grant with status {status}{}",
        code_suffix(.error.as_deref())
    )]
    TokenRejected {
        /// The grant.
        grant: GrantKind,
        /// The HTTP status.
        status: u16,
        /// The OAuth or OpenAI error code, such as `invalid_grant` or
        /// `refresh_token_expired`, when the body named one.
        error: Option<String>,
        /// The server's description, when it sent one.
        description: Option<String>,
    },

    /// The token endpoint answered a grant with a body that is not a token response.
    #[error(
        "the token endpoint answered the {grant} grant with a body that is not a token response"
    )]
    TokenDecode {
        /// The grant.
        grant: GrantKind,
        /// The error from the parser.
        #[source]
        source: serde_json::Error,
    },

    /// A token that should be a JWT is not one.
    #[error("a token is not a JWT: {problem}")]
    MalformedJwt {
        /// What is wrong with it.
        problem: &'static str,
    },

    /// The claims of a JWT are not the JSON object they should be.
    #[error("the claims of a token are not a valid JSON object")]
    JwtClaims {
        /// The error from the parser.
        #[source]
        source: serde_json::Error,
    },

    /// No credential is stored under the id, so there is nobody to act for.
    #[error("no credential is stored as {id}; log in first")]
    NotLoggedIn {
        /// The credential.
        id: CredentialId,
    },

    /// The stored credential is an API key, not the tokens of a login.
    #[error("the stored credential {id} is not an OAuth login")]
    NotOAuth {
        /// The credential.
        id: CredentialId,
    },

    /// The stored credential has no refresh token and its access token is expired or was
    /// rejected.
    #[error(
        "the stored credential {id} has no refresh token and its access token can no longer be used"
    )]
    NoRefreshToken {
        /// The credential.
        id: CredentialId,
    },

    /// The credential store failed.
    #[error("could not access the stored credential {id}")]
    Store {
        /// The credential.
        id: CredentialId,
        /// The store's error.
        #[source]
        source: CredentialsError,
    },

    /// The blocking task that ran a credential store call did not finish.
    #[error("the credential store task for {id} did not finish")]
    StoreTask {
        /// The credential.
        id: CredentialId,
        /// The error from the blocking pool.
        #[source]
        source: tokio::task::JoinError,
    },
}

impl OAuthError {
    /// True when only a new login can fix the failure: no credential, a credential of
    /// the wrong kind, no refresh token, or a refresh token that the server refused.
    /// The daemon uses it to tell the user to run `efr login openai` again.
    pub fn needs_login(&self) -> bool {
        match self {
            OAuthError::NotLoggedIn { .. }
            | OAuthError::NotOAuth { .. }
            | OAuthError::NoRefreshToken { .. } => true,
            OAuthError::TokenRejected { grant: GrantKind::RefreshToken, status, .. } => {
                matches!(status, 400 | 401)
            }
            _ => false,
        }
    }
}

fn code_suffix(error: Option<&str>) -> String {
    error.map(|code| format!(" ({code})")).unwrap_or_default()
}

#[cfg(test)]
mod tests;
