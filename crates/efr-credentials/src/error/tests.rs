use std::error::Error as _;
use std::io;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::CredentialsError;
use crate::CredentialId;

fn id() -> CredentialId {
    CredentialId::new("openai-api").unwrap()
}

#[test]
fn messages_name_what_failed_in_one_sentence() {
    let cases = [
        (
            CredentialsError::InvalidId { id: "../x".to_owned() },
            r#""../x" is not a valid credential id"#,
        ),
        (
            CredentialsError::InsecurePermissions {
                path: PathBuf::from("/d/secrets"),
                mode: 0o755,
            },
            "/d/secrets has mode 0755; other users can reach it (expected 0700 for the directory, 0600 for files)",
        ),
        (
            CredentialsError::UnexpectedFileType {
                path: PathBuf::from("/d/secrets/a.json"),
                expected: "file",
            },
            "/d/secrets/a.json is not a regular file",
        ),
        (
            CredentialsError::TooLarge { path: PathBuf::from("/d/secrets/a.json"), limit: 65536 },
            "/d/secrets/a.json is larger than 65536 bytes",
        ),
        (
            CredentialsError::MissingField { id: id(), field: "key" },
            "the stored credential openai-api has no key field",
        ),
        (
            CredentialsError::UnsupportedVersion { id: id(), version: 9 },
            "the stored credential openai-api has format version 9, which this build cannot read",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn io_failures_keep_their_source_out_of_the_message() {
    let error = CredentialsError::Read {
        path: PathBuf::from("/d/secrets/a.json"),
        source: io::Error::new(io::ErrorKind::PermissionDenied, "denied by the kernel"),
    };
    assert_eq!(error.to_string(), "could not read /d/secrets/a.json");
    assert_eq!(error.source().unwrap().to_string(), "denied by the kernel");
}

#[test]
fn keyring_failures_chain_the_boxed_source() {
    let error = CredentialsError::Keyring {
        id: id(),
        source: Box::new(io::Error::other("no secret service")),
    };
    assert_eq!(error.to_string(), "the keyring failed for the credential openai-api");
    assert_eq!(error.source().unwrap().to_string(), "no secret service");
}
