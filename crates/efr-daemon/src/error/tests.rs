use std::path::PathBuf;
use std::time::Duration;

use efr_conversation::ConversationError;
use efr_oauth_openai::OAuthError;
use efr_protocol::{
    CallId, CommandId, ConversationId, ErrorBody, ErrorCode, ErrorFrame, PtyId, ScopeName, Seq,
    TurnId,
};
use efr_shell::ShellError;
use efr_store::StoreError;
use efr_transport::TransportError;
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::DaemonError;

fn id(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

fn frame(error: DaemonError) -> ErrorFrame {
    ErrorFrame::from(error)
}

fn code(error: DaemonError) -> ErrorCode {
    frame(error).error.code
}

#[test]
fn request_errors_map_to_the_code_a_client_acts_on() {
    let conversation_id = ConversationId::from_uuid(id(1));
    let cases = [
        (
            DaemonError::Forbidden { method: "admin.status", scope: ScopeName::Admin },
            ErrorCode::Forbidden,
        ),
        (DaemonError::InvalidParams { reason: "the prompt is empty" }, ErrorCode::Invalid),
        (DaemonError::InvalidCursor { cursor: "x".to_owned() }, ErrorCode::Invalid),
        (DaemonError::ConversationNotFound { conversation_id }, ErrorCode::NotFound),
        (DaemonError::NoRunningTurn { conversation_id }, ErrorCode::Conflict),
        (
            DaemonError::ApprovalNotPending { call_id: CallId::from_uuid(id(2)) },
            ErrorCode::NotFound,
        ),
        (DaemonError::PtyNotFound { pty_id: PtyId::from_uuid(id(3)) }, ErrorCode::NotFound),
        (DaemonError::HelloRepeated, ErrorCode::Conflict),
        (
            DaemonError::CommandReused {
                command_id: CommandId::from_uuid(id(4)),
                method: "turn.steer".to_owned(),
            },
            ErrorCode::Conflict,
        ),
        (DaemonError::AlreadyRunning { path: PathBuf::from("/d/daemon.lock") }, ErrorCode::Busy),
        (DaemonError::TaskPanicked { task: "x" }, ErrorCode::Internal),
        (DaemonError::NotWired { method: "models.list" }, ErrorCode::Internal),
        (DaemonError::ReloadStopped, ErrorCode::Internal),
        (
            DaemonError::CallNotRunning { conversation_id, call_id: CallId::from_uuid(id(5)) },
            ErrorCode::NotFound,
        ),
        (
            DaemonError::NotWaitingForInput {
                call_id: CallId::from_uuid(id(5)),
                reason: "the terminal echoes what is typed",
            },
            ErrorCode::Conflict,
        ),
        (
            DaemonError::InvalidAnswer { reason: "it is longer than input.respond allows" },
            ErrorCode::Invalid,
        ),
    ];
    for (error, expected) in cases {
        let message = error.to_string();
        let body = frame(error).error;
        assert_eq!(body.code, expected, "{message}");
        assert_eq!(body.message, message);
    }
}

#[test]
fn store_errors_keep_their_meaning_on_the_wire() {
    let conversation_id = ConversationId::from_uuid(id(1));
    let call_id = CallId::from_uuid(id(2));

    let not_pending =
        DaemonError::from(StoreError::ApprovalNotPending { conversation_id, call_id });
    assert!(matches!(not_pending, DaemonError::ApprovalNotPending { call_id: c } if c == call_id));
    assert_eq!(code(not_pending), ErrorCode::NotFound);

    let missing = StoreError::ReceiptEventMissing {
        command_id: CommandId::from_uuid(id(3)),
        index: 4,
        events: 1,
    };
    assert_eq!(code(DaemonError::from(missing)), ErrorCode::Internal);

    assert_eq!(code(DaemonError::from(StoreError::WriterStopped)), ErrorCode::Internal);
    assert_eq!(
        code(DaemonError::from(StoreError::UnknownConversation { conversation_id })),
        ErrorCode::NotFound
    );
}

#[test]
fn conversation_errors_map_by_what_failed() {
    let conversation_id = ConversationId::from_uuid(id(1));
    let turn = |n| TurnId::from_uuid(id(n));
    let cases = [
        (ConversationError::QueueFull { conversation_id, limit: 16 }, ErrorCode::Busy),
        (ConversationError::NoRunningTurn { conversation_id }, ErrorCode::Conflict),
        (
            ConversationError::TurnMismatch { running: turn(2), requested: turn(3) },
            ErrorCode::Conflict,
        ),
        (
            ConversationError::WrongConversation {
                conversation_id,
                requested: ConversationId::from_uuid(id(9)),
            },
            ErrorCode::Invalid,
        ),
        (
            ConversationError::ApprovalNotPending { call_id: CallId::from_uuid(id(4)) },
            ErrorCode::NotFound,
        ),
        (ConversationError::Stopped, ErrorCode::Internal),
        (
            ConversationError::Store {
                source: StoreError::ApprovalNotPending {
                    conversation_id,
                    call_id: CallId::from_uuid(id(5)),
                },
            },
            ErrorCode::NotFound,
        ),
    ];
    for (error, expected) in cases {
        let message = error.to_string();
        let body = frame(DaemonError::from(error)).error;
        assert_eq!(body.code, expected, "{message}");
    }
}

#[test]
fn a_conversation_refusal_says_what_the_conversation_said() {
    let conversation_id = ConversationId::from_uuid(id(1));
    let error = ConversationError::QueueFull { conversation_id, limit: 16 };
    let message = error.to_string();

    assert_eq!(frame(DaemonError::from(error)).error.message, message);
}

#[test]
fn an_invalid_turn_setting_is_invalid_with_its_choices_as_data() {
    let error = ConversationError::InvalidSetting {
        setting: "effort",
        value: "ultra".to_owned(),
        model: Some("gpt-5.5".to_owned()),
        choices: vec!["low".to_owned(), "high".to_owned()],
        from_config: false,
    };
    let message = error.to_string();

    let body = frame(DaemonError::from(error)).error;

    assert_eq!(body.code, ErrorCode::Invalid);
    assert_eq!(body.message, message);
    assert_eq!(
        body.data,
        Some(json!({
            "setting": "effort",
            "value": "ultra",
            "model": "gpt-5.5",
            "choices": ["low", "high"],
        }))
    );
}

#[test]
fn a_stored_refusal_is_answered_exactly_as_stored() {
    let body = ErrorBody::new(ErrorCode::NotFound, "the call has no pending approval")
        .with_data(json!({"call_id": "x"}));

    assert_eq!(frame(DaemonError::Rejected { body: body.clone() }).error, body);
}

#[test]
fn an_overflow_carries_the_sequence_to_resume_after() {
    let error = DaemonError::from(TransportError::Overflow { last_seq: Seq::new(41) });

    let body = frame(error).error;

    assert_eq!(body.code, ErrorCode::Overflow);
    assert_eq!(body.last_seq(), Some(Seq::new(41)));
}

#[test]
fn shell_and_login_errors_map_to_their_codes() {
    let conversation = ConversationId::from_uuid(id(1));
    assert_eq!(code(DaemonError::from(ShellError::NoShell { conversation })), ErrorCode::NotFound);
    let login = |source| DaemonError::Login { source };
    assert_eq!(code(login(OAuthError::LoginInProgress)), ErrorCode::Busy);
    assert_eq!(
        code(login(OAuthError::TimedOut { after: Duration::from_secs(600) })),
        ErrorCode::Cancelled
    );
    assert_eq!(code(login(OAuthError::MissingCode)), ErrorCode::Internal);
}

#[test]
fn the_frame_names_no_request_until_the_transport_does() {
    assert_eq!(frame(DaemonError::HelloRepeated).id, None);
}
