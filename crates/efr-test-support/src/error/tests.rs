use std::error::Error as _;
use std::io;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::TestSupportError;

fn io_error() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "denied")
}

#[test]
fn messages_name_what_failed_in_one_sentence() {
    let cases = [
        (
            TestSupportError::CreateTempDir { source: io_error() },
            "could not create a temporary directory",
        ),
        (
            TestSupportError::CreateDir { path: PathBuf::from("/t/home"), source: io_error() },
            "could not create the directory /t/home",
        ),
        (
            TestSupportError::NoCrateForSource { source_file: PathBuf::from("src/x.rs") },
            "could not find the crate that holds src/x.rs",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn io_failures_keep_their_source() {
    let error = TestSupportError::CreateTempDir { source: io_error() };
    assert_eq!(error.source().unwrap().to_string(), "denied");
}
