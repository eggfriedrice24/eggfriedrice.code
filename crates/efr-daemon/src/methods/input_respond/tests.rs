use efr_protocol::{CallId, ConversationId, ErrorCode};
use efr_shell::ShellError;
use pretty_assertions::assert_eq;

use super::refused;

fn id(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

#[test]
fn an_answer_the_shell_refused_maps_to_the_code_a_client_acts_on() {
    let conversation = ConversationId::from_uuid(id(1));
    let call = CallId::from_uuid(id(2));
    let cases = [
        (ShellError::NoShell { conversation }, ErrorCode::NotFound),
        (ShellError::NoCall { conversation }, ErrorCode::NotFound),
        (ShellError::Exited { conversation, status: None }, ErrorCode::NotFound),
        (ShellError::NotWaiting { conversation, reason: "another call runs" }, ErrorCode::Conflict),
        (
            ShellError::InvalidAnswer { reason: "it contains a control character" },
            ErrorCode::Invalid,
        ),
        (ShellError::Busy { conversation }, ErrorCode::Internal),
    ];
    for (error, expected) in cases {
        let shown = format!("{error:?}");
        assert_eq!(refused(error, conversation, call).code(), expected, "{shown}");
    }
}

#[test]
fn a_refusal_names_the_call() {
    let conversation = ConversationId::from_uuid(id(1));
    let call = CallId::from_uuid(id(2));
    let not_waiting = ShellError::NotWaiting { conversation, reason: "another call runs" };
    let message = refused(not_waiting, conversation, call).to_string();
    assert!(message.contains(&call.to_string()), "{message}");
    assert!(message.contains("another call runs"), "{message}");
}
