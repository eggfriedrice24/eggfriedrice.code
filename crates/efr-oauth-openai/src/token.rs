//! The token endpoint: the authorization code exchange and the refresh grant.
//!
//! Both grants go through [`request`], so the body encoding is one switch,
//! [`ENCODING`]. opencode and goose form-encode both grants (`codex.ts:117-149`,
//! `chatgpt_codex.rs:534-590`); codex form-encodes the code exchange
//! (`codex-rs/login/src/server.rs:816`) but now sends its refresh as JSON
//! (`codex-rs/login/src/auth/manager.rs:1638`). The form is what every source accepts
//! for both grants today; open question 7 of the structure document keeps the JSON
//! variant one line away.

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use efr_credentials::OAuthTokens;
use efr_http::{HeaderValue, HttpClient, HttpRequest, RetryPolicy, StatusCode, header};
use jiff::{SignedDuration, Timestamp};
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use serde_json::Value;
use zeroize::Zeroize as _;

use crate::authorize::TOKEN_PATH;
use crate::{GrantKind, OAuthConfig, OAuthError, claims};

/// How a grant's parameters travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// `application/x-www-form-urlencoded`, as RFC 6749 section 4.1.3 specifies.
    Form,
    /// A JSON object of the same parameters.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the other side of the switch; tests send it")
    )]
    Json,
}

/// The encoding of every grant this crate sends: the one switch.
pub(crate) const ENCODING: Encoding = Encoding::Form;

/// The deadline of one token request, body included. The endpoint answers in well
/// under a second; the deadline only bounds a hung connection.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest server description an error keeps.
const MAX_DESCRIPTION_CHARS: usize = 300;

/// A grant and its secrets.
pub(crate) enum Grant<'a> {
    /// The code from the login callback, the redirect it was issued for, and the PKCE
    /// verifier that proves this client asked for it.
    AuthorizationCode { code: &'a SecretString, redirect_uri: &'a str, verifier: &'a SecretString },
    /// A refresh token.
    RefreshToken { refresh_token: &'a SecretString },
}

impl Grant<'_> {
    fn kind(&self) -> GrantKind {
        match self {
            Grant::AuthorizationCode { .. } => GrantKind::AuthorizationCode,
            Grant::RefreshToken { .. } => GrantKind::RefreshToken,
        }
    }

    /// The parameters in the order codex sends them
    /// (`codex-rs/login/src/oauth/client.rs:62-86`).
    fn parameters<'p>(&'p self, client_id: &'p str) -> Vec<(&'static str, &'p str)> {
        match self {
            Grant::AuthorizationCode { code, redirect_uri, verifier } => vec![
                ("grant_type", "authorization_code"),
                ("client_id", client_id),
                ("code", code.expose_secret()),
                ("redirect_uri", redirect_uri),
                ("code_verifier", verifier.expose_secret()),
            ],
            Grant::RefreshToken { refresh_token } => vec![
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("refresh_token", refresh_token.expose_secret()),
            ],
        }
    }

    /// The values that must never appear in an error, should the server echo them.
    fn secrets(&self) -> Vec<&str> {
        match self {
            Grant::AuthorizationCode { code, verifier, .. } => {
                vec![code.expose_secret(), verifier.expose_secret()]
            }
            Grant::RefreshToken { refresh_token } => vec![refresh_token.expose_secret()],
        }
    }
}

/// A successful answer of the token endpoint. A refresh may leave out the refresh
/// token and the ID token, which means the old ones stay valid.
#[derive(Debug, Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) access_token: SecretString,
    #[serde(default)]
    pub(crate) refresh_token: Option<SecretString>,
    #[serde(default)]
    pub(crate) id_token: Option<SecretString>,
    #[serde(default)]
    pub(crate) expires_in: Option<u64>,
}

impl TokenResponse {
    /// The record to keep, given the time of the answer and the record the grant
    /// renewed, if any.
    ///
    /// The expiry is `now + expires_in` when the server said, otherwise the access
    /// token's `exp` claim (codex reads only the claim,
    /// `codex-rs/login/src/auth/manager.rs:3011-3016`), otherwise unknown. The account
    /// comes from the claims and falls back to the one the old record named.
    pub(crate) fn into_tokens(self, now: Timestamp, previous: Option<&OAuthTokens>) -> OAuthTokens {
        let TokenResponse { access_token, refresh_token, id_token, expires_in } = self;
        let id_token = id_token.or_else(|| previous.and_then(|old| old.id_token.clone()));
        let claims = claims::of_tokens(id_token.as_ref(), &access_token);
        let expires_at = match expires_in {
            Some(seconds) => Some(add_seconds(now, seconds)),
            None => claims.expires_at,
        };
        let mut tokens = OAuthTokens::new(access_token);
        tokens.refresh_token =
            refresh_token.or_else(|| previous.and_then(|old| old.refresh_token.clone()));
        tokens.id_token = id_token;
        tokens.expires_at = expires_at;
        tokens.account_id =
            claims.account_id.or_else(|| previous.and_then(|old| old.account_id.clone()));
        tokens
    }
}

/// Sends `grant` to the token endpoint of `config` with `encoding`.
///
/// A connection that never opened and a 408, 429 or 503 are retried by `efr-http`'s
/// default policy; nothing that may have reached the server is sent twice, because an
/// authorization code works once and a refresh may rotate the refresh token.
pub(crate) async fn request(
    http: &HttpClient,
    config: &OAuthConfig,
    grant: Grant<'_>,
    encoding: Encoding,
) -> Result<TokenResponse, OAuthError> {
    let kind = grant.kind();
    let transport = |source| OAuthError::TokenRequest { grant: kind, source };
    let parameters = grant.parameters(&config.client_id);
    let request = HttpRequest::post(&config.endpoint(TOKEN_PATH))
        .map_err(transport)?
        .header(header::ACCEPT, HeaderValue::from_static("application/json"))
        .timeout(REQUEST_TIMEOUT);
    let request = match encoding {
        Encoding::Form => request.form(&parameters),
        Encoding::Json => request
            .json(&parameters.iter().copied().collect::<BTreeMap<_, _>>())
            .map_err(transport)?,
    };
    let response =
        http.send_with_retry(&request, &RetryPolicy::default()).await.map_err(transport)?;
    let status = response.status();
    let body = response.bytes().await.map_err(transport)?;
    let parsed = if status.is_success() {
        serde_json::from_slice(&body)
            .map_err(|source| OAuthError::TokenDecode { grant: kind, source })
    } else {
        Err(rejection(kind, status, &body, &grant.secrets()))
    };
    wipe(body);
    parsed
}

/// The error for a non-success answer. The code comes from `error` (RFC 6749),
/// `error.code` (OpenAI's shape) or `code`; the description from `error_description` or
/// `error.message`, the shapes codex reads (`codex-rs/login/src/oauth/error.rs:135-164`).
fn rejection(grant: GrantKind, status: StatusCode, body: &[u8], secrets: &[&str]) -> OAuthError {
    let json: Option<Value> = serde_json::from_slice(body).ok();
    let field = |name: &str| json.as_ref().and_then(|json| json.get(name));
    let nested = |name: &str| field("error").and_then(|error| error.get(name));
    let error =
        text(field("error")).or_else(|| text(nested("code"))).or_else(|| text(field("code")));
    let description = text(field("error_description")).or_else(|| text(nested("message")));
    OAuthError::TokenRejected {
        grant,
        status: status.as_u16(),
        error: error.map(|code| scrub(code, secrets)),
        description: description.map(|text| scrub(text, secrets)),
    }
}

/// The trimmed text of a JSON string, unless it is empty.
fn text(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty())
}

/// `text` with every secret replaced and then cut to a bounded length; replacing first
/// means a cut can never leave a prefix of a secret behind.
fn scrub(text: &str, secrets: &[&str]) -> String {
    let mut clean = text.to_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        clean = clean.replace(secret, efr_http::redact::REDACTED);
    }
    if clean.chars().count() > MAX_DESCRIPTION_CHARS {
        clean = clean.chars().take(MAX_DESCRIPTION_CHARS).chain(['.', '.', '.']).collect();
    }
    clean
}

/// Zeroes a body that held tokens, when this is its only reference; the HTTP layer may
/// keep none, so this is best effort.
fn wipe(body: Bytes) {
    if let Ok(mut owned) = body.try_into_mut() {
        owned.as_mut().zeroize();
    }
}

fn add_seconds(now: Timestamp, seconds: u64) -> Timestamp {
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    now.checked_add(SignedDuration::from_secs(seconds)).unwrap_or(Timestamp::MAX)
}

#[cfg(test)]
mod tests;
