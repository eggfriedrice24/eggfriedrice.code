use std::io;
use std::path::PathBuf;

use efr_protocol::{ErrorBody, ErrorCode};
use efr_transport::client::ClientError;
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
fn a_signal_that_ends_a_turn_exits_with_128_and_its_number_and_no_message() {
    let error = CliError::Ended { signal: 15 };
    assert_eq!(error.exit(), Exit::Signal(15));
    assert_eq!(error.exit().code(), 143);
    assert_eq!(Exit::Signal(1).code(), 129);
    assert!(error.is_silent());
}

#[test]
fn a_missing_daemon_exits_with_three_and_a_hint() {
    let error = not_running();
    assert_eq!(error.exit(), Exit::NotRunning);
    // TM-Endings 14: the command alone, so a double click copies it.
    assert_eq!(error.hint().as_deref(), Some("systemctl --user start efrd"));
    assert_eq!(
        error.hint_for("openai-api"),
        Some(super::Fix::Commands(vec!["systemctl --user start efrd".to_owned()]))
    );
}

#[test]
fn a_socket_path_too_long_exits_with_three_and_names_the_variables() {
    let socket = PathBuf::from(format!("/tmp/{}/daemon.sock", "x".repeat(120)));
    let error = CliError::Client(ClientError::SocketPathTooLong { socket });
    assert_eq!(error.exit(), Exit::NotRunning);
    assert_eq!(
        error.hint().as_deref(),
        Some("set EFR_RUNTIME_DIR, or EFR_HOME, to a shorter directory")
    );
}

#[test]
fn a_turn_without_usable_credentials_hints_at_the_login() {
    let body =
        ErrorBody::new(ErrorCode::Unauthorized, "no credentials are stored for the provider");
    let error = CliError::TurnFailed { body };
    assert_eq!(error.exit(), Exit::DaemonError);
    assert_eq!(error.hint().as_deref(), Some("efr login openai"));
}

#[test]
fn the_login_fix_names_the_command_of_the_provider_and_its_account() {
    let commands = ["openai-subscription", "openai-subscription.b", "openai-api", "anthropic-api"]
        .into_iter()
        .chain(["anthropic-api.work", "replay"])
        .map(super::login_command)
        .collect::<Vec<_>>();
    assert_eq!(
        commands,
        [
            "efr login openai",
            "efr login openai --account b",
            "efr login openai-api",
            "efr login anthropic-api",
            "efr login anthropic-api --account work",
            "efr login openai",
        ]
    );
}

#[test]
fn a_full_context_names_the_reason_and_the_commands_that_make_room() {
    let full = |way_out: &str| {
        ErrorBody::new(
            ErrorCode::Internal,
            format!(
                "the context is full: compaction did not free enough room (still 240k); {way_out}"
            ),
        )
        .with_data(serde_json::json!({ "cause": "context_overflow", "tokens": 240_000 }))
    };
    let (reason, fix) =
        super::turn_failure(&full("run ,compact or start a new conversation"), "openai-api");
    assert_eq!(reason, "the context is full: compaction did not free enough room (still 240k)");
    assert_eq!(fix.unwrap().text(), ",compact or efr new");
    let (_, only_new) = super::turn_failure(&full("start a new conversation"), "openai-api");
    assert_eq!(only_new.unwrap().text(), "efr new");
    // Another internal failure keeps the daemon's whole message and has no fix.
    let other = ErrorBody::new(ErrorCode::Internal, "the store failed; try again");
    assert_eq!(
        super::turn_failure(&other, "openai-api"),
        ("the store failed; try again".to_owned(), None)
    );
}

#[test]
fn a_refused_model_hints_at_the_config() {
    let body = ErrorBody::new(ErrorCode::Invalid, "the provider does not serve the model \"m\"")
        .with_data(serde_json::json!({ "model": "m" }));
    let error = CliError::TurnFailed { body };
    assert_eq!(error.exit(), Exit::DaemonError);
    let hint = error.hint().unwrap();
    assert!(hint.contains(",model <model> for this terminal"), "{hint}");
    assert!(hint.contains("under [model] in config.toml"), "{hint}");
    assert!(hint.contains("reloads without a restart"), "{hint}");

    let other = ErrorBody::new(ErrorCode::Invalid, "the prompt is too long");
    assert_eq!(CliError::TurnFailed { body: other }.hint(), None, "not about a model");
}

#[test]
fn a_turn_setting_that_no_longer_fits_hints_at_the_model_only_for_the_model() {
    let setting = |setting: &str| {
        let body = ErrorBody::new(ErrorCode::Invalid, "the turn's settings no longer fit")
            .with_data(serde_json::json!({
                "setting": setting, "value": "x", "model": "gpt-5.5", "choices": ["low"],
            }));
        CliError::TurnFailed { body }.hint()
    };

    assert_eq!(setting("model").as_deref(), Some(super::MODEL_HINT));
    assert_eq!(setting("effort"), None, "the message names the efforts");
}

#[test]
fn a_daemon_that_does_not_answer_points_at_its_log() {
    let after = std::time::Duration::from_secs(5);
    let socket = PathBuf::from("/run/user/1000/efr/daemon.sock");
    for error in [
        CliError::Client(ClientError::HelloTimedOut { after }),
        CliError::Client(ClientError::ConnectTimedOut { socket, after }),
    ] {
        assert_eq!(
            error.hint().as_deref(),
            Some("the daemon is not answering; its log: journalctl --user -u efrd")
        );
    }
}

#[test]
fn missing_directories_say_which_variable_to_set() {
    let runtime = CliError::Dirs { source: efr_stdx::StdxError::RuntimeDirUnset };
    assert!(runtime.hint().unwrap().contains("EFR_RUNTIME_DIR"));
    assert!(runtime.hint().unwrap().contains("EFR_HOME"));
    let home = CliError::Dirs { source: efr_stdx::StdxError::HomeNotFound };
    assert_eq!(home.hint().as_deref(), Some("set HOME, or EFR_HOME"));
}

#[test]
fn a_config_error_was_printed_by_its_command_and_exits_with_1() {
    assert!(CliError::ConfigInvalid.is_silent());
    assert_eq!(CliError::ConfigInvalid.exit(), Exit::Invalid);
    assert_eq!(Exit::Invalid.code(), 1);
}

#[test]
fn bad_input_is_a_usage_error() {
    let invalid = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    for error in [
        CliError::InvalidContext { input: "EFR_CONTEXT", source: invalid },
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
        CliError::LoginIncomplete,
    ] {
        assert_eq!(error.exit(), Exit::DaemonError, "{error}");
        assert_eq!(error.hint(), None);
    }
}

#[test]
fn a_failed_or_interrupted_turn_has_no_message_because_the_view_showed_its_end() {
    let body = ErrorBody::new(ErrorCode::Internal, "the provider is down");
    assert!(CliError::TurnFailed { body }.is_silent());
    assert!(CliError::TurnInterrupted.is_silent());
    assert!(!CliError::TurnCancelled.is_silent(), "the view shows no end for it");
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
