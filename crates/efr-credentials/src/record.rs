//! The credential record and its JSON form.
//!
//! Every store keeps a record as the same JSON document, so a record can move between
//! the file store and the keyring unchanged:
//!
//! ```json
//! {
//!   "version": 1,
//!   "kind": "oauth",
//!   "access_token": "...",
//!   "refresh_token": "...",
//!   "expires_at": "2026-10-04T12:00:00Z",
//!   "account_id": "..."
//! }
//! ```
//!
//! The serde types below are flat structs on purpose. An internally tagged enum or a
//! flattened field makes serde buffer every value in an intermediate tree, and those
//! copies of the secrets would never be zeroed.

use jiff::Timestamp;
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{CredentialId, CredentialsError};

/// The version of the JSON form that this crate writes and reads. A reader refuses
/// any other version, so an older efr never misreads a record from a newer one.
pub(crate) const FORMAT_VERSION: u32 = 1;

/// What a store keeps for one credential.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum CredentialRecord {
    /// A static API key, such as an OpenAI API key.
    ApiKey {
        /// The key.
        key: SecretString,
    },
    /// The tokens of an OAuth login.
    OAuth(OAuthTokens),
}

/// The tokens of an OAuth login.
///
/// Build one with [`OAuthTokens::new`] and set the optional fields directly; the
/// struct is `#[non_exhaustive]` so that a later field does not break callers.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct OAuthTokens {
    /// The bearer token for API requests.
    pub access_token: SecretString,
    /// The token that obtains a new access token, when the server issued one.
    pub refresh_token: Option<SecretString>,
    /// The OpenID Connect ID token (a JWT), when the server issued one.
    pub id_token: Option<SecretString>,
    /// When the access token stops working, when the server said.
    pub expires_at: Option<Timestamp>,
    /// The account the tokens belong to, for a provider that routes requests by
    /// account. It identifies the account but grants nothing, so it is not a secret.
    pub account_id: Option<String>,
}

impl OAuthTokens {
    /// Tokens with only an access token.
    pub fn new(access_token: SecretString) -> Self {
        OAuthTokens {
            access_token,
            refresh_token: None,
            id_token: None,
            expires_at: None,
            account_id: None,
        }
    }

    /// True when the access token expires at or before `deadline`.
    ///
    /// A caller that refreshes ahead of expiry passes `clock.now()` plus its margin. A
    /// token without a known expiry never counts as expiring; its end shows up as a
    /// 401 from the API instead.
    pub fn expires_before(&self, deadline: Timestamp) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= deadline)
    }
}

impl CredentialRecord {
    /// The JSON form, in a buffer that is zeroed when dropped.
    pub(crate) fn encode(&self, id: &CredentialId) -> Result<Zeroizing<Vec<u8>>, CredentialsError> {
        let stored = StoredRef::from(self);
        // NOTE: a buffer that grows leaves its old allocation behind unzeroed, so the
        // capacity covers the secrets twice over (room for escapes) plus the keys.
        let mut out = Zeroizing::new(Vec::with_capacity(stored.secret_len() * 2 + 512));
        serde_json::to_writer_pretty(&mut *out, &stored)
            .map_err(|source| CredentialsError::Encode { id: id.clone(), source })?;
        out.push(b'\n');
        Ok(out)
    }

    /// Parses the JSON form of the record stored under `id`.
    pub(crate) fn decode(id: &CredentialId, bytes: &[u8]) -> Result<Self, CredentialsError> {
        let decode_error = |source| CredentialsError::Decode { id: id.clone(), source };
        // The version comes first and alone: a newer version may change the other
        // fields, and that must read as "unsupported", not as a parse error.
        let Version { version } = serde_json::from_slice(bytes).map_err(decode_error)?;
        if version != FORMAT_VERSION {
            return Err(CredentialsError::UnsupportedVersion { id: id.clone(), version });
        }
        let stored: StoredOwned = serde_json::from_slice(bytes).map_err(decode_error)?;
        let missing = |field| CredentialsError::MissingField { id: id.clone(), field };
        match stored.kind {
            Kind::ApiKey => Ok(CredentialRecord::ApiKey { key: stored.key.ok_or(missing("key"))? }),
            Kind::OAuth => Ok(CredentialRecord::OAuth(OAuthTokens {
                access_token: stored.access_token.ok_or(missing("access_token"))?,
                refresh_token: stored.refresh_token,
                id_token: stored.id_token,
                expires_at: stored.expires_at,
                account_id: stored.account_id,
            })),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum Kind {
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "oauth")]
    OAuth,
}

#[derive(Deserialize)]
struct Version {
    version: u32,
}

/// The JSON form for writing, borrowing the secrets so that no copy is made.
#[derive(Serialize)]
struct StoredRef<'a> {
    version: u32,
    kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    access_token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id_token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<&'a str>,
}

impl StoredRef<'_> {
    fn secret_len(&self) -> usize {
        [self.key, self.access_token, self.refresh_token, self.id_token, self.account_id]
            .iter()
            .flatten()
            .map(|value| value.len())
            .sum()
    }
}

impl<'a> From<&'a CredentialRecord> for StoredRef<'a> {
    fn from(record: &'a CredentialRecord) -> Self {
        let empty = StoredRef {
            version: FORMAT_VERSION,
            kind: Kind::ApiKey,
            key: None,
            access_token: None,
            refresh_token: None,
            id_token: None,
            expires_at: None,
            account_id: None,
        };
        match record {
            CredentialRecord::ApiKey { key } => {
                StoredRef { kind: Kind::ApiKey, key: Some(key.expose_secret()), ..empty }
            }
            CredentialRecord::OAuth(tokens) => StoredRef {
                kind: Kind::OAuth,
                access_token: Some(tokens.access_token.expose_secret()),
                refresh_token: tokens.refresh_token.as_ref().map(|token| token.expose_secret()),
                id_token: tokens.id_token.as_ref().map(|token| token.expose_secret()),
                expires_at: tokens.expires_at,
                account_id: tokens.account_id.as_deref(),
                ..empty
            },
        }
    }
}

/// The JSON form for reading. Each secret goes from the parser into a `SecretString`,
/// which zeroes it when dropped.
#[derive(Deserialize)]
struct StoredOwned {
    kind: Kind,
    key: Option<SecretString>,
    access_token: Option<SecretString>,
    refresh_token: Option<SecretString>,
    id_token: Option<SecretString>,
    expires_at: Option<Timestamp>,
    account_id: Option<String>,
}

#[cfg(test)]
mod tests;
