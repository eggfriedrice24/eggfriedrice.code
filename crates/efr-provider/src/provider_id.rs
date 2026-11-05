//! The name of a configured provider.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ProviderError;

/// The name of one configured provider, such as `openai-subscription`, `openai-api`
/// or `replay`.
///
/// It names a provider and a way of reaching it, not a vendor: the subscription and an
/// API key are two providers of the same vendor, because their `provider_raw` items
/// and their credentials are not interchangeable. It appears in logs (the `provider`
/// span field), in the config file and in the event log, so it is restricted to 1 to 64
/// bytes of lowercase ASCII letters, digits and `-`, starting with a letter or a digit.
///
/// One more login of the same kind is an account: the kind, a `.` and the account's
/// name, such as `anthropic-api.work`. The name is 1 to [`ACCOUNT_MAX_LEN`] bytes of
/// lowercase letters, digits and `-`, starting with a letter. The kind alone is its
/// default account. Two accounts are two providers: their raw items and credentials
/// are not interchangeable either.
///
/// [`ACCOUNT_MAX_LEN`]: ProviderId::ACCOUNT_MAX_LEN
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderId(String);

impl ProviderId {
    /// The longest id, in bytes, with its account.
    pub const MAX_LEN: usize = 64;

    /// The longest account name, in bytes.
    pub const ACCOUNT_MAX_LEN: usize = 32;

    /// Checks `id` against the naming rules.
    pub fn new(id: impl Into<String>) -> Result<Self, ProviderError> {
        let id = id.into();
        let (kind, account) = match id.split_once('.') {
            Some((kind, account)) => (kind, Some(account)),
            None => (id.as_str(), None),
        };
        let kind_ok =
            kind.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                && kind.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if kind_ok && account.is_none_or(is_account_name) && id.len() <= Self::MAX_LEN {
            Ok(ProviderId(id))
        } else {
            Err(ProviderError::InvalidProviderId { id })
        }
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The kind of provider, such as `anthropic-api` of `anthropic-api.work`.
    pub fn kind(&self) -> &str {
        self.0.split_once('.').map_or(self.0.as_str(), |(kind, _)| kind)
    }

    /// The account's name, such as `work` of `anthropic-api.work`; `None` for the
    /// default account of the kind.
    pub fn account(&self) -> Option<&str> {
        self.0.split_once('.').map(|(_, account)| account)
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ProviderId {
    type Err = ProviderError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        ProviderId::new(text)
    }
}

impl TryFrom<String> for ProviderId {
    type Error = ProviderError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        ProviderId::new(text)
    }
}

impl From<ProviderId> for String {
    fn from(id: ProviderId) -> Self {
        id.0
    }
}

impl AsRef<str> for ProviderId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// True for an account name: 1 to [`ProviderId::ACCOUNT_MAX_LEN`] bytes of lowercase
/// letters, digits and `-`, starting with a letter.
fn is_account_name(name: &str) -> bool {
    name.len() <= ProviderId::ACCOUNT_MAX_LEN
        && name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests;
