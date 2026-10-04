//! Vendor-neutral credential storage.
//!
//! A [`CredentialRecord`] is what efr keeps for one login: a static API key or a set
//! of OAuth tokens. A [`SecretStore`] loads, saves and deletes records by
//! [`CredentialId`]. [`FileStore`] keeps one JSON file per record under
//! `$XDG_DATA_HOME/efr/secrets/` (directory 0700, files 0600) and is the default. The
//! `keyring` feature adds `keyring_store::KeyringStore`, which keeps the same JSON in
//! the platform keyring.
//!
//! Secrets are `secrecy::SecretString` values, whose `Debug` output is redacted, and
//! the serialised bytes of a record live in buffers that are zeroed when dropped.
//!
//! Allowed dependencies: `efr-stdx` only. What does not belong here: anything that
//! knows a vendor's token endpoint, refresh rules or claims (`efr-oauth-openai`), and
//! the choice of store (`efr-daemon`).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod error;
mod file_store;
mod id;
mod record;
mod store;

pub use error::CredentialsError;
pub use file_store::FileStore;
pub use id::CredentialId;
pub use record::{CredentialRecord, OAuthTokens};
pub use store::SecretStore;
