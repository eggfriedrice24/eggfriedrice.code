use std::error::Error as _;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use pretty_assertions::assert_eq;

use crate::StdxError;
use crate::env::Var;

#[test]
fn message_names_what_failed_and_leaves_the_source_to_the_chain() {
    let err = StdxError::WriteFile {
        path: PathBuf::from("/d/secrets/openai.json"),
        source: io::Error::other("no space left"),
    };
    assert_eq!(err.to_string(), "could not write /d/secrets/openai.json");
    assert_eq!(err.source().map(ToString::to_string).as_deref(), Some("no space left"));
}

#[test]
fn env_messages_name_the_variable_and_the_value() {
    let relative = StdxError::RelativeEnvPath { var: Var::DataDir, path: PathBuf::from("data") };
    assert_eq!(relative.to_string(), "EFR_DATA_DIR must hold an absolute path, not data");
    let invalid =
        StdxError::InvalidEnvValue { var: Var::TestZsh, value: "2".to_owned(), expected: "a flag" };
    assert_eq!(invalid.to_string(), "EFR_TEST_ZSH holds \"2\", which is not a flag");
}

#[test]
fn timeout_message_names_the_duration() {
    let err = StdxError::TimedOut { after: Duration::from_secs(30) };
    assert_eq!(err.to_string(), "the operation did not finish within 30s");
}

#[test]
fn error_can_cross_tasks_and_threads() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<StdxError>();
}
