//! OpenAI's login constants and the authorize URL the browser opens.
//!
//! Each constant was checked against the clients that use this login, in clones of
//! 2026-10-04 (codex `afb436df`, opencode `907b3bc5`, goose `591edd47`):
//!
//! - codex: `codex-rs/login/src/server.rs:77-78` (issuer, port 1455), `:194` (the
//!   redirect on `127.0.0.1`), `:586-618` (the authorize parameters),
//!   `codex-rs/login/src/oauth/authorization.rs:21-42` (the standard parameters, S256),
//!   `codex-rs/login/src/auth/manager.rs:212` (the token URL) and `:1718` (the client
//!   id).
//! - opencode: `packages/opencode/src/plugin/openai/codex.ts:10-13` (client id, issuer,
//!   port), `:88-102` (the authorize parameters, scopes), `:118` and `:136` (the token
//!   path).
//! - goose: `crates/goose/src/providers/chatgpt_codex.rs:37-43` (client id, issuer,
//!   scopes, port), `:508-524` (the authorize parameters), `:550` and `:577` (the token
//!   path), `:726` (the listener on `127.0.0.1`).
//!
//! Two choices differ from part of that set. The redirect names `127.0.0.1`, as codex
//! does, where opencode (`codex.ts:166`) and goose (`chatgpt_codex.rs:780`) name
//! `localhost`: the listener binds only the IPv4
//! loopback address, and a `localhost` redirect lets a browser try `::1` first, where
//! another program may listen (RFC 8252, section 8.3, recommends the IP literal for
//! the same reason). The scopes are the four that all three request; codex adds two
//! `api.connectors.*` scopes for a feature efr does not have.

use url::Url;

use crate::{OAuthConfig, OAuthError};

/// The authorization server.
pub(crate) const ISSUER: &str = "https://auth.openai.com";

/// The authorize endpoint, below the issuer.
pub(crate) const AUTHORIZE_PATH: &str = "/oauth/authorize";

/// The token endpoint, below the issuer.
pub(crate) const TOKEN_PATH: &str = "/oauth/token";

/// The public client id that codex, opencode and goose all use (open question 6 of the
/// structure document asks whether a third-party harness may).
pub(crate) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// The loopback port that the client id's redirect allowlist names.
pub(crate) const CALLBACK_PORT: u16 = 1455;

/// The path of the redirect.
pub(crate) const CALLBACK_PATH: &str = "/auth/callback";

/// The scopes; `offline_access` is what makes the server issue a refresh token.
pub(crate) const SCOPES: &str = "openid profile email offline_access";

/// The `originator` efr sends unless the config names another. opencode sends
/// `opencode` and goose `goose`; whether the backend accepts any value is open
/// question 7.
pub(crate) const DEFAULT_ORIGINATOR: &str = "efr";

/// The redirect URI for a listener on `port`.
pub(crate) fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}{CALLBACK_PATH}")
}

/// The URL the user opens to log in.
///
/// Beyond the standard parameters (RFC 6749 and RFC 7636 with S256), it carries the two
/// that all three reference clients send: `id_token_add_organizations=true`, which puts
/// the account claims into the ID token, and `codex_cli_simplified_flow=true`, which
/// selects the consent screen made for command-line clients.
pub(crate) fn authorize_url(
    config: &OAuthConfig,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
) -> Result<Url, OAuthError> {
    let endpoint = config.endpoint(AUTHORIZE_PATH);
    let mut url = Url::parse(&endpoint)
        .map_err(|source| OAuthError::InvalidIssuer { issuer: config.issuer.clone(), source })?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", state)
        .append_pair("originator", &config.originator);
    Ok(url)
}

#[cfg(test)]
mod tests;
