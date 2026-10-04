use std::error::Error as _;
use std::io;

use pretty_assertions::assert_eq;

use super::{KeyringStore, keyring_error};
use crate::{CredentialId, CredentialsError};

const SECRET: &[u8] = b"sk-very-secret";
/// The start of `SECRET` as `Debug` prints a byte vector, which is how a leak looks.
const SECRET_AS_DEBUG: &str = "115, 107, 45, 118";

fn id() -> CredentialId {
    CredentialId::new("openai-api").unwrap()
}

/// The `Debug` and `Display` text of the error and of every source below it.
fn chain_text(error: &CredentialsError) -> String {
    let mut text = format!("{error:?} {error}");
    let mut source = error.source();
    while let Some(current) = source {
        text.push_str(&format!(" {current:?} {current}"));
        source = current.source();
    }
    text
}

#[test]
fn default_service_is_efr() {
    assert_eq!(KeyringStore::default().service(), "efr");
    assert_eq!(KeyringStore::new("efr-test").service(), "efr-test");
}

#[test]
fn bad_encoding_drops_the_secret_bytes() {
    let error = keyring_error(&id(), keyring::Error::BadEncoding(SECRET.to_vec()));
    let text = chain_text(&error);
    assert!(!text.contains(SECRET_AS_DEBUG), "{text}");
    assert!(text.contains("not UTF-8"), "{text}");
}

#[test]
fn bad_data_format_drops_the_secret_bytes_and_keeps_the_cause() {
    let cause = Box::new(io::Error::other("bad padding"));
    let error = keyring_error(&id(), keyring::Error::BadDataFormat(SECRET.to_vec(), cause));
    let text = chain_text(&error);
    assert!(!text.contains(SECRET_AS_DEBUG), "{text}");
    assert!(text.contains("bad padding"), "{text}");
}

#[test]
fn other_errors_are_kept_as_the_source() {
    let error = keyring_error(&id(), keyring::Error::NoDefaultStore);
    let CredentialsError::Keyring { id: failed, source } = &error else {
        panic!("unexpected error {error:?}");
    };
    assert_eq!(failed, &id());
    assert!(matches!(
        source.downcast_ref::<keyring::Error>(),
        Some(keyring::Error::NoDefaultStore)
    ));
}
