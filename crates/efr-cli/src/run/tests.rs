use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::{ErrorBody, ErrorCode};
use efr_transport::client::ClientError;
use pretty_assertions::assert_eq;

use super::{message, run};
use crate::context::Context;
use crate::error::{CliError, Exit};
use crate::settings::Settings;
use crate::testing::{TestEnv, capture, command};

#[test]
fn the_runtime_runs_on_one_thread() {
    let runtime = super::runtime().unwrap();

    assert_eq!(runtime.handle().runtime_flavor(), tokio::runtime::RuntimeFlavor::CurrentThread);
    assert_eq!(runtime.metrics().num_workers(), 1);
}

#[test]
fn a_message_carries_its_sources_on_one_line() {
    let error = CliError::Client(ClientError::Connect {
        socket: PathBuf::from("/run/user/1000/efr/daemon.sock"),
        source: io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
    });
    assert_eq!(
        message(&error),
        "efr: could not connect to /run/user/1000/efr/daemon.sock: permission denied\n"
    );
}

#[test]
fn a_message_from_the_daemon_cannot_drive_the_terminal() {
    let body = ErrorBody::new(ErrorCode::Internal, "bad\u{1b}[31m\nnews");
    let text = message(&CliError::Client(ClientError::Server { body }));
    assert_eq!(text, "efr: the daemon failed the request with internal: bad\u{241b}[31m news\n");
}

#[test]
fn a_hint_follows_on_its_own_line() {
    let error = CliError::Client(ClientError::DaemonNotRunning { socket: PathBuf::from("/s") });
    assert_eq!(message(&error), "efr: no daemon listens on /s\n  systemctl --user start efrd\n");
}

/// TM-Endings frame 14: an error outside a turn, `efr:` in `error` bold and the message
/// in `text`, then the command that fixes it 2 columns in, in `code`, in the three
/// depths.
#[test]
fn an_error_outside_a_turn_is_the_endings_board() {
    use efr_test_support::design::{Depth, Frame, assert_rows};

    use crate::testing::design_options;

    let frame = Frame::load("TM-Endings", 14).unwrap();
    let socket = PathBuf::from("/run/user/1000/efr/efrd.sock");
    let error = CliError::Client(ClientError::DaemonNotRunning { socket });
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let text = super::message_for(&error, "openai-api", &options);
        assert_rows(&frame, 1..=2, &text, depth);
    }
    // A fix that is a sentence is muted.
    let none = design_options(Depth::NoColor, 80);
    let dirs = CliError::Dirs { source: efr_stdx::StdxError::HomeNotFound };
    assert_eq!(
        super::message_for(&dirs, "openai-api", &none),
        "\x1b[1mefr:\x1b[0m the efr directories could not be found: the home directory could not be found\n  \
         \x1b[2mset HOME, or EFR_HOME\x1b[0m\n"
    );
}

#[tokio::test]
async fn config_warnings_are_reported_before_other_commands() {
    let env = TestEnv::new();
    let settings = Settings::parse(Path::new("/c/config.toml"), "[render]\ntheme = \"neon\"\n");
    let ctx = Context { settings, ..env.context() };
    let (mut out, captured) = capture();
    // No daemon runs, so the prompt fails after the warning.
    let exit = run(&command(&["send", "--", "hi"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::NotRunning);
    let stderr = captured.stderr();
    assert!(
        stderr.starts_with(
            "efr: warning: /c/config.toml names the theme \"neon\", which does not exist\n"
        ),
        "{stderr}"
    );
}
