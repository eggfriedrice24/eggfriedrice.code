//! The storage boundary.

use std::fmt;

use crate::{CredentialId, CredentialRecord, CredentialsError};

/// Where credential records live.
///
/// The trait is the edge between code that obtains credentials (a login flow, a
/// configured API key) and code that uses them (a provider's token source). It knows
/// no vendor: a record is opaque secret material under a [`CredentialId`].
///
/// The methods block on file or keyring IO. Async code calls them inside
/// `tokio::task::spawn_blocking`, which is why the trait asks for `Send + Sync` and is
/// dyn-compatible: an `Arc<dyn SecretStore>` moves into the blocking task.
pub trait SecretStore: Send + Sync + fmt::Debug {
    /// The record saved under `id`, or `None` when there is none.
    fn load(&self, id: &CredentialId) -> Result<Option<CredentialRecord>, CredentialsError>;

    /// Saves `record` under `id`, replacing any earlier record. A reader sees the old
    /// record or the new one, never a mix.
    fn save(&self, id: &CredentialId, record: &CredentialRecord) -> Result<(), CredentialsError>;

    /// Deletes the record under `id`. Returns `false` when there was none, so a logout
    /// that runs twice is not an error.
    fn delete(&self, id: &CredentialId) -> Result<bool, CredentialsError>;
}
