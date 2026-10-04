use std::io;
use std::path::{Path, PathBuf};

use efr_client::ClientError;
use efr_protocol::{ErrorBody, ErrorCode};
use pretty_assertions::assert_eq;

use super::{message, run};
use crate::context::Context;
use crate::error::{CliError, Exit};
use crate::settings::Settings;
use crate::testing::{TestEnv, capture, command};

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
    assert_eq!(
        message(&error),
        "efr: no daemon is listening on /s\nefr: start the daemon with: systemctl --user start efrd, or `just run` in the efr checkout for a foreground one\n"
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
