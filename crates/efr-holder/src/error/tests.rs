use std::error::Error as _;
use std::io;
use std::path::PathBuf;

use efr_protocol::{PtyId, Size};
use pretty_assertions::assert_eq;

use super::HolderError;
use crate::Signal;

const PTY: &str = "01920000-0000-7000-8000-000000000001";

#[test]
fn messages_name_what_failed_in_one_sentence() {
    let cases = [
        (
            HolderError::ProgramNotAbsolute { program: PathBuf::from("zsh") },
            "the program zsh is not an absolute path",
        ),
        (
            HolderError::CwdNotAbsolute { cwd: PathBuf::from("src") },
            "the working directory src is not an absolute path",
        ),
        (
            HolderError::EmptySize { size: Size { cols: 0, rows: 24 } },
            "the terminal size 0x24 has no cells",
        ),
        (
            HolderError::InvalidEnvName { name: "A=B".to_owned() },
            r#""A=B" is not a valid environment variable name"#,
        ),
        (
            HolderError::NulInEnvValue { name: "TOKEN".to_owned() },
            "the value of the environment variable TOKEN contains a NUL byte",
        ),
        (
            HolderError::NulByte { field: "argument" },
            "the argument of the spawn spec contains a NUL byte",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn operation_messages_name_the_pty() {
    let pty_id: PtyId = PTY.parse().unwrap();
    let cases = [
        (HolderError::AlreadyExists { pty_id }, format!("the holder already holds the PTY {PTY}")),
        (HolderError::NotFound { pty_id }, format!("the holder holds no PTY {PTY}")),
        (HolderError::Exited { pty_id }, format!("the child of the PTY {PTY} has exited")),
        (
            HolderError::Spawn {
                program: PathBuf::from("/usr/bin/zsh"),
                source: io::Error::from(io::ErrorKind::NotFound),
            },
            "could not start /usr/bin/zsh on a new PTY".to_owned(),
        ),
        (
            HolderError::Resize { pty_id, source: io::Error::from(io::ErrorKind::BrokenPipe) },
            format!("could not resize the PTY {PTY}"),
        ),
        (
            HolderError::Signal {
                pty_id,
                signal: Signal::Interrupt,
                source: io::Error::from(io::ErrorKind::PermissionDenied),
            },
            format!("could not send SIGINT to the PTY {PTY}"),
        ),
        (
            HolderError::Foreground { pty_id, source: io::Error::from(io::ErrorKind::Other) },
            format!("could not read the foreground process group of the PTY {PTY}"),
        ),
        (
            HolderError::Release { pty_id, source: io::Error::from(io::ErrorKind::Other) },
            format!("could not release the PTY {PTY}"),
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn os_failures_keep_their_source_out_of_the_message() {
    let error = HolderError::Spawn {
        program: PathBuf::from("/usr/bin/zsh"),
        source: io::Error::other("no such file"),
    };
    assert!(!error.to_string().contains("no such file"));
    assert_eq!(error.source().unwrap().to_string(), "no such file");
}
