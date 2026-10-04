//! The ChatGPT subscription login: how efr obtains the OAuth tokens of a ChatGPT plan,
//! keeps them fresh, and hands them to the OpenAI provider.
//!
//! - [`OpenAiLogin`] runs the browser login that the daemon drives for
//!   `admin.login_openai`. [`OpenAiLogin::start`] binds a one-shot callback listener on
//!   `127.0.0.1:1455` and returns a [`PendingLogin`] with the authorize URL to show;
//!   [`PendingLogin::complete`] waits for the browser, exchanges the code (PKCE with
//!   S256) and saves the tokens through `efr_credentials::SecretStore`, returning a
//!   [`LoginCompleted`].
//! - [`OpenAiTokenSource`] is the `efr_provider::TokenSource` over the saved tokens. It
//!   returns the access token with its ChatGPT account id, refreshes when less than
//!   five minutes remain on the injected clock, sends one refresh for any number of
//!   concurrent callers, saves the new record, and refreshes once more after the
//!   provider reports a 401.
//! - [`OAuthConfig`] holds OpenAI's constants (issuer, client id, port, originator),
//!   checked against codex, opencode and goose, so tests can point the flow at a local
//!   server.
//!
//! Tokens, codes, the PKCE verifier and the `state` never reach `Debug` output, a log
//! line or an [`OAuthError`].
//!
//! Allowed dependencies: `efr-http`, `efr-credentials`, `efr-provider` and `efr-stdx`.
//! What does not belong here: the Responses API and its headers (`efr-provider-openai`,
//! which must never depend on this crate, so it never sees a refresh token), opening a
//! browser or printing the URL (`efr-cli`), and choosing the credential id or composing
//! providers (`efr-daemon`).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod authorize;
mod config;
mod error;
#[cfg(test)]
mod testing;

pub use config::OAuthConfig;
pub use error::{GrantKind, OAuthError};
