//! Logins with an API key: the checks of the key's text, the request that checks the
//! key with its provider, and the hint that `admin.status` shows of a stored key.
//!
//! The key arrives as `SecretText` and goes on as `SecretString`, whose buffers are
//! zeroed when they are dropped. It never reaches an error, a log line or an event: an
//! error names only the provider and what is wrong, and the hint shows the key's
//! known prefix and its last four characters.

use efr_http::HttpClient;
use efr_provider::{ProviderError, SecretString};
use efr_provider_anthropic::AnthropicConfig;
use efr_provider_openai::OpenAiConfig;

use super::{ANTHROPIC, API};
use crate::{DaemonError, KeyProblem};

/// The prefix of an OpenAI admin key, which manages an organization and cannot call
/// models.
const ADMIN_KEY_PREFIX: &str = "sk-admin-";

/// The prefixes that a hint shows, longest first: Anthropic's, OpenAI's project and
/// service account keys, then any other `sk-` key.
const KNOWN_PREFIXES: &[&str] = &["sk-ant-", "sk-proj-", "sk-svcacct-", "sk-"];

/// How many characters of the end of a key the hint shows.
const HINT_TAIL: usize = 4;

/// How many characters of a key, besides its prefix, stay hidden at least, so a short
/// key never shows most of itself.
const HINT_HIDDEN: usize = 8;

/// The providers that take an API key.
pub(crate) const KEY_PROVIDERS: &[&str] = &[API, ANTHROPIC];

/// The problem of `key` for `provider` before any check: empty, whitespace inside, a
/// character outside ASCII, or an OpenAI admin key.
pub(crate) fn validate(provider: &str, key: &str) -> Result<(), DaemonError> {
    let problem = if key.is_empty() {
        Some(KeyProblem::Empty)
    } else if key.chars().any(char::is_whitespace) {
        Some(KeyProblem::Whitespace)
    } else if !key.chars().all(|c| c.is_ascii_graphic()) {
        Some(KeyProblem::NotAscii)
    } else if provider == API && key.starts_with(ADMIN_KEY_PREFIX) {
        Some(KeyProblem::AdminKey)
    } else {
        None
    };
    match problem {
        Some(problem) => Err(DaemonError::InvalidApiKey { provider: provider.to_owned(), problem }),
        None => Ok(()),
    }
}

/// What a client may show of `key`: its known prefix and its last four characters,
/// such as `sk-ant-...a1b2`. A key too short to hide enough of shows no tail.
pub(crate) fn hint(key: &str) -> String {
    let prefix = KNOWN_PREFIXES.iter().copied().find(|prefix| key.starts_with(prefix));
    let prefix = prefix.unwrap_or_default();
    let rest = &key[prefix.len()..];
    let count = rest.chars().count();
    if count < HINT_TAIL + HINT_HIDDEN {
        return format!("{prefix}...");
    }
    let tail: String = rest.chars().skip(count - HINT_TAIL).collect();
    format!("{prefix}...{tail}")
}

/// The request that checks a key with its provider. The settings that it needs are
/// keys that apply after a restart, so they are built once at start.
#[derive(Debug)]
pub(crate) struct KeyChecks {
    http: HttpClient,
    openai: OpenAiConfig,
    anthropic: AnthropicConfig,
}

impl KeyChecks {
    /// Checks through `http`: OpenAI keys with `openai` (the API's base URL, the
    /// organization and the project), Anthropic keys with `anthropic` (the base URL and
    /// the workspace).
    pub(crate) fn new(http: HttpClient, openai: OpenAiConfig, anthropic: AnthropicConfig) -> Self {
        KeyChecks { http, openai, anthropic }
    }

    /// Checks `key` with `provider`, with one request that runs no model.
    pub(crate) async fn check(
        &self,
        provider: &str,
        key: &SecretString,
    ) -> Result<(), ProviderError> {
        match provider {
            ANTHROPIC => efr_provider_anthropic::check_key(&self.http, &self.anthropic, key).await,
            // NOTE: a login refuses every provider outside `KEY_PROVIDERS` before it
            // checks, so this is `openai-api`.
            _ => efr_provider_openai::check_key(&self.http, &self.openai, key).await,
        }
    }
}

#[cfg(test)]
mod tests;
