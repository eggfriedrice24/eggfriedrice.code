//! The optional store in the platform keyring (feature `keyring`).
//!
//! On Linux the keyring is the Secret Service over D-Bus, which a headless systemd
//! user service may not have. That is why [`FileStore`](crate::FileStore) stays the
//! default and this store is opt-in (structure document, open question 14).

use std::io;

use keyring::Entry;
use zeroize::{Zeroize as _, Zeroizing};

use crate::{CredentialId, CredentialRecord, CredentialsError, SecretStore};

/// Keeps each record as one keyring item: service [`KeyringStore::service`], account
/// the [`CredentialId`], secret the same JSON a [`FileStore`](crate::FileStore) writes.
///
/// The methods block on the keyring daemon; see [`SecretStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    /// The service name efr uses unless told otherwise.
    pub const DEFAULT_SERVICE: &'static str = "efr";

    /// A store whose items carry the service name `service`.
    pub fn new(service: impl Into<String>) -> Self {
        KeyringStore { service: service.into() }
    }

    /// The service name of every item this store touches.
    pub fn service(&self) -> &str {
        &self.service
    }

    fn entry(&self, id: &CredentialId) -> Result<Entry, CredentialsError> {
        Entry::new(&self.service, id.as_str()).map_err(|error| keyring_error(id, error))
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        KeyringStore::new(Self::DEFAULT_SERVICE)
    }
}

impl SecretStore for KeyringStore {
    fn load(&self, id: &CredentialId) -> Result<Option<CredentialRecord>, CredentialsError> {
        match self.entry(id)?.get_secret() {
            Ok(bytes) => CredentialRecord::decode(id, &Zeroizing::new(bytes)).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error(id, error)),
        }
    }

    fn save(&self, id: &CredentialId, record: &CredentialRecord) -> Result<(), CredentialsError> {
        let bytes = record.encode(id)?;
        self.entry(id)?.set_secret(&bytes).map_err(|error| keyring_error(id, error))
    }

    fn delete(&self, id: &CredentialId) -> Result<bool, CredentialsError> {
        match self.entry(id)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(error) => Err(keyring_error(id, error)),
        }
    }
}

/// Wraps a keyring error without the raw secret bytes that two of its variants carry,
/// because `CredentialsError` derives `Debug` and is logged.
fn keyring_error(id: &CredentialId, error: keyring::Error) -> CredentialsError {
    let source: Box<dyn std::error::Error + Send + Sync> = match error {
        keyring::Error::BadEncoding(mut bytes) => {
            bytes.zeroize();
            Box::new(io::Error::new(io::ErrorKind::InvalidData, "the stored secret is not UTF-8"))
        }
        keyring::Error::BadDataFormat(mut bytes, platform) => {
            bytes.zeroize();
            platform
        }
        other => Box::new(other),
    };
    CredentialsError::Keyring { id: id.clone(), source }
}

#[cfg(test)]
mod tests;
