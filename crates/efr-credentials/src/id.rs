//! The name a record is stored under.

use std::fmt;
use std::str::FromStr;

use crate::CredentialsError;

/// The name of one credential, such as `openai-subscription` or `openai-api`.
///
/// The file store uses it as a file name and the keyring store uses it as an
/// account name, so the set of names is narrow: 1 to 64 bytes of lowercase
/// ASCII letters, digits, `-`, `_` and `.`, starting with a letter or a digit. That
/// rules out path separators, hidden files, `.` and `..`, and names that differ only
/// in letter case.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CredentialId(String);

impl CredentialId {
    /// The longest id, in bytes.
    pub const MAX_LEN: usize = 64;

    /// Checks `id` against the rules above.
    pub fn new(id: impl Into<String>) -> Result<Self, CredentialsError> {
        let id = id.into();
        if is_valid(&id) { Ok(CredentialId(id)) } else { Err(CredentialsError::InvalidId { id }) }
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_valid(id: &str) -> bool {
    let bytes = id.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    bytes.len() <= CredentialId::MAX_LEN
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.iter().all(|&b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
        })
}

impl fmt::Display for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for CredentialId {
    type Err = CredentialsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        CredentialId::new(s)
    }
}

impl AsRef<str> for CredentialId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
