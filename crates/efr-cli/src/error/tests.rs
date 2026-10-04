use std::io;
use std::path::PathBuf;

use efr_client::ClientError;
use efr_protocol::{ErrorBody, ErrorCode};
use pretty_assertions::assert_eq;

use super::{CliError, Exit};

fn not_running() -> CliError {
    CliError::Client(ClientError::DaemonNotRunning {
        socket: PathBuf::from("/run/user/1000/efr/daemon.sock"),
    })
}

#[test]
fn exit_codes_are_the_documented_numbers() {
    let codes =
        [Exit::Success, Exit::DaemonError, Exit::Usage, Exit::NotRunning, Exit::Interrupted]
            .map(Exit::code);
    assert_eq!(codes, [0, 1, 2, 3, 130]);
}

#[test]
fn a_missing_daemon_exits_with_three_and_a_hint() {
    let error = not_running();
    assert_eq!(error.exit(), Exit::NotRunning);
    assert_eq!(error.hint(), Some("start the daemon with: systemctl --user start efrd"));
}

#[test]
fn bad_input_is_a_usage_error() {
    let invalid = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    for error in [
        CliError::InvalidContext { source: invalid },
        CliError::EmptyPrompt,
        CliError::SteerNeedsConversation,
        CliError::AmbiguousConversation { query: "019a".to_owned(), matches: 2 },
    ] {
        assert_eq!(error.exit(), Exit::Usage, "{error}");
    }
}

#[test]
fn daemon_failures_exit_with_one() {
    let body = ErrorBody::new(ErrorCode::Internal, "the provider is down");
    for error in [
        CliError::Client(ClientError::Server { body: body.clone() }),
        CliError::Client(ClientError::Closed),
        CliError::TurnFailed { body },
        CliError::TurnInterrupted,
        CliError::TurnCancelled,
        CliError::SubscriptionEnded,
        CliError::NoActiveConversation { tty: "/dev/pts/3".to_owned() },
        CliError::ConversationNotFound { query: "abcd".to_owned() },
    ] {
        assert_eq!(error.exit(), Exit::DaemonError, "{error}");
        assert_eq!(error.hint(), None);
    }
}

#[test]
fn ctrl_c_exits_with_130_and_no_message() {
    assert_eq!(CliError::Interrupted.exit(), Exit::Interrupted);
    assert!(CliError::Interrupted.is_silent());
}

#[test]
fn a_closed_pipe_is_silent_but_other_write_failures_are_not() {
    let pipe = CliError::Output { source: io::Error::from(io::ErrorKind::BrokenPipe) };
    assert!(pipe.is_silent());
    assert_eq!(pipe.exit(), Exit::DaemonError);
    let full = CliError::Output { source: io::Error::from(io::ErrorKind::StorageFull) };
    assert!(!full.is_silent());
}

#[test]
fn a_failed_turn_names_the_code_and_the_message() {
    let body = ErrorBody::new(ErrorCode::Busy, "rate limited");
    assert_eq!(
        CliError::TurnFailed { body }.to_string(),
        "the turn failed with busy: rate limited"
    );
}

#[test]
fn a_protocol_mismatch_hints_at_a_mixed_install() {
    let error = CliError::Client(ClientError::ProtocolMismatch { daemon: 2, client: 1 });
    assert_eq!(error.exit(), Exit::DaemonError);
    assert!(error.hint().unwrap().contains("different builds"));
}
