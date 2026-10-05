use std::error::Error as _;

use efr_holder::{HolderError, PtyId};
use efr_protocol::ConversationId;
use pretty_assertions::assert_eq;

use super::ShellError;

fn conversation() -> ConversationId {
    "01920000-0000-7000-8000-000000000001".parse().unwrap()
}

#[test]
fn messages_name_the_conversation_and_keep_the_source() {
    let pty_id: PtyId = "01920000-0000-7000-8000-000000000002".parse().unwrap();
    let error = ShellError::Spawn {
        conversation: conversation(),
        source: HolderError::NotFound { pty_id },
    };
    assert_eq!(
        error.to_string(),
        "could not start the hidden shell of conversation 01920000-0000-7000-8000-000000000001"
    );
    assert!(error.source().is_some());
}

#[test]
fn an_invalid_command_says_why_without_the_command() {
    let error = ShellError::InvalidCommand { reason: "it contains a NUL byte" };
    assert_eq!(
        error.to_string(),
        "the command cannot be typed into the shell: it contains a NUL byte"
    );
}

#[test]
fn answer_errors_say_why_without_the_answer() {
    let refused = ShellError::NotWaiting {
        conversation: conversation(),
        reason: "the terminal echoes what is typed",
    };
    assert_eq!(
        refused.to_string(),
        "the command does not wait for this input: the terminal echoes what is typed"
    );
    let invalid = ShellError::InvalidAnswer { reason: "it contains a control character" };
    assert_eq!(invalid.to_string(), "the answer cannot be typed: it contains a control character");
    let none = ShellError::NoCall { conversation: conversation() };
    assert_eq!(
        none.to_string(),
        "no command of conversation 01920000-0000-7000-8000-000000000001 runs in its hidden shell"
    );
    let failed = ShellError::Terminal {
        conversation: conversation(),
        source: std::io::Error::from(std::io::ErrorKind::WriteZero),
    };
    assert!(failed.source().is_some());
}
