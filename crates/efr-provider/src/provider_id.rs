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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderId(String);

impl ProviderId {
    /// The longest id, in bytes.
    pub const MAX_LEN: usize = 64;

    /// Checks `id` against the naming rules.
    pub fn new(id: impl Into<String>) -> Result<Self, ProviderError> {
        let id = id.into();
        let starts_well =
            id.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        let only_allowed =
            id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if starts_well && only_allowed && id.len() <= Self::MAX_LEN {
            Ok(ProviderId(id))
        } else {
            Err(ProviderError::InvalidProviderId { id })
        }
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
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

#[cfg(test)]
mod tests;
