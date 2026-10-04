//! The settings of the login and the token endpoint.

use std::time::Duration;

use crate::authorize::{CALLBACK_PORT, CLIENT_ID, DEFAULT_ORIGINATOR, ISSUER};

/// Where the login and the refresh talk to, and how long a login may wait.
///
/// [`OAuthConfig::default`] holds OpenAI's values; the daemon changes `originator` when
/// the config asks, and tests point `issuer` at a local server and set
/// `callback_port` to `0` for a free port. The struct is `#[non_exhaustive]` so that a
/// later field does not break callers; start from the default and set fields.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OAuthConfig {
    /// The authorization server: `https://auth.openai.com`. The authorize and token
    /// endpoints are `/oauth/authorize` and `/oauth/token` below it.
    pub issuer: String,
    /// The OAuth client id: `app_EMoamEEZ73f0CkXaXp7hrann`, the public client of the
    /// Codex CLI.
    pub client_id: String,
    /// The `originator` parameter of the authorize URL: `efr`.
    pub originator: String,
    /// The loopback port of the callback listener: 1455, the port the client id's
    /// redirect allowlist names. `0` picks a free port, for tests.
    pub callback_port: u16,
    /// How long a login waits for the browser to come back, on the injected clock: ten
    /// minutes, enough for a password manager and a second factor.
    pub login_timeout: Duration,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        OAuthConfig {
            issuer: ISSUER.to_owned(),
            client_id: CLIENT_ID.to_owned(),
            originator: DEFAULT_ORIGINATOR.to_owned(),
            callback_port: CALLBACK_PORT,
            login_timeout: Duration::from_secs(10 * 60),
        }
    }
}

impl OAuthConfig {
    /// The URL of `path` below the issuer, which may or may not end in a slash.
    pub(crate) fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.issuer.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests;
